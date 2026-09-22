//! Edit CLI: `tsk edit T<n> --title/--notes`. Uses temp dirs only.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tsk_tui::cli::{run_with, CliOutput};
use tsk_tui::domain::{DomainState, ProvenanceOrigin, TaskEventKind, TaskScope};
use tsk_tui::store::TaskStore;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_state_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tsk-cli-edit-{label}-{nanos}-{seq}"));
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
        "-n".into(),
        "original notes".into(),
        "--desk".into(),
        "--state-dir".into(),
        dir.to_string_lossy().into_owned(),
    ])
}

fn edit(dir: &Path, args: &[&str]) -> CliOutput {
    let mut command = vec![
        "tsk".into(),
        "edit".into(),
        "--state-dir".into(),
        dir.to_string_lossy().into_owned(),
    ];
    command.extend(args.iter().map(|argument| (*argument).to_string()));
    cli(command)
}

#[test]
fn edit_assigns_and_unassigns_only_known_profiles() {
    let dir = temp_state_dir("assignee");
    let _guard = TempDirGuard(dir.clone());
    fs::write(
        dir.join("agents.toml"),
        "[agent.reviewer]\ncommand = [\"true\"]\n",
    )
    .expect("write profiles");
    assert_eq!(add_task(&dir, "assign me").code, 0);

    let assigned = edit(&dir, &["T1", "--assignee", "Reviewer"]);
    assert_eq!(assigned.code, 0, "{}", assigned.stderr);
    assert_eq!(
        TaskStore::new(&dir).load().expect("load").tasks()[0]
            .assignee
            .as_deref(),
        Some("reviewer")
    );

    let unknown = edit(&dir, &["T1", "--assignee", "missing"]);
    assert_eq!(unknown.code, 1);
    assert!(
        unknown.stderr.contains("unknown-agent"),
        "{}",
        unknown.stderr
    );

    let unassigned = edit(&dir, &["T1", "--unassign"]);
    assert_eq!(unassigned.code, 0, "{}", unassigned.stderr);
    assert_eq!(
        TaskStore::new(&dir).load().expect("reload").tasks()[0].assignee,
        None
    );
}

#[test]
fn edit_loads_malformed_agent_profiles_only_for_assignment() {
    let dir = temp_state_dir("malformed-agents");
    let _guard = TempDirGuard(dir.clone());
    fs::write(
        dir.join("agents.toml"),
        "[agent.Reviewer]\ncommand = [\"true\"]\n",
    )
    .expect("write malformed profiles");
    assert_eq!(add_task(&dir, "edit me").code, 0);

    let ordinary = edit(&dir, &["T1", "--title", "edited without profiles"]);
    assert_eq!(ordinary.code, 0, "{ordinary:?}");
    let assigned = edit(&dir, &["T1", "--assignee", "reviewer"]);
    assert_eq!(assigned.code, 2, "{assigned:?}");
    assert!(assigned.stderr.contains("agents.toml"), "{assigned:?}");
    let state = TaskStore::new(&dir).load().expect("load state");
    assert_eq!(state.tasks()[0].title, "edited without profiles");
    assert_eq!(state.tasks()[0].assignee, None);
}

#[test]
fn edit_title_and_notes_and_repeat_is_idempotent() {
    let dir = temp_state_dir("round-trip");
    let _guard = TempDirGuard(dir.clone());
    assert_eq!(add_task(&dir, "old title").code, 0);

    let titled = edit(&dir, &["T1", "--title", "  new title  "]);
    assert_eq!(titled.code, 0, "{:?}", titled.stderr);
    assert_eq!(titled.stdout, "edited T1 new title\n");
    let state = TaskStore::new(&dir).load().expect("load");
    let task = &state.tasks()[0];
    assert_eq!(task.title, "new title");
    assert_eq!(task.notes.as_deref(), Some("original notes"));

    let noted = edit(&dir, &["T1", "--notes", "updated notes"]);
    assert_eq!(noted.code, 0, "{:?}", noted.stderr);
    let state = TaskStore::new(&dir).load().expect("load");
    let task = &state.tasks()[0];
    assert_eq!(task.title, "new title");
    assert_eq!(task.notes.as_deref(), Some("updated notes"));

    let events = task
        .history
        .iter()
        .filter(|event| event.kind == TaskEventKind::Edited)
        .count();
    let again = edit(
        &dir,
        &["T1", "--title", "new title", "--notes", "updated notes"],
    );
    assert_eq!(again.code, 0, "{:?}", again.stderr);
    assert_eq!(again.stdout, "edited T1 new title\n");
    let state = TaskStore::new(&dir).load().expect("load");
    let after = state.tasks()[0]
        .history
        .iter()
        .filter(|event| event.kind == TaskEventKind::Edited)
        .count();
    assert_eq!(after, events, "a repeat edit writes no second event");
}

#[test]
fn edit_clears_notes_and_keeps_scope_and_thread() {
    let dir = temp_state_dir("preserve");
    let _guard = TempDirGuard(dir.clone());
    let mut state = DomainState::new();
    let id = state
        .create(
            "keep scope",
            Some("drop these".into()),
            TaskScope::Project {
                path: "/tmp/edit-cli-project".into(),
            },
            ProvenanceOrigin::Manual,
            Some("v6".into()),
        )
        .expect("seed");
    TaskStore::new(&dir).save(&state).expect("save seed");
    let number = TaskStore::new(&dir)
        .load()
        .expect("reload")
        .get(id)
        .expect("task")
        .number
        .expect("number");

    let output = edit(&dir, &[&format!("T{number}"), "--notes="]);
    assert_eq!(output.code, 0, "{:?}", output.stderr);
    let task = TaskStore::new(&dir).load().expect("load").tasks()[0].clone();
    assert!(
        task.notes.is_none(),
        "whitespace-only notes clear the field"
    );
    assert_eq!(
        task.scope,
        TaskScope::Project {
            path: "/tmp/edit-cli-project".into(),
        }
    );
    assert_eq!(task.thread.as_deref(), Some("v6"));
    assert_eq!(task.title, "keep scope");
}

#[test]
fn edit_notes_keeps_newlines_and_tabs_like_add() {
    let dir = temp_state_dir("multiline");
    let _guard = TempDirGuard(dir.clone());
    assert_eq!(add_task(&dir, "keep me").code, 0);
    let notes = "line one\nline\ttwo";
    let output = edit(&dir, &["T1", "--notes", notes]);
    assert_eq!(output.code, 0, "{:?}", output.stderr);
    let task = TaskStore::new(&dir).load().expect("load").tasks()[0].clone();
    assert_eq!(task.notes.as_deref(), Some(notes));
}

#[test]
fn edit_refuses_empty_title_and_control_chars_without_mutation() {
    let dir = temp_state_dir("refusals");
    let _guard = TempDirGuard(dir.clone());
    assert_eq!(add_task(&dir, "keep me").code, 0);
    let state_file = dir.join("tsk.json");
    let before = fs::read(&state_file).expect("read seeded state");

    for (args, token) in [
        (["T1", "--title", "   "].as_slice(), "empty-title"),
        (["T1", "--title", "line\nbreak"].as_slice(), "invalid-title"),
    ] {
        let output = edit(&dir, args);
        assert_eq!(output.code, 1, "{token}: {:?}", output.stderr);
        assert!(output.stdout.is_empty());
        assert!(output.stderr.contains(token), "{token}: {}", output.stderr);
        assert_eq!(
            fs::read(&state_file).expect("read after refusal"),
            before,
            "{token} must persist nothing"
        );
    }
}

#[test]
fn edit_unknown_and_deleted_refuse() {
    let dir = temp_state_dir("missing");
    let _guard = TempDirGuard(dir.clone());
    let missing = edit(&dir, &["T9", "--title", "nope"]);
    assert_eq!(missing.code, 1);
    assert_eq!(
        missing.stderr,
        "tsk edit: unknown-task: T9 is not on the board\n"
    );

    assert_eq!(add_task(&dir, "delete me").code, 0);
    let mut state = TaskStore::new(&dir).load().expect("load");
    let id = state.tasks()[0].id;
    state.soft_delete(id).expect("soft delete");
    TaskStore::new(&dir).save(&state).expect("save deleted");
    let deleted = edit(&dir, &["T1", "--title", "nope"]);
    assert_eq!(deleted.code, 1);
    assert_eq!(
        deleted.stderr,
        "tsk edit: soft-deleted-task: T1 is deleted\n"
    );
}

#[test]
fn edit_without_fields_is_usage() {
    let dir = temp_state_dir("usage");
    let _guard = TempDirGuard(dir.clone());
    assert_eq!(add_task(&dir, "keep me").code, 0);
    let output = edit(&dir, &["T1"]);
    assert_eq!(output.code, 2);
    assert!(output
        .stderr
        .contains("title, notes, assignee, or --unassign is required"));
}

#[test]
fn edit_help_names_title_notes_and_equals_forms() {
    let output = cli(vec!["tsk".into(), "edit".into(), "--help".into()]);
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert!(output.stderr.is_empty());
    for term in [
        "usage: tsk edit",
        "--title",
        "--notes",
        "--title=<value>",
        "--notes=<value>",
        "Values",
        "Refusals (exit 1):",
        "empty-title",
        "invalid-title",
        "Exit:",
        "0 fields written",
        "2 usage, nothing persisted",
        "3 store I/O",
    ] {
        assert!(
            output.stdout.contains(term),
            "edit help should contain {term:?}"
        );
    }
}
