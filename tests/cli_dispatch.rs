//! Dispatch CLI: routing, parsing, stable refusal codes, and persistence.

use std::fs;
use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};

use std::path::{Path, PathBuf};

use tsk_tui::cli::parser::TaskAddress;
use tsk_tui::cli::run_with;
use tsk_tui::cli::{clean, dispatch};
use tsk_tui::dispatch::{CleanupInspection, CreatedWorktree, DispatchHost, WorktreeCleanup};
use tsk_tui::domain::{DomainState, HumanStatus, ProvenanceOrigin, TaskScope};
use tsk_tui::store::TaskStore;

static SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct FakeHost {
    inspection: Option<CleanupInspection>,
    removed: usize,
    full_ref: Option<String>,
}

impl DispatchHost for FakeHost {
    fn is_git_repo(&mut self, _project: &Path) -> Result<bool, String> {
        Ok(true)
    }

    fn resolve_base(&mut self, _project: &Path) -> Result<String, String> {
        Ok("main".into())
    }

    fn resolve_base_choice(
        &mut self,
        _project: &Path,
        explicit: Option<&str>,
    ) -> Result<tsk_tui::git_base::ResolvedBase, String> {
        Ok(tsk_tui::git_base::ResolvedBase {
            reference: explicit.unwrap_or("main").into(),
            full_ref: self.full_ref.clone(),
            commit: None,
            remote: None,
            warning: None,
        })
    }

    fn create_worktree(
        &mut self,
        _project: &Path,
        branch: &str,
        _base: Option<&str>,
        _label: &str,
    ) -> Result<CreatedWorktree, String> {
        Ok(CreatedWorktree {
            path: PathBuf::from("/tmp/T1-worktree"),
            branch: branch.into(),
            workspace_id: "workspace-1".into(),
            root_pane_id: "pane-1".into(),
        })
    }

    fn inspect_cleanup(
        &mut self,
        _project: &Path,
        _dispatch: &tsk_tui::domain::Dispatch,
        _in_herdr: bool,
    ) -> Result<CleanupInspection, String> {
        self.inspection
            .clone()
            .ok_or_else(|| "inspection missing".into())
    }

    fn remove_herdr_worktree(&mut self, _workspace_id: &str) -> Result<(), String> {
        self.removed += 1;
        Ok(())
    }

    fn delete_branch(&mut self, _project: &Path, _branch: &str) -> Result<(), String> {
        Ok(())
    }

    fn root_pane(&mut self, _workspace_id: &str) -> Result<String, String> {
        Ok("pane-1".into())
    }

    fn run_in_pane(&mut self, _pane_id: &str, _command: &str) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn dispatch_requires_a_task_and_accepts_again() {
    let missing = run_with(["tsk", "dispatch"], Cursor::new(Vec::<u8>::new()), true);
    assert_eq!(missing.code, 2);
    assert!(missing.stderr.contains("task number is required"));

    let help = run_with(
        ["tsk", "dispatch", "--help"],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(help.code, 0, "{}", help.stderr);
    assert!(help.stdout.contains("usage: tsk dispatch <task> [--again]"));
    assert!(help.stdout.contains("already-dispatched"));
}

#[test]
fn successful_cli_dispatch_persists_record_and_started_together() {
    let dir = std::env::temp_dir().join(format!(
        "tsk-cli-dispatch-success-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    fs::write(
        dir.join("agents.toml"),
        "[agent.implementer]\ncommand = [\"runner\", \"{prompt}\"]\n",
    )
    .expect("profiles");
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    state
        .create_assigned(
            "assigned",
            None,
            TaskScope::Project {
                path: "/repos/app".into(),
            },
            ProvenanceOrigin::Manual,
            None,
            Some("implementer".into()),
        )
        .expect("task");
    store.save(&state).expect("save");

    let result = dispatch::run_with_host(
        TaskAddress::Number(1),
        false,
        Some(dir.clone()),
        true,
        &mut FakeHost {
            full_ref: Some("refs/heads/main".into()),
            ..FakeHost::default()
        },
    )
    .expect("dispatch");
    assert_eq!(result.number, 1);
    let saved = store.load().expect("reload");
    let task = saved.tasks().first().expect("task");
    assert_eq!(task.status, HumanStatus::Started);
    let record = task.dispatch.as_ref().expect("record");
    assert_eq!(record.herdr_workspace_id, "workspace-1");
    assert_eq!(record.base.as_deref(), Some("main"));
    assert_eq!(record.base_commit, None);
    assert_eq!(record.base_ref.as_deref(), Some("refs/heads/main"));
    let relaunched = dispatch::run_with_host(
        TaskAddress::Number(1),
        true,
        Some(dir.clone()),
        true,
        &mut FakeHost::default(),
    )
    .unwrap();
    assert_eq!(
        relaunched.record.base_ref.as_deref(),
        Some("refs/heads/main")
    );
    fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn cli_dispatch_one_off_base_reaches_core_without_editing_task_base() {
    let dir = std::env::temp_dir().join(format!(
        "tsk-cli-dispatch-base-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    fs::write(
        dir.join("agents.toml"),
        "[agent.implementer]\ncommand = [\"runner\", \"{prompt}\"]\n",
    )
    .expect("profiles");
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    state
        .create_assigned(
            "assigned",
            None,
            TaskScope::Project {
                path: "/repos/app".into(),
            },
            ProvenanceOrigin::Manual,
            None,
            Some("implementer".into()),
        )
        .expect("task");
    store.save(&state).expect("save");

    let result = dispatch::run_with_host_base(
        TaskAddress::Number(1),
        false,
        Some(dir.clone()),
        true,
        Some("release"),
        &mut FakeHost::default(),
    )
    .expect("dispatch");
    assert_eq!(result.record.base.as_deref(), Some("release"));
    let loaded = store.load().expect("reload");
    let task = &loaded.tasks()[0];
    assert_eq!(task.base, None, "one-off override must not edit the task");
    assert_eq!(
        task.dispatch
            .as_ref()
            .and_then(|record| record.base.as_deref()),
        Some("release")
    );
    fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn clean_cli_help_and_success_report_each_removed_resource() {
    let help = run_with(
        ["tsk", "clean", "--help"],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(help.code, 0, "{}", help.stderr);
    assert!(help.stdout.contains("usage: tsk clean <task> [--json]"));
    for code in [
        "not-dispatched",
        "already-cleaned",
        "dirty-worktree",
        "herdr-failed",
        "store-error",
    ] {
        assert!(help.stdout.contains(code), "missing {code}");
    }

    let dir = std::env::temp_dir().join(format!(
        "tsk-cli-clean-success-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    fs::write(
        dir.join("agents.toml"),
        "[agent.implementer]\ncommand = [\"runner\", \"{prompt}\"]\n",
    )
    .expect("profiles");
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    state
        .create_assigned(
            "assigned",
            None,
            TaskScope::Project {
                path: "/repos/app".into(),
            },
            ProvenanceOrigin::Manual,
            None,
            Some("implementer".into()),
        )
        .expect("task");
    let mut launch = FakeHost::default();
    store.save(&state).expect("save");
    dispatch::run_with_host(
        TaskAddress::Number(1),
        false,
        Some(dir.clone()),
        true,
        &mut launch,
    )
    .expect("dispatch");

    let mut host = FakeHost {
        inspection: Some(CleanupInspection {
            warning: None,
            base_available: true,
            worktree_exists: true,
            dirty: false,
            branch_merged: true,
            workspace_exists: true,
            target_matches: true,
        }),
        ..FakeHost::default()
    };
    let result = clean::run_with_host(TaskAddress::Number(1), Some(dir.clone()), true, &mut host)
        .expect("clean");
    assert_eq!(result.worktree, WorktreeCleanup::Removed);
    assert_eq!(host.removed, 1);
    assert!(
        store.load().unwrap().tasks()[0]
            .dispatch
            .as_ref()
            .unwrap()
            .cleaned
    );
    fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn clean_cli_refusals_print_stable_codes() {
    let dir = std::env::temp_dir().join(format!(
        "tsk-cli-clean-refusal-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    state
        .create(
            "plain",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("task");
    store.save(&state).expect("save");
    let output = run_with(
        ["tsk", "clean", "T1", "--state-dir", dir.to_str().unwrap()],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(output.code, 1);
    assert_eq!(
        output.stderr,
        "tsk clean: not-dispatched: task has no dispatch to clean\n"
    );
    fs::remove_dir_all(dir).expect("cleanup");
}

#[cfg(unix)]
#[test]
fn dispatch_refusals_print_stable_codes_and_persist_nothing() {
    let dir = std::env::temp_dir().join(format!(
        "tsk-cli-dispatch-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    state
        .create(
            "unassigned",
            None,
            TaskScope::Project {
                path: "/repos/app".into(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("task");
    store.save(&state).expect("save");
    let before = fs::read(dir.join("tsk.json")).expect("state bytes");

    let output = run_with(
        [
            "tsk",
            "dispatch",
            "T1",
            "--state-dir",
            dir.to_str().expect("utf-8 path"),
        ],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(output.code, 1);
    assert_eq!(
        output.stderr,
        "tsk dispatch: no-assignee: no agent assigned, use tsk edit T<n> --assignee <name>\n"
    );
    assert_eq!(fs::read(dir.join("tsk.json")).expect("after"), before);

    let unknown = run_with(
        [
            "tsk",
            "dispatch",
            "T9",
            "--state-dir",
            dir.to_str().expect("utf-8 path"),
        ],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(unknown.code, 1);
    assert!(unknown.stderr.starts_with("tsk dispatch: unknown-task:"));
    fs::remove_dir_all(dir).expect("cleanup");
}

#[cfg(windows)]
#[test]
fn dispatch_refuses_on_windows_and_persists_nothing() {
    let dir = std::env::temp_dir().join(format!(
        "tsk-cli-dispatch-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    state
        .create(
            "assigned",
            None,
            TaskScope::Project {
                path: "/repos/app".into(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("task");
    store.save(&state).expect("save");
    let before = fs::read(dir.join("tsk.json")).expect("state bytes");

    let output = run_with(
        [
            "tsk",
            "dispatch",
            "T1",
            "--state-dir",
            dir.to_str().expect("utf-8 path"),
        ],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(output.code, 1);
    assert_eq!(
        output.stderr,
        "tsk dispatch: unsupported-platform: dispatch needs herdr on macOS or Linux\n"
    );
    assert_eq!(fs::read(dir.join("tsk.json")).expect("after"), before);
    fs::remove_dir_all(dir).expect("cleanup");
}

struct CleanupRepo {
    root: PathBuf,
    project: PathBuf,
    worktree: PathBuf,
}

impl CleanupRepo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "tsk-cli-clean-git-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let project = root.join("repo");
        let worktree = root.join("worktree");
        fs::create_dir_all(&project).unwrap();
        let repo = Self {
            root,
            project,
            worktree,
        };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ]);
        repo.git(&["branch", "base"]);
        repo.git(&[
            "worktree",
            "add",
            "-q",
            "-b",
            "tsk/t1-clean",
            repo.worktree.to_str().unwrap(),
            "base",
        ]);
        repo
    }

    fn git(&self, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.project)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn state(&self, base: &str, remote: Option<&str>) -> (DomainState, uuid::Uuid) {
        let mut state = DomainState::new();
        let id = state
            .create(
                "cleanup",
                None,
                TaskScope::Project {
                    path: self.project.to_string_lossy().into(),
                },
                ProvenanceOrigin::Manual,
                None,
            )
            .unwrap();
        let store = TaskStore::new(self.root.join("state"));
        store.save(&state).unwrap();
        state = store.load().unwrap();
        state
            .record_dispatch(
                id,
                tsk_tui::domain::Dispatch {
                    argv: vec!["agent".into()],
                    worktree: self.worktree.to_string_lossy().into(),
                    branch: "tsk/t1-clean".into(),
                    base: Some(base.into()),
                    base_ref: Some(format!(
                        "refs/{}/{}",
                        if remote.is_some() { "remotes" } else { "heads" },
                        base
                    )),
                    base_commit: None,
                    base_remote: remote.map(str::to_string),
                    herdr_workspace_id: "w1".into(),
                    at: std::time::SystemTime::now(),
                    cleaned: false,
                },
            )
            .unwrap();
        (state, id)
    }

    fn clean(&self, base: &str, remote: Option<&str>) -> tsk_tui::dispatch::CleanupResult {
        let (mut state, id) = self.state(base, remote);
        tsk_tui::dispatch::clean_with_host(
            &mut state,
            id,
            false,
            &mut tsk_tui::dispatch::SystemDispatchHost,
        )
        .expect("clean worktree even when base vanished")
    }
}

impl Drop for CleanupRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn cleanup_removes_clean_worktree_when_local_base_vanished() {
    let repo = CleanupRepo::new();
    repo.git(&["branch", "-D", "base"]);
    let result = repo.clean("base", None);
    assert_eq!(result.worktree, WorktreeCleanup::Removed);
    assert_eq!(result.branch, tsk_tui::dispatch::BranchCleanup::Kept);
    assert!(!repo.worktree.exists());
    let output = tsk_tui::cli::presenter::cleaned(result, false);
    assert!(
        output.stdout.contains("base no longer available"),
        "{}",
        output.stdout
    );
    assert!(!output.stdout.contains("squash-merged?"));
}

#[test]
fn cleanup_removes_clean_worktree_when_fetch_prunes_base() {
    let repo = CleanupRepo::new();
    let remote = repo.root.join("remote.git");
    let status = std::process::Command::new("git")
        .args(["init", "--bare", "-q"])
        .arg(&remote)
        .status()
        .unwrap();
    assert!(status.success());
    repo.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
    repo.git(&["push", "-q", "origin", "base"]);
    repo.git(&["config", "fetch.prune", "true"]);
    repo.git(&["push", "-q", "origin", ":base"]);
    // Keep a cached tracking ref until cleanup's fetch prunes it.
    repo.git(&["update-ref", "refs/remotes/origin/base", "base"]);
    let result = repo.clean("origin/base", Some("origin"));
    assert_eq!(result.branch, tsk_tui::dispatch::BranchCleanup::Kept);
    let output = tsk_tui::cli::presenter::cleaned(result, true);
    let value: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    assert!(value["branch"]["reason"]
        .as_str()
        .unwrap()
        .contains("base no longer available"));
}

#[test]
fn cleanup_offline_warning_reaches_human_and_json() {
    for merged in [true, false] {
        let repo = CleanupRepo::new();
        repo.git(&[
            "remote",
            "add",
            "origin",
            repo.root.join("missing.git").to_str().unwrap(),
        ]);
        repo.git(&["update-ref", "refs/remotes/origin/main", "main"]);
        if !merged {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo.worktree)
                .args([
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "user.name=t",
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    "unmerged",
                ])
                .output()
                .unwrap();
            assert!(output.status.success());
        }
        let result = repo.clean("origin/main", Some("origin"));
        let human = tsk_tui::cli::presenter::cleaned(result.clone(), false);
        assert!(
            human
                .stdout
                .contains("merged status computed from cached refs because fetch failed"),
            "{}",
            human.stdout
        );
        let json = tsk_tui::cli::presenter::cleaned(result, true);
        let value: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
        assert!(value["warning"]
            .as_str()
            .unwrap()
            .contains("merged status computed from cached refs because fetch failed"));
    }
}

#[test]
fn cleanup_checked_out_merged_branch_has_truthful_reason() {
    let repo = CleanupRepo::new();
    repo.git(&["checkout", "-q", "tsk/t1-clean", "--ignore-other-worktrees"]);
    let result = repo.clean("base", None);
    assert_eq!(result.branch, tsk_tui::dispatch::BranchCleanup::Kept);
    let human = tsk_tui::cli::presenter::cleaned(result.clone(), false);
    assert!(
        human.stdout.contains("checked out in another worktree"),
        "{}",
        human.stdout
    );
    assert!(!human.stdout.contains("squash-merged?"));
    let json = tsk_tui::cli::presenter::cleaned(result, true);
    assert!(!json.stdout.contains("squash-merged?"));
}

#[test]
fn cleanup_keeps_exact_local_namespace_after_adding_same_named_remote() {
    for merged in [false, true] {
        let repo = CleanupRepo::new();
        repo.git(&["branch", "integration/main", "base"]);
        let (mut state, id) = repo.state("integration/main", None);
        let mut record = state.get(id).unwrap().dispatch.clone().unwrap();
        record.base_ref = Some("refs/heads/integration/main".into());
        if merged {
            record.base = None; // The exact ref is sufficient, the display label is optional.
        }
        state.record_dispatch(id, record).unwrap();
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo.worktree)
            .args([
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "unmerged into local base",
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        if merged {
            repo.git(&["update-ref", "refs/heads/integration/main", "tsk/t1-clean"]);
        }
        let remote = repo.root.join("integration.git");
        assert!(std::process::Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&remote)
            .status()
            .unwrap()
            .success());
        repo.git(&["remote", "add", "integration", remote.to_str().unwrap()]);
        repo.git(&[
            "push",
            "-q",
            "integration",
            if merged {
                "base:main"
            } else {
                "tsk/t1-clean:main"
            },
        ]);
        let result = tsk_tui::dispatch::clean_with_host(
            &mut state,
            id,
            false,
            &mut tsk_tui::dispatch::SystemDispatchHost,
        )
        .unwrap();
        assert_eq!(
            result.branch,
            if merged {
                tsk_tui::dispatch::BranchCleanup::Removed
            } else {
                tsk_tui::dispatch::BranchCleanup::Kept
            }
        );
        assert_eq!(
            result.branch_reason,
            if merged {
                None
            } else {
                Some(tsk_tui::dispatch::BranchRetentionReason::NotMerged)
            }
        );
        assert_eq!(
            result.warning, None,
            "an exact local record must not fetch a newly added remote"
        );
        if !merged {
            repo.git(&["show-ref", "--verify", "refs/heads/tsk/t1-clean"]);
        }
    }
}

struct AdvancingCleanupHost;

impl DispatchHost for AdvancingCleanupHost {
    fn is_git_repo(&mut self, _: &Path) -> Result<bool, String> {
        Ok(true)
    }
    fn create_worktree(
        &mut self,
        _: &Path,
        _: &str,
        _: Option<&str>,
        _: &str,
    ) -> Result<CreatedWorktree, String> {
        unreachable!()
    }
    fn root_pane(&mut self, _: &str) -> Result<String, String> {
        unreachable!()
    }
    fn run_in_pane(&mut self, _: &str, _: &str) -> Result<(), String> {
        unreachable!()
    }
    fn inspect_cleanup(
        &mut self,
        project: &Path,
        record: &tsk_tui::domain::Dispatch,
        in_herdr: bool,
    ) -> Result<CleanupInspection, String> {
        tsk_tui::dispatch::SystemDispatchHost.inspect_cleanup(project, record, in_herdr)
    }
    fn remove_git_worktree(&mut self, project: &Path, worktree: &Path) -> Result<(), String> {
        tsk_tui::dispatch::SystemDispatchHost.remove_git_worktree(project, worktree)?;
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(project)
            .args([
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit-tree",
                "HEAD^{tree}",
                "-p",
                "refs/heads/tsk/t1-clean",
                "-m",
                "advanced after inspection",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let tip = String::from_utf8(output.stdout).unwrap();
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(project)
            .args(["update-ref", "refs/heads/tsk/t1-clean", tip.trim()])
            .output()
            .unwrap();
        assert!(output.status.success());
        Ok(())
    }
    fn delete_merged_branch_with_reason(
        &mut self,
        project: &Path,
        branch: &str,
        base: &str,
    ) -> Result<tsk_tui::dispatch::BranchDeletion, String> {
        tsk_tui::dispatch::SystemDispatchHost
            .delete_merged_branch_with_reason(project, branch, base)
    }
}

#[test]
fn cleanup_advanced_merged_branch_has_truthful_reason() {
    let repo = CleanupRepo::new();
    let (mut state, id) = repo.state("base", None);
    let result =
        tsk_tui::dispatch::clean_with_host(&mut state, id, false, &mut AdvancingCleanupHost)
            .unwrap();
    assert_eq!(result.branch, tsk_tui::dispatch::BranchCleanup::Kept);
    assert_eq!(
        result.branch_reason,
        Some(tsk_tui::dispatch::BranchRetentionReason::LatestTipNotMerged)
    );
    for json in [true, false] {
        let output = tsk_tui::cli::presenter::cleaned(result.clone(), json);
        assert!(
            output.stdout.contains("changed since inspection"),
            "{}",
            output.stdout
        );
        assert!(!output.stdout.contains("squash-merged?"));
    }
    repo.git(&["show-ref", "--verify", "refs/heads/tsk/t1-clean"]);
}

#[cfg(unix)]
#[test]
fn cleanup_git_status_is_bounded_even_when_fsmonitor_stalls() {
    use std::os::unix::fs::PermissionsExt;
    let repo = CleanupRepo::new();
    let hook = repo.root.join("slow-fsmonitor");
    fs::write(&hook, "#!/bin/sh\nsleep 10\nprintf 'token\\0'\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    repo.git(&["config", "core.fsmonitor", hook.to_str().unwrap()]);
    let (state, id) = repo.state("base", None);
    let record = state.get(id).unwrap().dispatch.as_ref().unwrap();
    let start = std::time::Instant::now();
    let result =
        tsk_tui::dispatch::SystemDispatchHost.inspect_cleanup(&repo.project, record, false);
    assert!(result.is_err(), "stalled Git must time out: {result:?}");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "local inspection must have a short deadline"
    );
}
