//! Artifact add, remove, and hand-back flags through the real CLI.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tsk_tui::cli::{run_with, CliOutput};
use tsk_tui::domain::{DomainState, HumanStatus, ProvenanceOrigin, TaskScope};
use tsk_tui::store::TaskStore;
use uuid::Uuid;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_state_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tsk-cli-artifacts-{label}-{nanos}-{seq}"));
    std::fs::create_dir_all(&dir).expect("create state directory");
    dir
}

fn seed(dir: &Path) -> Uuid {
    let mut state = DomainState::new();
    let id = state
        .create(
            "attach things",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("seed task");
    let deleted = state
        .create(
            "deleted",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("seed task");
    state.soft_delete(deleted).expect("delete");
    TaskStore::new(dir).save(&state).expect("seed store");
    id
}

fn cli(dir: &Path, verb: &str, rest: &[&str]) -> CliOutput {
    let args: Vec<String> = ["tsk", verb]
        .iter()
        .chain(rest)
        .map(|arg| (*arg).to_string())
        .chain(["--state-dir".into(), dir.to_string_lossy().into_owned()])
        .collect();
    run_with(args, Cursor::new(Vec::<u8>::new()), true)
}

fn json(dir: &Path, task: &str) -> Value {
    let output = cli(dir, "list", &[task, "--json"]);
    assert_eq!(output.code, 0, "{}", output.stderr);
    let rows: Vec<Value> = serde_json::from_str(&output.stdout).expect("json rows");
    rows.into_iter().next().expect("one task")
}

fn artifacts(dir: &Path, task: &str) -> Vec<Value> {
    json(dir, task)["artifacts"]
        .as_array()
        .expect("artifacts array")
        .clone()
}

fn store_bytes(dir: &Path) -> Vec<u8> {
    std::fs::read(dir.join("tsk.json")).expect("read store")
}

#[test]
fn add_is_repeatable_and_a_label_update_changes_only_the_label() {
    let dir = temp_state_dir("add");
    let id = seed(&dir);
    let target = "https://github.com/org/repo/pull/42";

    let first = cli(
        &dir,
        "artifacts",
        &["T1", "add", target, "--label", "PR #42"],
    );
    assert_eq!(first.code, 0, "{}", first.stderr);
    assert!(first.stdout.contains("PR #42") || first.stdout.contains(target));

    let repeat = cli(&dir, "artifacts", &["T1", "add", target]);
    assert_eq!(repeat.code, 0, "{}", repeat.stderr);
    assert!(repeat.stdout.contains("unchanged"), "{}", repeat.stdout);
    assert_eq!(
        artifacts(&dir, "T1").len(),
        1,
        "a repeat does not add another"
    );

    let label = cli(
        &dir,
        "artifacts",
        &["T1", "add", target, "--label=deployment"],
    );
    assert_eq!(label.code, 0, "{}", label.stderr);
    let rows = artifacts(&dir, "T1");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["label"], "deployment");
    assert_eq!(rows[0]["target"], target);
    assert_eq!(rows[0]["round"], 0);
    assert!(rows[0]["by"].as_str().is_some_and(|by| !by.is_empty()));
    assert!(rows[0]["at"].is_array());

    let state = TaskStore::new(&dir).load().expect("load store");
    let task = state.get(id).expect("seeded task");
    assert_eq!(task.artifacts.len(), 1);
    assert!(task
        .history
        .iter()
        .any(|event| event.kind == tsk_tui::domain::TaskEventKind::ArtifactAdded));
}

#[test]
fn add_accepts_an_absolute_path_and_remove_addresses_list_order() {
    let dir = temp_state_dir("remove");
    seed(&dir);
    let path = if cfg!(windows) {
        "C:\\tmp\\shot.png"
    } else {
        "/tmp/shot.png"
    };

    let url = cli(&dir, "artifacts", &["T1", "add", "https://example.com/a"]);
    let file = cli(&dir, "artifacts", &["T1", "add", path]);
    assert_eq!(url.code, 0, "{}", url.stderr);
    assert_eq!(file.code, 0, "{}", file.stderr);

    let removed = cli(&dir, "artifacts", &["T1", "remove", "1"]);
    assert_eq!(removed.code, 0, "{}", removed.stderr);
    let rows = artifacts(&dir, "T1");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["target"], path);

    let before = store_bytes(&dir);
    let missing = cli(&dir, "artifacts", &["T1", "remove", "2"]);
    assert_eq!(missing.code, 1);
    assert!(
        missing.stderr.contains("unknown-artifact"),
        "{}",
        missing.stderr
    );
    assert_eq!(store_bytes(&dir), before, "a refusal writes nothing");
}

#[test]
fn invalid_targets_and_deleted_tasks_refuse() {
    let dir = temp_state_dir("refuse");
    seed(&dir);

    let relative = cli(&dir, "artifacts", &["T1", "add", "docs/readme.md"]);
    assert_eq!(relative.code, 1);
    assert!(
        relative.stderr.contains("invalid-artifact"),
        "{}",
        relative.stderr
    );
    let unknown = cli(&dir, "artifacts", &["T99", "add", "https://example.com/a"]);
    assert_eq!(unknown.code, 1);
    assert!(
        unknown.stderr.contains("unknown-task"),
        "{}",
        unknown.stderr
    );
    let empty_label = cli(
        &dir,
        "artifacts",
        &["T1", "add", "https://example.com/a", "--label= "],
    );
    assert_eq!(empty_label.code, 1);
    assert!(empty_label.stderr.contains("empty-artifact-label"));
    let deleted = cli(&dir, "artifacts", &["T2", "add", "https://example.com/a"]);
    assert_eq!(deleted.code, 1);
    assert!(deleted.stderr.contains("soft-deleted-task"));
    assert!(artifacts(&dir, "T1").is_empty());
}

#[test]
fn status_artifacts_land_on_the_new_round_and_survive_it() {
    let dir = temp_state_dir("rounds");
    seed(&dir);
    let pr = "https://github.com/org/repo/pull/42";
    let shot = if cfg!(windows) {
        "C:\\tmp\\shot.png"
    } else {
        "/tmp/shot.png"
    };

    let review = cli(
        &dir,
        "status",
        &[
            "T1",
            "review",
            "--done",
            "built it",
            "--artifact",
            pr,
            "--artifact",
            shot,
        ],
    );
    assert_eq!(review.code, 0, "{}", review.stderr);
    let rows = artifacts(&dir, "T1");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["round"], 1);
    assert_eq!(rows[1]["round"], 1);
    assert_eq!(rows[0]["target"], pr);
    assert_eq!(rows[1]["target"], shot);

    let state = TaskStore::new(&dir).load().expect("load");
    let task = state.get(seed_id(&dir)).expect("task");
    assert_eq!(task.status, HumanStatus::Review);
    assert_eq!(task.block.as_ref().expect("round").round, 1);
    assert!(task
        .history
        .iter()
        .any(|event| event.kind == tsk_tui::domain::TaskEventKind::ArtifactAdded));

    let next = cli(&dir, "status", &["T1", "started", "--no-dispatch"]);
    assert_eq!(next.code, 0, "{}", next.stderr);
    let next = cli(&dir, "artifacts", &["T1", "remove", "1"]);
    assert_eq!(next.code, 0, "{}", next.stderr);
    let next = cli(&dir, "artifacts", &["T1", "add", "https://example.com/x"]);
    assert_eq!(next.code, 0, "{}", next.stderr);
    let next = cli(&dir, "status", &["T1", "review"]);
    assert_eq!(next.code, 0, "{}", next.stderr);

    let rows = artifacts(&dir, "T1");
    assert_eq!(rows.len(), 2, "artifacts survive the round");
    assert_eq!(rows[0]["target"], shot);
    assert_eq!(rows[0]["round"], 1, "the old artifact keeps its round");
    assert_eq!(rows[1]["round"], 0, "a plain add has no round");
}

/// The seeded id is stable for one board; read it back instead of re-seeding.
fn seed_id(dir: &Path) -> Uuid {
    TaskStore::new(dir)
        .load()
        .expect("load")
        .tasks()
        .iter()
        .find(|task| task.number == Some(1))
        .expect("T1")
        .id
}
