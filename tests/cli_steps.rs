use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tsk_tui::cli::run_with;
use tsk_tui::domain::{DomainState, ProvenanceOrigin, TaskScope};
use tsk_tui::store::TaskStore;
use uuid::Uuid;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_state_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tsk-cli-steps-{label}-{nanos}-{seq}"));
    std::fs::create_dir_all(&dir).expect("create state directory");
    dir
}

fn state_dir_arg(dir: &std::path::Path) -> String {
    dir.to_string_lossy().into_owned()
}

fn steps(args: &[String]) -> tsk_tui::cli::CliOutput {
    run_with(args, Cursor::new(Vec::<u8>::new()), true)
}

fn seed_task(dir: &std::path::Path, title: &str) -> Uuid {
    let mut state = DomainState::new();
    let id = state
        .create(
            title,
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("seed task");
    TaskStore::new(dir).save(&state).expect("seed store");
    id
}

fn steps_args(dir: &std::path::Path, task: Uuid) -> Vec<String> {
    vec![
        "tsk".into(),
        "steps".into(),
        task.to_string(),
        "--state-dir".into(),
        state_dir_arg(dir),
    ]
}

fn task_number(dir: &std::path::Path, task: Uuid) -> u64 {
    TaskStore::new(dir)
        .load()
        .expect("load state")
        .get(task)
        .expect("seed task")
        .number
        .expect("persisted task number")
}

/// One step from direct task JSON output.
struct StepLine {
    done: bool,
    short_id: String,
    text: String,
}

fn step_lines(output: &str) -> Vec<StepLine> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(output).expect("direct task JSON");
    rows[0]["steps"]
        .as_array()
        .expect("steps array")
        .iter()
        .map(|step| StepLine {
            done: step["done"].as_bool().expect("step done"),
            short_id: step["short_id"].as_str().expect("step short id").into(),
            text: step["text"].as_str().expect("step text").into(),
        })
        .collect()
}

#[test]
fn steps_add_then_toggle_round_trips_step_state() {
    let dir = temp_state_dir("round-trip");
    let task = seed_task(&dir, "round trip target");

    let mut add_first = steps_args(&dir, task);
    add_first.extend(["add".into(), "  first step  ".into()]);
    let add_first = steps(&add_first);
    assert_eq!(add_first.code, 0, "add first step: {}", add_first.stderr);
    assert!(add_first.stderr.is_empty());

    let mut add_second = steps_args(&dir, task);
    add_second.extend(["add".into(), "second step".into()]);
    let add_second = steps(&add_second);
    assert_eq!(add_second.code, 0, "add second step: {}", add_second.stderr);

    let listed = steps(&[
        "tsk".into(),
        "list".into(),
        task.to_string(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(listed.code, 0, "list task: {}", listed.stderr);
    let lines = step_lines(&listed.stdout);
    assert_eq!(lines.len(), 2, "one line per step: {}", listed.stdout);
    assert!(!lines[0].done);
    assert!(!lines[1].done);
    assert_eq!(
        lines[0].text, "first step",
        "text is trimmed: {}",
        listed.stdout
    );
    assert_eq!(lines[1].text, "second step");

    let mut toggle = steps_args(&dir, task);
    toggle.extend(["toggle".into(), lines[0].short_id.clone()]);
    let toggle = steps(&toggle);
    assert_eq!(toggle.code, 0, "toggle by short id: {}", toggle.stderr);

    let after_toggle = steps(&[
        "tsk".into(),
        "list".into(),
        task.to_string(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    let lines = step_lines(&after_toggle.stdout);
    assert!(lines[0].done, "first step checked: {}", after_toggle.stdout);
    assert!(
        !lines[1].done,
        "second step untouched: {}",
        after_toggle.stdout
    );
    assert_eq!(lines[0].short_id, step_lines(&listed.stdout)[0].short_id);

    let mut toggle_back = steps_args(&dir, task);
    toggle_back.extend(["toggle".into(), lines[0].short_id.clone()]);
    let toggle_back = steps(&toggle_back);
    assert_eq!(toggle_back.code, 0, "toggle back: {}", toggle_back.stderr);

    let state = TaskStore::new(&dir).load().expect("reload store");
    let steps_on_task = &state.tasks()[0].steps;
    assert_eq!(steps_on_task.len(), 2);
    assert!(
        !steps_on_task[0].done,
        "first step round-trips back to open"
    );
    assert!(!steps_on_task[1].done);
    assert_eq!(steps_on_task[0].text, "first step");
    assert_eq!(steps_on_task[1].text, "second step");

    let _ = std::fs::remove_dir_all(dir);
}

/// Two steps whose ids share the prefix `abcdef01`, shaped through the
/// store document because step ids are otherwise minted by the domain.
fn state_with_shared_prefix_steps() -> (DomainState, Uuid) {
    let mut state = DomainState::new();
    let task = state
        .create(
            "prefix target",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("seed task");
    state.add_step(task, "shared one").expect("seed first step");
    state
        .add_step(task, "shared two")
        .expect("seed second step");
    let mut document = serde_json::to_value(&state).expect("serialize seed state");
    for (index, id) in [
        "abcdef01-0000-4000-8000-000000000001",
        "abcdef01-0000-4000-8000-000000000002",
    ]
    .into_iter()
    .enumerate()
    {
        document["tasks"][0]["steps"][index]["id"] =
            serde_json::json!(Uuid::parse_str(id).expect("shaped step id"));
    }
    let shaped = serde_json::from_value(document).expect("state with shaped step ids");
    (shaped, task)
}

#[test]
fn steps_toggle_unknown_or_ambiguous_prefix_refuses_without_mutation() {
    let dir = temp_state_dir("prefix-refusal");
    let (state, task) = state_with_shared_prefix_steps();
    TaskStore::new(&dir).save(&state).expect("seed store");
    let state_file = dir.join("tsk.json");
    let before = std::fs::read(&state_file).expect("read seeded state");

    let mut ambiguous = steps_args(&dir, task);
    ambiguous.extend(["toggle".into(), "abcdef01".into()]);
    let ambiguous = steps(&ambiguous);
    assert_eq!(ambiguous.code, 1);
    assert!(ambiguous.stdout.is_empty());
    assert!(
        ambiguous.stderr.to_lowercase().contains("ambiguous"),
        "ambiguous refusal names ambiguity: {}",
        ambiguous.stderr
    );
    assert_eq!(
        std::fs::read(&state_file).expect("read store after ambiguous"),
        before,
        "ambiguous prefix must persist nothing"
    );

    let mut unknown = steps_args(&dir, task);
    unknown.extend(["toggle".into(), "12341234".into()]);
    let unknown = steps(&unknown);
    assert_eq!(unknown.code, 1);
    assert!(unknown.stdout.is_empty());
    assert!(unknown.stderr.contains("unknown-step"));
    assert_eq!(
        std::fs::read(&state_file).expect("read store after unknown"),
        before,
        "unknown prefix must persist nothing"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn steps_toggle_empty_operand_refuses_without_mutation() {
    let dir = temp_state_dir("empty-toggle");
    let mut state = DomainState::new();
    let task = state
        .create(
            "empty operand target",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("seed task");
    state.add_step(task, "only step").expect("seed one step");
    TaskStore::new(&dir).save(&state).expect("seed store");
    let state_file = dir.join("tsk.json");
    let before = std::fs::read(&state_file).expect("read seeded state");

    let mut empty = steps_args(&dir, task);
    empty.extend(["toggle".into(), String::new()]);
    let empty = steps(&empty);
    assert_eq!(empty.code, 1);
    assert!(empty.stdout.is_empty());
    assert!(
        empty.stderr.contains("unknown-step"),
        "empty operand must refuse with a stable token: {}",
        empty.stderr
    );
    assert_eq!(
        std::fs::read(&state_file).expect("read store after empty operand"),
        before,
        "the empty operand must address no step, not the sole one"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn steps_add_refuses_empty_and_control_char_text_without_mutation() {
    let dir = temp_state_dir("text-refusal");
    let task = seed_task(&dir, "text refusal target");
    let state_file = dir.join("tsk.json");
    let before = std::fs::read(&state_file).expect("read seeded state");

    for (text, token) in [
        ("", "empty-step-text"),
        ("   ", "empty-step-text"),
        // Controls refuse before trimming, same as task titles: a tab is C0.
        ("   \t ", "invalid-step-text"),
        ("line\nbreak", "invalid-step-text"),
    ] {
        let mut args = steps_args(&dir, task);
        args.extend(["add".into(), text.into()]);
        let output = steps(&args);
        assert_eq!(output.code, 1, "refuse {text:?}");
        assert!(output.stdout.is_empty(), "refuse {text:?}");
        assert!(
            output.stderr.contains(token),
            "refuse {text:?} with {token}: {}",
            output.stderr
        );
        assert!(
            !output.stderr.contains("invalid-item\n") && !output.stderr.contains("invalid-item "),
            "step refusals must not reuse the bulk-add invalid-item token: {}",
            output.stderr
        );
    }

    assert_eq!(
        std::fs::read(&state_file).expect("read store after refusals"),
        before,
        "refused step text must persist nothing"
    );
    let state = TaskStore::new(&dir).load().expect("reload store");
    assert!(
        state.tasks()[0].steps.is_empty(),
        "no step may be created by a refused add"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn steps_on_soft_deleted_task_refuses_without_mutation() {
    let dir = temp_state_dir("soft-deleted");
    let mut state = DomainState::new();
    let task = state
        .create(
            "deleted target",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("seed task");
    state.soft_delete(task).expect("soft delete seed");
    TaskStore::new(&dir).save(&state).expect("seed store");
    let state_file = dir.join("tsk.json");
    let before = std::fs::read(&state_file).expect("read seeded state");

    let mut add = steps_args(&dir, task);
    add.extend(["add".into(), "no scripting deleted tasks".into()]);
    let add = steps(&add);
    assert_eq!(add.code, 1);
    assert!(add.stdout.is_empty());
    assert_eq!(
        add.stderr,
        format!("tsk steps: soft-deleted-task: {task} is deleted\n")
    );
    assert_eq!(
        std::fs::read(&state_file).expect("read store after refusal"),
        before,
        "a deleted task's steps are not scriptable"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn steps_bare_digits_act_on_the_task() {
    let dir = temp_state_dir("number-address");
    let task = seed_task(&dir, "number target");
    let number = task_number(&dir, task);

    let output = steps(&[
        "tsk".into(),
        "steps".into(),
        number.to_string(),
        "add".into(),
        "addressed by number".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);

    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(
        TaskStore::new(&dir)
            .load()
            .expect("reload state")
            .get(task)
            .expect("task")
            .steps[0]
            .text,
        "addressed by number"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn steps_uuid_still_acts_on_the_task() {
    let dir = temp_state_dir("uuid-address");
    let task = seed_task(&dir, "uuid target");

    let mut args = steps_args(&dir, task);
    args.extend(["add".into(), "addressed by uuid".into()]);
    let output = steps(&args);

    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(
        TaskStore::new(&dir)
            .load()
            .expect("reload state")
            .get(task)
            .expect("task")
            .steps[0]
            .text,
        "addressed by uuid"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn steps_unknown_number_is_unknown_task() {
    let dir = temp_state_dir("unknown-number");
    let output = steps(&[
        "tsk".into(),
        "steps".into(),
        "999".into(),
        "add".into(),
        "missing target".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);

    assert_eq!(output.code, 1);
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        "tsk steps: unknown-task: T999 is not on the board\n"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn steps_help_uses_the_shared_reference() {
    let output = steps(&["tsk".into(), "steps".into(), "--help".into()]);
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert!(output.stderr.is_empty());
    for term in [
        "usage: tsk steps",
        "toggle",
        "rename",
        "remove",
        "Values",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:",
        "0 step created",
        "1 step refusal",
        "2 usage, nothing persisted",
        "3 store I/O",
    ] {
        assert!(
            output.stdout.contains(term),
            "steps help should contain {term:?}"
        );
    }
}

#[test]
fn steps_rename_then_remove_round_trips() {
    let dir = temp_state_dir("rename-remove");
    let task = seed_task(&dir, "rename target");

    let mut add = steps_args(&dir, task);
    add.extend(["add".into(), "  first draft  ".into()]);
    let add = steps(&add);
    assert_eq!(add.code, 0, "{}", add.stderr);

    let listed = steps(&[
        "tsk".into(),
        "list".into(),
        task.to_string(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    let lines = step_lines(&listed.stdout);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "first draft");

    let mut rename = steps_args(&dir, task);
    rename.extend([
        "rename".into(),
        lines[0].short_id.clone(),
        "  renamed step  ".into(),
    ]);
    let rename = steps(&rename);
    assert_eq!(rename.code, 0, "{}", rename.stderr);
    assert_eq!(
        rename.stdout,
        format!("renamed {} renamed step\n", lines[0].short_id)
    );

    let after_rename = TaskStore::new(&dir)
        .load()
        .expect("reload")
        .get(task)
        .expect("task")
        .steps[0]
        .text
        .clone();
    assert_eq!(after_rename, "renamed step");

    let mut again = steps_args(&dir, task);
    again.extend([
        "rename".into(),
        lines[0].short_id.clone(),
        "renamed step".into(),
    ]);
    let again = steps(&again);
    assert_eq!(again.code, 0, "{}", again.stderr);
    let events = TaskStore::new(&dir)
        .load()
        .expect("reload")
        .get(task)
        .expect("task")
        .history
        .iter()
        .filter(|event| event.kind == tsk_tui::domain::TaskEventKind::StepRenamed)
        .count();
    assert_eq!(events, 1, "a repeat rename writes no second event");

    let mut remove = steps_args(&dir, task);
    remove.extend(["remove".into(), lines[0].short_id.clone()]);
    let remove = steps(&remove);
    assert_eq!(remove.code, 0, "{}", remove.stderr);
    assert_eq!(
        remove.stdout,
        format!("removed {} renamed step\n", lines[0].short_id)
    );
    assert!(TaskStore::new(&dir)
        .load()
        .expect("reload")
        .get(task)
        .expect("task")
        .steps
        .is_empty());

    let mut missing = steps_args(&dir, task);
    missing.extend(["remove".into(), lines[0].short_id.clone()]);
    let missing = steps(&missing);
    assert_eq!(missing.code, 1);
    assert!(missing.stderr.contains("unknown-step"));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn steps_rename_refuses_empty_and_control_char_text_without_mutation() {
    let dir = temp_state_dir("rename-text");
    let task = seed_task(&dir, "rename refusal");
    let mut add = steps_args(&dir, task);
    add.extend(["add".into(), "keep me".into()]);
    assert_eq!(steps(&add).code, 0);
    let listed = steps(&[
        "tsk".into(),
        "list".into(),
        task.to_string(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    let short_id = step_lines(&listed.stdout)[0].short_id.clone();
    let state_file = dir.join("tsk.json");
    let before = std::fs::read(&state_file).expect("read seeded state");

    for (text, token) in [
        ("", "empty-step-text"),
        ("   ", "empty-step-text"),
        ("   \t ", "invalid-step-text"),
        ("line\nbreak", "invalid-step-text"),
    ] {
        let mut args = steps_args(&dir, task);
        args.extend(["rename".into(), short_id.clone(), text.into()]);
        let output = steps(&args);
        assert_eq!(output.code, 1, "refuse {text:?}");
        assert!(output.stderr.contains(token), "{token}: {}", output.stderr);
    }
    assert_eq!(
        std::fs::read(&state_file).expect("read after refusals"),
        before
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Seed a task with steps `a`, `b`, `c` (and `b` checked) and return the store's step ids.
fn seed_three_steps(dir: &std::path::Path) -> (Uuid, Vec<Uuid>) {
    let mut state = DomainState::new();
    let task = state
        .create(
            "order target",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("seed task");
    let ids: Vec<Uuid> = ["a", "b", "c"]
        .into_iter()
        .map(|text| state.add_step(task, text).expect("seed step"))
        .collect();
    state.toggle_step(task, ids[1]).expect("check b");
    TaskStore::new(dir).save(&state).expect("seed store");
    (task, ids)
}

fn stored_texts(dir: &std::path::Path) -> Vec<String> {
    TaskStore::new(dir).load().expect("reload store").tasks()[0]
        .steps
        .iter()
        .map(|step| step.text.clone())
        .collect()
}

fn run_steps(dir: &std::path::Path, task: Uuid, rest: &[&str]) -> tsk_tui::cli::CliOutput {
    let mut args = steps_args(dir, task);
    args.extend(rest.iter().map(|arg| arg.to_string()));
    steps(&args)
}

#[test]
fn steps_add_inserts_before_or_after_a_step() {
    let dir = temp_state_dir("add-placed");
    let (task, ids) = seed_three_steps(&dir);
    let b = ids[1].to_string();

    let before = run_steps(&dir, task, &["add", "before b", "--before", &b]);
    assert_eq!(before.code, 0, "add --before: {}", before.stderr);
    assert!(before.stdout.starts_with("added "), "{}", before.stdout);
    let after = run_steps(&dir, task, &[&format!("--after={b}"), "add", "after b"]);
    assert_eq!(after.code, 0, "add --after: {}", after.stderr);
    let first = run_steps(&dir, task, &["add", "top", "--first"]);
    assert_eq!(first.code, 0, "add --first: {}", first.stderr);
    assert_eq!(
        stored_texts(&dir),
        vec!["top", "a", "before b", "b", "after b", "c"]
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn steps_move_keeps_identity_short_id_and_done() {
    let dir = temp_state_dir("move");
    let (task, ids) = seed_three_steps(&dir);
    let list = || {
        steps(&[
            "tsk".into(),
            "list".into(),
            task.to_string(),
            "--json".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
        ])
    };
    let before = step_lines(&list().stdout);
    let b_short = before[1].short_id.clone();

    let moved = run_steps(&dir, task, &["move", &b_short, "--first"]);
    assert_eq!(moved.code, 0, "move --first: {}", moved.stderr);
    assert_eq!(moved.stdout, format!("moved {b_short} to 1/3 b\n"));
    let after = step_lines(&list().stdout);
    assert_eq!(
        after
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        vec!["b", "a", "c"]
    );
    assert_eq!(after[0].short_id, b_short, "the short id survives a move");
    assert!(after[0].done, "the done flag survives a move");
    let state = TaskStore::new(&dir).load().expect("reload store");
    assert_eq!(state.tasks()[0].steps[0].id, ids[1], "same step identity");

    let a = ids[0].to_string();
    let c = ids[2].to_string();
    let moved = run_steps(&dir, task, &["move", &b_short, "--after", &c]);
    assert_eq!(moved.code, 0, "move --after: {}", moved.stderr);
    assert_eq!(stored_texts(&dir), vec!["a", "c", "b"]);
    let moved = run_steps(&dir, task, &["move", &c, "--before", &a]);
    assert_eq!(moved.code, 0, "move --before: {}", moved.stderr);
    assert_eq!(stored_texts(&dir), vec!["c", "a", "b"]);
    let moved = run_steps(&dir, task, &["move", &c, "--last"]);
    assert_eq!(moved.code, 0, "move --last: {}", moved.stderr);
    assert_eq!(stored_texts(&dir), vec!["a", "b", "c"]);

    // Already in place: exit 0 and nothing written.
    let revision = TaskStore::new(&dir).load().expect("reload").tasks()[0].revision;
    let again = run_steps(&dir, task, &["move", &c, "--after", &ids[1].to_string()]);
    assert_eq!(again.code, 0, "no-op move: {}", again.stderr);
    let c_short = &step_lines(&list().stdout)[2].short_id;
    assert_eq!(again.stdout, format!("moved {c_short} to 3/3 c\n"));
    assert_eq!(
        TaskStore::new(&dir).load().expect("reload").tasks()[0].revision,
        revision,
        "a move onto its own slot persists nothing"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn steps_move_and_placed_add_refuse_without_mutation() {
    let dir = temp_state_dir("move-refusal");
    let (task, ids) = seed_three_steps(&dir);
    let a = ids[0].to_string();

    let same = run_steps(&dir, task, &["move", &a, "--before", &a]);
    assert_eq!(same.code, 1);
    assert_eq!(
        same.stderr,
        "tsk steps: same-step: a step cannot be placed relative to itself\n"
    );
    let unknown_anchor = run_steps(&dir, task, &["move", &a, "--after", "ffffffff"]);
    assert_eq!(unknown_anchor.code, 1);
    assert_eq!(
        unknown_anchor.stderr,
        format!("tsk steps: unknown-step: no step on {task} matches that id\n")
    );
    let unknown_step = run_steps(&dir, task, &["move", "ffffffff", "--first"]);
    assert_eq!(unknown_step.code, 1);
    assert!(unknown_step.stderr.contains("unknown-step"));
    let unknown_add_anchor = run_steps(&dir, task, &["add", "x", "--before", "ffffffff"]);
    assert_eq!(unknown_add_anchor.code, 1);
    assert!(unknown_add_anchor.stderr.contains("unknown-step"));

    let missing_place = run_steps(&dir, task, &["move", &a]);
    assert_eq!(missing_place.code, 2);
    assert!(missing_place
        .stderr
        .starts_with("tsk steps: move needs --before, --after, --first, or --last"));
    let wrong_verb = run_steps(&dir, task, &["toggle", &a, "--first"]);
    assert_eq!(wrong_verb.code, 2);

    assert_eq!(stored_texts(&dir), vec!["a", "b", "c"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn steps_move_resolves_ambiguous_anchor_as_a_refusal() {
    let dir = temp_state_dir("move-ambiguous");
    let (mut state, task) = state_with_shared_prefix_steps();
    state.add_step(task, "third").expect("third step");
    TaskStore::new(&dir).save(&state).expect("seed store");
    let third = state.get(task).expect("task").steps[2].id.to_string();

    let moved = run_steps(&dir, task, &["move", &third, "--before", "abcdef01"]);
    assert_eq!(moved.code, 1);
    assert!(moved.stderr.contains("ambiguous-step"), "{}", moved.stderr);
    assert_eq!(
        stored_texts(&dir),
        vec!["shared one", "shared two", "third"]
    );
    let _ = std::fs::remove_dir_all(dir);
}
