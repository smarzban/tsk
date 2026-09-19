use std::io::{Cursor, Read};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use tsk_tui::cli::{parser, run_with};
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

fn temp_state_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tsk-cli-add-{label}-{nanos}-{seq}"));
    std::fs::create_dir_all(&dir).expect("create state directory");
    dir
}

fn add(args: &[String], stdin_is_tty: bool) -> tsk_tui::cli::CliOutput {
    run_with(args, Cursor::new(Vec::<u8>::new()), stdin_is_tty)
}

fn state_dir_arg(dir: &std::path::Path) -> String {
    dir.to_string_lossy().into_owned()
}

struct PanicOnRead;

impl Read for PanicOnRead {
    fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
        panic!("stdin must not be read")
    }
}

fn task_store(dir: &std::path::Path) -> TaskStore {
    TaskStore::new(dir)
}

#[test]
fn add_does_not_seed_agent_profiles() {
    let _env = env_lock();
    let dir = temp_state_dir("no-agent-seed");
    let output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
            "--title".into(),
            "headless task".into(),
        ],
        true,
    );

    assert_eq!(output.code, 0);
    assert!(!dir.join("agents.toml").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn add_thread_flag_applies_to_every_item_and_round_trips() {
    let _env = env_lock();
    let dir = temp_state_dir("thread-flag");
    let output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
            "--title".into(),
            "threaded task".into(),
            "--thread".into(),
            "Release-2026".into(),
        ],
        true,
    );

    assert_eq!(output.code, 0);
    assert_eq!(output.stdout, "added threaded task\n");
    let state = task_store(&dir).load().expect("reload threaded task");
    assert_eq!(state.tasks().len(), 1);
    assert_eq!(state.tasks()[0].thread.as_deref(), Some("release-2026"));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn thread_flag_with_file_is_usage_error_exit_2() {
    let _env = env_lock();
    let dir = temp_state_dir("thread-file-plan");
    let plan = dir.join("plan.json");
    std::fs::write(&plan, r#"[{"title":"from file"}]"#).expect("write plan");
    let output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
            "--thread".into(),
            "release".into(),
            "--file".into(),
            state_dir_arg(&plan),
        ],
        true,
    );
    assert_eq!(output.code, 2);
    assert!(output.stdout.is_empty());
    assert!(output
        .stderr
        .contains("item flags cannot be used with --file"));
    assert!(task_store(&dir)
        .load()
        .expect("load file state")
        .tasks()
        .is_empty());

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn thread_flag_ignores_piped_plan_and_creates_only_flag_task() {
    let _env = env_lock();
    let dir = temp_state_dir("thread-stdin-plan");
    let output = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&dir),
            "--thread",
            "release",
            "-t",
            "flag title",
        ],
        Cursor::new(r#"[{"title":"from stdin"}]"#),
        false,
    );

    assert_eq!(output.code, 0);
    assert_eq!(output.stdout, "added flag title\n");
    let state = task_store(&dir).load().expect("load stdin state");
    assert_eq!(state.tasks().len(), 1);
    assert_eq!(state.tasks()[0].title, "flag title");
    assert_eq!(state.tasks()[0].thread.as_deref(), Some("release"));
    assert!(state.tasks().iter().all(|task| task.title != "from stdin"));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn plan_item_thread_applies_per_item() {
    let _env = env_lock();
    let dir = temp_state_dir("plan-item-thread");
    let plan = dir.join("plan.json");
    std::fs::write(
        &plan,
        r#"[{"title":"threaded from file","thread":"Release-2026"},{"title":"unthreaded from file","thread":null}]"#,
    )
    .expect("write plan");
    let output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
            "--file".into(),
            state_dir_arg(&plan),
        ],
        true,
    );

    assert_eq!(output.code, 0);
    let state = task_store(&dir).load().expect("load file plan state");
    assert_eq!(state.tasks().len(), 2);
    assert_eq!(state.tasks()[0].thread.as_deref(), Some("release-2026"));
    assert_eq!(state.tasks()[1].thread, None);

    let stdin_dir = temp_state_dir("plan-item-thread-stdin");
    let stdin = run_with(
        ["tsk", "add", "--state-dir", &state_dir_arg(&stdin_dir)],
        Cursor::new(r#"[{"title":"threaded from stdin","thread":"ops"}]"#),
        false,
    );
    assert_eq!(stdin.code, 0);
    assert_eq!(
        task_store(&stdin_dir)
            .load()
            .expect("load stdin plan state")
            .tasks()[0]
            .thread
            .as_deref(),
        Some("ops")
    );

    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(stdin_dir);
}

#[test]
fn invalid_thread_flag_is_usage_error_exit_2_nothing_persisted() {
    let _env = env_lock();
    let dir = temp_state_dir("invalid-thread-flag");
    let output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
            "--title".into(),
            "valid title".into(),
            "--thread".into(),
            "not_valid".into(),
        ],
        true,
    );

    assert_eq!(output.code, 2);
    assert!(output.stdout.is_empty());
    assert!(output.stderr.contains("invalid thread"));
    assert!(task_store(&dir)
        .load()
        .expect("load state")
        .tasks()
        .is_empty());

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn invalid_plan_item_thread_fails_item_exit_1_others_persist() {
    let _env = env_lock();
    let dir = temp_state_dir("invalid-plan-thread");
    let output = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&dir),
            "--file",
            "-",
        ],
        Cursor::new(
            r#"[{"title":"valid thread","thread":"release"},{"title":"invalid thread","thread":"bad_name"},{"title":"valid unthreaded","thread":null}]"#,
        ),
        true,
    );

    assert_eq!(output.code, 1);
    let result: serde_json::Value = serde_json::from_str(&output.stdout).expect("plan result");
    assert_eq!(result["created"].as_array().expect("created").len(), 2);
    assert_eq!(result["failed"][0]["i"], 1);
    assert_eq!(result["failed"][0]["title"], "invalid thread");
    assert_eq!(result["failed"][0]["code"], "invalid-thread");
    let state = task_store(&dir).load().expect("load state");
    assert_eq!(state.tasks().len(), 2);
    assert_eq!(state.tasks()[0].thread.as_deref(), Some("release"));
    assert_eq!(state.tasks()[1].thread, None);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn idempotency_key_includes_thread_both_directions() {
    let _env = env_lock();
    let threaded_first_dir = temp_state_dir("thread-idempotency-threaded-first");
    let threaded_first = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&threaded_first_dir),
            "--title".into(),
            "same title".into(),
            "--desk".into(),
            "--thread".into(),
            "Release".into(),
        ],
        true,
    );
    assert_eq!(threaded_first.code, 0);
    let unthreaded_second = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&threaded_first_dir),
            "--title".into(),
            "same title".into(),
            "--desk".into(),
        ],
        true,
    );
    assert_eq!(unthreaded_second.code, 0);
    assert_eq!(
        task_store(&threaded_first_dir)
            .load()
            .expect("load threaded-first state")
            .tasks()
            .len(),
        2
    );

    let unthreaded_first_dir = temp_state_dir("thread-idempotency-unthreaded-first");
    let unthreaded_first = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&unthreaded_first_dir),
            "--title".into(),
            "same title".into(),
            "--desk".into(),
        ],
        true,
    );
    assert_eq!(unthreaded_first.code, 0);
    let threaded_second = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&unthreaded_first_dir),
            "--title".into(),
            "same title".into(),
            "--desk".into(),
            "--thread".into(),
            "release".into(),
        ],
        true,
    );
    assert_eq!(threaded_second.code, 0);
    let normalized_duplicate = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&unthreaded_first_dir),
            "--title".into(),
            "same title".into(),
            "--desk".into(),
            "--thread".into(),
            "RELEASE".into(),
        ],
        true,
    );
    assert_eq!(normalized_duplicate.code, 0);
    assert_eq!(normalized_duplicate.stdout, "task already exists\n");
    assert_eq!(
        task_store(&unthreaded_first_dir)
            .load()
            .expect("load unthreaded-first state")
            .tasks()
            .len(),
        2
    );

    let plan_dir = temp_state_dir("thread-plan-idempotency");
    let plan = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&plan_dir),
            "--file",
            "-",
        ],
        Cursor::new(
            r#"[{"title":"plan title","project":null,"thread":null},{"title":"plan title","project":null,"thread":"Release"},{"title":"plan title","project":null,"thread":"release"}]"#,
        ),
        true,
    );
    assert_eq!(plan.code, 0);
    let result: serde_json::Value = serde_json::from_str(&plan.stdout).expect("plan result");
    assert_eq!(result["created"].as_array().expect("created").len(), 2);
    assert_eq!(result["existing"][0]["i"], 2);
    assert_eq!(
        task_store(&plan_dir)
            .load()
            .expect("load plan state")
            .tasks()
            .len(),
        2
    );

    let _ = std::fs::remove_dir_all(threaded_first_dir);
    let _ = std::fs::remove_dir_all(unthreaded_first_dir);
    let _ = std::fs::remove_dir_all(plan_dir);
}

#[cfg(unix)]
struct ReadOnlyDir {
    path: PathBuf,
    permissions: std::fs::Permissions,
}

#[cfg(unix)]
impl ReadOnlyDir {
    fn new(path: &std::path::Path) -> Self {
        let permissions = std::fs::metadata(path)
            .expect("stat state directory")
            .permissions();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o555))
            .expect("make state directory read-only");
        Self {
            path: path.to_owned(),
            permissions,
        }
    }
}

#[cfg(unix)]
impl Drop for ReadOnlyDir {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.path, self.permissions.clone());
    }
}

#[test]
fn flag_add_creates_ready_task_and_prints_added_title() {
    let _env = env_lock();
    let dir = temp_state_dir("flag");
    let output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
            "--title".into(),
            "  hello  world  ".into(),
        ],
        true,
    );

    assert_eq!(output.code, 0);
    assert_eq!(output.stdout, "added hello  world\n");
    assert!(output.stderr.is_empty());
    let state = task_store(&dir).load().expect("load state");
    assert_eq!(state.tasks().len(), 1);
    let task = &state.tasks()[0];
    assert_eq!(task.title, "hello  world");
    assert_eq!(task.status, HumanStatus::Open);
    assert!(task.notes.is_none());
    assert_eq!(task.provenance, ProvenanceOrigin::Capture);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn flag_add_existing_trimmed_title_and_scope_is_a_successful_noop() {
    let _env = env_lock();
    let dir = temp_state_dir("flag-existing");
    let first = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
            "--title".into(),
            "same task".into(),
            "--notes".into(),
            "original notes".into(),
            "--desk".into(),
        ],
        true,
    );
    assert_eq!(first.code, 0);
    let id = task_store(&dir).load().expect("load task").tasks()[0].id;
    let mut state = task_store(&dir).load().expect("load task for status");
    state
        .set_status(id, HumanStatus::Done)
        .expect("set existing task status");
    task_store(&dir).save(&state).expect("save status");
    let state_file = dir.join("tsk.json");
    let before = std::fs::read(&state_file).expect("read seeded state");
    let before_mtime = std::fs::metadata(&state_file)
        .expect("stat seeded state")
        .modified()
        .expect("state mtime");
    #[cfg(unix)]
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555))
        .expect("make state dir unwritable");

    let duplicate = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
            "--title".into(),
            "  same task  ".into(),
            "--notes".into(),
            "different notes".into(),
            "--desk".into(),
        ],
        true,
    );

    #[cfg(unix)]
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
        .expect("restore state dir permissions");

    assert_eq!(duplicate.code, 0);
    assert_eq!(duplicate.stdout, "task already exists\n");
    assert!(duplicate.stderr.is_empty());
    assert_eq!(
        std::fs::read(&state_file).expect("read unchanged state"),
        before
    );
    assert_eq!(
        std::fs::metadata(&state_file)
            .expect("stat unchanged state")
            .modified()
            .expect("state mtime"),
        before_mtime
    );
    let state = task_store(&dir).load().expect("reload state");
    assert_eq!(state.tasks().len(), 1);
    assert_eq!(state.tasks()[0].status, HumanStatus::Done);
    assert_eq!(state.tasks()[0].notes.as_deref(), Some("original notes"));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn flag_add_ignores_soft_deleted_title_and_scope_matches() {
    let _env = env_lock();
    let dir = temp_state_dir("flag-soft-deleted");
    let store = task_store(&dir);
    let mut seeded = DomainState::new();
    let id = seeded
        .create(
            "same task",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("seed task");
    seeded.soft_delete(id).expect("soft delete seed");
    store.save(&seeded).expect("save seed");

    let output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
            "--title".into(),
            "same task".into(),
            "--desk".into(),
        ],
        true,
    );

    assert_eq!(output.code, 0);
    assert_eq!(output.stdout, "added same task\n");
    assert_eq!(store.load().expect("reload").tasks().len(), 2);

    let _ = std::fs::remove_dir_all(dir);
}

/// Seeded notice rows live at the desk with no `T` number, so a title collision must
/// create an ordinary task instead of resolving the notice as `existing` (which would
/// unwrap-panic on its missing number).
#[test]
fn flag_add_with_a_seeded_notice_title_creates_an_ordinary_task() {
    let _env = env_lock();
    let dir = temp_state_dir("flag-notice-title");
    let store = task_store(&dir);
    assert_eq!(tsk_tui::guides::seed_on_open(&store), Ok(4));
    let notice_title = tsk_tui::guides::CATALOG[0].title;

    let output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
            "--title".into(),
            notice_title.into(),
            "--desk".into(),
        ],
        true,
    );

    assert_eq!(output.code, 0, "a notice title must not panic the CLI");
    assert_eq!(output.stdout, format!("added {notice_title}\n"));
    let state = store.load().expect("reload state");
    assert_eq!(
        state.tasks().iter().filter(|task| task.is_notice()).count(),
        4
    );
    let ordinary: Vec<_> = state
        .tasks()
        .iter()
        .filter(|task| !task.is_notice())
        .collect();
    assert_eq!(ordinary.len(), 1);
    assert_eq!(ordinary[0].title, notice_title);
    assert_eq!(ordinary[0].scope, TaskScope::Global);
    assert_eq!(ordinary[0].board_identifier().as_deref(), Some("T1"));

    let _ = std::fs::remove_dir_all(dir);
}

/// The JSON plan shares `existing_task` with flag add: a notice-titled item must land in
/// `created` with a `T` number, never panic mid-batch.
#[test]
fn plan_add_with_a_seeded_notice_title_creates_an_ordinary_task() {
    let _env = env_lock();
    let dir = temp_state_dir("plan-notice-title");
    let store = task_store(&dir);
    assert_eq!(tsk_tui::guides::seed_on_open(&store), Ok(4));
    let notice_title = tsk_tui::guides::CATALOG[0].title;

    let plan = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&dir),
            "--file",
            "-",
        ],
        Cursor::new(
            serde_json::to_string(&serde_json::json!([
                {"title": notice_title, "project": null},
                {"title": "ordinary plan item", "project": null}
            ]))
            .expect("plan payload"),
        ),
        true,
    );

    assert_eq!(plan.code, 0, "a notice title must not panic the plan batch");
    let result: serde_json::Value = serde_json::from_str(&plan.stdout).expect("plan result");
    assert_eq!(result["failed"].as_array().expect("failed").len(), 0);
    let created = result["created"].as_array().expect("created");
    assert_eq!(created.len(), 2);
    assert_eq!(created[0]["i"], 0);
    assert_eq!(created[0]["number"], 1);
    let state = store.load().expect("reload state");
    let ordinary: Vec<_> = state
        .tasks()
        .iter()
        .filter(|task| !task.is_notice())
        .collect();
    assert_eq!(
        ordinary
            .iter()
            .map(|task| task.title.as_str())
            .collect::<Vec<_>>(),
        vec![notice_title, "ordinary plan item"]
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn flag_add_does_not_read_piped_stdin() {
    let _env = env_lock();
    let dir = temp_state_dir("piped-stdin");
    let output = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&dir),
            "-t",
            "pipe safe",
        ],
        PanicOnRead,
        false,
    );

    assert_eq!(output.code, 0);
    assert_eq!(output.stdout, "added pipe safe\n");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn flag_add_keeps_non_whitespace_notes_and_drops_whitespace_notes() {
    let _env = env_lock();
    let notes_dir = temp_state_dir("notes");
    let notes_output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&notes_dir),
            "-t".into(),
            "notes".into(),
            "--notes".into(),
            "line one\nline two".into(),
        ],
        true,
    );
    assert_eq!(notes_output.code, 0);
    assert_eq!(
        task_store(&notes_dir).load().expect("load notes").tasks()[0]
            .notes
            .as_deref(),
        Some("line one\nline two")
    );

    let blank_dir = temp_state_dir("blank-notes");
    let blank_output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&blank_dir),
            "-t".into(),
            "blank notes".into(),
            "-n".into(),
            " \t\n ".into(),
        ],
        true,
    );
    assert_eq!(blank_output.code, 0);
    assert!(task_store(&blank_dir)
        .load()
        .expect("load blank notes")
        .tasks()[0]
        .notes
        .is_none());

    let _ = std::fs::remove_dir_all(notes_dir);
    let _ = std::fs::remove_dir_all(blank_dir);
}

#[test]
fn flag_add_refuses_an_unknown_project_without_persisting() {
    let _env = env_lock();
    let dir = temp_state_dir("unknown-project");
    let output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
            "-t".into(),
            "misfiled task".into(),
            "-p".into(),
            "atlss".into(),
        ],
        true,
    );

    assert_eq!(output.code, 1, "{output:?}");
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        "tsk add: unknown-project: project atlss is not on the board\n"
    );
    assert!(task_store(&dir)
        .load()
        .expect("load state")
        .tasks()
        .is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn flag_add_resolves_global_and_project_basename_scopes() {
    let _env = env_lock();
    let global_dir = temp_state_dir("global");
    let global_output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&global_dir),
            "-t".into(),
            "global task".into(),
            "--desk".into(),
        ],
        true,
    );
    assert_eq!(global_output.code, 0);
    assert_eq!(
        task_store(&global_dir).load().expect("load global").tasks()[0].scope,
        TaskScope::Global
    );

    let project_dir = temp_state_dir("project");
    let store = task_store(&project_dir);
    let mut seeded = DomainState::new();
    seeded
        .create(
            "existing project",
            None,
            TaskScope::Project {
                path: "/projects/Widget".into(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("seed project");
    store.save(&seeded).expect("save seed");
    let project_output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&project_dir),
            "-t".into(),
            "project task".into(),
            "--project".into(),
            "widget".into(),
        ],
        true,
    );
    assert_eq!(project_output.code, 0);
    assert_eq!(
        task_store(&project_dir)
            .load()
            .expect("load project")
            .tasks()[1]
            .scope,
        TaskScope::Project {
            path: "/projects/Widget".into()
        }
    );

    let _ = std::fs::remove_dir_all(global_dir);
    let _ = std::fs::remove_dir_all(project_dir);
}

#[test]
fn flag_add_uses_the_invocation_default_scope() {
    let _env = env_lock();
    let repo = temp_state_dir("invocation-repo");
    std::fs::create_dir(repo.join(".git")).expect("create git marker");
    let dir = temp_state_dir("invocation-state");
    let prior = std::env::var_os("HERDR_PLUGIN_CONTEXT_JSON");
    let context = format!(
        r#"{{"focused_pane_cwd":{}}}"#,
        serde_json::to_string(&repo).unwrap()
    );
    // SAFETY: ENV_LOCK serializes this test's process-wide environment mutation.
    unsafe { std::env::set_var("HERDR_PLUGIN_CONTEXT_JSON", context) };

    let output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
            "-t".into(),
            "invocation scope".into(),
        ],
        true,
    );

    match prior {
        Some(value) => {
            // SAFETY: ENV_LOCK serializes this test's process-wide environment mutation.
            unsafe { std::env::set_var("HERDR_PLUGIN_CONTEXT_JSON", value) };
        }
        None => {
            // SAFETY: ENV_LOCK serializes this test's process-wide environment mutation.
            unsafe { std::env::remove_var("HERDR_PLUGIN_CONTEXT_JSON") };
        }
    }

    assert_eq!(output.code, 0);
    assert_eq!(
        task_store(&dir)
            .load()
            .expect("load invocation state")
            .tasks()[0]
            .scope,
        TaskScope::Project {
            path: repo.to_string_lossy().into_owned()
        }
    );

    let _ = std::fs::remove_dir_all(repo);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn flag_add_outside_git_defaults_to_desk() {
    let _env = env_lock();
    let outside = temp_state_dir("invocation-outside");
    let dir = temp_state_dir("outside-state");
    let prior = std::env::var_os("HERDR_PLUGIN_CONTEXT_JSON");
    let context = format!(
        r#"{{"focused_pane_cwd":{}}}"#,
        serde_json::to_string(&outside).unwrap()
    );
    // SAFETY: ENV_LOCK serializes this test's process-wide environment mutation.
    unsafe { std::env::set_var("HERDR_PLUGIN_CONTEXT_JSON", context) };

    let output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
            "-t".into(),
            "outside scope".into(),
        ],
        true,
    );

    match prior {
        Some(value) => {
            // SAFETY: ENV_LOCK serializes this test's process-wide environment mutation.
            unsafe { std::env::set_var("HERDR_PLUGIN_CONTEXT_JSON", value) };
        }
        None => {
            // SAFETY: ENV_LOCK serializes this test's process-wide environment mutation.
            unsafe { std::env::remove_var("HERDR_PLUGIN_CONTEXT_JSON") };
        }
    }

    assert_eq!(output.code, 0);
    assert_eq!(
        task_store(&dir).load().expect("load outside state").tasks()[0].scope,
        TaskScope::Global
    );

    let _ = std::fs::remove_dir_all(outside);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn flag_add_requires_a_title() {
    let _env = env_lock();
    let dir = temp_state_dir("missing-title");
    let output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
        ],
        true,
    );

    assert_eq!(output.code, 2);
    assert!(output.stdout.is_empty());
    assert!(task_store(&dir)
        .load()
        .expect("load state")
        .tasks()
        .is_empty());

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn flag_add_refuses_empty_and_control_character_titles() {
    let _env = env_lock();
    let empty_dir = temp_state_dir("empty-title");
    let empty = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&empty_dir),
            "-t".into(),
            "".into(),
        ],
        true,
    );
    assert_eq!(empty.code, 1);
    assert!(empty.stdout.is_empty());
    assert!(task_store(&empty_dir)
        .load()
        .expect("load empty state")
        .tasks()
        .is_empty());

    let invalid_dir = temp_state_dir("invalid-title");
    let invalid = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&invalid_dir),
            "-t".into(),
            "hello\nworld".into(),
        ],
        true,
    );
    assert_eq!(invalid.code, 1);
    assert!(invalid.stdout.is_empty());
    assert!(invalid.stderr.contains("invalid-title"));
    assert!(task_store(&invalid_dir)
        .load()
        .expect("load invalid state")
        .tasks()
        .is_empty());

    let end_control_dir = temp_state_dir("end-control-title");
    let end_control = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&end_control_dir),
            "-t".into(),
            "hello\n".into(),
        ],
        true,
    );
    assert_eq!(end_control.code, 1);
    assert!(end_control.stdout.is_empty());
    assert!(end_control.stderr.contains("invalid-title"));
    assert!(task_store(&end_control_dir)
        .load()
        .expect("load end-control state")
        .tasks()
        .is_empty());

    let whitespace_dir = temp_state_dir("whitespace-title");
    let whitespace = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&whitespace_dir),
            "-t".into(),
            "   ".into(),
        ],
        true,
    );
    assert_eq!(whitespace.code, 1);
    assert!(whitespace.stdout.is_empty());
    assert!(task_store(&whitespace_dir)
        .load()
        .expect("load whitespace state")
        .tasks()
        .is_empty());

    let _ = std::fs::remove_dir_all(empty_dir);
    let _ = std::fs::remove_dir_all(invalid_dir);
    let _ = std::fs::remove_dir_all(end_control_dir);
    let _ = std::fs::remove_dir_all(whitespace_dir);
}

#[test]
fn flag_add_rejects_global_with_project() {
    let _env = env_lock();
    let dir = temp_state_dir("global-project");
    let output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
            "-t".into(),
            "conflict".into(),
            "--desk".into(),
            "-p".into(),
            "/projects/a".into(),
        ],
        true,
    );

    assert_eq!(output.code, 2);
    assert!(output.stdout.is_empty());
    assert!(task_store(&dir)
        .load()
        .expect("load state")
        .tasks()
        .is_empty());

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn add_without_item_flags_on_a_tty_is_usage() {
    let _env = env_lock();
    let dir = temp_state_dir("tty");
    let output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
        ],
        true,
    );

    assert_eq!(output.code, 2);
    assert!(output.stdout.is_empty());
    assert!(task_store(&dir)
        .load()
        .expect("load state")
        .tasks()
        .is_empty());

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn mixed_plan_persists_only_valid_items_exits_1() {
    let _env = env_lock();
    let dir = temp_state_dir("mixed-plan");
    let fixture = format!(
        "{}/tests/fixtures/cli_add_mixed_plan.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
            "--file".into(),
            fixture,
        ],
        true,
    );

    assert_eq!(output.code, 1);
    assert!(output.stderr.is_empty());
    let result: serde_json::Value = serde_json::from_str(&output.stdout).expect("tiny result JSON");
    let created = result["created"].as_array().expect("created array");
    assert_eq!(created.len(), 1);
    assert_eq!(created[0]["i"], 0);
    assert!(created[0]["id"].as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(created[0]["title"], "ok");
    assert!(created[0].get("notes").is_none());
    let failed = result["failed"].as_array().expect("failed array");
    assert_eq!(failed.len(), 2);
    assert_eq!(failed[0]["i"], 1);
    assert_eq!(failed[0]["title"], "");
    assert_eq!(failed[0]["code"], "empty-title");
    assert_eq!(failed[1]["i"], 2);
    assert!(failed[1]["title"].is_null());
    assert_eq!(failed[1]["code"], "invalid-item");
    assert_eq!(
        task_store(&dir).load().expect("load state").tasks()[0].title,
        "ok"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn plan_with_only_existing_tasks_is_a_successful_read_only_noop() {
    let _env = env_lock();
    let dir = temp_state_dir("plan-existing");
    let store = task_store(&dir);
    let mut seeded = DomainState::new();
    let id = seeded
        .create(
            "same task",
            Some("old notes".into()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("seed task");
    seeded
        .set_status(id, HumanStatus::Review)
        .expect("set status");
    store.save(&seeded).expect("save seed");
    let state_file = dir.join("tsk.json");
    let before = std::fs::read(&state_file).expect("read seeded state");
    let before_mtime = std::fs::metadata(&state_file)
        .expect("stat seeded state")
        .modified()
        .expect("state mtime");

    let output = {
        let _read_only = ReadOnlyDir::new(&dir);
        run_with(
            [
                "tsk",
                "add",
                "--state-dir",
                &state_dir_arg(&dir),
                "--file",
                "-",
            ],
            Cursor::new(r#"[{"title":"  same task  ","notes":"new notes","project":null}]"#),
            true,
        )
    };

    assert_eq!(output.code, 0);
    let result: serde_json::Value = serde_json::from_str(&output.stdout).expect("tiny result");
    assert!(result["created"].as_array().expect("created").is_empty());
    assert_eq!(
        result["existing"],
        serde_json::json!([{"i":0,"id":id,"number":1,"title":"same task"}])
    );
    assert!(result["failed"].as_array().expect("failed").is_empty());
    assert_eq!(
        std::fs::read(&state_file).expect("read unchanged state"),
        before
    );
    assert_eq!(
        std::fs::metadata(&state_file)
            .expect("stat unchanged state")
            .modified()
            .expect("state mtime"),
        before_mtime
    );
    assert_eq!(store.load().expect("reload").tasks().len(), 1);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn mixed_plan_exit_1_preserves_created_and_existing_rows_for_failed_only_retry() {
    let _env = env_lock();
    let dir = temp_state_dir("mixed-existing-failed");
    let store = task_store(&dir);
    let mut seeded = DomainState::new();
    let existing_id = seeded
        .create(
            "already exists",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("seed existing task");
    store.save(&seeded).expect("save seed");

    let initial = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&dir),
            "--file",
            "-",
        ],
        Cursor::new(
            r#"[{"title":"created now","project":null},{"title":"already exists","project":null},{"title":"\u0000","project":null}]"#,
        ),
        true,
    );

    assert_eq!(initial.code, 1);
    let result: serde_json::Value = serde_json::from_str(&initial.stdout).expect("tiny result");
    assert_eq!(result["created"].as_array().expect("created").len(), 1);
    assert_eq!(result["created"][0]["i"], 0);
    assert_eq!(
        result["existing"],
        serde_json::json!([{"i":1,"id":existing_id,"number":1,"title":"already exists"}])
    );
    assert_eq!(result["failed"].as_array().expect("failed").len(), 1);
    assert_eq!(result["failed"][0]["i"], 2);
    assert_eq!(result["failed"][0]["code"], "invalid-title");

    let retry = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&dir),
            "--file",
            "-",
        ],
        Cursor::new(r#"[{"title":"recovered failed item","project":null}]"#),
        true,
    );

    assert_eq!(retry.code, 0);
    let retry_result: serde_json::Value =
        serde_json::from_str(&retry.stdout).expect("retry result");
    assert_eq!(
        retry_result["created"].as_array().expect("created").len(),
        1
    );
    assert!(retry_result["existing"]
        .as_array()
        .expect("existing")
        .is_empty());
    assert!(retry_result["failed"]
        .as_array()
        .expect("failed")
        .is_empty());
    let state = store.load().expect("load state after retry");
    assert_eq!(state.tasks().len(), 3);
    assert_eq!(
        state
            .tasks()
            .iter()
            .filter(|task| task.title == "already exists")
            .count(),
        1
    );
    assert_eq!(
        state
            .tasks()
            .iter()
            .filter(|task| task.title == "created now")
            .count(),
        1
    );
    assert_eq!(
        state
            .tasks()
            .iter()
            .filter(|task| task.title == "recovered failed item")
            .count(),
        1
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn plan_marks_earlier_accepted_duplicate_as_existing_and_keeps_scopes_distinct() {
    let _env = env_lock();
    let dir = temp_state_dir("plan-duplicate");
    let project = dir.join("widget");
    std::fs::create_dir(&project).expect("create project directory");
    let plan = format!(
        r#"[{{"title":"same task","project":null}},{{"title":" same task ","project":null}},{{"title":"same task","project":{}}}]"#,
        serde_json::to_string(&project).expect("serialize project path")
    );
    let output = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&dir),
            "--file",
            "-",
        ],
        Cursor::new(plan),
        true,
    );

    assert_eq!(output.code, 0);
    let result: serde_json::Value = serde_json::from_str(&output.stdout).expect("tiny result");
    let created = result["created"].as_array().expect("created");
    assert_eq!(created.len(), 2);
    assert_eq!(created[0]["i"], 0);
    assert_eq!(created[1]["i"], 2);
    assert_eq!(result["existing"][0]["i"], 1);
    assert_eq!(result["existing"][0]["id"], created[0]["id"]);
    assert!(result["failed"].as_array().expect("failed").is_empty());
    let state = task_store(&dir).load().expect("load state");
    assert_eq!(state.tasks().len(), 2);
    assert_eq!(state.tasks()[0].scope, TaskScope::Global);
    assert_eq!(
        state.tasks()[1].scope,
        TaskScope::Project {
            path: project.to_string_lossy().into_owned()
        }
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn plan_reads_dash_file_and_piped_stdin_and_allows_empty_array() {
    let _env = env_lock();
    let dash_dir = temp_state_dir("dash-plan");
    let dash = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&dash_dir),
            "--file",
            "-",
        ],
        Cursor::new(r#"[{"title":"from dash"}]"#),
        true,
    );
    assert_eq!(dash.code, 0);
    assert_eq!(
        task_store(&dash_dir)
            .load()
            .expect("load dash state")
            .tasks()[0]
            .title,
        "from dash"
    );

    let piped_dir = temp_state_dir("piped-plan");
    let piped = run_with(
        ["tsk", "add", "--state-dir", &state_dir_arg(&piped_dir)],
        Cursor::new(r#"[{"title":"from pipe"}]"#),
        false,
    );
    assert_eq!(piped.code, 0);
    assert_eq!(
        task_store(&piped_dir)
            .load()
            .expect("load piped state")
            .tasks()[0]
            .title,
        "from pipe"
    );

    let empty_dir = temp_state_dir("empty-plan");
    let empty = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&empty_dir),
            "--file",
            "-",
        ],
        Cursor::new("[]"),
        true,
    );
    assert_eq!(empty.code, 0);
    assert_eq!(
        empty.stdout,
        "{\"created\":[],\"existing\":[],\"failed\":[]}\n"
    );
    assert!(task_store(&empty_dir)
        .load()
        .expect("load empty state")
        .tasks()
        .is_empty());

    let _ = std::fs::remove_dir_all(dash_dir);
    let _ = std::fs::remove_dir_all(piped_dir);
    let _ = std::fs::remove_dir_all(empty_dir);
}

#[test]
fn plan_usage_errors_persist_nothing_and_do_not_read_mixed_stdin() {
    let _env = env_lock();
    let object_dir = temp_state_dir("object-plan");
    let object = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&object_dir),
            "--file",
            "-",
        ],
        Cursor::new(r#"{"title":"not an array"}"#),
        true,
    );
    assert_eq!(object.code, 2);
    assert!(object.stdout.is_empty());
    let malformed = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&object_dir),
            "--file",
            "-",
        ],
        Cursor::new("["),
        true,
    );
    assert_eq!(malformed.code, 2);
    assert!(malformed.stdout.is_empty());
    assert!(task_store(&object_dir)
        .load()
        .expect("load object state")
        .tasks()
        .is_empty());

    let mixed_dir = temp_state_dir("mixed-plan-flags");
    let mixed = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&mixed_dir),
            "-t",
            "never read",
            "--file",
            "-",
        ],
        PanicOnRead,
        false,
    );
    assert_eq!(mixed.code, 2);
    assert!(mixed.stdout.is_empty());
    assert!(task_store(&mixed_dir)
        .load()
        .expect("load mixed state")
        .tasks()
        .is_empty());

    let _ = std::fs::remove_dir_all(object_dir);
    let _ = std::fs::remove_dir_all(mixed_dir);
}

#[test]
fn plan_projects_and_item_validation_follow_the_contract() {
    let _env = env_lock();
    let repo = temp_state_dir("plan-default-repo");
    std::fs::create_dir(repo.join(".git")).expect("create git marker");
    let dir = temp_state_dir("plan-validation");
    let prior = std::env::var_os("HERDR_PLUGIN_CONTEXT_JSON");
    let context = format!(
        r#"{{"focused_pane_cwd":{}}}"#,
        serde_json::to_string(&repo).expect("serialize repo")
    );
    // SAFETY: ENV_LOCK serializes this test's process-wide environment mutation.
    unsafe { std::env::set_var("HERDR_PLUGIN_CONTEXT_JSON", context) };

    let output = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&dir),
            "--file",
            "-",
        ],
        Cursor::new(
            r#"[{"title":"global","project":null},{"title":"default"},{"title":"extra","unknown":true,"notes":null},{"title":"  bad notes  ","notes":42},{"title":"bad project","project":42},{"title":"bad\n"}]"#,
        ),
        true,
    );

    match prior {
        Some(value) => {
            // SAFETY: ENV_LOCK serializes this test's process-wide environment mutation.
            unsafe { std::env::set_var("HERDR_PLUGIN_CONTEXT_JSON", value) };
        }
        None => {
            // SAFETY: ENV_LOCK serializes this test's process-wide environment mutation.
            unsafe { std::env::remove_var("HERDR_PLUGIN_CONTEXT_JSON") };
        }
    }

    assert_eq!(output.code, 1);
    let result: serde_json::Value = serde_json::from_str(&output.stdout).expect("tiny result JSON");
    assert_eq!(result["created"].as_array().expect("created").len(), 3);
    assert_eq!(result["failed"][0]["code"], "invalid-item");
    assert_eq!(result["failed"][0]["title"], "bad notes");
    assert_eq!(result["failed"][1]["code"], "invalid-item");
    assert_eq!(result["failed"][1]["title"], "bad project");
    assert_eq!(result["failed"][2]["code"], "invalid-title");
    let state = task_store(&dir).load().expect("load state");
    assert_eq!(state.tasks()[0].scope, TaskScope::Global);
    assert_eq!(
        state.tasks()[1].scope,
        TaskScope::Project {
            path: repo.to_string_lossy().into_owned()
        }
    );
    assert_eq!(state.tasks()[2].title, "extra");
    assert!(state.tasks()[2].notes.is_none());

    let _ = std::fs::remove_dir_all(repo);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn plan_unknown_project_refuses_only_that_item_and_persists_valid_siblings() {
    let _env = env_lock();
    let dir = temp_state_dir("plan-unknown-project");
    let output = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&dir),
            "--file",
            "-",
        ],
        Cursor::new(
            r#"[{"title":"typo","project":"atlss"},{"title":"desk sibling","project":null}]"#,
        ),
        true,
    );

    assert_eq!(output.code, 1, "{output:?}");
    let report: serde_json::Value = serde_json::from_str(&output.stdout).expect("plan report JSON");
    assert_eq!(report["failed"].as_array().expect("failed").len(), 1);
    assert_eq!(report["failed"][0]["i"], 0);
    assert_eq!(report["failed"][0]["title"], "typo");
    assert_eq!(report["failed"][0]["code"], "unknown-project");
    assert_eq!(
        report["failed"][0]["error"],
        "project atlss is not on the board"
    );
    assert_eq!(report["created"].as_array().expect("created").len(), 1);
    assert_eq!(report["created"][0]["title"], "desk sibling");
    let state = task_store(&dir).load().expect("load state");
    assert_eq!(state.tasks().len(), 1);
    assert_eq!(state.tasks()[0].title, "desk sibling");
    assert_eq!(state.tasks()[0].scope, TaskScope::Global);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn plan_project_string_uses_the_shared_basename_resolver() {
    let _env = env_lock();
    let dir = temp_state_dir("plan-project");
    let store = task_store(&dir);
    let mut seeded = DomainState::new();
    seeded
        .create(
            "existing project",
            None,
            TaskScope::Project {
                path: "/projects/Widget".into(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("seed project");
    store.save(&seeded).expect("save seed");

    let output = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&dir),
            "--file",
            "-",
        ],
        Cursor::new(r#"[{"title":"resolved","project":"widget"}]"#),
        true,
    );

    assert_eq!(output.code, 0);
    assert_eq!(
        task_store(&dir).load().expect("load state").tasks()[1].scope,
        TaskScope::Project {
            path: "/projects/Widget".into()
        }
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn plan_project_resolution_is_independent_of_item_order() {
    let _env = env_lock();
    let forward_dir = temp_state_dir("plan-project-order-forward");
    let forward_project = forward_dir.join("new-widget");
    std::fs::create_dir(&forward_project).expect("create forward project");
    let forward_plan = format!(
        r#"[{{"title":"explicit first","project":{}}},{{"title":"bare second","project":"new-widget"}}]"#,
        serde_json::to_string(&forward_project).expect("serialize forward project")
    );
    let forward = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&forward_dir),
            "--file",
            "-",
        ],
        Cursor::new(forward_plan),
        true,
    );
    assert_eq!(forward.code, 1, "{forward:?}");
    let forward_report: serde_json::Value =
        serde_json::from_str(&forward.stdout).expect("forward report");
    assert_eq!(forward_report["created"][0]["i"], 0);
    assert_eq!(forward_report["failed"][0]["i"], 1);
    assert_eq!(forward_report["failed"][0]["code"], "unknown-project");
    let forward_state = task_store(&forward_dir).load().expect("load forward state");
    assert_eq!(forward_state.tasks().len(), 1);
    assert_eq!(
        forward_state.tasks()[0].scope,
        TaskScope::Project {
            path: forward_project.to_string_lossy().into_owned()
        }
    );

    let reverse_dir = temp_state_dir("plan-project-order-reverse");
    let reverse_project = reverse_dir.join("new-widget");
    std::fs::create_dir(&reverse_project).expect("create reverse project");
    let reverse_plan = format!(
        r#"[{{"title":"bare first","project":"new-widget"}},{{"title":"explicit second","project":{}}}]"#,
        serde_json::to_string(&reverse_project).expect("serialize reverse project")
    );
    let reverse = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&reverse_dir),
            "--file",
            "-",
        ],
        Cursor::new(reverse_plan),
        true,
    );
    assert_eq!(reverse.code, 1, "{reverse:?}");
    let reverse_report: serde_json::Value =
        serde_json::from_str(&reverse.stdout).expect("reverse report");
    assert_eq!(reverse_report["failed"][0]["i"], 0);
    assert_eq!(reverse_report["failed"][0]["code"], "unknown-project");
    assert_eq!(reverse_report["created"][0]["i"], 1);
    let reverse_state = task_store(&reverse_dir).load().expect("load reverse state");
    assert_eq!(reverse_state.tasks().len(), 1);
    assert_eq!(
        reverse_state.tasks()[0].scope,
        TaskScope::Project {
            path: reverse_project.to_string_lossy().into_owned()
        }
    );

    let _ = std::fs::remove_dir_all(forward_dir);
    let _ = std::fs::remove_dir_all(reverse_dir);
}

#[test]
fn flag_add_rejects_flag_like_item_values_without_persisting() {
    let _env = env_lock();

    for (label, item_flags) in [
        ("title", vec!["--title", "--desk"]),
        (
            "notes",
            vec!["--title", "ordinary title", "--notes", "--desk"],
        ),
        (
            "project",
            vec!["--title", "ordinary title", "--project", "--desk"],
        ),
    ] {
        let dir = temp_state_dir(label);
        let mut args = vec![
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
        ];
        args.extend(item_flags.into_iter().map(String::from));

        let output = add(&args, true);

        assert_eq!(output.code, 2, "{label}");
        assert!(output.stdout.is_empty(), "{label}");
        assert!(output.stderr.contains("missing value"), "{label}");
        assert!(task_store(&dir)
            .load()
            .expect("load state")
            .tasks()
            .is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn flag_add_equals_forms_allow_dash_leading_values() {
    let _env = env_lock();
    let dir = temp_state_dir("dash-leading-values");
    let mut seeded = DomainState::new();
    seeded
        .create(
            "project anchor",
            None,
            TaskScope::Project {
                path: "/projects/-maintenance".into(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create project anchor");
    task_store(&dir).save(&seeded).expect("save project anchor");
    let output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
            "--title=-fix parser".into(),
            "--notes=-5 degrees".into(),
            "--project=-maintenance".into(),
        ],
        true,
    );

    assert_eq!(output.code, 0);
    assert_eq!(output.stdout, "added -fix parser\n");
    let state = task_store(&dir).load().expect("load state");
    let task = &state.tasks()[1];
    assert_eq!(task.title, "-fix parser");
    assert_eq!(task.notes.as_deref(), Some("-5 degrees"));
    assert_eq!(
        task.scope,
        TaskScope::Project {
            path: "/projects/-maintenance".into()
        }
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn flag_add_equals_state_dir_and_file_forms_accept_dash_leading_values() {
    let cwd = temp_state_dir("equals-dash-paths");
    std::fs::write(
        cwd.join("-plan.json"),
        r#"[{"title":"equals file task","project":null}]"#,
    )
    .expect("write dash-leading plan");
    let binary =
        std::env::var("CARGO_BIN_EXE_tsk").expect("Cargo must provide the tsk binary path");

    let output = Command::new(binary)
        .current_dir(&cwd)
        .args(["add", "--state-dir=-state", "--file=-plan.json"])
        .output()
        .expect("run equals forms");

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    assert_eq!(
        task_store(&cwd.join("-state"))
            .load()
            .expect("load equals state")
            .tasks()[0]
            .title,
        "equals file task"
    );

    let stdin = parser::parse_flag_add(&["tsk".into(), "add".into(), "--file=-".into()])
        .expect("parse stdin file marker");
    assert_eq!(stdin.file, Some(PathBuf::from("-")));

    let _ = std::fs::remove_dir_all(cwd);
}

#[test]
fn flag_add_rejects_flag_like_state_dir_and_file_values_without_mutating() {
    let _env = env_lock();
    let cwd = temp_state_dir("flag-like-global-value");
    let state_dir = temp_state_dir("flag-like-global-state");
    let binary =
        std::env::var("CARGO_BIN_EXE_tsk").expect("Cargo must provide the tsk binary path");

    let state_dir_output = Command::new(&binary)
        .current_dir(&cwd)
        .env("TSK_STATE_DIR", &state_dir)
        .args(["add", "--state-dir", "--desk", "--title", "junk"])
        .output()
        .expect("run flag-like state-dir value");
    assert_eq!(state_dir_output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&state_dir_output.stderr).contains("missing value for --state-dir")
    );
    assert!(!cwd.join("--desk").exists());

    let file = Command::new(binary)
        .current_dir(&cwd)
        .env("TSK_STATE_DIR", &state_dir)
        .args(["add", "--file", "--desk"])
        .output()
        .expect("run flag-like file value");
    assert_eq!(file.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&file.stderr).contains("missing value for --file"));
    assert!(!cwd.join("--desk").exists());

    let _ = std::fs::remove_dir_all(cwd);
    let _ = std::fs::remove_dir_all(state_dir);
}

#[test]
fn flag_add_json_reports_created_and_existing_resolved_tasks() {
    let _env = env_lock();
    let dir = temp_state_dir("json");
    let project = dir.join("project");
    std::fs::create_dir(&project).expect("create project directory");
    let project_text = project.to_string_lossy().into_owned();
    let args = [
        "tsk".into(),
        "add".into(),
        "--json".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
        "-t".into(),
        "  json task  ".into(),
        "-p".into(),
        project_text.clone(),
    ];

    let created = add(&args, true);
    assert_eq!(created.code, 0);
    assert!(created.stderr.is_empty());
    let created: serde_json::Value = serde_json::from_str(&created.stdout).expect("created JSON");
    assert_eq!(created["outcome"], "created");
    assert_eq!(created["title"], "json task");
    assert_eq!(created["project"], project_text);
    let id = created["id"].as_str().expect("created id").to_owned();
    assert_eq!(created.as_object().expect("created object").len(), 5);

    let existing = add(&args, true);
    assert_eq!(existing.code, 0);
    assert!(existing.stderr.is_empty());
    let existing: serde_json::Value =
        serde_json::from_str(&existing.stdout).expect("existing JSON");
    assert_eq!(existing["outcome"], "existing");
    assert_eq!(existing["id"], id);
    assert_eq!(existing["title"], "json task");
    assert_eq!(existing["project"], project.to_string_lossy().as_ref());
    assert_eq!(existing.as_object().expect("existing object").len(), 5);

    let global = add(
        &[
            "tsk".into(),
            "add".into(),
            "--json".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
            "-t".into(),
            "global JSON task".into(),
            "--desk".into(),
        ],
        true,
    );
    assert_eq!(global.code, 0);
    let global: serde_json::Value = serde_json::from_str(&global.stdout).expect("global JSON");
    assert_eq!(global["outcome"], "created");
    assert!(global["project"].is_null());

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn json_created_and_existing_include_number() {
    let _env = env_lock();
    let dir = temp_state_dir("json-number");
    let args = [
        "tsk".into(),
        "add".into(),
        "--json".into(),
        "--desk".into(),
        "--state-dir".into(),
        state_dir_arg(&dir),
        "--title".into(),
        "numbered task".into(),
    ];

    let created = add(&args, true);
    assert_eq!(created.code, 0, "{}", created.stderr);
    let created: serde_json::Value = serde_json::from_str(&created.stdout).expect("created JSON");
    assert_eq!(created["outcome"], "created");
    assert_eq!(created["number"], 1);

    let existing = add(&args, true);
    assert_eq!(existing.code, 0, "{}", existing.stderr);
    let existing: serde_json::Value =
        serde_json::from_str(&existing.stdout).expect("existing JSON");
    assert_eq!(existing["outcome"], "existing");
    assert_eq!(existing["number"], 1);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn plan_created_and_existing_include_number() {
    let _env = env_lock();
    let dir = temp_state_dir("plan-number");
    let run_plan = || {
        run_with(
            [
                "tsk",
                "add",
                "--state-dir",
                &state_dir_arg(&dir),
                "--file",
                "-",
            ],
            Cursor::new(r#"[{"title":"numbered plan task","project":null}]"#),
            true,
        )
    };

    let created = run_plan();
    assert_eq!(created.code, 0, "{}", created.stderr);
    let created: serde_json::Value = serde_json::from_str(&created.stdout).expect("created plan");
    assert_eq!(created["created"][0]["number"], 1);

    let existing = run_plan();
    assert_eq!(existing.code, 0, "{}", existing.stderr);
    let existing: serde_json::Value =
        serde_json::from_str(&existing.stdout).expect("existing plan");
    assert_eq!(existing["existing"][0]["number"], 1);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn existing_add_does_not_advance_the_counter() {
    let _env = env_lock();
    let dir = temp_state_dir("existing-number-counter");
    let existing = || {
        add(
            &[
                "tsk".into(),
                "add".into(),
                "--desk".into(),
                "--state-dir".into(),
                state_dir_arg(&dir),
                "--title".into(),
                "first task".into(),
            ],
            true,
        )
    };
    assert_eq!(existing().code, 0);
    assert_eq!(existing().code, 0);

    let next = add(
        &[
            "tsk".into(),
            "add".into(),
            "--json".into(),
            "--desk".into(),
            "--state-dir".into(),
            state_dir_arg(&dir),
            "--title".into(),
            "second task".into(),
        ],
        true,
    );
    assert_eq!(next.code, 0, "{}", next.stderr);
    let next: serde_json::Value = serde_json::from_str(&next.stdout).expect("next JSON");
    assert_eq!(next["number"], 2);
    assert_eq!(
        task_store(&dir)
            .load()
            .expect("load state")
            .next_task_number,
        3
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn add_when_state_dir_is_a_file_exits_3() {
    let _env = env_lock();
    let parent = temp_state_dir("state-dir-file");
    let state_file = parent.join("not-a-directory");
    std::fs::write(&state_file, "not a directory").expect("create state-dir file");

    let output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&state_file),
            "--title".into(),
            "cannot persist".into(),
        ],
        true,
    );

    assert_eq!(output.code, 3);
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());

    let _ = std::fs::remove_dir_all(parent);
}

#[test]
fn state_dir_flag_wins_over_environment() {
    let _env = env_lock();
    let environment_dir = temp_state_dir("environment");
    let argument_dir = temp_state_dir("argument");
    let prior = std::env::var_os("TSK_STATE_DIR");
    // SAFETY: ENV_LOCK serializes this test's process-wide environment mutation.
    unsafe { std::env::set_var("TSK_STATE_DIR", &environment_dir) };

    let output = add(
        &[
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            state_dir_arg(&argument_dir),
            "-t".into(),
            "argument state".into(),
        ],
        true,
    );

    match prior {
        Some(value) => {
            // SAFETY: ENV_LOCK serializes this test's process-wide environment mutation.
            unsafe { std::env::set_var("TSK_STATE_DIR", value) };
        }
        None => {
            // SAFETY: ENV_LOCK serializes this test's process-wide environment mutation.
            unsafe { std::env::remove_var("TSK_STATE_DIR") };
        }
    }

    assert_eq!(output.code, 0);
    assert_eq!(
        task_store(&argument_dir)
            .load()
            .expect("load argument state")
            .tasks()
            .len(),
        1
    );
    assert!(task_store(&environment_dir)
        .load()
        .expect("load environment state")
        .tasks()
        .is_empty());

    let _ = std::fs::remove_dir_all(environment_dir);
    let _ = std::fs::remove_dir_all(argument_dir);
}

#[test]
fn add_into_an_archived_project_exits_1_with_project_archived_and_names_the_ways_out() {
    let _env = env_lock();
    let repo = temp_state_dir("archived-repo");
    std::fs::create_dir(repo.join(".git")).expect("create git marker");
    let dir = temp_state_dir("archived-add");
    // Seed a task in the repo scope and archive the project.
    let seeded = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&dir),
            "-t",
            "seed",
            "-p",
            &repo.to_string_lossy(),
        ],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(seeded.code, 0, "{:?}", seeded.stderr);
    let mut writable = TaskStore::new(&dir).load().expect("load store");
    writable
        .archive_project(&repo.to_string_lossy())
        .expect("archive the repo project");
    TaskStore::new(&dir)
        .save(&writable)
        .expect("persist record");

    let prior = std::env::var_os("HERDR_PLUGIN_CONTEXT_JSON");
    let context = format!(
        r#"{{"focused_pane_cwd":{}}}"#,
        serde_json::to_string(&repo).expect("serialize repo")
    );
    // SAFETY: ENV_LOCK serializes this test's process-wide environment mutation.
    unsafe { std::env::set_var("HERDR_PLUGIN_CONTEXT_JSON", context) };

    // Explicit -p into the archived project: exit 1, the refusal names the ways out.
    let explicit = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&dir),
            "-t",
            "x",
            "-p",
            &repo.to_string_lossy(),
        ],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(explicit.code, 1, "{explicit:?}");
    for needle in ["project-archived", "--desk", "-p", "tsk project unarchive"] {
        assert!(
            explicit.stderr.contains(needle),
            "stderr must contain {needle:?}: {:?}",
            explicit.stderr
        );
    }

    // The cwd default resolves to the same archived project: same refusal.
    let from_cwd = run_with(
        ["tsk", "add", "--state-dir", &state_dir_arg(&dir), "-t", "x"],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(from_cwd.code, 1);
    assert!(from_cwd.stderr.contains("project-archived"));

    // Nothing was persisted.
    let listed = run_with(
        [
            "tsk",
            "list",
            "--all",
            "--json",
            "--state-dir",
            &state_dir_arg(&dir),
        ],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(listed.code, 0);
    assert!(
        !listed.stdout.contains("\"x\""),
        "no refused draft persisted: {listed:?}"
    );

    // --desk still succeeds.
    let desk = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&dir),
            "-t",
            "desk side task",
            "--desk",
        ],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(desk.code, 0, "{desk:?}");

    match prior {
        Some(value) => unsafe { std::env::set_var("HERDR_PLUGIN_CONTEXT_JSON", value) },
        None => unsafe { std::env::remove_var("HERDR_PLUGIN_CONTEXT_JSON") },
    }
    let _ = std::fs::remove_dir_all(repo);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn plan_items_resolving_to_an_archived_project_refuse_with_project_archived() {
    let _env = env_lock();
    let repo = temp_state_dir("plan-archived-repo");
    std::fs::create_dir(repo.join(".git")).expect("create git marker");
    let dir = temp_state_dir("plan-archived");
    let seeded = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&dir),
            "-t",
            "seed",
            "-p",
            &repo.to_string_lossy(),
        ],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(seeded.code, 0, "{:?}", seeded.stderr);
    let mut writable = TaskStore::new(&dir).load().expect("load store");
    writable
        .archive_project(&repo.to_string_lossy())
        .expect("archive the repo project");
    TaskStore::new(&dir)
        .save(&writable)
        .expect("persist record");

    let prior = std::env::var_os("HERDR_PLUGIN_CONTEXT_JSON");
    let context = format!(
        r#"{{"focused_pane_cwd":{}}}"#,
        serde_json::to_string(&repo).expect("serialize repo")
    );
    // SAFETY: ENV_LOCK serializes this test's process-wide environment mutation.
    unsafe { std::env::set_var("HERDR_PLUGIN_CONTEXT_JSON", context) };

    // Item 0 omits `project` and resolves to the archived cwd default; item 1 names it
    // explicitly; item 2 is a desk item and must persist.
    let output = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&dir),
            "--file",
            "-",
        ],
        Cursor::new(
            format!(
                r#"[{{"title":"plan default"}},{{"title":"plan explicit","project":{}}},{{"title":"plan desk","project":null}}]"#,
                serde_json::to_string(&repo.to_string_lossy()).expect("serialize path")
            )
            .into_bytes(),
        ),
        true,
    );

    match prior {
        Some(value) => unsafe { std::env::set_var("HERDR_PLUGIN_CONTEXT_JSON", value) },
        None => unsafe { std::env::remove_var("HERDR_PLUGIN_CONTEXT_JSON") },
    }

    assert_eq!(output.code, 1, "{output:?}");
    let report: serde_json::Value = serde_json::from_str(&output.stdout).expect("plan report JSON");
    let failed = report["failed"].as_array().expect("failed rows");
    assert_eq!(
        failed.len(),
        2,
        "both archived-project items refuse: {report}"
    );
    assert_eq!(failed[0]["code"], "project-archived");
    assert_eq!(failed[0]["title"], "plan default");
    assert_eq!(
        failed[0]["error"],
        "project is archived: use --desk, -p, or tsk project unarchive"
    );
    assert_eq!(failed[1]["code"], "project-archived");
    assert_eq!(failed[1]["title"], "plan explicit");
    let created = report["created"].as_array().expect("created rows");
    assert_eq!(created.len(), 1);
    assert_eq!(created[0]["title"], "plan desk");

    let store = TaskStore::new(&dir).load().expect("load store");
    assert!(
        store
            .tasks()
            .iter()
            .all(|task| task.title != "plan default" && task.title != "plan explicit"),
        "refused plan items persisted nothing"
    );
    assert!(
        store.tasks().iter().any(|task| task.title == "plan desk"),
        "the desk item persisted"
    );
    let _ = std::fs::remove_dir_all(repo);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn plan_failed_rows_keep_item_order_when_an_archived_refusal_precedes_a_parse_failure() {
    let _env = env_lock();
    let repo = temp_state_dir("plan-archived-order-repo");
    std::fs::create_dir(repo.join(".git")).expect("create git marker");
    let dir = temp_state_dir("plan-archived-order");
    let seeded = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&dir),
            "-t",
            "seed",
            "-p",
            &repo.to_string_lossy(),
        ],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(seeded.code, 0, "{:?}", seeded.stderr);
    let mut writable = TaskStore::new(&dir).load().expect("load store");
    writable
        .archive_project(&repo.to_string_lossy())
        .expect("archive the repo project");
    TaskStore::new(&dir)
        .save(&writable)
        .expect("persist record");

    // Item 0 is an archived-project refusal (decided inside the transaction), item 1 is a
    // parse failure (decided before it), item 2 persists. `failed` must still read 0, 1.
    let output = run_with(
        [
            "tsk",
            "add",
            "--state-dir",
            &state_dir_arg(&dir),
            "--file",
            "-",
        ],
        Cursor::new(
            format!(
                r#"[{{"title":"archived first","project":{}}},{{"title":"   "}},{{"title":"desk last","project":null}}]"#,
                serde_json::to_string(&repo.to_string_lossy()).expect("serialize path")
            )
            .into_bytes(),
        ),
        true,
    );
    assert_eq!(output.code, 1, "{output:?}");
    let report: serde_json::Value = serde_json::from_str(&output.stdout).expect("plan report JSON");
    let failed = report["failed"].as_array().expect("failed rows");
    let indices: Vec<u64> = failed
        .iter()
        .map(|row| row["i"].as_u64().expect("index"))
        .collect();
    assert_eq!(
        indices,
        vec![0, 1],
        "failed rows are in item order: {report}"
    );
    assert_eq!(failed[0]["code"], "project-archived");
    assert_eq!(failed[1]["code"], "empty-title");
    let _ = std::fs::remove_dir_all(repo);
    let _ = std::fs::remove_dir_all(dir);
}
