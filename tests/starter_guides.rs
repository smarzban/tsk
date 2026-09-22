#![cfg(unix)]
#[path = "support/pty.rs"]
mod pty;

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use tsk_tui::agents::AgentProfiles;
use tsk_tui::announcements;
use tsk_tui::app::{load_board, load_board_for_quick_capture, load_board_model};
use tsk_tui::context::{build_snapshot, RawHostContext, CONTEXT_JSON_ENV};
use tsk_tui::delivery;
use tsk_tui::domain::{DomainState, HumanStatus, ProvenanceOrigin, Task, TaskScope};
use tsk_tui::guides::CATALOG;
use tsk_tui::store::TaskStore;
use tsk_tui::ui::board::{apply_intent, IntentOutcome};
use tsk_tui::ui::input::BoardIntent;
use tsk_tui::ui::queue::NavTab;
use tsk_tui::update::suppress_background_fetch;

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

struct StateDirEnv {
    previous: Option<std::ffi::OsString>,
    dir: PathBuf,
}

impl StateDirEnv {
    fn set(label: &str) -> Self {
        let dir = pty::scratch_root(label).join("state");
        let previous = std::env::var_os("TSK_STATE_DIR");
        std::env::set_var("TSK_STATE_DIR", &dir);
        Self { previous, dir }
    }
}

impl Drop for StateDirEnv {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var("TSK_STATE_DIR", value),
            None => std::env::remove_var("TSK_STATE_DIR"),
        }
        let _ = fs::remove_dir_all(self.dir.parent().unwrap_or(&self.dir));
    }
}

struct ContextEnv(Option<std::ffi::OsString>);

impl ContextEnv {
    fn set(cwd: &std::path::Path) -> Self {
        let previous = std::env::var_os(CONTEXT_JSON_ENV);
        let context = serde_json::json!({"focused_pane_cwd": cwd}).to_string();
        std::env::set_var(CONTEXT_JSON_ENV, context);
        Self(previous)
    }
}

impl Drop for ContextEnv {
    fn drop(&mut self) {
        match self.0.take() {
            Some(value) => std::env::set_var(CONTEXT_JSON_ENV, value),
            None => std::env::remove_var(CONTEXT_JSON_ENV),
        }
    }
}

fn notices(state: &DomainState) -> Vec<&Task> {
    state
        .tasks()
        .iter()
        .filter(|task| task.is_notice())
        .collect()
}

fn catalog_ids() -> BTreeSet<String> {
    CATALOG
        .iter()
        .map(|guide| guide.catalog_id.to_string())
        .collect()
}

const STARTER_AGENTS: &str = "# tsk agent profiles. Assign with `!a name`, dispatch with ctrl+g.\n\
# Placeholders in command and prompt: {number} {title} {notes} {steps} {worktree} {branch}\n\
# The prompt is appended to the command as its last argument. Omit `prompt` for the default:\n\
#   You were dispatched to T{number} in this worktree. Run `tsk guide`, then `tsk list {number}`.\n\
#   Set the task to review when done, or blocked when a human is needed.\n\
\n\
# [agent.grok]\n\
# command = [\"pi\", \"--model\", \"xai/grok-4.6\", \"--thinking\", \"high\"]\n\
\n\
# [agent.opus]\n\
# command = [\"claude\", \"--model\", \"opus\", \"--effort\", \"high\"]\n\
# prompt = \"Review the branch for T{number}: {title}. Leave findings as steps on the task, then set review.\"\n\
\n\
# [agent.fable]\n\
# command = [\"fable\"]\n";

#[test]
fn full_board_open_seeds_a_parseable_commented_agent_profile_file() {
    let _lock = env_lock();
    suppress_background_fetch();
    let env = StateDirEnv::set("agents-file");

    let _ = load_board_model().expect("full board open");

    assert_eq!(
        fs::read_to_string(env.dir.join("agents.toml")).expect("seeded agents.toml"),
        STARTER_AGENTS
    );
    assert!(
        AgentProfiles::load(&env.dir)
            .expect("seeded file parses")
            .is_empty(),
        "all starter profiles are commented out"
    );
}

#[test]
fn full_board_open_never_overwrites_an_existing_agent_profile_file() {
    let _lock = env_lock();
    suppress_background_fetch();
    for (label, content) in [
        ("agents-existing-empty", ""),
        ("agents-existing-content", "owner content\n"),
    ] {
        let env = StateDirEnv::set(label);
        fs::create_dir_all(&env.dir).expect("state dir");
        fs::write(env.dir.join("agents.toml"), content).expect("existing agents.toml");

        let _ = load_board_model().expect("full board open");

        assert_eq!(
            fs::read_to_string(env.dir.join("agents.toml")).expect("existing agents.toml"),
            content
        );
    }
}

#[test]
fn full_board_open_seeds_the_guides_once_beside_existing_tasks() {
    let _lock = env_lock();
    suppress_background_fetch();
    let env = StateDirEnv::set("board-open");
    let store = TaskStore::new(&env.dir);
    let mut seeded = DomainState::new();
    seeded
        .create(
            "existing work",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("task");
    store.save(&seeded).expect("save");
    // An existing desk gets the four starter tasks, plus one What's new row only when the
    // bundled catalog has something to announce.
    let bundled = announcements::catalog().expect("bundled catalog");
    let newest = bundled.last().map(|entry| entry.id).unwrap_or(0);
    let whats_new = usize::from(!bundled.is_empty());

    let _ = load_board_model().expect("first open");
    let state = store.load().expect("load");
    assert_eq!(
        notices(&state).len(),
        4 + whats_new,
        "four starter tasks plus What's new for an existing desk when the catalog has entries"
    );
    assert_eq!(
        notices(&state)
            .iter()
            .filter(|task| task.title == announcements::TITLE)
            .count(),
        whats_new
    );
    assert_eq!(
        state
            .tasks()
            .iter()
            .filter(|task| !task.is_notice())
            .map(|task| (task.title.as_str(), task.number))
            .collect::<Vec<_>>(),
        vec![("existing work", Some(1))]
    );
    let record = delivery::load(&env.dir);
    assert_eq!(record.guides, catalog_ids());
    assert_eq!(record.announcement_watermark, newest);

    let _ = load_board_model().expect("second open");
    assert_eq!(notices(&store.load().expect("reload")).len(), 4 + whats_new);
}

#[test]
fn a_fresh_full_board_open_records_the_bundled_announcements_without_seeding_them() {
    let _lock = env_lock();
    suppress_background_fetch();
    let env = StateDirEnv::set("announcements");
    let bundled = announcements::catalog().expect("bundled catalog");
    let newest = bundled.last().map(|entry| entry.id).unwrap_or(0);

    let _ = load_board_model().expect("first open");
    let state = TaskStore::new(&env.dir).load().expect("load");
    assert_eq!(notices(&state).len(), 4, "starter tasks only");
    assert!(notices(&state)
        .iter()
        .all(|task| task.title != announcements::TITLE));
    let record = delivery::load(&env.dir);
    assert_eq!(record.announcement_watermark, newest);
    assert_eq!(record.guides, catalog_ids());
}

#[test]
fn non_git_full_board_open_keeps_the_directory_in_slot_two_and_captures_on_desk() {
    let _lock = env_lock();
    suppress_background_fetch();
    let env = StateDirEnv::set("non-git-board");
    let outside = env.dir.parent().unwrap().join("outside");
    fs::create_dir_all(&outside).expect("outside directory");
    let _context = ContextEnv::set(&outside);

    let (_store, mut state, mut model) = load_board().expect("full board open");
    assert_eq!(model.nav_tab(), NavTab::Desk);
    assert_eq!(model.selected_project(), Some(outside.as_path()));

    let snapshot = build_snapshot(
        &RawHostContext {
            focused_pane_cwd: Some(outside.to_string_lossy().into_owned()),
            ..RawHostContext::default()
        },
        PathBuf::new(),
    );
    assert_eq!(
        apply_intent(
            &mut state,
            &mut model,
            BoardIntent::OpenCapture,
            Some(&snapshot),
        )
        .expect("open quick add"),
        IntentOutcome::None
    );
    apply_intent(
        &mut state,
        &mut model,
        BoardIntent::QuickAddInsertText("desk capture".into()),
        None,
    )
    .expect("type title");
    assert_eq!(
        apply_intent(&mut state, &mut model, BoardIntent::QuickAddSave, None)
            .expect("save quick add"),
        IntentOutcome::Persist
    );
    assert_eq!(
        state
            .tasks()
            .iter()
            .find(|task| task.title == "desk capture")
            .expect("saved task")
            .scope,
        TaskScope::Global
    );
}

#[test]
fn quick_capture_open_seeds_nothing() {
    let _lock = env_lock();
    suppress_background_fetch();
    let env = StateDirEnv::set("capture-open");
    let (store, state, _model) = load_board_for_quick_capture().expect("capture open");
    assert!(notices(&state).is_empty());
    assert!(notices(&store.load().expect("load")).is_empty());
    assert!(!env.dir.join(delivery::DELIVERY_FILE).exists());
    assert!(
        !env.dir.join("agents.toml").exists(),
        "quick capture does not seed agent profiles"
    );
}

fn drop_delivery_record(state_dir: &std::path::Path) {
    fs::remove_file(state_dir.join(delivery::DELIVERY_FILE)).unwrap();
}

fn seed_pty_task(root: &std::path::Path, cwd: &std::path::Path, title: &str) {
    let store = TaskStore::new(root.join("state"));
    let mut state = DomainState::new();
    state
        .create(
            title,
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("seed PTY desk task");
    let project_id = state
        .create(
            "T64 project tab task",
            None,
            TaskScope::Project {
                path: cwd.to_string_lossy().into_owned(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("seed PTY project task");
    state
        .set_status(project_id, HumanStatus::Ready)
        .expect("put PTY project task on deck");
    store.save(&state).expect("save PTY task");
}

#[test]
fn t64_real_board_ctrl_q_exits_from_a_stored_task_page() {
    let root = pty::scratch_root("t64-task-page-quit");
    let cwd = root.join("project");
    fs::create_dir_all(&cwd).expect("project directory");
    seed_pty_task(&root, &cwd, "T64 stored page target");

    let mut session = pty::Session::spawn(root, &cwd, &[], &[], 24, 78);
    session.output_until("target");
    session.send(b"k");
    std::thread::sleep(Duration::from_millis(100));
    session.send(b"\r");
    session.output_until("target");
    session.send(b"\x11");
    assert!(session.wait_exit(Duration::from_secs(5)).success());
}

fn assert_real_root_esc_exits(label: &str, tab_key: u8) {
    let root = pty::scratch_root(&format!("t64-root-esc-{label}"));
    let cwd = root.join("project");
    fs::create_dir_all(&cwd).expect("project directory");
    seed_pty_task(&root, &cwd, &format!("T64 {label} root"));

    let mut session = pty::Session::spawn(root, &cwd, &[], &[], 24, 78);
    session.output_until("root");
    session.send(&[tab_key]);
    std::thread::sleep(Duration::from_millis(100));
    session.send(b"\x1b[27u");
    assert!(session.wait_exit(Duration::from_secs(5)).success());
}

#[test]
fn t64_real_board_root_esc_exits_from_desk() {
    assert_real_root_esc_exits("desk", b'1');
}

#[test]
fn t64_real_board_root_esc_exits_from_project_board() {
    assert_real_root_esc_exits("project", b'2');
}

#[test]
fn t64_real_board_root_esc_exits_from_projects_index() {
    assert_real_root_esc_exits("projects", b'3');
}

#[test]
fn t64_real_board_editor_ctrl_q_stays_live_then_task_page_ctrl_q_exits() {
    let root = pty::scratch_root("t64-editor-quit");
    let cwd = root.join("project");
    fs::create_dir_all(&cwd).expect("project directory");
    seed_pty_task(&root, &cwd, "T64 editor target");

    let mut session = pty::Session::spawn(root, &cwd, &[], &[], 24, 78);
    session.output_until("target");
    session.send(b"k");
    std::thread::sleep(Duration::from_millis(100));
    session.send(b"\r");
    std::thread::sleep(Duration::from_millis(100));
    session.send(b"\x05");
    session.output_until("editing…");
    session.send(b"\x11");
    session.send(b"ZXQMARK");
    session.output_until("\x1b[1mK");
    session.send(b"\x1b");
    std::thread::sleep(Duration::from_millis(100));
    session.send(b"\x11");
    assert!(session.wait_exit(Duration::from_secs(5)).success());
}

#[test]
fn completing_a_guide_on_the_real_board_records_its_dismissal() {
    let root = pty::scratch_root("dismiss");
    let outside = root.join("outside");
    fs::create_dir_all(&outside).unwrap();
    let state_dir = root.join("state");
    let mut session = pty::Session::spawn(root, &outside, &[], &[], 24, 78);
    // Painted words are split by cursor moves, so wait on the last starter task's `N` prefix.
    let painted = session.output_until("N4");
    for prefix in ["N1", "N2", "N3"] {
        assert!(painted.contains(prefix), "{prefix} painted: {painted}");
    }
    let store = TaskStore::new(&state_dir);
    assert_eq!(delivery::load(&state_dir).guides, catalog_ids());
    drop_delivery_record(&state_dir);

    session.send(b"\x04");
    session.send(b"\x11");
    assert!(session.wait_exit(Duration::from_secs(5)).success());

    let state = store.load().unwrap();
    let done: Vec<&Task> = notices(&state)
        .into_iter()
        .filter(|task| task.status == HumanStatus::Done)
        .collect();
    assert_eq!(done.len(), 1, "ctrl+d completed the selected guide");
    assert_eq!(notices(&state).len(), 4, "the other starter tasks stay");
    let dismissed = done[0].notice.as_ref().unwrap().catalog_id.clone();
    assert_eq!(
        delivery::load(&state_dir).guides,
        [dismissed].into_iter().collect::<BTreeSet<_>>()
    );
}
