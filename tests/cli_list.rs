#[cfg(unix)]
#[allow(dead_code)]
#[path = "support/pty.rs"]
mod pty;

use std::ffi::OsString;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
#[cfg(unix)]
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use tsk_tui::cli::{run_with, run_with_terminal_width};
use tsk_tui::domain::{DomainState, HumanStatus, ProvenanceOrigin, TaskScope};
use tsk_tui::store::TaskStore;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);
static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("lock environment")
}

struct EnvironmentGuard {
    key: &'static str,
    prior: Option<OsString>,
}

impl EnvironmentGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let prior = std::env::var_os(key);
        // SAFETY: ENV_LOCK serializes this process-wide environment mutation.
        unsafe { std::env::set_var(key, value) };
        Self { key, prior }
    }

    fn context_for(cwd: &Path) -> Self {
        let context = format!(
            r#"{{"focused_pane_cwd":{}}}"#,
            serde_json::to_string(cwd).expect("serialize repo path")
        );
        Self::set("HERDR_PLUGIN_CONTEXT_JSON", context)
    }
}

impl Drop for EnvironmentGuard {
    fn drop(&mut self) {
        match &self.prior {
            Some(value) => {
                // SAFETY: ENV_LOCK remains held until this guard is dropped.
                unsafe { std::env::set_var(self.key, value) };
            }
            None => {
                // SAFETY: ENV_LOCK remains held until this guard is dropped.
                unsafe { std::env::remove_var(self.key) };
            }
        }
    }
}

fn temp_state_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tsk-cli-list-{label}-{nanos}-{seq}"));
    std::fs::create_dir_all(&dir).expect("create state directory");
    dir
}

fn project_repo(label: &str) -> PathBuf {
    let repo = temp_state_dir(label);
    std::fs::create_dir(repo.join(".git")).expect("create git marker");
    repo
}

fn state_dir_arg(dir: &Path) -> String {
    dir.to_string_lossy().into_owned()
}

fn list(args: &[String]) -> tsk_tui::cli::CliOutput {
    run_with(args, Cursor::new(Vec::<u8>::new()), true)
}

fn list_at_width(args: &[String], terminal_width: usize) -> tsk_tui::cli::CliOutput {
    run_with_terminal_width(
        args,
        Cursor::new(Vec::<u8>::new()),
        true,
        Some(terminal_width),
    )
}

#[test]
fn list_does_not_seed_agent_profiles() {
    let _env = env_lock();
    let dir = temp_state_dir("no-agent-seed");
    let output = list(&[
        "tsk".into(),
        "list".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
        "--all".into(),
    ]);

    assert_eq!(output.code, 0);
    assert!(!dir.join("agents.toml").exists());
    let _ = std::fs::remove_dir_all(dir);
}

fn create_task(state: &mut DomainState, title: &str, scope: TaskScope, status: HumanStatus) {
    let id = state
        .create(title, None, scope, ProvenanceOrigin::Manual, None)
        .expect("create task");
    state.set_status(id, status).expect("set status");
}

fn create_task_with_thread(
    state: &mut DomainState,
    title: &str,
    scope: TaskScope,
    status: HumanStatus,
    thread: Option<&str>,
) -> uuid::Uuid {
    let id = state
        .create(
            title,
            None,
            scope,
            ProvenanceOrigin::Manual,
            thread.map(str::to_owned),
        )
        .expect("create task");
    state.set_status(id, status).expect("set status");
    id
}

#[test]
fn list_filters_by_assignee_and_json_includes_nullable_assignee() {
    let _env = env_lock();
    let dir = temp_state_dir("assignee");
    let mut state = DomainState::new();
    let assigned = state
        .create(
            "assigned",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create assigned");
    state
        .assign(assigned, Some("reviewer".into()))
        .expect("assign");
    state
        .create(
            "unassigned",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create unassigned");
    TaskStore::new(&dir).save(&state).expect("save");

    let filtered = list(&[
        "tsk".into(),
        "list".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
        "--all".into(),
        "--assignee".into(),
        "reviewer".into(),
    ]);
    assert_eq!(filtered.code, 0, "{}", filtered.stderr);
    assert!(filtered.stdout.contains("assigned"));
    assert!(!filtered.stdout.contains("T2"), "{}", filtered.stdout);

    let json = list(&[
        "tsk".into(),
        "list".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
        "--all".into(),
        "--json".into(),
    ]);
    assert_eq!(json.code, 0, "{}", json.stderr);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&json.stdout).expect("json rows");
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|row| row["assignee"] == "reviewer"));
    assert!(rows.iter().any(|row| row["assignee"].is_null()));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_defaults_to_invocation_project_open_tasks_in_human_and_json_group_order() {
    let _env = env_lock();
    let repo = project_repo("default-repo");
    let _context = EnvironmentGuard::context_for(&repo);
    let dir = temp_state_dir("default-rows");
    let project = TaskScope::Project {
        path: repo.to_string_lossy().into_owned(),
    };
    let mut state = DomainState::new();
    create_task(
        &mut state,
        "review target",
        project.clone(),
        HumanStatus::Review,
    );
    create_task(
        &mut state,
        "ready target",
        project.clone(),
        HumanStatus::Ready,
    );
    create_task(
        &mut state,
        "started target",
        project.clone(),
        HumanStatus::Started,
    );
    create_task(
        &mut state,
        "blocked target",
        project.clone(),
        HumanStatus::Blocked,
    );
    create_task(
        &mut state,
        "open target",
        project.clone(),
        HumanStatus::Open,
    );
    create_task(
        &mut state,
        "global hidden",
        TaskScope::Global,
        HumanStatus::Ready,
    );
    create_task(
        &mut state,
        "other hidden",
        TaskScope::Project {
            path: "/projects/other".into(),
        },
        HumanStatus::Ready,
    );
    create_task(
        &mut state,
        "done hidden",
        project.clone(),
        HumanStatus::Done,
    );
    let deleted = state
        .create(
            "deleted hidden",
            None,
            project.clone(),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create deleted task");
    state.soft_delete(deleted).expect("soft delete task");
    TaskStore::new(&dir).save(&state).expect("seed store");

    let human = list(&[
        "tsk".into(),
        "list".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(human.code, 0);
    assert!(human.stderr.is_empty());
    assert_eq!(
        human.stdout,
        "STARTED\n - 3 started target\n\nREADY\n - 2 ready target\n\nOPEN\n - 5 open target\n\nBLOCKED\n - 4 blocked target\n\nREVIEW\n - 1 review target\n"
    );

    let json = list(&[
        "tsk".into(),
        "list".into(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(json.code, 0);
    assert!(json.stderr.is_empty());
    let rows: Vec<serde_json::Value> = serde_json::from_str(&json.stdout).expect("JSON rows");
    assert_eq!(
        rows.iter()
            .map(|row| row["title"].as_str().expect("title"))
            .collect::<Vec<_>>(),
        vec![
            "started target",
            "ready target",
            "open target",
            "blocked target",
            "review target"
        ]
    );
    for row in &rows {
        assert!(row["id"].as_str().is_some_and(|id| !id.is_empty()));
        assert_eq!(row["project"], repo.to_string_lossy().as_ref());
    }

    let _ = std::fs::remove_dir_all(repo);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_resolves_named_and_global_scopes() {
    let _env = env_lock();
    let repo = project_repo("scope-repo");
    let _context = EnvironmentGuard::context_for(&repo);
    let dir = temp_state_dir("scopes");
    let mut state = DomainState::new();
    create_task(
        &mut state,
        "widget task",
        TaskScope::Project {
            path: "/projects/Widget".into(),
        },
        HumanStatus::Ready,
    );
    create_task(
        &mut state,
        "global task",
        TaskScope::Global,
        HumanStatus::Ready,
    );
    TaskStore::new(&dir).save(&state).expect("seed store");

    let named = list(&[
        "tsk".into(),
        "list".into(),
        "-p".into(),
        "widget".into(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(named.code, 0);
    let named_rows: Vec<serde_json::Value> =
        serde_json::from_str(&named.stdout).expect("named JSON rows");
    assert_eq!(named_rows.len(), 1);
    assert_eq!(named_rows[0]["title"], "widget task");
    assert_eq!(named_rows[0]["project"], "/projects/Widget");

    let global = list(&[
        "tsk".into(),
        "list".into(),
        "--desk".into(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(global.code, 0);
    let global_rows: Vec<serde_json::Value> =
        serde_json::from_str(&global.stdout).expect("global JSON rows");
    assert_eq!(global_rows.len(), 1);
    assert_eq!(global_rows[0]["title"], "global task");
    assert!(global_rows[0]["project"].is_null());

    let _ = std::fs::remove_dir_all(repo);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_done_and_deleted_filters_are_status_and_soft_delete_specific() {
    let _env = env_lock();
    let repo = project_repo("filters-repo");
    let _context = EnvironmentGuard::context_for(&repo);
    let dir = temp_state_dir("filters");
    let project = TaskScope::Project {
        path: repo.to_string_lossy().into_owned(),
    };
    let mut state = DomainState::new();
    create_task(
        &mut state,
        "done visible",
        project.clone(),
        HumanStatus::Done,
    );
    let deleted_ready = state
        .create(
            "deleted ready",
            None,
            project.clone(),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create deleted ready");
    state.soft_delete(deleted_ready).expect("soft delete ready");
    let deleted_done = state
        .create(
            "deleted done",
            None,
            project.clone(),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create deleted done");
    state.complete(deleted_done).expect("complete deleted task");
    state.soft_delete(deleted_done).expect("soft delete done");
    create_task(
        &mut state,
        "other done",
        TaskScope::Global,
        HumanStatus::Done,
    );
    TaskStore::new(&dir).save(&state).expect("seed store");

    let done = list(&[
        "tsk".into(),
        "list".into(),
        "--done".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(done.code, 0);
    assert_eq!(done.stdout, "DONE\n - 1 done visible\n");

    let deleted = list(&[
        "tsk".into(),
        "list".into(),
        "--deleted".into(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(deleted.code, 0);
    let deleted_rows: Vec<serde_json::Value> =
        serde_json::from_str(&deleted.stdout).expect("deleted JSON rows");
    // Newest deletion first. "deleted ready" was deleted earlier, so a later
    // undoable action finalized it into trash.jsonl; "deleted done" is still a
    // live soft-delete. The deleted view merges both.
    assert_eq!(
        deleted_rows
            .iter()
            .map(|row| row["title"].as_str().expect("title"))
            .collect::<Vec<_>>(),
        vec!["deleted done", "deleted ready"]
    );
    assert_eq!(deleted_rows[0]["status"], "done");
    assert_eq!(deleted_rows[1]["status"], "open");

    let deleted_human = list(&[
        "tsk".into(),
        "list".into(),
        "--deleted".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(deleted_human.code, 0);
    assert_eq!(
        deleted_human.stdout,
        "DELETED\n - 3 deleted done\n - 2 deleted ready\n"
    );

    let _ = std::fs::remove_dir_all(repo);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_all_groups_each_status_by_concise_scope_for_every_filter() {
    let _env = env_lock();
    let repo = project_repo("all-repo");
    let _context = EnvironmentGuard::context_for(&repo);
    let dir = temp_state_dir("all");
    let project = TaskScope::Project {
        path: repo.to_string_lossy().into_owned(),
    };
    let mut state = DomainState::new();
    create_task(
        &mut state,
        "global started",
        TaskScope::Global,
        HumanStatus::Started,
    );
    create_task(
        &mut state,
        "project started",
        project.clone(),
        HumanStatus::Started,
    );
    create_task(
        &mut state,
        "other started",
        TaskScope::Project {
            path: "/projects/other".into(),
        },
        HumanStatus::Started,
    );
    create_task(
        &mut state,
        "project ready",
        project.clone(),
        HumanStatus::Ready,
    );
    create_task(
        &mut state,
        "other blocked",
        TaskScope::Project {
            path: "/projects/other".into(),
        },
        HumanStatus::Blocked,
    );
    create_task(
        &mut state,
        "global review",
        TaskScope::Global,
        HumanStatus::Review,
    );
    create_task(&mut state, "project done", project, HumanStatus::Done);
    create_task(
        &mut state,
        "global done",
        TaskScope::Global,
        HumanStatus::Done,
    );
    let deleted_global = state
        .create(
            "global deleted",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create global deleted task");
    state
        .soft_delete(deleted_global)
        .expect("soft delete global task");
    let deleted_other = state
        .create(
            "other deleted",
            None,
            TaskScope::Project {
                path: "/projects/other".into(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create other deleted task");
    state
        .soft_delete(deleted_other)
        .expect("soft delete other task");
    TaskStore::new(&dir).save(&state).expect("seed store");
    let project_name = repo
        .file_name()
        .and_then(|name| name.to_str())
        .expect("project basename");

    let open = list(&[
        "tsk".into(),
        "list".into(),
        "--all".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(open.code, 0);
    assert_eq!(
        open.stdout,
        format!(
            "STARTED\n  desk\n    - 1 global started\n  {project_name}\n    - 2 project started\n  other\n    - 3 other started\n\nREADY\n  {project_name}\n    - 4 project ready\n\nBLOCKED\n  other\n    - 5 other blocked\n\nREVIEW\n  desk\n    - 6 global review\n"
        )
    );

    let open_json = list(&[
        "tsk".into(),
        "list".into(),
        "--all".into(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    let open_rows: Vec<serde_json::Value> =
        serde_json::from_str(&open_json.stdout).expect("open JSON rows");
    assert_eq!(
        open_rows
            .iter()
            .map(|row| row["title"].as_str().expect("title"))
            .collect::<Vec<_>>(),
        vec![
            "global started",
            "project started",
            "other started",
            "project ready",
            "other blocked",
            "global review",
        ]
    );

    let done = list(&[
        "tsk".into(),
        "list".into(),
        "--all".into(),
        "--done".into(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(done.code, 0);
    let done_rows: Vec<serde_json::Value> = serde_json::from_str(&done.stdout).expect("JSON rows");
    assert_eq!(
        done_rows
            .iter()
            .map(|row| row["title"].as_str().expect("title"))
            .collect::<Vec<_>>(),
        vec!["project done", "global done"]
    );
    for row in &done_rows {
        assert_eq!(
            row.as_object()
                .expect("JSON row")
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["assignee", "id", "number", "project", "status", "thread", "title"]
        );
    }
    let done_human = list(&[
        "tsk".into(),
        "list".into(),
        "--all".into(),
        "--done".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(
        done_human.stdout,
        format!("DONE\n  {project_name}\n    - 7 project done\n  desk\n    - 8 global done\n")
    );

    let deleted = list(&[
        "tsk".into(),
        "list".into(),
        "--all".into(),
        "--deleted".into(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(deleted.code, 0);
    let deleted_rows: Vec<serde_json::Value> =
        serde_json::from_str(&deleted.stdout).expect("JSON rows");
    assert_eq!(
        deleted_rows
            .iter()
            .map(|row| row["title"].as_str().expect("title"))
            .collect::<Vec<_>>(),
        // Newest deletion first: "other deleted" was soft-deleted after "global deleted".
        vec!["other deleted", "global deleted"]
    );
    let deleted_human = list(&[
        "tsk".into(),
        "list".into(),
        "--all".into(),
        "--deleted".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(
        deleted_human.stdout,
        "DELETED\n  other\n    - 10 other deleted\n  desk\n    - 9 global deleted\n"
    );

    let _ = std::fs::remove_dir_all(repo);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_all_distinguishes_global_from_project_global_and_uses_visible_scope_labels() {
    let _env = env_lock();
    let dir = temp_state_dir("all-colliding-scopes");
    let mut state = DomainState::new();
    create_task(
        &mut state,
        "global task",
        TaskScope::Global,
        HumanStatus::Ready,
    );
    create_task(
        &mut state,
        "project global token",
        TaskScope::Project {
            path: "global".into(),
        },
        HumanStatus::Ready,
    );
    create_task(
        &mut state,
        "project global path",
        TaskScope::Project {
            path: "/work/global".into(),
        },
        HumanStatus::Ready,
    );
    create_task(
        &mut state,
        "work api",
        TaskScope::Project {
            path: "/work/api".into(),
        },
        HumanStatus::Ready,
    );
    create_task(
        &mut state,
        "personal api",
        TaskScope::Project {
            path: "/personal/api".into(),
        },
        HumanStatus::Ready,
    );
    create_task(
        &mut state,
        "whitespace scope",
        TaskScope::Project {
            path: " \t ".into(),
        },
        HumanStatus::Ready,
    );
    TaskStore::new(&dir).save(&state).expect("seed store");

    let output = list(&[
        "tsk".into(),
        "list".into(),
        "--all".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);

    assert_eq!(output.code, 0);
    assert_eq!(
        output.stdout,
        "READY\n  desk\n    - 1 global task\n  project: global\n    - 2 project global token\n  work/global\n    - 3 project global path\n  work/api\n    - 4 work api\n  personal/api\n    - 5 personal api\n  project: <empty project 1>\n    - 6 whitespace scope\n"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_all_uses_shortest_unique_trailing_scope_labels_across_statuses() {
    let _env = env_lock();
    let dir = temp_state_dir("all-cross-status-scopes");
    let mut state = DomainState::new();
    create_task(
        &mut state,
        "project global",
        TaskScope::Project {
            path: "global".into(),
        },
        HumanStatus::Started,
    );
    create_task(
        &mut state,
        "work api",
        TaskScope::Project {
            path: "/work/api".into(),
        },
        HumanStatus::Started,
    );
    create_task(&mut state, "global", TaskScope::Global, HumanStatus::Ready);
    create_task(
        &mut state,
        "blank one",
        TaskScope::Project { path: " ".into() },
        HumanStatus::Ready,
    );
    create_task(
        &mut state,
        "personal api",
        TaskScope::Project {
            path: "/personal/api".into(),
        },
        HumanStatus::Blocked,
    );
    create_task(
        &mut state,
        "blank two",
        TaskScope::Project { path: "\t".into() },
        HumanStatus::Review,
    );
    TaskStore::new(&dir).save(&state).expect("seed store");

    let output = list(&[
        "tsk".into(),
        "list".into(),
        "--all".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);

    assert_eq!(output.code, 0);
    assert_eq!(
        output.stdout,
        "STARTED\n  global\n    - 1 project global\n  work/api\n    - 2 work api\n\nREADY\n  desk\n    - 3 global\n  project: <empty project 1>\n    - 4 blank one\n\nBLOCKED\n  personal/api\n    - 5 personal api\n\nREVIEW\n  project: <empty project 2>\n    - 6 blank two\n"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_all_preserves_raw_scope_syntax_after_trailing_segments_are_exhausted() {
    let _env = env_lock();
    let dir = temp_state_dir("all-raw-scope-syntax");
    let mut state = DomainState::new();
    for (title, path) in [
        ("absolute api", "/work/api"),
        ("relative api", "work/api"),
        ("trailing api", "/work/api/"),
        ("repeated api", "//work//api"),
    ] {
        create_task(
            &mut state,
            title,
            TaskScope::Project { path: path.into() },
            HumanStatus::Ready,
        );
    }
    TaskStore::new(&dir).save(&state).expect("seed store");

    let output = list(&[
        "tsk".into(),
        "list".into(),
        "--all".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);

    assert_eq!(output.code, 0);
    assert_eq!(
        output.stdout,
        "READY\n  project: \"/work/api\"\n    - 1 absolute api\n  project: \"work/api\"\n    - 2 relative api\n  project: \"/work/api/\"\n    - 3 trailing api\n  project: \"//work//api\"\n    - 4 repeated api\n"
    );
    assert!(
        !output.stdout.contains("(scope "),
        "syntactically distinct scopes must remain distinguishable without synthetic suffixes"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_all_visibly_escapes_and_disambiguates_control_scope_labels() {
    let _env = env_lock();
    let dir = temp_state_dir("all-escaped-scope-label");
    let mut state = DomainState::new();
    for (title, path) in [
        ("control scope task", "\u{001b}[2Japi"),
        ("literal escape scope task", "\\u{001b}[2Japi"),
    ] {
        create_task(
            &mut state,
            title,
            TaskScope::Project { path: path.into() },
            HumanStatus::Ready,
        );
    }
    TaskStore::new(&dir).save(&state).expect("seed store");

    let output = list(&[
        "tsk".into(),
        "list".into(),
        "--all".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);

    assert_eq!(output.code, 0);
    assert!(!output.stdout.contains('\u{001b}'));
    let labels = output
        .stdout
        .lines()
        .filter(|line| line.starts_with("  ") && !line.starts_with("    "))
        .collect::<Vec<_>>();
    assert_eq!(labels.len(), 2);
    assert_ne!(labels[0], labels[1]);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn human_list_escapes_terminal_control_titles_without_changing_json() {
    let _env = env_lock();
    let dir = temp_state_dir("terminal-control-title");
    let title = "control\u{001b}]52;c;clipboard\u{0007}";
    let escaped = "control\\u{001b}]52;c;clipboard\\u{0007}";
    let mut state = DomainState::new();
    create_task(&mut state, title, TaskScope::Global, HumanStatus::Ready);
    create_task(&mut state, title, TaskScope::Global, HumanStatus::Done);
    let deleted = state
        .create(
            title,
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create deleted task");
    state.soft_delete(deleted).expect("soft delete task");
    TaskStore::new(&dir).save(&state).expect("seed store");

    for view in [vec![], vec!["--done"], vec!["--deleted"]] {
        let mut args = vec![
            "tsk".into(),
            "list".into(),
            "--desk".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
        ];
        args.extend(view.into_iter().map(String::from));
        let human = list(&args);
        assert_eq!(human.code, 0);
        assert!(human.stderr.is_empty());
        assert!(human.stdout.contains(escaped));
        assert!(!human.stdout.contains('\u{001b}'));
        assert!(!human.stdout.contains('\u{0007}'));
    }

    let json = list(&[
        "tsk".into(),
        "list".into(),
        "--desk".into(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(json.code, 0);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&json.stdout).expect("JSON rows");
    assert_eq!(rows[0]["title"], title);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn direct_human_list_wraps_notes_with_a_hanging_indent() {
    let dir = temp_state_dir("wrapped-notes");
    let mut state = DomainState::new();
    let title = "wrap target with a title long enough to wrap at fifty columns";
    let notes = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda";
    let step = "implement the surprisingly long step and verify every continuation remains aligned";
    let task = state
        .create(
            title,
            Some(notes.into()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            Some("release-2026-long-thread".into()),
        )
        .expect("create wrapping notes task");
    state.add_step(task, step).expect("seed wrapping step");
    TaskStore::new(&dir).save(&state).expect("seed store");

    let output = list_at_width(
        &[
            "tsk".into(),
            "list".into(),
            task.to_string(),
            "--state-dir".into(),
            state_dir_arg(&dir),
        ],
        50,
    );

    assert_eq!(output.code, 0);
    assert_eq!(
        output.stdout,
        "OPEN\n - 1 wrap target with a title long enough to wrap \n     at fifty columns\n   alpha beta gamma delta epsilon zeta eta theta \n   iota kappa lambda\n\n   [ ] implement the surprisingly long step and \n       verify every continuation remains aligned\n\n   #release-2026-long-thread\n"
    );
    assert!(
        output.stdout.lines().all(|line| line.len() <= 50),
        "every explicit row fits the reported terminal width: {}",
        output.stdout
    );

    let redirected = list(&[
        "tsk".into(),
        "list".into(),
        task.to_string(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    for logical_line in [
        format!(" - 1 {title}"),
        format!("   {notes}"),
        format!("   [ ] {step}"),
    ] {
        assert!(
            redirected.stdout.lines().any(|line| line == logical_line),
            "redirected output split {logical_line:?}: {}",
            redirected.stdout
        );
    }

    let json = list_at_width(
        &[
            "tsk".into(),
            "list".into(),
            task.to_string(),
            "--json".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
        ],
        50,
    );
    assert_eq!(json.stdout.lines().count(), 1, "JSON must not wrap");
    let rows: Vec<serde_json::Value> = serde_json::from_str(&json.stdout).expect("valid JSON");
    assert_eq!(rows[0]["notes"], notes);
    assert_eq!(rows[0]["steps"][0]["text"], step);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn scoped_human_list_wraps_task_rows_and_scope_labels() {
    let dir = temp_state_dir("wrapped-list-rows");
    let mut state = DomainState::new();
    create_task_with_thread(
        &mut state,
        "a very long task title that wraps across the supported terminal floor",
        TaskScope::Project {
            path: "/projects/a-very-long-project-name-that-needs-wrapping-at-fifty-columns".into(),
        },
        HumanStatus::Ready,
        Some("release-2026-long"),
    );
    TaskStore::new(&dir).save(&state).expect("seed store");

    let output = list_at_width(
        &[
            "tsk".into(),
            "list".into(),
            "--all".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
        ],
        50,
    );

    assert_eq!(output.code, 0);
    assert!(
        output.stdout.lines().all(|line| line.len() <= 50),
        "every list row fits the reported terminal width: {}",
        output.stdout
    );

    assert!(
        output
            .stdout
            .lines()
            .any(|line| line.starts_with("  ") && line.contains("fifty")),
        "scope label did not wrap with its hanging indent: {}",
        output.stdout
    );
    assert!(
        output
            .stdout
            .lines()
            .any(|line| line.starts_with("        ") && line.contains("terminal")),
        "scoped task row did not wrap with its hanging indent: {}",
        output.stdout
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn unscoped_human_list_wraps_threaded_task_rows() {
    let dir = temp_state_dir("wrapped-unscoped-row");
    let mut state = DomainState::new();
    create_task_with_thread(
        &mut state,
        "a default list task title that must wrap across the supported fifty column floor",
        TaskScope::Global,
        HumanStatus::Ready,
        Some("release-2026-long"),
    );
    TaskStore::new(&dir).save(&state).expect("seed store");

    let output = list_at_width(
        &[
            "tsk".into(),
            "list".into(),
            "--desk".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
        ],
        50,
    );

    assert_eq!(output.code, 0);
    assert!(output.stdout.lines().all(|line| line.len() <= 50));
    assert!(
        output
            .stdout
            .lines()
            .any(|line| line.starts_with("     ") && line.contains("supported")),
        "unscoped task row did not wrap with its hanging indent: {}",
        output.stdout
    );
    assert!(output.stdout.contains("#release-2026-long"));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_help_wraps_at_eighty_while_usage_errors_follow_the_terminal_width() {
    let help = list_at_width(&["tsk".into(), "list".into(), "--help".into()], 50);
    let wide_help = list_at_width(&["tsk".into(), "list".into(), "--help".into()], 100);
    assert_eq!(help.code, 0);
    assert_eq!(help.stdout, wide_help.stdout);
    let help_width = help
        .stdout
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap();
    assert!(
        help_width <= 80,
        "help exceeded its reference width: {}",
        help.stdout
    );

    let usage = list_at_width(&["tsk".into(), "list".into(), "--bogus".into()], 50);
    assert_eq!(usage.code, 2);
    assert!(usage.stdout.is_empty());
    assert!(
        usage.stderr.lines().all(|line| line.chars().count() <= 50),
        "usage exceeded terminal width: {}",
        usage.stderr
    );
}

#[cfg(unix)]
#[test]
fn binary_uses_stdout_terminal_width_and_leaves_redirects_unwrapped() {
    let root = pty::scratch_root("list-terminal-width");
    let title =
        "a terminal width handoff title deliberately longer than fifty columns at the boundary";
    let mut state = DomainState::new();
    let task = create_task_with_thread(
        &mut state,
        title,
        TaskScope::Global,
        HumanStatus::Ready,
        None,
    );
    TaskStore::new(root.join("state"))
        .save(&state)
        .expect("seed PTY store");
    let number = TaskStore::new(root.join("state"))
        .load()
        .expect("reload PTY store")
        .get(task)
        .unwrap()
        .number
        .unwrap()
        .to_string();
    let args = ["list", number.as_str()];

    let redirected = pty::run_with_tty_stdin_and_piped_output(
        &root,
        &std::env::current_dir().unwrap(),
        &args,
        24,
        50,
    );
    assert!(redirected.status.success());
    let redirected = String::from_utf8(redirected.stdout).unwrap();
    assert!(
        redirected
            .lines()
            .any(|line| line == format!(" - 1 {title}")),
        "piped stdout must stay unwrapped even when stdin is a terminal: {redirected}"
    );

    let mut terminal =
        pty::Session::spawn(root, &std::env::current_dir().unwrap(), &args, &[], 24, 50);
    let rendered = terminal.output_until("boundary").replace("\r\n", "\n");
    assert!(terminal.wait_exit(Duration::from_secs(2)).success());
    assert!(
        rendered.lines().all(|line| line.chars().count() <= 50),
        "terminal output exceeded its width: {rendered}"
    );
    assert!(
        rendered
            .lines()
            .any(|line| line.starts_with("     ") && line.contains("fifty")),
        "binary did not hand the terminal width to list rendering: {rendered}"
    );
}

#[test]
fn direct_human_list_keeps_note_lines_and_escapes_other_controls() {
    let _env = env_lock();
    let dir = temp_state_dir("terminal-control-notes");
    let notes = "first\tcell\nsecond\u{001b}]52;c;clipboard\u{0007}";
    let mut state = DomainState::new();
    let task = state
        .create(
            "notes target",
            Some(notes.into()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            Some("release".into()),
        )
        .expect("create notes task");
    TaskStore::new(&dir).save(&state).expect("seed store");

    let human = list(&[
        "tsk".into(),
        "list".into(),
        task.to_string(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(human.code, 0);
    assert_eq!(
        human.stdout,
        "OPEN\n - 1 notes target\n   first\\u{0009}cell\n   second\\u{001b}]52;c;clipboard\\u{0007}\n\n   #release\n"
    );
    assert!(!human.stdout.contains('\t'));
    assert!(!human.stdout.contains('\u{001b}'));
    assert!(!human.stdout.contains('\u{0007}'));

    let json = list(&[
        "tsk".into(),
        "list".into(),
        task.to_string(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&json.stdout).expect("JSON rows");
    assert_eq!(rows[0]["notes"], notes);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn human_list_escapes_terminal_control_thread_markers_without_changing_json() {
    let _env = env_lock();
    let dir = temp_state_dir("terminal-control-thread");
    let thread = "release\u{001b}]52;c;clipboard\u{0007}";
    let escaped = "release\\u{001b}]52;c;clipboard\\u{0007}";
    let mut state = DomainState::new();
    create_task_with_thread(
        &mut state,
        "threaded task",
        TaskScope::Global,
        HumanStatus::Ready,
        Some(thread),
    );
    TaskStore::new(&dir).save(&state).expect("seed store");

    let human = list(&[
        "tsk".into(),
        "list".into(),
        "--desk".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(human.code, 0);
    assert!(human.stdout.contains(escaped));
    assert!(!human.stdout.contains('\u{001b}'));
    assert!(!human.stdout.contains('\u{0007}'));

    let json = list(&[
        "tsk".into(),
        "list".into(),
        "--desk".into(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(json.code, 0);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&json.stdout).expect("JSON rows");
    assert_eq!(rows[0]["thread"], thread);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_equals_state_dir_form_accepts_dash_leading_value() {
    let cwd = temp_state_dir("equals-dash-state");
    let state_dir = cwd.join("-state");
    let mut state = DomainState::new();
    create_task(
        &mut state,
        "equals state task",
        TaskScope::Global,
        HumanStatus::Ready,
    );
    TaskStore::new(&state_dir).save(&state).expect("seed state");
    let binary =
        std::env::var("CARGO_BIN_EXE_tsk").expect("Cargo must provide the tsk binary path");

    let output = std::process::Command::new(binary)
        .current_dir(&cwd)
        .args(["list", "--desk", "--json", "--state-dir=-state"])
        .output()
        .expect("run equals state directory");

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).expect("JSON rows");
    assert_eq!(rows[0]["title"], "equals state task");

    let _ = std::fs::remove_dir_all(cwd);
}

#[test]
fn bare_list_outside_a_repo_stays_on_desk_while_the_directory_is_addressable() {
    let _env = env_lock();
    let outside = temp_state_dir("outside-repo");
    let _context = EnvironmentGuard::context_for(&outside);
    let dir = temp_state_dir("outside-rows");
    let mut state = DomainState::new();
    create_task(
        &mut state,
        "global task",
        TaskScope::Global,
        HumanStatus::Ready,
    );
    create_task(
        &mut state,
        "directory task",
        TaskScope::Project {
            path: outside.to_string_lossy().into_owned(),
        },
        HumanStatus::Ready,
    );
    TaskStore::new(&dir).save(&state).expect("seed store");

    let output = list(&[
        "tsk".into(),
        "list".into(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);

    assert_eq!(output.code, 0);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&output.stdout).expect("JSON rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["title"], "global task");
    assert!(rows[0]["project"].is_null());

    let project = list(&[
        "tsk".into(),
        "list".into(),
        "--project".into(),
        outside.to_string_lossy().into_owned(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(project.code, 0);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&project.stdout).expect("project JSON");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["title"], "directory task");
    assert_eq!(rows[0]["project"], outside.to_string_lossy().as_ref());

    let _ = std::fs::remove_dir_all(outside);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_rejects_conflicting_scope_and_filter_flags_and_missing_project_values() {
    let _env = env_lock();
    let dir = temp_state_dir("conflicts");

    for args in [
        vec![
            "tsk".into(),
            "list".into(),
            "--all".into(),
            "--project".into(),
            "/projects/a".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
        ],
        vec![
            "tsk".into(),
            "list".into(),
            "--all".into(),
            "--desk".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
        ],
        vec![
            "tsk".into(),
            "list".into(),
            "--desk".into(),
            "--project".into(),
            "/projects/a".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
        ],
        vec![
            "tsk".into(),
            "list".into(),
            "--done".into(),
            "--deleted".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
        ],
        vec![
            "tsk".into(),
            "list".into(),
            "-p".into(),
            "--json".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
        ],
        vec![
            "tsk".into(),
            "list".into(),
            "-p".into(),
            "-maintenance".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
        ],
    ] {
        let output = list(&args);
        assert_eq!(output.code, 2);
        assert!(output.stdout.is_empty());
        assert!(output.stderr.contains("usage: tsk list"));
    }

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_thread_parse_rejects_invalid_space_and_equals_forms() {
    let _env = env_lock();
    let dir = temp_state_dir("invalid-thread");

    for thread in ["--thread", "--thread=bad_name"] {
        let mut args = vec!["tsk".into(), "list".into(), thread.into()];
        if thread == "--thread" {
            args.push("bad_name".into());
        }
        args.extend(["--state-dir".into(), state_dir_arg(&dir)]);
        let output = list(&args);
        assert_eq!(output.code, 2, "{thread}");
        assert!(output.stdout.is_empty(), "{thread}");
        assert!(output.stderr.contains("invalid thread name"), "{thread}");
    }

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_equals_project_form_accepts_dash_leading_scope() {
    let _env = env_lock();
    let dir = temp_state_dir("equals-project");
    let mut state = DomainState::new();
    create_task(
        &mut state,
        "maintenance task",
        TaskScope::Project {
            path: "-maintenance".into(),
        },
        HumanStatus::Ready,
    );
    TaskStore::new(&dir).save(&state).expect("seed store");

    let output = list(&[
        "tsk".into(),
        "list".into(),
        "--project=-maintenance".into(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);

    assert_eq!(output.code, 0);
    assert_eq!(
        serde_json::from_str::<Vec<serde_json::Value>>(&output.stdout).expect("JSON rows"),
        vec![serde_json::json!({
            "id": state.tasks()[0].id,
            "number": 1,
            "title": "maintenance task",
            "status": "ready",
            "project": "-maintenance",
            "assignee": null,
            "thread": null,
        })]
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_when_state_dir_is_a_file_exits_3() {
    let _env = env_lock();
    let parent = temp_state_dir("state-dir-file");
    let state_file = parent.join("not-a-directory");
    std::fs::write(&state_file, "not a directory").expect("create state-dir file");

    let output = list(&[
        "tsk".into(),
        "list".into(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&state_file),
    ]);

    assert_eq!(output.code, 3);
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());

    let _ = std::fs::remove_dir_all(parent);
}

#[test]
fn list_state_dir_flag_wins_over_environment() {
    let _env = env_lock();
    let environment_dir = temp_state_dir("environment");
    let argument_dir = temp_state_dir("argument");
    let mut environment_state = DomainState::new();
    create_task(
        &mut environment_state,
        "environment task",
        TaskScope::Global,
        HumanStatus::Ready,
    );
    TaskStore::new(&environment_dir)
        .save(&environment_state)
        .expect("save environment state");
    let mut argument_state = DomainState::new();
    create_task(
        &mut argument_state,
        "argument task",
        TaskScope::Global,
        HumanStatus::Ready,
    );
    TaskStore::new(&argument_dir)
        .save(&argument_state)
        .expect("save argument state");
    let _state_dir = EnvironmentGuard::set("TSK_STATE_DIR", &environment_dir);

    let output = list(&[
        "tsk".into(),
        "list".into(),
        "--desk".into(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&argument_dir),
    ]);

    assert_eq!(output.code, 0);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&output.stdout).expect("JSON rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["title"], "argument task");

    let _ = std::fs::remove_dir_all(environment_dir);
    let _ = std::fs::remove_dir_all(argument_dir);
}

#[test]
fn list_uses_environment_state_dir_by_default() {
    let _env = env_lock();
    let dir = temp_state_dir("default");
    let mut state = DomainState::new();
    create_task(
        &mut state,
        "environment default task",
        TaskScope::Global,
        HumanStatus::Ready,
    );
    TaskStore::new(&dir)
        .save(&state)
        .expect("save default state");
    let _state_dir = EnvironmentGuard::set("TSK_STATE_DIR", &dir);

    let output = list(&[
        "tsk".into(),
        "list".into(),
        "--desk".into(),
        "--json".into(),
    ]);

    assert_eq!(output.code, 0);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&output.stdout).expect("JSON rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["title"], "environment default task");

    let _ = std::fs::remove_dir_all(dir);
}

/// One task whose steps carry chosen ids `aaa1…`/`aaa2…`, shaped through
/// the store document because step ids are otherwise minted by the domain.
fn state_with_steps(done_first: bool) -> (DomainState, tsk_tui::domain::Step) {
    let mut state = DomainState::new();
    let id = state
        .create(
            "steps target",
            Some("First note\nSecond note".into()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            Some("release".into()),
        )
        .expect("seed steps task");
    state.add_step(id, "First step").expect("seed first step");
    state.add_step(id, "Second step").expect("seed second step");
    if done_first {
        let first = state.tasks()[0].steps[0].id;
        state.toggle_step(id, first).expect("seed first step done");
    }
    let mut document = serde_json::to_value(&state).expect("serialize seed state");
    for (index, id) in [
        "aaa11111-0000-4000-8000-000000000001",
        "aaa22222-0000-4000-8000-000000000002",
    ]
    .into_iter()
    .enumerate()
    {
        document["tasks"][0]["steps"][index]["id"] =
            serde_json::json!(uuid::Uuid::parse_str(id).expect("shaped step id"));
    }
    let shaped: DomainState = serde_json::from_value(document).expect("state with shaped step ids");
    let first_step = shaped.tasks()[0].steps[0].clone();
    (shaped, first_step)
}

#[test]
fn list_task_prints_step_lines_with_state_and_short_id() {
    let dir = temp_state_dir("step-lines");
    let (state, first_step) = state_with_steps(true);
    let task = state.tasks()[0].id;
    TaskStore::new(&dir).save(&state).expect("seed store");

    let output = list(&[
        "tsk".into(),
        "list".into(),
        task.to_string(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);

    assert_eq!(output.code, 0, "{}", output.stderr);
    assert!(output.stderr.is_empty());
    assert_eq!(
        output.stdout,
        "OPEN\n - 1 steps target\n   First note\n   Second note\n\n   [x] First step\n   [ ] Second step\n\n   #release\n",
        "direct detail separates notes, steps, and the trailing thread"
    );
    assert!(
        first_step.id.to_string().starts_with("aaa1"),
        "printed short id must prefix the step identity"
    );

    let json = list(&[
        "tsk".into(),
        "list".into(),
        task.to_string(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(json.code, 0);
    let expected_json = format!(
        "[{{\"id\":\"{}\",\"number\":1,\"project\":null,\"status\":\"open\",\"title\":\"steps target\",\"notes\":\"First note\\nSecond note\",\"steps\":[{{\"id\":\"{}\",\"done\":true,\"short_id\":\"aaa1\",\"text\":\"First step\"}},{{\"id\":\"aaa22222-0000-4000-8000-000000000002\",\"done\":false,\"short_id\":\"aaa2\",\"text\":\"Second step\"}}],\"assignee\":null,\"thread\":\"release\"}}]\n",
        task, first_step.id
    );
    assert_eq!(
        json.stdout, expected_json,
        "direct JSON keeps title, notes, steps, and thread together in contract order"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_task_without_steps_keeps_task_rows_and_rejects_conflicting_flags() {
    let dir = temp_state_dir("task-without-steps");
    let mut state = DomainState::new();
    create_task(
        &mut state,
        "plain target",
        TaskScope::Global,
        HumanStatus::Ready,
    );
    TaskStore::new(&dir).save(&state).expect("seed store");
    let task = state.tasks()[0].id;

    let plain = list(&[
        "tsk".into(),
        "list".into(),
        task.to_string(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(plain.code, 0);
    assert_eq!(plain.stdout, "READY\n - 1 plain target\n");

    let plain_json = list(&[
        "tsk".into(),
        "list".into(),
        task.to_string(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(plain_json.code, 0);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&plain_json.stdout).expect("JSON rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["notes"], serde_json::Value::Null);
    assert_eq!(rows[0]["steps"], serde_json::json!([]));
    assert_eq!(rows[0]["thread"], serde_json::Value::Null);
    let raw = plain_json.stdout.as_str();
    assert!(
        raw.contains("\"status\":\"ready\",\"title\":\"plain target\",\"notes\":null,\"steps\":[],\"assignee\":null,\"thread\":null"),
        "direct JSON keeps empty detail fields and their contract order: {raw}"
    );

    for extra in ["--desk", "--all", "--done", "--deleted"] {
        let output = list(&[
            "tsk".into(),
            "list".into(),
            task.to_string(),
            extra.into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
        ]);
        assert_eq!(output.code, 2, "task id with {extra} is usage");
        assert!(output.stdout.is_empty());
        assert!(output.stderr.contains("usage: tsk list"));
    }
    let threaded = list(&[
        "tsk".into(),
        "list".into(),
        task.to_string(),
        "--thread".into(),
        "release".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(threaded.code, 2, "task id with --thread is usage");
    assert!(threaded.stdout.is_empty());
    assert!(threaded.stderr.contains("usage: tsk list"));

    let invalid = list(&[
        "tsk".into(),
        "list".into(),
        "not-a-uuid".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(invalid.code, 2);
    assert!(invalid.stdout.is_empty());
    assert!(invalid.stderr.contains("invalid task id"));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_thread_filters_after_scope_selection() {
    let _env = env_lock();
    let dir = temp_state_dir("thread-filter");
    let mut state = DomainState::new();
    create_task_with_thread(
        &mut state,
        "selected thread",
        TaskScope::Project {
            path: "/projects/selected".into(),
        },
        HumanStatus::Ready,
        Some("release"),
    );
    create_task_with_thread(
        &mut state,
        "selected other thread",
        TaskScope::Project {
            path: "/projects/selected".into(),
        },
        HumanStatus::Ready,
        Some("ops"),
    );
    create_task_with_thread(
        &mut state,
        "other scope same thread",
        TaskScope::Project {
            path: "/projects/other".into(),
        },
        HumanStatus::Ready,
        Some("release"),
    );
    TaskStore::new(&dir).save(&state).expect("seed store");

    let scoped = list(&[
        "tsk".into(),
        "list".into(),
        "--project".into(),
        "selected".into(),
        "--thread".into(),
        "Release".into(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);

    assert_eq!(scoped.code, 0, "{}", scoped.stderr);
    assert_eq!(
        serde_json::from_str::<Vec<serde_json::Value>>(&scoped.stdout)
            .expect("scoped JSON rows")
            .iter()
            .map(|row| row["title"].as_str().expect("title"))
            .collect::<Vec<_>>(),
        vec!["selected thread"]
    );

    let all = list(&[
        "tsk".into(),
        "list".into(),
        "--all".into(),
        "--thread=RELEASE".into(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(all.code, 0, "{}", all.stderr);
    assert_eq!(
        serde_json::from_str::<Vec<serde_json::Value>>(&all.stdout)
            .expect("all JSON rows")
            .iter()
            .map(|row| row["title"].as_str().expect("title"))
            .collect::<Vec<_>>(),
        vec!["selected thread", "other scope same thread"]
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn json_rows_always_carry_thread_field() {
    let _env = env_lock();
    let dir = temp_state_dir("thread-json");
    let mut state = DomainState::new();
    create_task_with_thread(
        &mut state,
        "threaded",
        TaskScope::Global,
        HumanStatus::Ready,
        Some("release"),
    );
    create_task(
        &mut state,
        "unthreaded",
        TaskScope::Global,
        HumanStatus::Ready,
    );
    TaskStore::new(&dir).save(&state).expect("seed store");

    let output = list(&[
        "tsk".into(),
        "list".into(),
        "--desk".into(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);

    assert_eq!(output.code, 0, "{}", output.stderr);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&output.stdout).expect("JSON rows");
    assert_eq!(rows[0]["thread"], "release");
    assert!(rows[1].get("thread").expect("unthreaded field").is_null());

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn json_list_rows_include_number() {
    let _env = env_lock();
    let dir = temp_state_dir("json-number");
    let mut state = DomainState::new();
    create_task(
        &mut state,
        "numbered task",
        TaskScope::Global,
        HumanStatus::Ready,
    );
    TaskStore::new(&dir).save(&state).expect("seed store");

    let output = list(&[
        "tsk".into(),
        "list".into(),
        "--desk".into(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);

    assert_eq!(output.code, 0, "{}", output.stderr);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&output.stdout).expect("JSON rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["number"], 1);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn human_list_rows_show_bare_digits() {
    let _env = env_lock();
    let dir = temp_state_dir("human-number");
    let mut state = DomainState::new();
    create_task(
        &mut state,
        "numbered task",
        TaskScope::Global,
        HumanStatus::Ready,
    );
    TaskStore::new(&dir).save(&state).expect("seed store");

    let output = list(&[
        "tsk".into(),
        "list".into(),
        "--desk".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);

    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(output.stdout, "READY\n - 1 numbered task\n");
    assert!(!output.stdout.contains("#1"));
    assert!(!output.stdout.contains("T1"));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_displayed_or_bare_number_finds_the_task_from_another_cwd() {
    let _env = env_lock();
    let invocation_repo = project_repo("number-invocation");
    let _context = EnvironmentGuard::context_for(&invocation_repo);
    let dir = temp_state_dir("number-other-cwd");
    let mut state = DomainState::new();
    let task = create_task_with_thread(
        &mut state,
        "other project task",
        TaskScope::Project {
            path: "/projects/other".into(),
        },
        HumanStatus::Ready,
        None,
    );
    TaskStore::new(&dir).save(&state).expect("seed store");
    let number = TaskStore::new(&dir)
        .load()
        .expect("load store")
        .get(task)
        .expect("task")
        .number
        .expect("task number");

    for operand in [
        number.to_string(),
        format!("T{number}"),
        format!("t{number}"),
    ] {
        let output = list(&[
            "tsk".into(),
            "list".into(),
            operand,
            "--json".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
        ]);

        assert_eq!(output.code, 0, "{}", output.stderr);
        let rows: Vec<serde_json::Value> = serde_json::from_str(&output.stdout).expect("JSON rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], task.to_string());
    }
    let _ = std::fs::remove_dir_all(invocation_repo);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_bare_digits_finds_done_and_deleted_tasks() {
    let dir = temp_state_dir("number-done-deleted");
    let mut state = DomainState::new();
    let done = state
        .create(
            "done target",
            Some("done notes".into()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            Some("done-thread".into()),
        )
        .expect("create done task");
    state
        .set_status(done, HumanStatus::Done)
        .expect("finish task");
    state.add_step(done, "done step").expect("add done step");
    let deleted = state
        .create(
            "deleted target",
            Some("deleted notes".into()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            Some("deleted-thread".into()),
        )
        .expect("create deleted task");
    state
        .add_step(deleted, "deleted step")
        .expect("add deleted step");
    state.soft_delete(deleted).expect("soft delete task");
    TaskStore::new(&dir).save(&state).expect("seed store");

    let persisted = TaskStore::new(&dir).load().expect("load store");
    for (task, label) in [(done, "done"), (deleted, "deleted")] {
        let number = persisted
            .get(task)
            .expect("task")
            .number
            .expect("task number");
        let args = [
            "tsk".into(),
            "list".into(),
            number.to_string(),
            "--state-dir".into(),
            state_dir_arg(&dir),
        ];
        let human = list(&args);
        assert_eq!(human.code, 0, "{}", human.stderr);
        let notes_at = human.stdout.find(&format!("   {label} notes")).unwrap();
        let step_at = human.stdout.find(&format!("   [ ] {label} step")).unwrap();
        let thread_at = human.stdout.find(&format!("   #{label}-thread")).unwrap();
        assert!(
            notes_at < step_at && step_at < thread_at,
            "{}",
            human.stdout
        );

        let mut json_args = args.to_vec();
        json_args.insert(3, "--json".into());
        let output = list(&json_args);
        assert_eq!(output.code, 0, "{}", output.stderr);
        let rows: Vec<serde_json::Value> = serde_json::from_str(&output.stdout).expect("JSON rows");
        assert_eq!(rows[0]["id"], task.to_string());
        assert_eq!(rows[0]["notes"], format!("{label} notes"));
        assert_eq!(rows[0]["steps"][0]["text"], format!("{label} step"));
        assert_eq!(rows[0]["thread"], format!("{label}-thread"));
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_uuid_still_finds_the_task() {
    let dir = temp_state_dir("uuid-address");
    let mut state = DomainState::new();
    let task = create_task_with_thread(
        &mut state,
        "uuid target",
        TaskScope::Global,
        HumanStatus::Ready,
        None,
    );
    TaskStore::new(&dir).save(&state).expect("seed store");

    let output = list(&[
        "tsk".into(),
        "list".into(),
        task.to_string(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(
        serde_json::from_str::<Vec<serde_json::Value>>(&output.stdout).expect("JSON rows")[0]["id"],
        task.to_string()
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_bare_digits_with_scope_or_status_flags_is_usage() {
    let dir = temp_state_dir("number-filter-usage");
    let mut state = DomainState::new();
    let task = create_task_with_thread(
        &mut state,
        "number target",
        TaskScope::Global,
        HumanStatus::Ready,
        None,
    );
    TaskStore::new(&dir).save(&state).expect("seed store");
    let number = TaskStore::new(&dir)
        .load()
        .expect("load store")
        .get(task)
        .expect("task")
        .number
        .expect("task number")
        .to_string();

    for flag in ["--desk", "--all", "--thread=release", "--done", "--deleted"] {
        let output = list(&[
            "tsk".into(),
            "list".into(),
            number.clone(),
            flag.into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
        ]);
        assert_eq!(output.code, 2, "number with {flag}");
        assert!(output.stdout.is_empty());
        assert!(output.stderr.contains("usage: tsk list"));
        assert!(output.stderr.contains("cannot be used with"));
        assert!(!output.stderr.contains("invalid task id"));
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_unknown_number_matches_unknown_uuid_refusal() {
    let dir = temp_state_dir("unknown-number");
    let unknown_number = list(&[
        "tsk".into(),
        "list".into(),
        "999".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    let unknown_uuid = list(&[
        "tsk".into(),
        "list".into(),
        "00000000-0000-4000-8000-000000000001".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(unknown_number.code, unknown_uuid.code);
    assert!(unknown_number.stdout.is_empty());
    assert_eq!(unknown_number.stderr, unknown_uuid.stderr);
    assert!(unknown_number.stderr.contains("unknown task"));
    assert!(!unknown_number.stderr.contains("invalid task id"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn human_output_appends_thread_marker_iff_row_threaded_snapshots() {
    let _env = env_lock();
    let repo = project_repo("thread-human");
    let _context = EnvironmentGuard::context_for(&repo);
    let dir = temp_state_dir("thread-human");
    let project = TaskScope::Project {
        path: repo.to_string_lossy().into_owned(),
    };
    let mut state = DomainState::new();
    create_task_with_thread(
        &mut state,
        "scoped threaded",
        project.clone(),
        HumanStatus::Ready,
        Some("alpha"),
    );
    create_task(
        &mut state,
        "scoped unthreaded",
        project.clone(),
        HumanStatus::Ready,
    );
    create_task_with_thread(
        &mut state,
        "global threaded",
        TaskScope::Global,
        HumanStatus::Ready,
        Some("ops"),
    );
    let step = create_task_with_thread(
        &mut state,
        "step threaded",
        TaskScope::Global,
        HumanStatus::Done,
        Some("steps"),
    );
    state.add_step(step, "Keep this line").expect("add step");
    TaskStore::new(&dir).save(&state).expect("seed store");
    let project_name = repo
        .file_name()
        .and_then(|name| name.to_str())
        .expect("project basename");

    let scoped = list(&[
        "tsk".into(),
        "list".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(
        scoped.stdout,
        "READY\n - 1 scoped threaded #alpha\n - 2 scoped unthreaded\n"
    );

    let all = list(&[
        "tsk".into(),
        "list".into(),
        "--all".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(
        all.stdout,
        format!(
            "READY\n  {project_name}\n    - 1 scoped threaded #alpha\n    - 2 scoped unthreaded\n  desk\n    - 3 global threaded #ops\n"
        )
    );

    let single = list(&[
        "tsk".into(),
        "list".into(),
        step.to_string(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(
        single.stdout,
        "DONE\n - 4 step threaded\n   [ ] Keep this line\n\n   #steps\n"
    );

    let _ = std::fs::remove_dir_all(repo);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn default_list_views_exclude_archived_tasks_and_tasks_of_archived_projects() {
    let dir = temp_state_dir("archived-excluded");
    let mut state = DomainState::new();
    create_task(
        &mut state,
        "archived open task",
        TaskScope::Global,
        HumanStatus::Ready,
    );
    create_task(
        &mut state,
        "visible open task",
        TaskScope::Global,
        HumanStatus::Ready,
    );
    create_task(
        &mut state,
        "archived done task",
        TaskScope::Global,
        HumanStatus::Done,
    );
    create_task(
        &mut state,
        "visible done task",
        TaskScope::Global,
        HumanStatus::Done,
    );
    let project_archived_id = state
        .create(
            "project archived open",
            None,
            TaskScope::Project {
                path: "/repos/gone".into(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    state
        .archive_project("/repos/gone")
        .expect("archive project");
    for title in ["archived open task", "archived done task"] {
        let id = state
            .tasks()
            .iter()
            .find(|task| task.title == title)
            .expect("created task")
            .id;
        state.archive_task(id).expect("archive task");
    }
    let _ = project_archived_id;
    TaskStore::new(&dir).save(&state).expect("seed store");

    let output = list(&[
        "tsk".into(),
        "list".into(),
        "--all".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(output.code, 0);
    assert!(
        output.stdout.contains("visible open task"),
        "the live task lists: {output:?}"
    );
    for absent in [
        "archived open task",
        "archived done task",
        "project archived open",
    ] {
        assert!(
            !output.stdout.contains(absent),
            "the open view must exclude {absent:?}: {output:?}"
        );
    }

    let done = list(&[
        "tsk".into(),
        "list".into(),
        "--done".into(),
        "--all".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(done.code, 0);
    assert!(
        done.stdout.contains("visible done task") && !done.stdout.contains("archived done task"),
        "the done view excludes archived tasks: {done:?}"
    );
}

#[test]
fn every_list_view_omits_notices_and_a_notice_uuid_is_unknown() {
    let dir = temp_state_dir("notices-omitted");
    let mut state = DomainState::new();
    create_task(
        &mut state,
        "ordinary task",
        TaskScope::Global,
        HumanStatus::Ready,
    );
    create_task(
        &mut state,
        "ordinary done",
        TaskScope::Global,
        HumanStatus::Done,
    );
    let notice = |state: &mut DomainState, catalog_id: &str, title: &str, status| {
        state
            .create_notice(
                catalog_id,
                title,
                None,
                status,
                TaskScope::Global,
                Vec::new(),
            )
            .expect("create notice")
    };
    let open_notice = notice(&mut state, "welcome", "open notice", HumanStatus::Ready);
    notice(&mut state, "seen", "done notice", HumanStatus::Done);
    let deleted_notice = notice(&mut state, "gone", "deleted notice", HumanStatus::Ready);
    state.soft_delete(deleted_notice).expect("soft delete");
    let archived_notice = notice(&mut state, "filed", "archived notice", HumanStatus::Ready);
    state.archive_task(archived_notice).expect("archive");
    TaskStore::new(&dir).save(&state).expect("seed store");

    let run = |extra: &[&str]| {
        let mut args: Vec<String> = vec!["tsk".into(), "list".into(), "--desk".into()];
        args.extend(extra.iter().map(|flag| flag.to_string()));
        args.push("--state-dir".into());
        args.push(state_dir_arg(&dir));
        list(&args)
    };

    let open = run(&[]);
    assert_eq!(open.code, 0);
    assert_eq!(open.stdout, "READY\n - 1 ordinary task\n");

    let done = run(&["--done"]);
    assert_eq!(done.code, 0);
    assert_eq!(done.stdout, "DONE\n - 2 ordinary done\n");

    let deleted = run(&["--deleted"]);
    assert_eq!(deleted.code, 0);
    assert_eq!(deleted.stdout, "", "a deleted notice is not a deleted row");

    let archived = run(&["--archived"]);
    assert_eq!(archived.code, 0);
    assert_eq!(
        archived.stdout, "",
        "an archived notice is not an archived row"
    );

    let json = run(&["--json"]);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&json.stdout).expect("JSON rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["number"], 1);
    assert!(rows[0].get("notice").is_none());

    let by_uuid = list(&[
        "tsk".into(),
        "list".into(),
        open_notice.to_string(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(by_uuid.code, 2);
    assert!(by_uuid.stderr.contains("unknown task"), "{by_uuid:?}");
}

#[test]
fn list_archived_marks_task_and_project_rows_once_per_id() {
    let dir = temp_state_dir("archived-view");
    let mut state = DomainState::new();
    create_task(
        &mut state,
        "individually archived",
        TaskScope::Global,
        HumanStatus::Ready,
    );
    let individually = state
        .tasks()
        .iter()
        .find(|task| task.title == "individually archived")
        .expect("created")
        .id;
    state.archive_task(individually).expect("archive it");
    create_task(
        &mut state,
        "in archived project",
        TaskScope::Project {
            path: "/repos/gone".into(),
        },
        HumanStatus::Started,
    );
    let _in_project = state
        .tasks()
        .iter()
        .find(|task| task.title == "in archived project")
        .expect("created")
        .id;
    let both_id = state
        .create(
            "archived in archived project",
            None,
            TaskScope::Project {
                path: "/repos/gone".into(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    state.archive_task(both_id).expect("archive it");
    state
        .archive_project("/repos/gone")
        .expect("archive project");
    // One soft-deleted archived task never lists.
    let deleted_id = state
        .create(
            "archived deleted",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    state.archive_task(deleted_id).expect("archive it");
    state.soft_delete(deleted_id).expect("soft delete it");
    TaskStore::new(&dir).save(&state).expect("seed store");

    let human = list(&[
        "tsk".into(),
        "list".into(),
        "--archived".into(),
        "--all".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(human.code, 0, "{:?}", human.stderr);
    let listed: Vec<&str> = human
        .stdout
        .lines()
        .filter(|line| line.trim_start().starts_with("- "))
        .collect();
    assert_eq!(listed.len(), 3, "one row per archived task id: {human:?}");
    assert!(
        human.stdout.contains("individually archived · archived"),
        "{human:?}"
    );
    assert!(
        human
            .stdout
            .contains("in archived project · project archived"),
        "{human:?}"
    );
    assert!(
        human
            .stdout
            .contains("archived in archived project · archived"),
        "{human:?}"
    );
    assert!(
        !human.stdout.contains("archived deleted"),
        "soft-deleted archived tasks never list: {human:?}"
    );

    let json = list(&[
        "tsk".into(),
        "list".into(),
        "--archived".into(),
        "--all".into(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(json.code, 0);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&json.stdout).expect("JSON rows");
    assert_eq!(rows.len(), 3, "one row per id");
    let row_for = |title: &str| {
        rows.iter()
            .find(|row| row["title"] == title)
            .unwrap_or_else(|| panic!("no row for {title}"))
    };
    assert_eq!(row_for("individually archived")["archived"], "archived");
    assert_eq!(
        row_for("in archived project")["archived"],
        "project archived"
    );
    assert_eq!(
        row_for("archived in archived project")["archived"],
        "archived",
        "the task flag wins over the project mark"
    );
}

#[test]
fn archived_conflicts_are_usage_errors() {
    let dir = temp_state_dir("archived-conflicts");
    for (extra, label) in [
        (vec!["--done".to_string()], "--done"),
        (vec!["--deleted".to_string()], "--deleted"),
    ] {
        let mut args = vec![
            "tsk".to_string(),
            "list".to_string(),
            "--archived".to_string(),
            "--state-dir".to_string(),
            state_dir_arg(&dir),
        ];
        args.extend(extra);
        let output = list(&args);
        assert_eq!(output.code, 2, "{label}: {output:?}");
        assert!(
            output.stderr.contains("--archived"),
            "{label} conflict must name --archived: {:?}",
            output.stderr
        );
    }

    let with_task = list(&[
        "tsk".into(),
        "list".into(),
        "--archived".into(),
        "T1".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(with_task.code, 2, "{with_task:?}");
    assert!(
        with_task.stderr.contains("--archived"),
        "task-operand conflict must name --archived: {:?}",
        with_task.stderr
    );
}

#[test]
fn list_open_and_ready_filters_and_json_status() {
    let dir = temp_state_dir("open-ready-filter");
    let mut state = DomainState::new();
    create_task(
        &mut state,
        "inbox row",
        TaskScope::Global,
        HumanStatus::Open,
    );
    create_task(
        &mut state,
        "picked row",
        TaskScope::Global,
        HumanStatus::Ready,
    );
    create_task(
        &mut state,
        "started row",
        TaskScope::Global,
        HumanStatus::Started,
    );
    TaskStore::new(&dir).save(&state).expect("seed store");

    let open = list(&[
        "tsk".into(),
        "list".into(),
        "--open".into(),
        "--json".into(),
        "--desk".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(open.code, 0, "{}", open.stderr);
    let open_rows: Vec<serde_json::Value> = serde_json::from_str(&open.stdout).expect("open JSON");
    assert_eq!(open_rows.len(), 1);
    assert_eq!(open_rows[0]["title"], "inbox row");
    assert_eq!(open_rows[0]["status"], "open");

    let ready = list(&[
        "tsk".into(),
        "list".into(),
        "--ready".into(),
        "--json".into(),
        "--desk".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(ready.code, 0, "{}", ready.stderr);
    let ready_rows: Vec<serde_json::Value> =
        serde_json::from_str(&ready.stdout).expect("ready JSON");
    assert_eq!(ready_rows.len(), 1);
    assert_eq!(ready_rows[0]["title"], "picked row");
    assert_eq!(ready_rows[0]["status"], "ready");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_open_and_ready_conflicts_are_usage_errors() {
    let dir = temp_state_dir("open-ready-conflicts");
    for extra in [
        vec!["--ready".to_string()],
        vec!["--done".to_string()],
        vec!["--deleted".to_string()],
        vec!["--archived".to_string()],
    ] {
        let mut args = vec![
            "tsk".to_string(),
            "list".to_string(),
            "--open".to_string(),
            "--state-dir".to_string(),
            state_dir_arg(&dir),
        ];
        args.extend(extra.clone());
        let output = list(&args);
        assert_eq!(output.code, 2, "{extra:?}: {output:?}");
        assert!(output.stdout.is_empty());
        assert!(output.stderr.contains("cannot be used"));
    }

    let with_task = list(&[
        "tsk".into(),
        "list".into(),
        "T1".into(),
        "--ready".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
    ]);
    assert_eq!(with_task.code, 2, "{with_task:?}");
    assert!(with_task.stderr.contains("cannot be used"));

    let _ = std::fs::remove_dir_all(dir);
}
