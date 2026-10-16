//! The real board and quick capture against backups: an open session holds `board.lock`, so
//! `tsk restore` refuses until it quits, and a full board open backs up a due `daily`
//! target. Isolated PTY, store, and a local bare repository; never a live Herdr session.
#![cfg(unix)]
use crate::pty;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn tsk(state: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(args)
        .arg("--state-dir")
        .arg(state)
        .env("TSK_NO_UPDATE_CHECK", "1")
        .env_remove("TSK_AGENT")
        .output()
        .expect("run tsk")
}

/// Commits in the bare repository `repo`.
fn copies(repo: &Path) -> usize {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-list", "--count", "--all"])
        .output()
        .expect("git");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

/// A state dir with one task backed up once to a local bare repository, plus that version's id.
fn seeded(root: &Path, every: &str) -> (PathBuf, PathBuf, String) {
    let state = root.join("state");
    let repo = root.join("remote.git");
    fs::create_dir_all(&repo).expect("repo");
    let init = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["init", "--quiet", "--bare"])
        .output()
        .expect("git init");
    assert!(init.status.success());
    let added = tsk(&state, &["add", "-t", "kept in the backup", "--desk"]);
    assert_eq!(added.status.code(), Some(0));
    let target = tsk(
        &state,
        &[
            "backup",
            "add",
            repo.to_str().expect("utf-8"),
            "--every",
            every,
        ],
    );
    // `backup add` pushes the first backup.
    assert_eq!(
        target.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&target.stderr)
    );
    let listed = String::from_utf8_lossy(&tsk(&state, &["restore"]).stdout).into_owned();
    let id = listed
        .lines()
        .nth(1)
        .and_then(|row| row.split_whitespace().nth(1))
        .expect("a version row")
        .to_string();
    (state, repo, id)
}

fn assert_restore_refused(state: &Path, id: &str) {
    let refused = tsk(state, &["restore", id]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("board or quick capture is open"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
}

#[test]
fn an_open_board_blocks_restore_and_backs_up_due_daily_targets() {
    let root = pty::scratch_root("board-backup");
    let (state, folder, id) = seeded(&root, "daily");
    assert_eq!(copies(&folder), 1);
    // Forget the first backup, so the daily target is due again when the board opens.
    fs::remove_file(state.join("backup-status.json")).expect("forget status");
    let cwd = root.clone();
    let mut session = pty::Session::spawn(root, &cwd, &[], &[], 30, 100);
    // Any painted frame (here its header) comes after the session took board.lock.
    session.output_until("backup");

    assert_restore_refused(&state, &id);
    // The open found the daily target due, and the store had changed (starter tasks).
    let deadline = Instant::now() + Duration::from_secs(20);
    while copies(&folder) < 2 {
        assert!(
            Instant::now() < deadline,
            "the board open backed nothing up"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    session.send(b"\x11");
    assert!(session.wait_exit(Duration::from_secs(20)).success());
    let restored = tsk(&state, &["restore", &id]);
    assert_eq!(
        restored.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&restored.stderr)
    );
}

#[test]
fn an_open_quick_capture_blocks_restore() {
    let root = pty::scratch_root("capture-backup");
    let (state, _folder, id) = seeded(&root, "save");
    let cwd = root.clone();
    let mut session = pty::Session::spawn(root, &cwd, &["capture"], &[], 24, 78);
    session.output_until("cancel");
    assert_restore_refused(&state, &id);
}

#[test]
fn a_board_with_ten_tasks_and_no_target_suggests_a_first_backup() {
    let root = pty::scratch_root("nudge");
    let state = root.join("state");
    for n in 0..10 {
        let added = tsk(&state, &["add", "-t", &format!("task {n}"), "--desk"]);
        assert_eq!(added.status.code(), Some(0));
    }
    let cwd = root.clone();
    let mut session = pty::Session::spawn(root, &cwd, &[], &[], 30, 100);
    // The board paints word by word; "backup" is only in the suggestion.
    session.output_until("backup");
    let status = fs::read_to_string(state.join("backup-status.json")).expect("recorded");
    assert!(status.contains("\"nudged\": true"), "{status}");
}
