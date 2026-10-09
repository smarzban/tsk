use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tsk_tui::domain::{DomainState, ProvenanceOrigin, TaskScope};
use tsk_tui::store::TaskStore;

#[cfg(unix)]
#[path = "support/spawn.rs"]
mod spawn;

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

/// A TUI never exits on its own, so the ceiling only has to outlast a loaded machine starting a
/// fresh binary (a first-launch scan on macOS), not race it.
const TUI_CEILING: Duration = Duration::from_secs(30);

fn wait_with_output_before_deadline(mut child: Child, description: &str) -> Output {
    let deadline = Instant::now() + TUI_CEILING;
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
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("make copied binary executable");
    }
    let mut command = Command::new(&executable);
    command
        .arg("update")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = wait_with_output_before_deadline(
        spawn::spawn_fresh_copy(&mut command).expect("spawn Homebrew update"),
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
    let deadline = Instant::now() + TUI_CEILING;
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

#[test]
fn a_dispatched_agent_signs_its_block_and_reply_with_tsk_agent() {
    let dir = temp_state_dir("agent-actor");
    let mut state = DomainState::new();
    state
        .create(
            "agent work",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("seed task");
    TaskStore::new(&dir).save(&state).expect("seed state");
    let run = |args: &[&str], agent: Option<&str>| {
        let mut command = Command::new(binary());
        command
            .args(args)
            .args(["--state-dir", dir.to_str().expect("UTF-8 state dir")])
            .env_remove("TSK_AGENT")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(agent) = agent {
            command.env("TSK_AGENT", agent);
        }
        let output = wait_with_output_before_deadline(command.spawn().expect("spawn"), "tsk");
        assert_eq!(output.status.code(), Some(0), "{output:?}");
    };
    run(
        &["status", "T1", "blocked", "--why", "which one?"],
        Some("Claude"),
    );
    run(&["reply", "T1", "this one"], None);
    run(&["reply", "T1", "thanks"], Some("claude"));

    let state = TaskStore::new(&dir).load().expect("load");
    let block = state.tasks()[0].block.as_ref().expect("open block");
    assert_eq!(block.by, "claude");
    let authors: Vec<_> = block
        .replies
        .iter()
        .map(|reply| reply.by.as_str())
        .collect();
    assert_eq!(authors, ["you", "claude"]);
    assert!(!block.answered());
    let _ = std::fs::remove_dir_all(dir);
}

/// `tsk status N started` on an assigned task that was never dispatched goes through the start
/// route, not the old plain status flip: the task's own agent (`TSK_AGENT` names the assignee)
/// gets a plain start, and anyone else outside Herdr gets a plain start that says why nothing
/// launched.
#[test]
fn an_assigned_start_outside_herdr_starts_plainly_and_says_why() {
    let dir = temp_state_dir("assigned-start");
    std::fs::create_dir_all(&dir).expect("state dir");
    std::fs::write(
        dir.join("config.toml"),
        "[agent.builder]\ncommand = [\"true\"]\n",
    )
    .expect("profiles");
    let mut state = DomainState::new();
    state
        .create_assigned(
            "assigned work",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
            Some("builder".into()),
        )
        .expect("seed task");
    TaskStore::new(&dir).save(&state).expect("seed state");
    let run = |agent: Option<&str>| {
        let mut command = Command::new(binary());
        command
            .args(["status", "T1", "started"])
            .args(["--state-dir", dir.to_str().expect("UTF-8 state dir")])
            .env_remove("TSK_AGENT")
            .env_remove("HERDR_ENV")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(agent) = agent {
            command.env("TSK_AGENT", agent);
        }
        wait_with_output_before_deadline(command.spawn().expect("spawn"), "tsk")
    };
    let status = || TaskStore::new(&dir).load().expect("load").tasks()[0].status;

    let own = run(Some("builder"));
    assert_eq!(own.status.code(), Some(0), "{own:?}");
    assert!(
        !String::from_utf8_lossy(&own.stdout).contains("no launch"),
        "{own:?}"
    );
    assert_eq!(status(), tsk_tui::domain::HumanStatus::Started);

    let mut state = TaskStore::new(&dir).load().expect("load");
    let id = state.tasks()[0].id;
    state
        .set_status(id, tsk_tui::domain::HumanStatus::Open)
        .expect("reopen");
    TaskStore::new(&dir).save(&state).expect("save");

    let started = run(None);
    assert_eq!(started.status.code(), Some(0), "{started:?}");
    let stdout = String::from_utf8_lossy(&started.stdout);
    assert!(stdout.starts_with("status T1 started"), "{stdout}");
    assert!(stdout.contains("no launch: not in Herdr"), "{stdout}");
    assert!(started.stderr.is_empty(), "{started:?}");
    assert_eq!(status(), tsk_tui::domain::HumanStatus::Started);
    let _ = std::fs::remove_dir_all(dir);
}

/// A blocked project task assigned to `builder`, dispatched into Herdr workspace `w0`.
fn blocked_dispatched_task(dir: &std::path::Path) {
    use tsk_tui::domain::{BlockDraft, Dispatch};
    let store = TaskStore::new(dir);
    let mut state = DomainState::new();
    let id = state
        .create_assigned(
            "agent asks",
            None,
            TaskScope::Project {
                path: "/repos/app".into(),
            },
            ProvenanceOrigin::Manual,
            None,
            Some("builder".into()),
        )
        .expect("seed task");
    store.reload_merge_save(&mut state).expect("save");
    state
        .record_dispatch(
            id,
            Dispatch {
                argv: vec!["true".into()],
                worktree: "/tmp/tsk-process-reply-worktree".into(),
                branch: "tsk/t1".into(),
                base: Some("main".into()),
                base_ref: None,
                base_commit: None,
                base_remote: None,
                herdr_workspace_id: "w0".into(),
                at: SystemTime::now(),
                cleaned: false,
            },
        )
        .expect("record");
    store.reload_merge_save(&mut state).expect("save");
    let draft =
        BlockDraft::from_input(Some("which db?"), None, &[], Default::default()).expect("draft");
    state.block(id, draft, "builder").expect("block");
    store.reload_merge_save(&mut state).expect("save");
}

fn reply_send(dir: &std::path::Path, text: &str, env: &[(&str, &str)], herdr: bool) -> Output {
    let mut command = Command::new(binary());
    command
        .args(["reply", "T1", "--send", "--state-dir"])
        .arg(dir)
        .arg("--")
        .arg(text)
        .env_remove("TSK_AGENT")
        .env_remove("HERDR_ENV")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if herdr {
        command.env("HERDR_ENV", "1");
    }
    for (key, value) in env {
        command.env(key, value);
    }
    wait_with_output_before_deadline(command.spawn().expect("spawn"), "tsk reply --send")
}

fn still_blocked(dir: &std::path::Path) -> bool {
    TaskStore::new(dir).load().expect("load").tasks()[0].status
        == tsk_tui::domain::HumanStatus::Blocked
}

/// Outside Herdr `--send` stores the reply, says why nothing went out, and exits 0.
#[test]
fn reply_send_outside_herdr_stores_the_reply_and_says_not_sent() {
    let dir = temp_state_dir("reply-send-outside");
    blocked_dispatched_task(&dir);
    let output = reply_send(&dir, "use postgres", &[], false);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(lines[0].starts_with("replied T1 as you"), "{stdout}");
    assert_eq!(lines[1], "not sent: not in Herdr");
    assert!(still_blocked(&dir));
    let state = TaskStore::new(&dir).load().expect("load");
    let block = state.tasks()[0].block.as_ref().expect("open block");
    assert_eq!(
        block.replies.last().map(|reply| reply.text.as_str()),
        Some("use postgres")
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// The real host's prompt goes through `herdr agent prompt <pane> <text>` with the text as
/// one unchanged argument, after checking the pane's agent is the dispatched one; Herdr's
/// `agent_blocked` reads as a waiting agent.
#[cfg(unix)]
#[test]
fn reply_send_runs_herdr_agent_prompt_with_the_text_as_one_argument() {
    let dir = temp_state_dir("reply-send-herdr");
    let bin = dir.join("fake-bin");
    std::fs::create_dir_all(&bin).expect("bin dir");
    let source = dir.join("herdr.sh");
    std::fs::write(
        &source,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_HERDR_CALLS"
case "$1 $2" in
  "pane list")
    if [ "$4" != w0 ]; then
      echo '{"error":{"code":"workspace_not_found","message":"no such workspace"}}' >&2
      exit 1
    fi
    echo '{"result":{"panes":[{"pane_id":"w0:p1"}]}}' ;;
  "agent get")
    printf '{"result":{"agent":{"name":"%s","pane_id":"%s"}}}\n' "$FAKE_HERDR_NAME" "$3" ;;
  "agent prompt")
    printf '%s\0' "$@" > "$FAKE_HERDR_PROMPT_ARGS"
    if [ "$FAKE_HERDR_PROMPT" = blocked ]; then
      echo '{"error":{"code":"agent_blocked","message":"agent is blocked"}}' >&2
      exit 1
    fi
    echo '{"result":{"type":"agent_prompted"}}' ;;
  *)
    echo '{"error":{"code":"unexpected","message":"unexpected call"}}' >&2
    exit 1 ;;
esac
"#,
    )
    .expect("write fake herdr");
    // Copy it into place from another process, so no write descriptor of ours is open
    // when tsk executes it (ETXTBSY on Linux).
    let herdr = bin.join("herdr");
    let copied = Command::new("cp")
        .arg(&source)
        .arg(&herdr)
        .status()
        .expect("cp");
    assert!(copied.success());
    let made = Command::new("chmod")
        .arg("+x")
        .arg(&herdr)
        .status()
        .expect("chmod");
    assert!(made.success());

    blocked_dispatched_task(&dir);
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let calls = dir.join("calls.log");
    let args = dir.join("prompt-args");
    let text = "line one\n\"quoted\" 'two' $HOME";
    let env = |name: &'static str, prompt: &'static str| {
        vec![
            ("PATH", path.clone()),
            ("FAKE_HERDR_CALLS", calls.display().to_string()),
            ("FAKE_HERDR_PROMPT_ARGS", args.display().to_string()),
            ("FAKE_HERDR_NAME", name.to_string()),
            ("FAKE_HERDR_PROMPT", prompt.to_string()),
        ]
    };
    let run = |vars: Vec<(&'static str, String)>| {
        let pairs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let output = reply_send(&dir, text, &pairs, true);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .nth(1)
            .expect("second line")
            .to_string()
    };

    assert_eq!(run(env("t1-builder", "ok")), "sent to @builder");
    let captured = std::fs::read(&args).expect("prompt argv");
    let argv: Vec<String> = captured
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect();
    assert_eq!(
        argv,
        [
            "agent".to_string(),
            "prompt".to_string(),
            "w0:p1".to_string(),
            format!("[tsk T1 reply] {text}"),
        ]
    );
    let log = std::fs::read_to_string(&calls).expect("calls");
    assert!(log.contains("pane list --workspace w0"), "{log}");
    assert!(log.contains("agent get w0:p1"), "{log}");

    std::fs::remove_file(&args).expect("reset argv");
    assert_eq!(
        run(env("t1-builder", "blocked")),
        "not sent: @builder is waiting on a prompt"
    );
    std::fs::remove_file(&args).expect("reset argv");
    assert_eq!(
        run(env("reviewer", "ok")),
        "not sent: @builder is not in its pane"
    );
    assert!(!args.exists(), "another agent never gets a prompt");
    assert!(still_blocked(&dir));
    let _ = std::fs::remove_dir_all(dir);
}
