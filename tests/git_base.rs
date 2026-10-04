//! Real Git integration, outside unit suites that temporarily mutate PATH.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use tsk_tui::git_base::*;
static SEQ: AtomicU64 = AtomicU64::new(0);
struct Repo {
    root: PathBuf,
    remote: PathBuf,
    local: PathBuf,
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn git(path: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}
fn repo() -> Repo {
    let root = std::env::temp_dir().join(format!(
        "tsk-base-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let remote = root.join("remote");
    let local = root.join("local");
    std::fs::create_dir(&remote).unwrap();
    git(&remote, &["init", "-b", "main"]);
    git(&remote, &["config", "user.email", "test@example.com"]);
    git(&remote, &["config", "user.name", "test"]);
    git(&remote, &["commit", "--allow-empty", "-m", "initial"]);
    git(
        &root,
        &["clone", remote.to_str().unwrap(), local.to_str().unwrap()],
    );
    git(&local, &["config", "user.email", "test@example.com"]);
    git(&local, &["config", "user.name", "test"]);
    Repo {
        root,
        remote,
        local,
    }
}
#[test]
fn default_ignores_checkout_and_fetches_remote_tip() {
    let r = repo();
    git(&r.local, &["checkout", "-b", "unrelated"]);
    git(
        &r.remote,
        &["commit", "--allow-empty", "-m", "new remote commit"],
    );
    let expected = git(&r.remote, &["rev-parse", "HEAD"]);
    let base = resolve(&r.local, None).unwrap();
    assert_eq!(base.reference, "origin/main");
    assert_eq!(base.commit.as_deref(), Some(expected.as_str()));
    assert!(base.warning.is_none());
    assert_eq!(default_branch_name(&r.local).as_deref(), Some("main"));
}
#[test]
fn explicit_local_upstream_is_fetched_but_local_only_stays_local() {
    let r = repo();
    git(&r.remote, &["branch", "feature"]);
    git(&r.local, &["fetch", "origin"]);
    git(
        &r.local,
        &["branch", "--track", "feature", "origin/feature"],
    );
    git(&r.remote, &["checkout", "feature"]);
    git(
        &r.remote,
        &["commit", "--allow-empty", "-m", "advanced feature"],
    );
    let base = resolve(&r.local, Some("feature")).unwrap();
    assert_eq!(base.reference, "origin/feature");
    assert_eq!(base.commit.unwrap(), git(&r.remote, &["rev-parse", "HEAD"]));
    git(&r.local, &["branch", "local-only"]);
    assert_eq!(
        resolve(&r.local, Some("local-only")).unwrap().reference,
        "local-only"
    );
    assert_eq!(
        resolve(&r.local, Some("origin/feature")).unwrap().reference,
        "origin/feature"
    );
}
#[test]
fn overlapping_remote_names_use_upstream_provenance_for_fetch_and_prompt() {
    let r = repo();
    git(&r.remote, &["branch", "team/main"]);
    git(&r.local, &["fetch", "origin"]);
    git(
        &r.local,
        &["branch", "--track", "release", "origin/team/main"],
    );
    let other = r.root.join("other-remote");
    std::fs::create_dir(&other).unwrap();
    git(&other, &["init", "-b", "main"]);
    git(
        &other,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-m",
            "unrelated",
        ],
    );
    // Git remote add rejects overlapping names, but a hand-written/config-imported
    // remote section is still accepted and fetchable by Git.
    git(
        &r.local,
        &["config", "remote.origin/team.url", other.to_str().unwrap()],
    );
    git(
        &r.local,
        &[
            "config",
            "remote.origin/team.fetch",
            "+refs/heads/*:refs/remotes/origin/team/*",
        ],
    );
    git(&r.remote, &["checkout", "team/main"]);
    git(
        &r.remote,
        &["commit", "--allow-empty", "-m", "new upstream commit"],
    );
    let expected = git(&r.remote, &["rev-parse", "HEAD"]);
    let base = resolve(&r.local, Some("release")).unwrap();
    assert_eq!(base.reference, "origin/team/main");
    assert_eq!(base.remote.as_deref(), Some("origin"));
    assert_eq!(base.commit.as_deref(), Some(expected.as_str()));
    assert_eq!(
        short_name_for_remote(&base.reference, base.remote.as_deref()),
        "team/main"
    );
}

#[test]
fn rejects_tags_commits_and_option_like_values() {
    let r = repo();
    git(&r.local, &["tag", "release"]);
    for invalid in [
        "release",
        "HEAD",
        "--help",
        "origin/HEAD",
        "missing",
        &git(&r.local, &["rev-parse", "HEAD"]),
    ] {
        assert!(
            validate_branch(&r.local, invalid).is_err(),
            "accepted {invalid}"
        );
    }
    validate_branch(&r.local, "main").unwrap();
    validate_branch(&r.local, "origin/main").unwrap();
}
#[test]
fn colliding_tag_or_local_remote_name_cannot_change_the_recorded_base() {
    let r = repo();
    git(&r.local, &["tag", "main"]);
    let local = resolve(&r.local, Some("main")).unwrap();
    assert_eq!(local.reference, "origin/main");
    assert_eq!(
        recorded_branch_ref(&r.local, &local.reference).unwrap(),
        "refs/remotes/origin/main"
    );
    git(&r.local, &["branch", "origin/main", "refs/heads/main"]);
    let colliding = resolve(&r.local, Some("origin/main")).unwrap();
    assert_eq!(colliding.reference, "refs/heads/origin/main");
    assert_eq!(
        short_name_for_remote(&colliding.reference, colliding.remote.as_deref()),
        "origin/main"
    );
    assert_eq!(
        recorded_branch_ref(&r.local, &colliding.reference).unwrap(),
        "refs/heads/origin/main"
    );
}

#[test]
fn offline_keeps_cached_remote_ref_and_reports_fallback() {
    let r = repo();
    git(
        &r.local,
        &[
            "remote",
            "set-url",
            "origin",
            r.root.join("missing").to_str().unwrap(),
        ],
    );
    let base = resolve(&r.local, None).unwrap();
    assert_eq!(base.reference, "origin/main");
    assert!(base.warning.unwrap().contains("cached"));
}
#[test]
fn cleanup_fetches_recorded_upstream_without_a_local_pull_and_keeps_squash_branches() {
    use tsk_tui::dispatch::{DispatchHost, SystemDispatchHost};
    use tsk_tui::domain::Dispatch;
    let r = repo();
    let worktree = r.root.join("task-worktree");
    git(
        &r.local,
        &[
            "worktree",
            "add",
            "-b",
            "tsk/test",
            worktree.to_str().unwrap(),
            "origin/main",
        ],
    );
    git(&worktree, &["commit", "--allow-empty", "-m", "task commit"]);
    let initial_local = git(&r.local, &["rev-parse", "main"]);
    git(&r.remote, &["fetch", r.local.to_str().unwrap(), "tsk/test"]);
    git(&r.remote, &["merge", "--ff-only", "FETCH_HEAD"]);
    let record = Dispatch {
        argv: vec![],
        worktree: worktree.to_string_lossy().into_owned(),
        branch: "tsk/test".into(),
        base: Some("origin/main".into()),
        base_commit: Some(initial_local.clone()),
        base_remote: Some("origin".into()),
        herdr_workspace_id: "not-used".into(),
        at: std::time::SystemTime::now(),
        cleaned: false,
    };
    let mut host = SystemDispatchHost;
    let inspection = host.inspect_cleanup(&r.local, &record, false).unwrap();
    assert!(
        inspection.branch_merged,
        "merge is visible after fetch without pull"
    );
    assert_eq!(git(&r.local, &["rev-parse", "main"]), initial_local);
    host.remove_git_worktree(&r.local, &worktree).unwrap();
    assert!(host
        .delete_merged_branch(&r.local, "tsk/test", "origin/main")
        .unwrap());

    git(
        &r.local,
        &[
            "worktree",
            "add",
            "-b",
            "tsk/squash",
            worktree.to_str().unwrap(),
            "origin/main",
        ],
    );
    git(
        &worktree,
        &["commit", "--allow-empty", "-m", "squashed work"],
    );
    git(
        &r.remote,
        &["commit", "--allow-empty", "-m", "squash merge"],
    );
    let record = Dispatch {
        branch: "tsk/squash".into(),
        ..record
    };
    assert!(
        !host
            .inspect_cleanup(&r.local, &record, false)
            .unwrap()
            .branch_merged
    );
}

#[test]
fn cleanup_retains_a_branch_advanced_after_inspection() {
    use tsk_tui::dispatch::{DispatchHost, SystemDispatchHost};
    let r = repo();
    git(&r.local, &["branch", "tsk/race", "origin/main"]);
    // The old tip is merged. A concurrent worker then advances the branch before
    // cleanup deletes it, the final ancestry check must reject that new tip.
    git(&r.local, &["checkout", "tsk/race"]);
    git(
        &r.local,
        &["commit", "--allow-empty", "-m", "unmerged late commit"],
    );
    let new_tip = git(&r.local, &["rev-parse", "HEAD"]);
    git(&r.local, &["checkout", "main"]);
    assert!(!SystemDispatchHost
        .delete_merged_branch(&r.local, "tsk/race", "origin/main")
        .unwrap());
    assert_eq!(git(&r.local, &["rev-parse", "tsk/race"]), new_tip);
}

#[cfg(unix)]
#[test]
fn fetch_timeout_is_bounded_and_returns_cached_base() {
    use std::os::unix::fs::PermissionsExt;
    let r = repo();
    let upload = r.root.join("slow-upload");
    std::fs::write(&upload, "#!/bin/sh\nsleep 30\n").unwrap();
    std::fs::set_permissions(&upload, std::fs::Permissions::from_mode(0o700)).unwrap();
    git(
        &r.local,
        &[
            "config",
            "remote.origin.uploadpack",
            upload.to_str().unwrap(),
        ],
    );
    let started = std::time::Instant::now();
    let base = resolve(&r.local, None).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(8));
    assert_eq!(base.reference, "origin/main");
    assert!(base.warning.unwrap().contains("timed out"));
}

#[test]
fn no_origin_default_ref_refuses_instead_of_using_head() {
    let r = repo();
    git(&r.local, &["remote", "remove", "origin"]);
    assert!(resolve(&r.local, None).is_err());
}

#[test]
fn quick_add_tokens_lift_base_and_validate_it_in_the_effective_task_repo() {
    use tsk_tui::capture::lift_quick_add_tokens;
    use tsk_tui::domain::{DomainState, TaskScope};
    let r = repo();
    git(&r.local, &["branch", "release"]);
    let scope = TaskScope::Project {
        path: r.local.to_string_lossy().into_owned(),
    };
    let state = DomainState::new();
    let lifted =
        lift_quick_add_tokens("Ship it !b release !t Launch", &state, None, &scope, &[]).unwrap();
    assert_eq!(lifted.title, "Ship it");
    assert_eq!(lifted.thread.as_deref(), Some("launch"));
    assert_eq!(lifted.base.as_deref(), Some("release"));
    let error =
        lift_quick_add_tokens("Do not save !b missing", &state, None, &scope, &[]).unwrap_err();
    assert!(error.contains("missing"));
}

fn await_default_name(model: &tsk_tui::ui::BoardModel, repo: &Path, expected: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let actual = model.default_branch_name(repo);
        if actual == expected {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "cached {actual}, expected {expected}"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn footer_default_cache_refreshes_after_dispatch_updates_origin_head() {
    use tsk_tui::domain::{Dispatch, DomainState, ProvenanceOrigin, TaskScope};
    use tsk_tui::ui::BoardModel;
    let r = repo();
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "cache",
            None,
            TaskScope::Project {
                path: r.local.to_string_lossy().into_owned(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .unwrap();
    let mut model = BoardModel::from_domain(&domain, Some(r.local.clone()));
    await_default_name(&model, &r.local, "main");
    git(&r.remote, &["checkout", "-b", "dispatch"]);
    let base = resolve(&r.local, None).unwrap();
    assert_eq!(base.reference, "origin/dispatch");
    assert_eq!(default_branch_name(&r.local).as_deref(), Some("dispatch"));
    domain
        .record_dispatch(
            id,
            Dispatch {
                argv: vec![],
                worktree: r.root.join("not-opened").to_string_lossy().into_owned(),
                branch: "tsk/cache".into(),
                base: Some(base.reference),
                base_commit: base.commit,
                base_remote: base.remote,
                herdr_workspace_id: "not-opened".into(),
                at: std::time::SystemTime::now(),
                cleaned: false,
            },
        )
        .unwrap();
    model.sync_from_domain(&domain);
    await_default_name(&model, &r.local, "dispatch");
}

#[test]
fn footer_default_cache_eventually_refreshes_external_metadata_without_a_task_change() {
    let r = repo();
    let model = tsk_tui::ui::BoardModel::from_tasks(vec![], None);
    await_default_name(&model, &r.local, "main");
    git(&r.remote, &["branch", "other-default"]);
    git(&r.local, &["fetch", "origin"]);
    git(
        &r.local,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/other-default",
        ],
    );
    await_default_name(&model, &r.local, "other-default");
}
