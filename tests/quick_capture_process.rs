//! Real capture entrypoint, isolated PTY and store. Never uses a live Herdr session.
#![cfg(unix)]
#[path = "support/pty.rs"]
mod pty;

use std::ffi::OsString;
use std::fs;
use std::time::Duration;

use tsk_tui::domain::{DomainState, ProvenanceOrigin, TaskScope};
use tsk_tui::store::TaskStore;

#[test]
fn t64_capture_ctrl_q_stays_inert_and_ctrl_c_still_exits_with_a_draft() {
    let root = pty::scratch_root("t64-capture-quit-keys");
    let cwd = root.join("outside");
    fs::create_dir_all(&cwd).unwrap();
    let store = TaskStore::new(root.join("state"));
    let mut session = pty::Session::spawn(root, &cwd, &["capture"], &[], 24, 78);
    session.output_until("cancel");
    session.send(b"unsaved popup draft\t\t");
    std::thread::sleep(Duration::from_millis(100));
    // Step selection is CapturePage, not a text editor. Ctrl+Q must still do nothing.
    session.send(b"\x11?");
    session.output_until("search");
    // The pre-existing Ctrl+C route from Help must still exit without saving the draft.
    session.send(b"\x03");
    assert!(session.wait_exit(Duration::from_secs(5)).success());
    assert!(store.load().unwrap().tasks().is_empty());
}

#[test]
fn capture_entrypoint_skips_launch_card_and_exits_on_escape() {
    let root = pty::scratch_root("capture");
    fs::create_dir_all(root.join("repo/.git")).unwrap();
    let repo = root.join("repo");
    let store = TaskStore::new(root.join("state"));
    let mut state = DomainState::new();
    state
        .create(
            "archived seed",
            None,
            TaskScope::Project {
                path: repo.display().to_string(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .unwrap();
    state.archive_project(repo.to_str().unwrap()).unwrap();
    store.save(&state).unwrap();
    let context = OsString::from(
        serde_json::json!({"focused_pane_cwd":repo,"selected_text":"PTY capture"}).to_string(),
    );
    let mut session = pty::Session::spawn(
        root.clone(),
        &repo,
        &["capture"],
        &[("HERDR_PLUGIN_CONTEXT_JSON", context.as_os_str())],
        13,
        78,
    );
    let output = session.output_until("cancel");
    assert!(
        output.contains("+ step"),
        "capture must open the expanded page: {output}"
    );
    assert!(
        output.contains("desk"),
        "archived launch falls back to desk: {output}"
    );
    assert!(!output.contains("would you like to unarchive"));
    session.send(b"\x1b[27u");
    assert!(
        session.wait_exit(Duration::from_secs(5)).success(),
        "one Escape must exit real capture loop"
    );
    let tasks = store.load().unwrap();
    assert_eq!(tasks.tasks().len(), 1, "cancel saves no task");
    assert!(
        tasks.tasks().iter().all(|task| !task.is_notice()),
        "quick capture never seeds the starter guides"
    );
    assert!(
        !store.path().join("delivery.json").exists(),
        "quick capture writes no delivery record"
    );
}

/// A store that knows `work/alpha` as the symlink `links/beta`, launched from
/// `work/alpha`. Returns the root, the launch directory, the stored scope, and context.
fn aliased_launch(label: &str) -> (std::path::PathBuf, std::path::PathBuf, TaskScope, OsString) {
    let root = pty::scratch_root(label);
    let real = root.join("work").join("alpha");
    fs::create_dir_all(real.join(".git")).unwrap();
    fs::create_dir_all(root.join("links")).unwrap();
    let alias = root.join("links").join("beta");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let stored = TaskScope::Project {
        path: alias.to_string_lossy().into_owned(),
    };
    let mut state = DomainState::new();
    state
        .create(
            "seeded",
            None,
            stored.clone(),
            ProvenanceOrigin::Manual,
            None,
        )
        .unwrap();
    TaskStore::new(root.join("state")).save(&state).unwrap();
    let context = OsString::from(serde_json::json!({"focused_pane_cwd": real}).to_string());
    (root, real, stored, context)
}

fn saved_scope(root: &std::path::Path, title: &str) -> TaskScope {
    TaskStore::new(root.join("state"))
        .load()
        .unwrap()
        .tasks()
        .iter()
        .find(|task| task.title == title)
        .unwrap_or_else(|| panic!("{title} was not saved"))
        .scope
        .clone()
}

#[test]
fn board_quick_add_from_an_aliased_launch_repo_saves_the_stored_scope() {
    let (root, real, stored, context) = aliased_launch("t152-board-alias");
    let mut session = pty::Session::spawn(
        root.clone(),
        &real,
        &[],
        &[("HERDR_PLUGIN_CONTEXT_JSON", context.as_os_str())],
        24,
        100,
    );
    session.output_until("seeded");
    // The desk tab hands quick add the launch default rather than the open project.
    session.send(b"1");
    std::thread::sleep(Duration::from_millis(100));
    session.send(b"+");
    std::thread::sleep(Duration::from_millis(100));
    session.output_until("add to");
    session.send(b"board alias capture\r");
    std::thread::sleep(Duration::from_millis(300));
    session.send(b"\x11");
    assert!(session.wait_exit(Duration::from_secs(5)).success());
    assert_eq!(saved_scope(&root, "board alias capture"), stored);
}

#[test]
fn capture_popup_from_an_aliased_launch_repo_saves_the_stored_scope() {
    let (root, real, stored, context) = aliased_launch("t152-popup-alias");
    let mut session = pty::Session::spawn(
        root.clone(),
        &real,
        &["capture"],
        &[("HERDR_PLUGIN_CONTEXT_JSON", context.as_os_str())],
        24,
        100,
    );
    session.output_until("cancel");
    session.send(b"popup alias capture");
    std::thread::sleep(Duration::from_millis(100));
    session.send(b"\x1b[13;2u");
    assert!(session.wait_exit(Duration::from_secs(5)).success());
    assert_eq!(saved_scope(&root, "popup alias capture"), stored);
}
