//! Status CLI: `tsk status T<n> <status>`. Uses temp dirs only.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tsk_tui::cli::{run_with, CliOutput};
use tsk_tui::domain::{HumanStatus, TaskEventKind};
use tsk_tui::store::TaskStore;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_state_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tsk-cli-status-{label}-{nanos}-{seq}"));
    fs::create_dir_all(&dir).expect("create temp state dir");
    dir
}

struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn cli(args: Vec<String>) -> CliOutput {
    run_with(args, Cursor::new(Vec::<u8>::new()), true)
}

fn add_task(dir: &Path, title: &str) -> CliOutput {
    cli(vec![
        "tsk".into(),
        "add".into(),
        "-t".into(),
        title.into(),
        "--desk".into(),
        "--state-dir".into(),
        dir.to_string_lossy().into_owned(),
    ])
}

fn status(dir: &Path, task: &str, value: &str) -> CliOutput {
    cli(vec![
        "tsk".into(),
        "status".into(),
        task.into(),
        value.into(),
        "--state-dir".into(),
        dir.to_string_lossy().into_owned(),
    ])
}

fn loaded_status(dir: &Path) -> (HumanStatus, usize) {
    let state = TaskStore::new(dir).load().expect("load store");
    let task = state.tasks().first().expect("seeded task");
    (
        task.status,
        task.history
            .iter()
            .filter(|event| event.kind == TaskEventKind::StatusSet)
            .count(),
    )
}

#[test]
fn status_sets_each_agent_status_and_repeat_is_idempotent() {
    let dir = temp_state_dir("round-trip");
    let _guard = TempDirGuard(dir.clone());
    let added = add_task(&dir, "move me");
    assert_eq!(added.code, 0, "{:?}", added.stderr);

    for value in ["started", "blocked", "review", "ready", "open"] {
        let output = status(&dir, "T1", value);
        assert_eq!(output.code, 0, "{value}: {:?}", output.stderr);
        assert_eq!(output.stdout, format!("status T1 {value} move me\n"));
        let (current, _) = loaded_status(&dir);
        assert_eq!(
            current,
            match value {
                "started" => HumanStatus::Started,
                "blocked" => HumanStatus::Blocked,
                "review" => HumanStatus::Review,
                "ready" => HumanStatus::Ready,
                "open" => HumanStatus::Open,
                _ => unreachable!(),
            }
        );
    }

    let (_, events) = loaded_status(&dir);
    let again = status(&dir, "T1", "open");
    assert_eq!(again.code, 0, "{:?}", again.stderr);
    assert_eq!(again.stdout, "status T1 open move me\n");
    let (current, after) = loaded_status(&dir);
    assert_eq!(current, HumanStatus::Open);
    assert_eq!(after, events, "a repeat status writes no second event");
}

#[test]
fn status_start_is_an_alias_for_started() {
    let dir = temp_state_dir("start-alias");
    let _guard = TempDirGuard(dir.clone());
    assert_eq!(add_task(&dir, "move me").code, 0);

    let output = status(&dir, "T1", "start");
    assert_eq!(output.code, 0, "{:?}", output.stderr);
    assert_eq!(output.stdout, "status T1 started move me\n");
    assert_eq!(loaded_status(&dir).0, HumanStatus::Started);

    let again = status(&dir, "T1", "start");
    assert_eq!(again.code, 0);
    assert_eq!(again.stdout, "status T1 started move me\n");
    assert_eq!(loaded_status(&dir).1, 1, "start then start writes once");
}

#[test]
fn status_done_sets_done_and_repeat_is_idempotent() {
    let dir = temp_state_dir("done");
    let _guard = TempDirGuard(dir.clone());
    assert_eq!(add_task(&dir, "finish me").code, 0);

    let output = status(&dir, "T1", "done");
    assert_eq!(output.code, 0, "{:?}", output.stderr);
    assert_eq!(output.stdout, "status T1 done finish me\n");
    assert_eq!(loaded_status(&dir).0, HumanStatus::Done);

    let again = status(&dir, "T1", "done");
    assert_eq!(again.code, 0);
    assert_eq!(again.stdout, "status T1 done finish me\n");
    assert_eq!(loaded_status(&dir).1, 1, "done then done writes once");
}

#[test]
fn status_unknown_and_deleted_refuse_without_mutation() {
    let dir = temp_state_dir("refusals");
    let _guard = TempDirGuard(dir.clone());
    let missing = status(&dir, "T9", "started");
    assert_eq!(missing.code, 1);
    assert_eq!(
        missing.stderr,
        "tsk status: unknown-task: T9 is not on the board\n"
    );

    assert_eq!(add_task(&dir, "delete me").code, 0);
    let mut state = TaskStore::new(&dir).load().expect("load");
    let id = state.tasks()[0].id;
    state.soft_delete(id).expect("soft delete");
    TaskStore::new(&dir).save(&state).expect("save deleted");
    let state_file = dir.join("tsk.json");
    let before = fs::read(&state_file).expect("read deleted state");

    let deleted = status(&dir, "T1", "started");
    assert_eq!(deleted.code, 1);
    assert_eq!(
        deleted.stderr,
        "tsk status: soft-deleted-task: T1 is deleted\n"
    );
    assert_eq!(fs::read(&state_file).expect("read after"), before);
}

#[test]
fn status_done_clean_persists_done_even_when_cleanup_refuses() {
    let dir = temp_state_dir("done-clean-refusal");
    let _guard = TempDirGuard(dir.clone());
    assert_eq!(add_task(&dir, "finish and clean").code, 0);

    let output = cli(vec![
        "tsk".into(),
        "status".into(),
        "T1".into(),
        "done".into(),
        "--clean".into(),
        "--state-dir".into(),
        dir.to_string_lossy().into_owned(),
    ]);
    assert_eq!(output.code, 1);
    assert_eq!(output.stdout, "status T1 done finish and clean\n");
    assert_eq!(
        output.stderr,
        "tsk status: not-dispatched: task has no dispatch to clean\n"
    );
    assert_eq!(loaded_status(&dir).0, HumanStatus::Done);

    let invalid = cli(vec![
        "tsk".into(),
        "status".into(),
        "T1".into(),
        "ready".into(),
        "--clean".into(),
        "--state-dir".into(),
        dir.to_string_lossy().into_owned(),
    ]);
    assert_eq!(invalid.code, 2);
    assert!(invalid.stderr.contains("--clean requires done status"));
    assert_eq!(loaded_status(&dir).0, HumanStatus::Done);
}

#[test]
fn status_unknown_name_is_usage() {
    let dir = temp_state_dir("usage");
    let _guard = TempDirGuard(dir.clone());
    assert_eq!(add_task(&dir, "keep me").code, 0);
    let output = status(&dir, "T1", "nope");
    assert_eq!(output.code, 2);
    assert!(output.stdout.is_empty());
    assert!(output.stderr.contains("unknown status"));
}

#[test]
fn status_help_names_statuses_and_done_token() {
    let output = cli(vec!["tsk".into(), "status".into(), "--help".into()]);
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert!(output.stderr.is_empty());
    for term in [
        "usage: tsk status",
        "ready",
        "started",
        "start",
        "blocked",
        "review",
        "done",
        "Values",
        "Refusals (exit 1):",
        "Exit:",
        "0 status set",
        "1 status refusal",
        "2 usage, nothing persisted",
        "3 store I/O",
    ] {
        assert!(
            output.stdout.contains(term),
            "status help should contain {term:?}"
        );
    }
}
