use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tsk_tui::cli::{run_with, CliOutput};
use tsk_tui::domain::{
    BlockDraft, BlockOn, CheckState, DomainState, HumanStatus, ProvenanceOrigin, ReviewDraft,
    TaskEventKind, TaskScope,
};
use tsk_tui::store::TaskStore;
use uuid::Uuid;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_state_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tsk-cli-checks-{label}-{nanos}-{seq}"));
    std::fs::create_dir_all(&dir).expect("create state directory");
    dir
}

fn create(state: &mut DomainState, title: &str) -> Uuid {
    state
        .create(
            title,
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("seed task")
}

/// A board with T1 in review (checks A, B, C, all open), T2 blocked, T3 open, T4 deleted in
/// review.
fn seed(dir: &Path) {
    let mut state = DomainState::new();
    let checks = ["A works".to_string(), "B works".into(), "C works".into()];
    let review =
        || ReviewDraft::from_input(Some("built it"), &checks, None, BlockOn::You).expect("draft");
    let in_review = create(&mut state, "in review");
    state
        .review(in_review, review(), "builder")
        .expect("review");
    let blocked = create(&mut state, "blocked");
    state
        .block(
            blocked,
            BlockDraft::from_input(Some("why"), None, &[], BlockOn::You).expect("draft"),
            "builder",
        )
        .expect("block");
    create(&mut state, "open");
    let deleted = create(&mut state, "deleted");
    state.review(deleted, review(), "builder").expect("review");
    state.soft_delete(deleted).expect("delete");
    TaskStore::new(dir).save(&state).expect("seed store");
}

fn store_bytes(dir: &Path) -> Vec<u8> {
    std::fs::read(dir.join("tsk.json")).expect("read store")
}

/// A save replaces the file, so an unchanged modification time means nothing was written.
fn store_mtime(dir: &Path) -> SystemTime {
    std::fs::metadata(dir.join("tsk.json"))
        .and_then(|meta| meta.modified())
        .expect("store mtime")
}

fn checks(dir: &Path, rest: &[&str]) -> CliOutput {
    let args: Vec<String> = ["tsk", "checks"]
        .iter()
        .chain(rest)
        .map(|arg| arg.to_string())
        .chain(["--state-dir".into(), dir.to_string_lossy().into_owned()])
        .collect();
    run_with(args, Cursor::new(Vec::<u8>::new()), true)
}

fn states(dir: &Path, number: u64) -> Vec<CheckState> {
    let state = TaskStore::new(dir).load().expect("load");
    let task = state
        .tasks()
        .iter()
        .find(|task| task.number == Some(number))
        .expect("task");
    task.block
        .as_ref()
        .expect("review round")
        .checks
        .iter()
        .map(|check| check.state)
        .collect()
}

fn undo_len(dir: &Path) -> usize {
    let value: serde_json::Value =
        serde_json::from_slice(&store_bytes(dir)).expect("store is JSON");
    value["undo_stack"].as_array().map_or(0, Vec::len)
}

#[test]
fn set_marks_one_check_by_position_and_echoes_its_text() {
    use CheckState::{Failed, Open, Passed};
    let dir = temp_state_dir("set");
    seed(&dir);
    let undo_before = undo_len(&dir);

    let output = checks(&dir, &["T1", "set", "2", "passed"]);
    assert_eq!(output.code, 0, "{output:?}");
    assert_eq!(output.stdout, "check 2 passed: B works\n");
    assert!(output.stderr.is_empty());
    assert_eq!(states(&dir, 1), [Open, Passed, Open]);

    let output = checks(&dir, &["T1", "set", "3", "failed"]);
    assert_eq!(output.stdout, "check 3 failed: C works\n");
    assert_eq!(states(&dir, 1), [Open, Passed, Failed]);

    let output = checks(&dir, &["1", "set", "2", "open"]);
    assert_eq!(output.stdout, "check 2 open: B works\n");
    assert_eq!(states(&dir, 1), [Open, Open, Failed]);

    // Like Enter on the board, a check set leaves no undo entry and records `check_set`.
    assert_eq!(undo_len(&dir), undo_before);
    let state = TaskStore::new(&dir).load().expect("load");
    let task = state
        .tasks()
        .iter()
        .find(|task| task.number == Some(1))
        .expect("task");
    let last = task.history.last().expect("event");
    assert_eq!(last.kind, TaskEventKind::CheckSet);
    assert_eq!(last.by.as_deref(), Some("you"));
    assert_eq!(task.status, HumanStatus::Review, "status never moves");
}

#[test]
fn setting_the_state_a_check_already_has_succeeds_and_saves_nothing() {
    let dir = temp_state_dir("repeat");
    seed(&dir);
    assert_eq!(checks(&dir, &["T1", "set", "1", "passed"]).code, 0);
    let before = store_bytes(&dir);
    let written = store_mtime(&dir);

    let output = checks(&dir, &["T1", "set", "1", "passed"]);
    assert_eq!(output.code, 0, "{output:?}");
    assert_eq!(output.stdout, "check 1 passed: A works\n");
    assert_eq!(
        store_bytes(&dir),
        before,
        "a repeat must not change the store"
    );
    assert_eq!(
        store_mtime(&dir),
        written,
        "a repeat must not write the store"
    );

    let output = checks(&dir, &["T1", "set", "2", "open"]);
    assert_eq!(output.code, 0, "{output:?}");
    assert_eq!(store_bytes(&dir), before);
    assert_eq!(
        store_mtime(&dir),
        written,
        "an open check set to open must not write"
    );
}

#[test]
fn refusals_exit_1_with_their_code_and_save_nothing() {
    let dir = temp_state_dir("refuse");
    seed(&dir);
    let before = store_bytes(&dir);
    for (rest, line) in [
        (
            &["T99", "set", "1", "passed"][..],
            "tsk checks: unknown-task: T99 is not on the board\n",
        ),
        (
            &["T4", "set", "1", "passed"],
            "tsk checks: soft-deleted-task: T4 is deleted\n",
        ),
        (
            &["T2", "set", "1", "passed"],
            "tsk checks: not-in-review: T2 has no open review round\n",
        ),
        (
            &["T3", "set", "1", "passed"],
            "tsk checks: not-in-review: T3 has no open review round\n",
        ),
        (
            &["T1", "set", "4", "passed"],
            "tsk checks: unknown-check: T1's review round has no check at that position\n",
        ),
    ] {
        let output = checks(&dir, rest);
        assert_eq!(output.code, 1, "{rest:?}: {output:?}");
        assert_eq!(output.stderr, line, "{rest:?}");
        assert!(output.stdout.is_empty(), "{rest:?}");
        assert_eq!(store_bytes(&dir), before, "{rest:?} must not save");
    }
}

#[test]
fn usage_errors_exit_2_and_save_nothing() {
    let dir = temp_state_dir("usage");
    seed(&dir);
    let before = store_bytes(&dir);
    for rest in [
        &["T1", "set", "0", "passed"][..],
        &["T1", "set", "1", "done"],
        &["T1", "set", "1"],
        &["T1", "toggle", "1"],
        &["T1"],
    ] {
        let output = checks(&dir, rest);
        assert_eq!(output.code, 2, "{rest:?}: {output:?}");
        assert!(output.stderr.starts_with("tsk checks: "), "{rest:?}");
        assert!(output.stderr.contains("usage: tsk checks <task> set <n>"));
        assert_eq!(store_bytes(&dir), before, "{rest:?} must not save");
    }
}

/// The binary records the change as the agent named by `TSK_AGENT`, like every other verb.
#[test]
fn an_agent_check_set_is_recorded_as_that_agent() {
    let dir = temp_state_dir("agent");
    seed(&dir);
    let output = Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(["checks", "T1", "set", "1", "failed", "--state-dir"])
        .arg(&dir)
        .env("TSK_AGENT", "claude")
        .output()
        .expect("run tsk");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8"),
        "check 1 failed: A works\n"
    );
    let state = TaskStore::new(&dir).load().expect("load");
    let task = state
        .tasks()
        .iter()
        .find(|task| task.number == Some(1))
        .expect("task");
    let last = task.history.last().expect("event");
    assert_eq!(last.kind, TaskEventKind::CheckSet);
    assert_eq!(last.by.as_deref(), Some("claude"));
}
