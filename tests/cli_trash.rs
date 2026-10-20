//! Trash CLI: `tsk list --deleted` includes trash; `tsk trash restore` round-trips (AC-15).
//! Uses temp dirs only; never writes real plugin state.

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tsk_tui::cli::{run_with, CliOutput};
use tsk_tui::domain::{HumanStatus, TaskEventKind, TaskScope};
use tsk_tui::store::TaskStore;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_state_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tsk-cli-trash-{label}-{nanos}-{seq}"));
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

fn list_deleted(dir: &Path) -> CliOutput {
    cli(vec![
        "tsk".into(),
        "list".into(),
        "--deleted".into(),
        "--desk".into(),
        "--state-dir".into(),
        dir.to_string_lossy().into_owned(),
    ])
}

fn restore(dir: &Path, address: &str) -> CliOutput {
    cli(vec![
        "tsk".into(),
        "trash".into(),
        "restore".into(),
        address.into(),
        "--state-dir".into(),
        dir.to_string_lossy().into_owned(),
    ])
}

/// Trash one task through the real save boundary: soft-delete it, then complete
/// another so the delete is no longer the top undo entry.
fn trash_task(dir: &Path, deleted_title: &str, other_title: &str) -> uuid::Uuid {
    let store = TaskStore::new(dir);
    let mut state = store.load().expect("load");
    let deleted = state
        .tasks()
        .iter()
        .find(|task| task.title == deleted_title)
        .expect("deleted task exists")
        .id;
    let other = state
        .tasks()
        .iter()
        .find(|task| task.title == other_title)
        .expect("other task exists")
        .id;
    state.soft_delete(deleted).expect("soft delete");
    state.complete(other).expect("later action");
    store.save(&state).expect("save");
    deleted
}

fn trash_line_count(dir: &Path) -> usize {
    match fs::read_to_string(dir.join("trash.jsonl")) {
        Ok(content) => content.lines().count(),
        Err(_) => 0,
    }
}

#[test]
fn list_deleted_shows_a_trashed_task_with_its_number() {
    let dir = temp_state_dir("list-shows-trash");
    let _guard = TempDirGuard(dir.clone());
    for title in ["one", "two", "three"] {
        let output = add_task(&dir, title);
        assert_eq!(output.code, 0, "add {title}: {}", output.stderr);
    }
    trash_task(&dir, "two", "three");

    let output = list_deleted(&dir);
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert!(
        output.stdout.contains("2 two"),
        "the trashed task keeps its number:\n{}",
        output.stdout
    );
}

#[test]
fn trash_restore_round_trip_refusals_and_events() {
    let dir = temp_state_dir("restore-round-trip");
    let _guard = TempDirGuard(dir.clone());
    for title in ["one", "two", "three"] {
        let output = add_task(&dir, title);
        assert_eq!(output.code, 0, "add {title}: {}", output.stderr);
    }
    let deleted_id = trash_task(&dir, "two", "three");
    assert_eq!(trash_line_count(&dir), 1);

    let output = restore(&dir, "T2");
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert!(
        output.stdout.contains("T2") && output.stdout.contains("two"),
        "restore names the task:\n{}",
        output.stdout
    );

    // The trash line is gone and the task is live again: same status as before
    // delete, not soft-deleted, with a restored event, the same number, and a
    // fresh revision.
    assert_eq!(trash_line_count(&dir), 0);
    let store = TaskStore::new(&dir);
    let state = store.load().expect("load");
    let task = state.get(deleted_id).expect("task is live");
    assert!(!task.soft_deleted, "restored is not soft-deleted");
    assert_eq!(
        task.status,
        HumanStatus::Open,
        "restored with its prior status"
    );
    assert_eq!(task.number, Some(2), "the number survives the round trip");
    assert!(
        task.history
            .iter()
            .any(|event| event.kind == TaskEventKind::Restored),
        "a restored event is journaled"
    );

    let output = list_deleted(&dir);
    assert_eq!(output.code, 0);
    assert!(
        !output.stdout.contains("2 two"),
        "the restored task no longer lists as deleted:\n{}",
        output.stdout
    );

    // Refusals: a live number and an unknown number both refuse non-zero.
    let output = restore(&dir, "T2");
    assert_eq!(output.code, 1, "a live number is not in trash");
    assert!(
        output.stderr.contains("T2") && output.stderr.contains("not in trash"),
        "{}",
        output.stderr
    );
    let output = restore(&dir, "T99");
    assert_eq!(output.code, 1, "an unknown number is not in trash");
    assert!(output.stderr.contains("T99"), "{}", output.stderr);

    // Usage errors.
    let output = cli(vec![
        "tsk".into(),
        "trash".into(),
        "restore".into(),
        "--state-dir".into(),
        dir.to_string_lossy().into_owned(),
    ]);
    assert_eq!(output.code, 2, "a missing operand is usage");
    let output = cli(vec!["tsk".into(), "trash".into(), "bogus".into()]);
    assert_eq!(output.code, 2, "an unknown trash action is usage");
    let output = cli(vec!["tsk".into(), "trash".into(), "--help".into()]);
    assert_eq!(output.code, 0, "trash help exits zero");
    assert!(output.stdout.contains("restore"), "{}", output.stdout);
}

#[test]
fn restore_accepts_the_bare_number_and_the_uuid() {
    let dir = temp_state_dir("restore-addresses");
    let _guard = TempDirGuard(dir.clone());
    for title in ["one", "two"] {
        assert_eq!(add_task(&dir, title).code, 0);
    }
    let deleted_id = trash_task(&dir, "one", "two");

    // The bare number works too.
    let output = restore(&dir, "1");
    assert_eq!(output.code, 0, "{}", output.stderr);

    // Trash it again, then restore by UUID.
    trash_task(&dir, "one", "two");
    let output = cli(vec![
        "tsk".into(),
        "trash".into(),
        "restore".into(),
        deleted_id.to_string(),
        "--state-dir".into(),
        dir.to_string_lossy().into_owned(),
    ]);
    assert_eq!(output.code, 0, "{}", output.stderr);
}

#[test]
fn restore_refuses_when_the_task_is_already_live() {
    // Crash-duplication shape: the task is in both places; the live copy wins,
    // so restore refuses instead of inserting a second copy.
    let dir = temp_state_dir("restore-live-duplicate");
    let _guard = TempDirGuard(dir.clone());
    assert_eq!(add_task(&dir, "one").code, 0);

    let store = TaskStore::new(&dir);
    let state = store.load().expect("load");
    let task = state.tasks().first().expect("task").clone();
    let line = serde_json::json!({
        "deleted_at": [1u64, 0u32],
        "task": serde_json::to_value(&task).expect("serialize task"),
    });
    fs::write(dir.join("trash.jsonl"), format!("{line}\n")).expect("seed duplicate line");

    let output = restore(&dir, "T1");
    assert_eq!(output.code, 1, "a live task is not in trash");
    assert!(
        output.stderr.contains("T1") && output.stderr.contains("not in trash"),
        "{}",
        output.stderr
    );
    let state = store.load().expect("reload");
    assert_eq!(state.tasks().len(), 1, "no second copy is inserted");
}

#[test]
fn list_deleted_dedupes_duplicate_trash_lines() {
    let dir = temp_state_dir("list-dedupes");
    let _guard = TempDirGuard(dir.clone());
    for title in ["one", "two"] {
        assert_eq!(add_task(&dir, title).code, 0);
    }
    trash_task(&dir, "one", "two");

    // Append a duplicate line for the same task with a later deleted_at.
    let store = TaskStore::new(&dir);
    let mut duplicate = store.load_trash().expect("load trash")[0].clone();
    duplicate.deleted_at += std::time::Duration::from_secs(60);
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(dir.join("trash.jsonl"))
        .expect("open trash");
    writeln!(
        file,
        "{}",
        serde_json::to_string(&duplicate).expect("encode")
    )
    .expect("append");

    let output = list_deleted(&dir);
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(
        output.stdout.matches("1 one").count(),
        1,
        "duplicate trash lines list once:\n{}",
        output.stdout
    );
}

#[test]
fn list_deleted_skips_malformed_trash_lines() {
    let dir = temp_state_dir("list-skips-malformed");
    let _guard = TempDirGuard(dir.clone());
    for title in ["one", "two"] {
        assert_eq!(add_task(&dir, title).code, 0);
    }
    trash_task(&dir, "one", "two");
    fs::write(
        dir.join("trash.jsonl"),
        format!(
            "{{\"torn\": tru\n{}",
            fs::read_to_string(dir.join("trash.jsonl")).expect("read trash")
        ),
    )
    .expect("prepend a malformed line");

    let output = list_deleted(&dir);
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert!(output.stdout.contains("1 one"), "{}", output.stdout);
    assert!(!output.stdout.contains("torn"), "{}", output.stdout);
}

#[test]
fn restore_notice_by_uuid_prints_n_id_and_does_not_panic() {
    let dir = temp_state_dir("restore-notice");
    let _guard = TempDirGuard(dir.clone());
    assert_eq!(add_task(&dir, "ordinary").code, 0);

    let store = TaskStore::new(&dir);
    let mut state = store.load().expect("load");
    let notice_id = state
        .create_notice(
            "announce.1",
            "What's new in tsk",
            Some("notes".into()),
            HumanStatus::Ready,
            TaskScope::Global,
            Vec::new(),
        )
        .expect("notice");
    let ordinary = state
        .tasks()
        .iter()
        .find(|task| task.title == "ordinary")
        .expect("ordinary")
        .id;
    state.soft_delete(notice_id).expect("soft delete notice");
    state.complete(ordinary).expect("flush undo");
    store.save(&state).expect("save");
    assert_eq!(trash_line_count(&dir), 1);

    let output = restore(&dir, &notice_id.to_string());
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert!(
        output.stdout.contains("N1") && output.stdout.contains("What's new in tsk"),
        "restore paints the notice id:\n{}",
        output.stdout
    );
    let state = store.load().expect("reload");
    let restored = state.get(notice_id).expect("live");
    assert!(!restored.soft_deleted);
    assert!(restored.is_notice());
    assert_eq!(restored.number, None);
    assert_eq!(restored.board_identifier().as_deref(), Some("N1"));
}

#[test]
fn restore_renumbers_a_task_whose_number_a_live_task_holds() {
    let elsewhere = temp_state_dir("renumber-source");
    let _elsewhere = TempDirGuard(elsewhere.clone());
    for title in ["one", "two", "three"] {
        assert_eq!(add_task(&elsewhere, title).code, 0);
    }
    trash_task(&elsewhere, "two", "three");
    // This board already has its own T2 when the trash line carrying T2 arrives.
    let dir = temp_state_dir("renumber");
    let _guard = TempDirGuard(dir.clone());
    for title in ["first", "second"] {
        assert_eq!(add_task(&dir, title).code, 0);
    }
    fs::copy(elsewhere.join("trash.jsonl"), dir.join("trash.jsonl")).expect("copy trash");

    let output = restore(&dir, "T2");
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(
        output.stdout,
        "restored T3 two (was T2: another task holds that number)\n"
    );
    let state = TaskStore::new(&dir).load().expect("load");
    let mut numbers: Vec<_> = state
        .tasks()
        .iter()
        .filter_map(|task| task.number)
        .collect();
    numbers.sort_unstable();
    assert_eq!(numbers, [1, 2, 3], "no two tasks share a number");
}
