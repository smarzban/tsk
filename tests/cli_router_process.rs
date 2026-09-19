use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tsk_tui::domain::{DomainState, ProvenanceOrigin, TaskScope};
use tsk_tui::store::TaskStore;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn binary() -> String {
    std::env::var("CARGO_BIN_EXE_tsk").expect("Cargo must provide the tsk binary path")
}

fn temp_state_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tsk-process-{label}-{nanos}-{seq}"));
    std::fs::create_dir_all(&dir).expect("create state directory");
    dir
}

fn wait_with_output_before_deadline(mut child: Child, description: &str) -> Output {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if child.try_wait().expect("poll child process").is_some() {
            return child
                .wait_with_output()
                .expect("collect child process output");
        }
        if Instant::now() >= deadline {
            child.kill().expect("stop process after timeout");
            let _ = child.wait();
            panic!("{description} kept a TUI open");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn top_level_help_names_subcommands_and_their_help() {
    let output = wait_with_output_before_deadline(
        Command::new(binary())
            .arg("--help")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn global help"),
        "top-level help",
    );

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 help");
    assert!(stdout.contains("add"));
    assert!(stdout.contains("list"));
    assert!(stdout.contains("status"));
    assert!(stdout.contains("edit"));
    assert!(stdout.contains("update"));
    assert!(stdout.contains("archive") && stdout.contains("unarchive"));
    assert!(stdout.contains("help [<command>]"));
    assert!(stdout.contains("--version"));
    assert!(stdout.contains("tsk help <command>"));
    assert!(stdout.contains("tsk <command> --help"));
}

#[test]
fn update_help_explains_installer_and_homebrew_behavior_without_updating() {
    let output = wait_with_output_before_deadline(
        Command::new(binary())
            .args(["update", "--help"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn update help"),
        "update help",
    );

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 help");
    assert!(stdout.contains("usage: tsk update"));
    assert!(stdout.contains("Homebrew"));
    assert!(stdout.contains("installer"));
}

#[cfg(unix)]
#[test]
fn update_directs_a_homebrew_binary_to_brew_without_a_path_lookup() {
    let dir = temp_state_dir("update-homebrew");
    let executable = dir.join("Cellar/tsk/0.7.0/bin/tsk");
    std::fs::create_dir_all(executable.parent().expect("Homebrew binary parent"))
        .expect("create Homebrew test directory");
    std::fs::copy(binary(), &executable).expect("copy tsk into Homebrew Cellar");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("make copied binary executable");
    }
    let output = wait_with_output_before_deadline(
        Command::new(&executable)
            .arg("update")
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn Homebrew update"),
        "Homebrew update guidance",
    );

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 guidance"),
        "tsk was installed with Homebrew. Run:\n  brew update && brew upgrade tsk\n"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn top_level_help_lists_guide_and_ends_with_agent_footer() {
    let output = wait_with_output_before_deadline(
        Command::new(binary())
            .arg("--help")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn global help"),
        "top-level help",
    );

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 help");
    assert!(
        stdout.contains("guide"),
        "top-level help must list the guide command"
    );
    let trimmed = stdout.trim_end();
    assert!(
        trimmed.ends_with("Agents: run `tsk guide`, or read https://gettsk.sh/docs/agents.md"),
        "help must end with the agent footer, got {trimmed:?}"
    );
}

#[test]
fn executable_accepts_piped_json_plan_and_persists_it() {
    let dir = temp_state_dir("piped-plan");
    let mut child = Command::new(binary())
        .args(["add", "--state-dir", dir.to_str().expect("UTF-8 state dir")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn plan add");
    let mut stdin = child.stdin.take().expect("plan stdin");
    let writer = thread::spawn(move || {
        stdin.write_all(br#"[{"title":"from executable pipe","project":null}]"#)
    });
    let output = wait_with_output_before_deadline(child, "piped plan add");
    writer
        .join()
        .expect("join plan writer")
        .expect("write plan");

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).expect("plan JSON");
    assert_eq!(result["created"][0]["title"], "from executable pipe");
    assert!(result["existing"].as_array().expect("existing").is_empty());
    assert!(result["failed"].as_array().expect("failed").is_empty());
    let state = TaskStore::new(&dir).load().expect("load persisted plan");
    assert_eq!(state.tasks().len(), 1);
    assert_eq!(state.tasks()[0].title, "from executable pipe");
    assert_eq!(state.tasks()[0].scope, TaskScope::Global);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn executable_list_json_reads_state_without_opening_the_board() {
    let dir = temp_state_dir("list-json");
    let mut state = DomainState::new();
    state
        .create(
            "listed by executable",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("seed task");
    TaskStore::new(&dir).save(&state).expect("seed state");

    let output = wait_with_output_before_deadline(
        Command::new(binary())
            .args([
                "list",
                "--desk",
                "--json",
                "--state-dir",
                dir.to_str().expect("UTF-8 state dir"),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn list"),
        "JSON list",
    );

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).expect("list JSON");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["title"], "listed by executable");
    assert_eq!(rows[0]["project"], serde_json::Value::Null);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn unknown_positional_exits_2_without_opening_the_board() {
    let mut child = Command::new(binary())
        .arg("foo")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn tsk foo");
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll child process") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("stop TUI process after timeout");
            let _ = child.wait();
            panic!("unknown positional kept a TUI open");
        }
        thread::sleep(Duration::from_millis(10));
    };

    assert_eq!(status.code(), Some(2));
}

#[cfg(windows)]
#[test]
fn windows_uses_localappdata_without_home_or_userprofile() {
    let root = temp_state_dir("windows-localappdata");
    let local = root.join("local");
    let output = Command::new(binary())
        .args(["list", "--all"])
        .env_remove("HOME")
        .env_remove("USERPROFILE")
        .env_remove("TSK_STATE_DIR")
        .env("LOCALAPPDATA", &local)
        .output()
        .expect("run tsk with LOCALAPPDATA only");
    assert!(output.status.success(), "{output:?}");
    assert!(
        local.join("tsk").join("tsk.json.lock").exists(),
        "the default Windows store must live under %LOCALAPPDATA%\\tsk"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(windows)]
#[test]
fn windows_falls_back_to_userprofile_without_localappdata() {
    let root = temp_state_dir("windows-userprofile");
    let profile = root.join("profile");
    let output = Command::new(binary())
        .args(["list", "--all"])
        .env_remove("HOME")
        .env_remove("LOCALAPPDATA")
        .env_remove("TSK_STATE_DIR")
        .env("USERPROFILE", &profile)
        .output()
        .expect("run tsk with USERPROFILE only");
    assert!(output.status.success(), "{output:?}");
    assert!(profile.join(".tsk").join("tsk.json.lock").exists());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_missing_home_refuses_instead_of_creating_a_board_in_the_working_directory() {
    let cwd = temp_state_dir("nohome");
    let output = Command::new(binary())
        .args(["list"])
        .current_dir(&cwd)
        .env_remove("HOME")
        .env_remove("TSK_STATE_DIR")
        .env_remove("USERPROFILE")
        .env_remove("LOCALAPPDATA")
        .output()
        .expect("run tsk without HOME");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not set"), "{stderr}");
    assert!(stderr.contains("TSK_STATE_DIR"), "{stderr}");
    assert!(
        !cwd.join(".tsk-state").exists(),
        "no cwd-relative store may appear"
    );

    // An empty HOME is the same as none.
    let output = Command::new(binary())
        .args(["list"])
        .current_dir(&cwd)
        .env("HOME", "")
        .env_remove("TSK_STATE_DIR")
        .env_remove("USERPROFILE")
        .env_remove("LOCALAPPDATA")
        .output()
        .expect("run tsk with empty HOME");
    assert_eq!(output.status.code(), Some(1), "{output:?}");

    // `--state-dir` on the verb is enough too.
    let explicit = temp_state_dir("nohome-explicit");
    let output = Command::new(binary())
        .args(["list", "--all", "--state-dir"])
        .arg(&explicit)
        .current_dir(&cwd)
        .env_remove("HOME")
        .env_remove("TSK_STATE_DIR")
        .env_remove("USERPROFILE")
        .env_remove("LOCALAPPDATA")
        .output()
        .expect("run tsk with --state-dir only");
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    // The equals spelling counts too.
    let output = Command::new(binary())
        .args(["list", "--all"])
        .arg(format!("--state-dir={}", explicit.display()))
        .current_dir(&cwd)
        .env_remove("HOME")
        .env_remove("TSK_STATE_DIR")
        .env_remove("USERPROFILE")
        .env_remove("LOCALAPPDATA")
        .output()
        .expect("run tsk with --state-dir= only");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let _ = std::fs::remove_dir_all(&explicit);

    // The board and quick capture open the store too; each refuses before any terminal or
    // file is touched.
    for args in [vec![] as Vec<&str>, vec!["capture"]] {
        let mut child = Command::new(binary())
            .args(&args)
            .current_dir(&cwd)
            .env_remove("HOME")
            .env_remove("TSK_STATE_DIR")
            .env_remove("USERPROFILE")
            .env_remove("LOCALAPPDATA")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn tsk without HOME");
        drop(child.stdin.take());
        let output = wait_with_output_before_deadline(child, &format!("tsk {args:?} without HOME"));
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("not set"), "{args:?}: {stderr}");
        assert!(
            !cwd.join(".tsk-state").exists(),
            "{args:?} created a cwd store"
        );
    }

    // So is `tsk setup herdr --help`, which never opens the store.
    let output = Command::new(binary())
        .args(["setup", "herdr", "--help"])
        .current_dir(&cwd)
        .env_remove("HOME")
        .env_remove("TSK_STATE_DIR")
        .env_remove("USERPROFILE")
        .env_remove("LOCALAPPDATA")
        .output()
        .expect("run tsk setup without HOME");
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    // The override alone is enough.
    let state = temp_state_dir("nohome-state");
    let output = Command::new(binary())
        .args(["list", "--all"])
        .current_dir(&cwd)
        .env_remove("HOME")
        .env("TSK_STATE_DIR", &state)
        .output()
        .expect("run tsk with TSK_STATE_DIR only");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let _ = std::fs::remove_dir_all(cwd);
    let _ = std::fs::remove_dir_all(state);
}
