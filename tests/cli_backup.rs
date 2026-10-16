//! `tsk backup` and `tsk restore` through the real binary: saves start detached backups that
//! never fail the save, and a git target restores into a fresh state dir (the new-laptop flow).
//! Temp dirs only; never the real store or a real remote.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tsk-cli-backup-{label}-{nanos}-{seq}"));
        fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn tsk(state: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(args)
        .arg("--state-dir")
        .arg(state)
        .env_remove("TSK_AGENT")
        .env("TSK_NO_UPDATE_CHECK", "1")
        .output()
        .expect("run tsk")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn add(state: &Path, title: &str) {
    let output = tsk(state, &["add", "-t", title, "--desk"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
}

fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

fn copies(folder: &Path) -> Vec<PathBuf> {
    let Ok(machines) = fs::read_dir(folder.join("tsk-backups")) else {
        return Vec::new();
    };
    machines
        .flatten()
        .flat_map(|machine| fs::read_dir(machine.path()).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("tsk-") && name.ends_with(".json"))
        })
        .collect()
}

#[test]
fn a_save_backs_up_to_a_folder_in_the_background() {
    let state = TempDir::new("state");
    let folder = TempDir::new("folder");
    let added = tsk(
        &state.0,
        &["backup", "add", folder.0.to_str().expect("utf-8")],
    );
    assert_eq!(added.status.code(), Some(0), "{}", stderr(&added));
    assert!(
        stdout(&added).contains("(every save)"),
        "{}",
        stdout(&added)
    );

    add(&state.0, "first task");
    wait_for("the first copy", || copies(&folder.0).len() == 1);
    let live = fs::read(state.0.join("tsk.json")).expect("live store");
    assert_eq!(
        fs::read(&copies(&folder.0)[0]).expect("copy"),
        live,
        "byte for byte"
    );

    // The child records its success just after the copy lands.
    wait_for("the recorded success", || {
        stdout(&tsk(&state.0, &["backup", "list"])).contains("last backup: 20")
    });
}

#[test]
fn a_failing_backup_never_fails_or_slows_the_save() {
    let state = TempDir::new("failing");
    let missing = state.0.join("unplugged-drive");
    let config = format!(
        "[[backup]]\nkind = \"folder\"\npath = {:?}\n\n[[backup]]\nkind = \"git\"\nurl = \"file:///nonexistent/tsk-t220-repo.git\"\n",
        missing.to_str().expect("utf-8")
    );
    fs::write(state.0.join("config.toml"), config).expect("config");

    let started = Instant::now();
    add(&state.0, "saved anyway");
    assert!(started.elapsed() < Duration::from_secs(10));
    let listed = tsk(&state.0, &["list", "--desk", "--json"]);
    assert!(stdout(&listed).contains("saved anyway"));

    wait_for("the recorded failure", || {
        let listed = stdout(&tsk(&state.0, &["backup", "list"]));
        listed.matches("last error").count() == 2
    });
    let listed = stdout(&tsk(&state.0, &["backup", "list"]));
    assert!(listed.contains("not found"), "{listed}");
    assert!(listed.contains("git clone failed"), "{listed}");

    let now = tsk(&state.0, &["backup"]);
    assert_eq!(now.status.code(), Some(1));
    assert!(stderr(&now).contains("not found"), "{}", stderr(&now));
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(output.status.success(), "git {args:?}: {}", stderr(&output));
}

#[test]
fn a_git_target_restores_into_a_fresh_state_dir() {
    let root = TempDir::new("git");
    let repo = root.0.join("remote.git");
    fs::create_dir_all(&repo).expect("repo dir");
    git(&repo, &["init", "--quiet", "--bare"]);
    let repo_arg = repo.to_str().expect("utf-8");

    let laptop = root.0.join("old-laptop");
    add(&laptop, "written on the old laptop");
    add(&laptop, "and another");
    let added = tsk(&laptop, &["backup", "add", repo_arg]);
    assert_eq!(added.status.code(), Some(0), "{}", stderr(&added));
    assert!(stdout(&added).contains("git target"), "{}", stdout(&added));
    assert!(stdout(&added).contains("(daily)"), "{}", stdout(&added));
    // `add` tests the target with a first backup.
    assert!(
        stdout(&added).contains("backed up to"),
        "{}",
        stdout(&added)
    );
    assert!(stdout(&added).contains("machines/"), "{}", stdout(&added));
    let again = tsk(&laptop, &["backup"]);
    assert!(stdout(&again).contains("up to date"), "{}", stdout(&again));

    let fresh = root.0.join("new-laptop");
    let added = tsk(&fresh, &["backup", "add", "--git", repo_arg]);
    assert_eq!(added.status.code(), Some(0), "{}", stderr(&added));
    assert!(
        stdout(&added).contains("reached the repository"),
        "{}",
        stdout(&added)
    );
    let fresh_config = fs::read(fresh.join("config.toml")).expect("fresh config");
    let listed = tsk(&fresh, &["restore"]);
    assert_eq!(listed.status.code(), Some(0), "{}", stderr(&listed));
    let table = stdout(&listed);
    let row = table.lines().nth(1).expect("one version row");
    let id = row.split_whitespace().nth(1).expect("id column");
    assert!(row.contains(" 2 "), "two tasks: {row}");

    let restored = tsk(&fresh, &["restore", id]);
    assert_eq!(restored.status.code(), Some(0), "{}", stderr(&restored));
    assert!(stdout(&restored).starts_with(&format!("restored {id}")));
    // Restore leaves config.toml alone and says where the backed-up one is.
    assert!(
        stdout(&restored).contains("config.toml was not changed")
            && stdout(&restored).contains("machines/"),
        "{}",
        stdout(&restored)
    );
    assert_eq!(
        fs::read(fresh.join("config.toml")).expect("config"),
        fresh_config
    );
    assert_eq!(
        fs::read(fresh.join("tsk.json")).expect("restored"),
        fs::read(laptop.join("tsk.json")).expect("original")
    );
    let tasks = stdout(&tsk(&fresh, &["list", "--desk", "--json"]));
    assert!(tasks.contains("written on the old laptop"), "{tasks}");

    let unknown = tsk(&fresh, &["restore", "deadbeef"]);
    assert_eq!(unknown.status.code(), Some(1));
    assert!(stderr(&unknown).contains("unknown version"));
    assert!(!stderr(&unknown).contains("trash restore"));
    let task = tsk(&fresh, &["restore", "T12"]);
    assert_eq!(task.status.code(), Some(1));
    assert!(
        stderr(&task).contains("did you mean `tsk trash restore T12`?"),
        "{}",
        stderr(&task)
    );
}

#[test]
fn backup_add_tests_the_target_and_keeps_it_when_the_first_backup_fails() {
    let state = TempDir::new("add-test");
    add(&state.0, "needs a home");
    let missing = state.0.join("no-such-repo.git");
    let added = tsk(
        &state.0,
        &["backup", "add", "--git", missing.to_str().expect("utf-8")],
    );
    assert_eq!(added.status.code(), Some(1), "{}", stdout(&added));
    assert!(
        stderr(&added).contains("the first backup to"),
        "{}",
        stderr(&added)
    );
    assert!(
        stderr(&added).contains("git clone failed"),
        "{}",
        stderr(&added)
    );
    assert!(
        stderr(&added).contains("then run `tsk backup`"),
        "{}",
        stderr(&added)
    );
    let listed = stdout(&tsk(&state.0, &["backup", "list"]));
    assert!(
        listed.contains("no-such-repo.git"),
        "the target is kept: {listed}"
    );

    let folder = TempDir::new("add-test-folder");
    let added = tsk(
        &state.0,
        &["backup", "add", folder.0.to_str().expect("utf-8")],
    );
    assert_eq!(added.status.code(), Some(0), "{}", stderr(&added));
    assert!(
        stdout(&added).contains("backed up to"),
        "{}",
        stdout(&added)
    );
    assert!(
        stdout(&added).contains("check your sync app"),
        "{}",
        stdout(&added)
    );
    assert_eq!(
        copies(&folder.0).len(),
        1,
        "the first copy is written by add"
    );
}

#[test]
fn restore_refuses_without_targets_and_backup_refuses_a_missing_folder() {
    let state = TempDir::new("none");
    let restore = tsk(&state.0, &["restore"]);
    assert_eq!(restore.status.code(), Some(1));
    assert!(stderr(&restore).contains("no backup targets"));
    let backup = tsk(&state.0, &["backup"]);
    assert_eq!(backup.status.code(), Some(1));
    let missing = state.0.join("nope");
    let add = tsk(
        &state.0,
        &[
            "backup",
            "add",
            "--folder",
            missing.to_str().expect("utf-8"),
        ],
    );
    assert_eq!(add.status.code(), Some(1));
    assert!(stderr(&add).contains("not found"));
    assert!(!state.0.join("config.toml").exists());
    let usage = tsk(&state.0, &["backup", "add", "/x", "--every", "hourly"]);
    assert_eq!(usage.status.code(), Some(2));
}
