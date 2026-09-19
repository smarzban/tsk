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
}

impl DispatchHost for FakeHost {
    fn is_git_repo(&mut self, _project: &Path) -> Result<bool, String> {
        Ok(true)
    }

    fn create_worktree(
        &mut self,
        _project: &Path,
        branch: &str,
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
        &mut FakeHost::default(),
    )
    .expect("dispatch");
    assert_eq!(result.number, 1);
    let saved = store.load().expect("reload");
    let task = saved.tasks().first().expect("task");
    assert_eq!(task.status, HumanStatus::Started);
    assert_eq!(
        task.dispatch.as_ref().expect("record").herdr_workspace_id,
        "workspace-1"
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
            worktree_exists: true,
            dirty: false,
            branch_merged: true,
            workspace_exists: true,
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
        "tsk dispatch: no-assignee: no agent assigned, use !a name\n"
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
