//! Check CLI: `tsk check <task> <n> <state>` sets one check of the open review round.
//! Uses temp dirs only.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::backend::TestBackend;
use ratatui::Terminal;
use tsk_tui::cli::{run_with, CliOutput};
use tsk_tui::domain::TaskEventKind;
use tsk_tui::store::TaskStore;
use tsk_tui::ui::board::{apply_intent, draw_board, BoardModel};
use tsk_tui::ui::input::BoardIntent;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_state_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tsk-cli-check-{label}-{nanos}-{seq}"));
    fs::create_dir_all(&dir).expect("create temp state dir");
    dir
}

struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn cli(args: &[&str], dir: &Path) -> CliOutput {
    let mut args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    args.extend(["--state-dir".into(), dir.to_string_lossy().into_owned()]);
    run_with(args, Cursor::new(Vec::<u8>::new()), true)
}

/// T1 in review with two checks and one feedback reply.
fn seed_review(dir: &Path) {
    let added = cli(&["tsk", "add", "-t", "build the card", "--desk"], dir);
    assert_eq!(added.code, 0, "{}", added.stderr);
    let review = cli(
        &[
            "tsk",
            "status",
            "T1",
            "review",
            "--done",
            "Opened PR #42",
            "--check",
            "tests pass",
            "--check",
            "no flicker",
        ],
        dir,
    );
    assert_eq!(review.code, 0, "{}", review.stderr);
    let reply = cli(&["tsk", "reply", "T1", "the flicker is back"], dir);
    assert_eq!(reply.code, 0, "{}", reply.stderr);
}

fn check(dir: &Path, task: &str, index: &str, state: &str) -> CliOutput {
    cli(&["tsk", "check", task, index, state], dir)
}

fn listed(dir: &Path) -> serde_json::Value {
    let output = cli(&["tsk", "list", "T1", "--json"], dir);
    assert_eq!(output.code, 0, "{}", output.stderr);
    serde_json::from_str::<serde_json::Value>(&output.stdout).expect("list json")[0].clone()
}

fn check_set_events(dir: &Path) -> usize {
    let state = TaskStore::new(dir).load().expect("load store");
    state.tasks()[0]
        .history
        .iter()
        .filter(|event| event.kind == TaskEventKind::CheckSet)
        .count()
}

#[test]
fn check_sets_each_state_and_keeps_everything_else() {
    let dir = temp_state_dir("states");
    let _guard = TempDirGuard(dir.clone());
    seed_review(&dir);
    let before = listed(&dir);

    for state in ["passed", "failed", "open"] {
        let output = check(&dir, "T1", "2", state);
        assert_eq!(output.code, 0, "{state}: {}", output.stderr);
        assert_eq!(output.stdout, format!("T1 check 2 {state}\n"));
        let task = listed(&dir);
        assert_eq!(task["review"]["feedback"][0]["text"], "the flicker is back");
        assert_eq!(
            task["review"]["checks"],
            serde_json::json!([
                {"text": "tests pass", "state": "open"},
                {"text": "no flicker", "state": state}
            ])
        );
        assert_eq!(task["status"], "review");
        assert_eq!(task["review"]["round"], before["review"]["round"]);
        assert_eq!(task["review"]["done"], before["review"]["done"]);
        assert_eq!(task["review"]["feedback"], before["review"]["feedback"]);
    }
    assert_eq!(
        check_set_events(&dir),
        3,
        "each change writes one check_set"
    );
}

#[test]
fn check_repeat_is_idempotent_and_writes_nothing() {
    let dir = temp_state_dir("repeat");
    let _guard = TempDirGuard(dir.clone());
    seed_review(&dir);
    assert_eq!(check(&dir, "1", "1", "passed").code, 0);
    let before = fs::read(dir.join("tsk.json")).expect("read");

    let again = check(&dir, "t1", "1", "passed");
    assert_eq!(again.code, 0, "{}", again.stderr);
    assert_eq!(again.stdout, "T1 check 1 passed\n");
    assert_eq!(
        fs::read(dir.join("tsk.json")).expect("read"),
        before,
        "a repeat saves nothing"
    );
    assert_eq!(check_set_events(&dir), 1);
}

#[test]
fn check_refusals_change_nothing() {
    let dir = temp_state_dir("refusals");
    let _guard = TempDirGuard(dir.clone());
    seed_review(&dir);
    let added = cli(&["tsk", "add", "-t", "not in review", "--desk"], &dir);
    assert_eq!(added.code, 0, "{}", added.stderr);
    let before = fs::read(dir.join("tsk.json")).expect("read");

    for (args, code, prefix) in [
        (["T9", "1", "passed"], 1, "tsk check: unknown-task: T9 "),
        (["T2", "1", "passed"], 1, "tsk check: not-in-review: T2 "),
        (["T1", "0", "passed"], 1, "tsk check: unknown-check: "),
        (["T1", "3", "passed"], 1, "tsk check: unknown-check: "),
        (
            ["T1", "99999999999999999999999", "passed"],
            1,
            "tsk check: unknown-check: ",
        ),
        (["T1", "1", "done"], 2, "tsk check: "),
        (["T1", "x", "passed"], 2, "tsk check: "),
        (["T1", "-1", "passed"], 2, "tsk check: "),
    ] {
        let output = check(&dir, args[0], args[1], args[2]);
        assert_eq!(output.code, code, "{args:?}: {}", output.stderr);
        assert!(
            output.stderr.starts_with(prefix),
            "{args:?}: {}",
            output.stderr
        );
        assert!(output.stdout.is_empty(), "{args:?}");
    }
    let missing = cli(&["tsk", "check", "T1", "1"], &dir);
    assert_eq!(missing.code, 2, "{}", missing.stderr);
    assert_eq!(fs::read(dir.join("tsk.json")).expect("read"), before);
}

#[test]
fn check_refuses_a_deleted_task() {
    let dir = temp_state_dir("deleted");
    let _guard = TempDirGuard(dir.clone());
    seed_review(&dir);
    let store = TaskStore::new(&dir);
    let mut state = store.load().expect("load");
    let id = state.tasks()[0].id;
    state.soft_delete(id).expect("delete");
    store.save(&state).expect("save");
    let before = fs::read(dir.join("tsk.json")).expect("read");

    let output = check(&dir, "T1", "1", "passed");
    assert_eq!(output.code, 1, "{}", output.stderr);
    assert!(
        output
            .stderr
            .starts_with("tsk check: soft-deleted-task: T1 "),
        "{}",
        output.stderr
    );
    assert_eq!(fs::read(dir.join("tsk.json")).expect("read"), before);
}

/// The board, reloading the store after a `tsk check`, paints the new state on the task page.
#[test]
fn the_board_sees_the_new_check_state_on_reload() {
    let dir = temp_state_dir("reload");
    let _guard = TempDirGuard(dir.clone());
    seed_review(&dir);
    assert_eq!(check(&dir, "T1", "1", "failed").code, 0);

    let mut domain = TaskStore::new(&dir).load().expect("load");
    let id = domain.tasks()[0].id;
    let mut model = BoardModel::from_domain(&domain, None);
    model.sync_from_domain(&domain);
    let index = model
        .visible_ids()
        .iter()
        .position(|visible| *visible == id)
        .expect("task visible");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(index),
        None,
    )
    .expect("select");
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");

    let (width, height) = (100, 60);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let rows: Vec<String> = (0..height)
        .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
        .collect();
    let row = |text: &str| {
        rows.iter()
            .find(|row| row.contains(text))
            .unwrap_or_else(|| panic!("{text:?} missing:\n{}", rows.join("\n")))
            .clone()
    };
    assert!(row("tests pass").contains('✗'), "{}", rows.join("\n"));
    assert!(!row("no flicker").contains('✗'), "{}", rows.join("\n"));
}

/// A check adds no undo entry, like `Enter` on a check on the board.
#[test]
fn check_adds_no_undo_entry() {
    let dir = temp_state_dir("undo");
    let _guard = TempDirGuard(dir.clone());
    seed_review(&dir);
    let undo_stack = || {
        let raw = fs::read_to_string(dir.join("tsk.json")).expect("read");
        serde_json::from_str::<serde_json::Value>(&raw).expect("store json")["undo_stack"].clone()
    };
    let before = undo_stack();
    assert_eq!(check(&dir, "T1", "2", "passed").code, 0);
    assert_eq!(undo_stack(), before);
}
