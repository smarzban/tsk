//! Real Git integration, outside unit suites that temporarily mutate PATH.

#[cfg(unix)]
use crate::stub;
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
    stretch_default_deadlines_for_tests();
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
    assert_eq!(base.full_ref.as_deref(), Some("refs/remotes/origin/main"));
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
    let (branches, warning) = list_branches_with_warning(&r.local).unwrap();
    assert!(branches.contains(&"origin/main".to_string()));
    assert!(warning.unwrap().contains("cached"));
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
        base_ref: Some("refs/remotes/origin/main".into()),
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
    // Asserts the shipped 5s fetch deadline, not the stretched one parallel tests use.
    exact_fetch_deadline_on_this_thread();
    let r = repo();
    let upload = r.root.join("slow-upload");
    stub::write_stub(&upload, "#!/bin/sh\nsleep 30\n", 0o700);
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
                base_ref: base.full_ref,
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

#[test]
fn local_upstream_dot_keeps_the_selected_local_tip() {
    let r = repo();
    git(&r.local, &["branch", "--track", "feature", "main"]);
    git(&r.local, &["checkout", "feature"]);
    git(
        &r.local,
        &["commit", "--allow-empty", "-m", "local feature work"],
    );
    let expected = git(&r.local, &["rev-parse", "HEAD"]);
    let base = resolve(&r.local, Some("feature")).unwrap();
    assert_eq!(base.reference, "feature");
    assert_eq!(base.full_ref.as_deref(), Some("refs/heads/feature"));
    assert_eq!(base.commit.as_deref(), Some(expected.as_str()));
    assert_eq!(base.remote, None);
}

#[test]
fn explicit_remote_base_fetches_a_newly_pushed_branch_before_validation() {
    let r = repo();
    git(&r.remote, &["checkout", "-b", "new-release"]);
    git(&r.remote, &["commit", "--allow-empty", "-m", "new release"]);
    assert!(validate_branch(&r.local, "origin/new-release").is_err());
    let base = resolve(&r.local, Some("origin/new-release")).unwrap();
    assert_eq!(base.reference, "origin/new-release");
    assert_eq!(
        base.full_ref.as_deref(),
        Some("refs/remotes/origin/new-release")
    );
    assert_eq!(base.remote.as_deref(), Some("origin"));
    assert_eq!(base.commit.unwrap(), git(&r.remote, &["rev-parse", "HEAD"]));
}

#[cfg(unix)]
#[test]
fn local_git_queries_are_bounded_and_kill_output_holding_children() {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    // Mutate PATH only in a dedicated subprocess, never the shared harness.
    if let Ok(root) = std::env::var("TSK_GIT_BASE_TIMEOUT_CHILD") {
        let path = Path::new(&root);
        // Allow script/DD startup overhead for the saturation probe; the local
        // metadata calls below still use the production 250ms deadline.
        let captured =
            git_process_output_timeout(path, &["capture-output"], Duration::from_secs(2)).unwrap();
        assert!(!captured.status.success());
        assert_eq!(captured.stdout.len(), 256 * 1024);
        assert_eq!(captured.stderr.len(), 256 * 1024);
        for query in 0..6 {
            let started = Instant::now();
            match query {
                0 => assert_eq!(default_branch_name(path), None),
                1 => assert!(validate_branch(path, "main").is_err()),
                2 => assert!(validate_branch(path, "origin/main").is_err()),
                3 => assert_eq!(remote_for_ref(path, "origin/main"), None),
                4 => assert!(list_cached_branches(path).is_err()),
                _ => assert!(resolve(path, None).is_err()),
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "query {query} stalled"
            );
        }
        return;
    }
    let r = repo();
    let fake = r.root.join("git");
    let escaped_root = r.root.to_string_lossy().replace('\'', "'\\''");
    stub::write_stub(&fake, format!("#!/bin/sh\ncase \"$3:$4\" in fetch:*|remote:set-head) exit 0;; capture-output:*) dd if=/dev/zero bs=262144 count=1 2>/dev/null; dd if=/dev/zero bs=262144 count=1 1>&2 2>/dev/null; exit 7;; esac\n(sleep 1; echo escaped > '{escaped_root}/escaped') &\n# More than a pipe buffer on both streams, then hold them open.\ndd if=/dev/zero bs=262144 count=1 2>/dev/null\ndd if=/dev/zero bs=262144 count=1 1>&2 2>/dev/null\nsleep 30\n"), 0o700);
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "git_base::local_git_queries_are_bounded_and_kill_output_holding_children",
            "--nocapture",
        ])
        .env("TSK_GIT_BASE_TIMEOUT_CHILD", &r.root)
        .env("PATH", format!("{}:/usr/bin:/bin", r.root.display()))
        .stdin(Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(8) {
            // SAFETY: the child was launched into its own process group.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            panic!("local Git query exceeded its deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success());
    std::thread::sleep(Duration::from_millis(1200));
    assert!(
        !r.root.join("escaped").exists(),
        "Git descendants escaped timeout cancellation"
    );
}

#[test]
fn exact_local_base_survives_a_new_remote_named_after_its_prefix() {
    let r = repo();
    git(&r.local, &["branch", "integration/main"]);
    let base = resolve(&r.local, Some("integration/main")).unwrap();
    assert_eq!(base.reference, "integration/main");
    assert_eq!(
        base.full_ref.as_deref(),
        Some("refs/heads/integration/main")
    );
    git(
        &r.local,
        &["remote", "add", "integration", r.remote.to_str().unwrap()],
    );
    git(&r.local, &["fetch", "integration"]);
    assert_eq!(
        recorded_branch_ref(&r.local, base.full_ref.as_deref().unwrap()).unwrap(),
        "refs/heads/integration/main"
    );
}

#[test]
fn fresh_validation_fetches_remote_but_board_validation_is_local_only() {
    let r = repo();
    git(&r.remote, &["branch", "fresh-validation"]);
    assert!(validate_branch(&r.local, "origin/fresh-validation").is_err());
    assert!(git(
        &r.local,
        &["for-each-ref", "refs/remotes/origin/fresh-validation"]
    )
    .is_empty());
    validate_branch_fresh(&r.local, "origin/fresh-validation").unwrap();
    validate_branch(&r.local, "origin/fresh-validation").unwrap();
}

#[test]
fn dispatch_exact_base_ref_is_optional_in_v6_and_round_trips() {
    use tsk_tui::domain::{Dispatch, STORE_FORMAT_VERSION};
    let r = repo();
    let old = serde_json::json!({
        "argv": [], "worktree": r.local, "branch": "tsk/test", "base": "integration/main",
        "herdr_workspace_id": "unused", "at": [0, 0]
    });
    let mut dispatch: Dispatch = serde_json::from_value(old).unwrap();
    assert_eq!(STORE_FORMAT_VERSION, 6);
    assert_eq!(dispatch.base_ref, None);
    assert!(serde_json::to_value(&dispatch)
        .unwrap()
        .get("base_ref")
        .is_none());
    dispatch.base_ref = Some("refs/heads/integration/main".into());
    let loaded: Dispatch =
        serde_json::from_value(serde_json::to_value(&dispatch).unwrap()).unwrap();
    assert_eq!(loaded.base_ref, dispatch.base_ref);
}

#[test]
fn fetch_window_reuses_a_fetch_from_the_last_minute() {
    let r = repo();
    assert!(!fetch_is_fresh(&r.local, "origin"));
    assert_eq!(
        fetch_remote_outcome(&r.local, "origin"),
        Ok(FetchOutcome::Fetched)
    );
    assert!(fetch_is_fresh(&r.local, "origin"));
    git(&r.remote, &["branch", "pushed-after-fetch"]);
    assert_eq!(
        fetch_remote_outcome(&r.local, "origin"),
        Ok(FetchOutcome::Reused)
    );
    assert!(!list_cached_branches(&r.local)
        .unwrap()
        .contains(&"origin/pushed-after-fetch".to_string()));
    // A repository without the remote never waits on a fetch it cannot run.
    assert!(fetch_is_fresh(&r.local, "upstream"));
}

#[cfg(unix)]
#[test]
fn cleanup_confirmed_during_a_background_check_waits_for_that_one_fetch() {
    use tsk_tui::dispatch::{DispatchHost, SystemDispatchHost};
    use tsk_tui::domain::Dispatch;
    let r = repo();
    let worktree = r.root.join("checking-worktree");
    git(
        &r.local,
        &[
            "worktree",
            "add",
            "-b",
            "tsk/checking",
            worktree.to_str().unwrap(),
            "origin/main",
        ],
    );
    let counter = r.root.join("upload-count");
    let upload = r.root.join("counted-upload");
    stub::write_stub(
        &upload,
        format!(
            "#!/bin/sh\nprintf 'fetch\\n' >> '{}'\nsleep 1\nexit 1\n",
            counter.display()
        ),
        0o700,
    );
    git(
        &r.local,
        &[
            "config",
            "remote.origin.uploadpack",
            upload.to_str().unwrap(),
        ],
    );
    let record = Dispatch {
        argv: vec![],
        worktree: worktree.to_string_lossy().into_owned(),
        branch: "tsk/checking".into(),
        base: Some("origin/main".into()),
        base_ref: Some("refs/remotes/origin/main".into()),
        base_commit: None,
        base_remote: Some("origin".into()),
        herdr_workspace_id: "not-used".into(),
        at: std::time::SystemTime::now(),
        cleaned: false,
    };
    let mut host = SystemDispatchHost;
    let cached = host
        .inspect_cleanup_cached(&r.local, &record, false)
        .unwrap();
    assert!(cached.branch_merged && cached.warning.is_none());
    assert!(!counter.exists(), "cached inspection never fetches");
    let check = host
        .begin_merge_check(&r.local, &record)
        .expect("background check starts");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !counter.exists() {
        assert!(std::time::Instant::now() < deadline, "fetch did not start");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // `y` arrives mid-fetch: its inspection waits for that fetch and shares its failure.
    let confirmed = host.inspect_cleanup(&r.local, &record, false).unwrap();
    assert_eq!(
        std::fs::read_to_string(&counter).unwrap().lines().count(),
        1
    );
    assert!(confirmed
        .warning
        .unwrap()
        .contains("merged status not confirmed"));
    assert_eq!(confirmed.unreachable_remote.as_deref(), Some("origin"));
    let verdict = loop {
        if let Some(verdict) = check.take() {
            break verdict;
        }
        assert!(std::time::Instant::now() < deadline, "check did not land");
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    // The refs on disk still read as merged, but nothing fetched could confirm it.
    assert!(verdict.branch_merged && verdict.confirmed);
    assert_eq!(verdict.unreachable_remote.as_deref(), Some("origin"));
    assert!(verdict.warning.unwrap().contains("fetch failed"));
}

#[test]
fn a_picker_fetch_refreshes_the_remote_default_for_a_reused_dispatch() {
    let r = repo();
    git(&r.remote, &["branch", "trunk"]);
    git(&r.remote, &["symbolic-ref", "HEAD", "refs/heads/trunk"]);
    // Picker-style fetch fills the window; the default dispatch then reuses it.
    fetch_remote(&r.local, "origin").unwrap();
    let base = resolve(&r.local, None).unwrap();
    assert_eq!(base.reference, "origin/trunk");
    assert_eq!(base.warning, None);
}

#[test]
fn a_branch_pushed_inside_the_window_still_resolves_by_name() {
    let r = repo();
    assert_eq!(
        fetch_remote_outcome(&r.local, "origin"),
        Ok(FetchOutcome::Fetched)
    );
    git(&r.remote, &["branch", "new-release"]);
    let base = resolve(&r.local, Some("origin/new-release")).unwrap();
    assert_eq!(base.reference, "origin/new-release");
    assert!(validate_branch_fresh(&r.local, "origin/new-release").is_ok());
}

#[test]
fn a_background_merge_check_sees_a_remote_merge_the_cached_refs_miss() {
    use tsk_tui::dispatch::{DispatchHost, SystemDispatchHost};
    use tsk_tui::domain::Dispatch;
    let r = repo();
    let worktree = r.root.join("merged-remotely");
    git(
        &r.local,
        &[
            "worktree",
            "add",
            "-b",
            "tsk/remote-merge",
            worktree.to_str().unwrap(),
            "origin/main",
        ],
    );
    git(&worktree, &["commit", "--allow-empty", "-m", "task commit"]);
    git(
        &r.remote,
        &["fetch", r.local.to_str().unwrap(), "tsk/remote-merge"],
    );
    git(&r.remote, &["merge", "--ff-only", "FETCH_HEAD"]);
    let record = Dispatch {
        argv: vec![],
        worktree: worktree.to_string_lossy().into_owned(),
        branch: "tsk/remote-merge".into(),
        base: Some("origin/main".into()),
        base_ref: Some("refs/remotes/origin/main".into()),
        base_commit: None,
        base_remote: Some("origin".into()),
        herdr_workspace_id: "not-used".into(),
        at: std::time::SystemTime::now(),
        cleaned: false,
    };
    let mut host = SystemDispatchHost;
    assert!(
        !host
            .inspect_cleanup_cached(&r.local, &record, false)
            .unwrap()
            .branch_merged,
        "the cached refs predate the remote merge"
    );
    let check = host.begin_merge_check(&r.local, &record).expect("check");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let verdict = loop {
        if let Some(verdict) = check.take() {
            break verdict;
        }
        assert!(std::time::Instant::now() < deadline, "check did not land");
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert!(verdict.branch_merged && verdict.base_available);
    assert_eq!(verdict.warning, None);
    assert_eq!(verdict.unreachable_remote, None);
}

#[cfg(unix)]
#[test]
fn two_tsk_processes_on_one_state_dir_share_one_fetch() {
    let r = repo();
    let counter = r.root.join("upload-count");
    let upload = r.root.join("counted-upload");
    stub::write_stub(
        &upload,
        format!(
            "#!/bin/sh\nprintf 'fetch\\n' >> '{}'\nexec git-upload-pack \"$@\"\n",
            counter.display()
        ),
        0o700,
    );
    git(
        &r.local,
        &[
            "config",
            "remote.origin.uploadpack",
            upload.to_str().unwrap(),
        ],
    );
    let state = r.root.join("state");
    for title in ["first", "second"] {
        let output = Command::new(env!("CARGO_BIN_EXE_tsk"))
            .args(["add", "--state-dir"])
            .arg(&state)
            .arg("-p")
            .arg(&r.local)
            .args(["-t", title, "--base", "origin/main"])
            .env("TSK_NO_UPDATE_CHECK", "1")
            .env(STRETCH_DEADLINES_ENV, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(state.join("fetch-stamps.json").exists());
    assert_eq!(
        std::fs::read_to_string(&counter).unwrap().lines().count(),
        1,
        "the second process reuses the first one's fetch"
    );
}

#[cfg(unix)]
#[test]
fn inherited_git_config_entries_survive_the_remote_default_refresh() {
    let r = repo();
    git(&r.remote, &["branch", "trunk"]);
    git(&r.remote, &["symbolic-ref", "HEAD", "refs/heads/trunk"]);
    let counter = r.root.join("inherited-upload-count");
    let upload = r.root.join("inherited-upload");
    stub::write_stub(
        &upload,
        format!(
            "#!/bin/sh\nprintf 'fetch\\n' >> '{}'\nexec git-upload-pack \"$@\"\n",
            counter.display()
        ),
        0o700,
    );
    let state = r.root.join("state");
    // The caller's own entry 0 routes upload-pack; tsk must append, not overwrite it.
    let output = Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(["add", "--state-dir"])
        .arg(&state)
        .arg("-p")
        .arg(&r.local)
        .args(["-t", "inherited", "--base", "origin/main"])
        .env("TSK_NO_UPDATE_CHECK", "1")
        .env(STRETCH_DEADLINES_ENV, "1")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "remote.origin.uploadpack")
        .env("GIT_CONFIG_VALUE_0", &upload)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        std::fs::read_to_string(&counter).is_ok_and(|text| text.lines().count() >= 1),
        "the inherited config entry was honoured"
    );
    assert_eq!(
        git(&r.local, &["symbolic-ref", "refs/remotes/origin/HEAD"]),
        "refs/remotes/origin/trunk",
        "and the remote default was refreshed alongside it"
    );
}
