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
fn status_done_clean_without_a_dispatch_succeeds_with_nothing_to_clean() {
    let dir = temp_state_dir("done-clean-nothing");
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
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(
        output.stdout,
        "status T1 done finish and clean\nnothing to clean\n"
    );
    assert_eq!(output.stderr, "");
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

fn block_args(dir: &Path, task: &str, flags: &[&str]) -> Vec<String> {
    let mut args = vec![
        "tsk".to_string(),
        "status".into(),
        task.into(),
        "blocked".into(),
    ];
    args.extend(flags.iter().map(|flag| flag.to_string()));
    args.extend(["--state-dir".into(), dir.to_string_lossy().into_owned()]);
    args
}

fn reply(dir: &Path, task: &str, text: &str) -> CliOutput {
    cli(vec![
        "tsk".into(),
        "reply".into(),
        task.into(),
        text.into(),
        "--state-dir".into(),
        dir.to_string_lossy().into_owned(),
    ])
}

fn listed(dir: &Path, task: &str) -> serde_json::Value {
    let output = cli(vec![
        "tsk".into(),
        "list".into(),
        task.into(),
        "--json".into(),
        "--state-dir".into(),
        dir.to_string_lossy().into_owned(),
    ]);
    assert_eq!(output.code, 0, "{}", output.stderr);
    serde_json::from_str::<serde_json::Value>(&output.stdout).expect("list json")[0].clone()
}

#[test]
fn blocked_with_reasons_shows_in_list_json_and_a_repeat_edits_the_same_block() {
    let dir = temp_state_dir("block-reasons");
    let _guard = TempDirGuard(dir.clone());
    assert_eq!(add_task(&dir, "pick a db").code, 0);
    let actor = tsk_tui::domain::actor_from_env();

    let output = cli(block_args(
        &dir,
        "T1",
        &[
            "--why",
            "Which database?",
            "--needs",
            "a decision",
            "--option",
            "postgres",
            "--option=-sqlite",
        ],
    ));
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(output.stdout, "status T1 blocked pick a db\n");

    let task = listed(&dir, "T1");
    assert_eq!(task["status"], "blocked");
    let block = &task["block"];
    assert_eq!(block["why"], "Which database?");
    assert_eq!(block["needs"], "a decision");
    assert_eq!(block["options"], serde_json::json!(["postgres", "-sqlite"]));
    assert_eq!(block["on"], "you");
    assert_eq!(block["by"], actor);
    assert_eq!(block["edited"], false);
    assert_eq!(block["replies"], serde_json::json!([]));
    assert_eq!(block["answered"], false);
    assert!(task.get("past_blocks").is_none());

    let output = cli(block_args(
        &dir,
        "T1",
        &["--why", "Which database, really?"],
    ));
    assert_eq!(output.code, 0, "{}", output.stderr);
    let state = TaskStore::new(&dir).load().expect("load");
    let task = &state.tasks()[0];
    assert!(task.past_blocks.is_empty(), "never a second open block");
    let block = task.block.as_ref().expect("open block");
    assert_eq!(block.why.as_deref(), Some("Which database, really?"));
    assert_eq!(
        block.needs.as_deref(),
        Some("a decision"),
        "unset flags stay"
    );
    assert_eq!(block.options.len(), 2);
    assert_eq!(listed(&dir, "T1")["block"]["edited"], true);

    let human = cli(vec![
        "tsk".into(),
        "list".into(),
        "--desk".into(),
        "--state-dir".into(),
        dir.to_string_lossy().into_owned(),
    ]);
    assert!(
        human.stdout.contains("blocked: Which database, really?"),
        "{}",
        human.stdout
    );
}

#[test]
fn plain_blocked_still_works_and_leaving_blocked_closes_the_block() {
    let dir = temp_state_dir("block-plain");
    let _guard = TempDirGuard(dir.clone());
    assert_eq!(add_task(&dir, "plain").code, 0);
    assert_eq!(status(&dir, "T1", "blocked").code, 0);
    let task = listed(&dir, "T1");
    assert_eq!(task["block"]["why"], serde_json::Value::Null);
    assert_eq!(reply(&dir, "T1", "noted").code, 0);
    assert_eq!(listed(&dir, "T1")["block"]["answered"], true);

    assert_eq!(status(&dir, "T1", "ready").code, 0);
    let task = listed(&dir, "T1");
    assert!(task.get("block").is_none());
    let past = &task["past_blocks"][0];
    assert_eq!(past["replies"][0]["text"], "noted");
    assert!(past["closed_at"].is_array());
    assert_eq!(past["closed_by"], tsk_tui::domain::actor_from_env());

    let refused = reply(&dir, "T1", "too late");
    assert_eq!(refused.code, 1);
    assert!(
        refused.stderr.starts_with("tsk reply: not-blocked: "),
        "{}",
        refused.stderr
    );
}

#[test]
fn block_flags_refuse_bad_input_without_mutation() {
    let dir = temp_state_dir("block-refusals");
    let _guard = TempDirGuard(dir.clone());
    assert_eq!(add_task(&dir, "one").code, 0);
    let before = fs::read(dir.join("tsk.json")).expect("read");

    let output = cli(block_args(&dir, "T1", &["--why", &"x".repeat(4097)]));
    assert_eq!(output.code, 1);
    assert!(
        output.stderr.starts_with("tsk status: text-too-long: "),
        "{}",
        output.stderr
    );
    for on in ["T1", "T99"] {
        let output = cli(block_args(&dir, "T1", &["--why", "w", "--on", on]));
        assert_eq!(output.code, 1, "{on}");
        assert!(
            output.stderr.starts_with("tsk status: invalid-blocker: "),
            "{}",
            output.stderr
        );
    }
    let mut args = block_args(&dir, "T1", &["--why", "w"]);
    args[3] = "ready".into();
    assert_eq!(cli(args).code, 2);
    assert_eq!(fs::read(dir.join("tsk.json")).expect("read"), before);

    assert_eq!(reply(&dir, "T1", "x").code, 1);
    assert_eq!(fs::read(dir.join("tsk.json")).expect("read"), before);
}

#[test]
fn blocked_on_another_task_records_it() {
    let dir = temp_state_dir("block-on-task");
    let _guard = TempDirGuard(dir.clone());
    assert_eq!(add_task(&dir, "api").code, 0);
    assert_eq!(add_task(&dir, "client").code, 0);
    let output = cli(block_args(
        &dir,
        "T2",
        &["--why", "needs the api", "--on", "t1"],
    ));
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(listed(&dir, "T2")["block"]["on"], "task:1");
    let output = cli(block_args(&dir, "T2", &["--on", "legal sign-off"]));
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(listed(&dir, "T2")["block"]["on"], "other:legal sign-off");
}

#[test]
fn a_legacy_blocked_task_without_a_block_gains_one_without_a_status_change() {
    let dir = temp_state_dir("block-legacy");
    let _guard = TempDirGuard(dir.clone());
    assert_eq!(add_task(&dir, "old store").code, 0);
    // A v6 store's blocked task carries no block.
    let path = dir.join("tsk.json");
    let mut document: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
    document["tasks"][0]["status"] = serde_json::json!("blocked");
    fs::write(&path, serde_json::to_vec_pretty(&document).expect("encode")).expect("write");
    let (_, status_events) = loaded_status(&dir);

    let output = cli(block_args(&dir, "T1", &["--why", "now with a reason"]));
    assert_eq!(output.code, 0, "{}", output.stderr);
    let state = TaskStore::new(&dir).load().expect("load");
    let task = &state.tasks()[0];
    assert_eq!(task.status, HumanStatus::Blocked);
    assert_eq!(
        task.block.as_ref().and_then(|block| block.why.as_deref()),
        Some("now with a reason")
    );
    assert_eq!(loaded_status(&dir).1, status_events, "no status event");
    assert_eq!(reply(&dir, "T1", "ok").code, 0);
}
