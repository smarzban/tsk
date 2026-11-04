//! Rex calls behind the dispatch host (T411 spike).
//!
//! One Rex session per dispatched task. tsk creates the worktree with `git worktree add` under
//! the dispatch worktree root (`host::worktree_root`, shared with Herdr), then
//! `session.create` opens the session with one block whose command is the launcher, so no
//! task text and no `env` value ever passes through a shell Rex typed into. The session and the
//! block are both labelled with the agent name (`t<n>-<profile>`); the session id is recorded.
//!
//! Liveness reads the agent's block, matched by label pattern, never by session label: Rex can
//! let an agent rename its session. A missing session or block is gone. Otherwise the agent's
//! own report (`program_status`, OSC 7501) decides, cross-checked against the foreground
//! process, because a report outlives the process that sent it; with no current report the
//! foreground process alone decides. A shell in the foreground is gone: after a Rex server
//! restart every terminal comes back as a fresh shell with the agent gone.
//!
//! The agent's block stays open after its program exits (`exit.on_completion: false`), so an
//! agent that fails at once leaves its error on screen; with no process left it reads as gone.
//! A relaunch types the launch line into a block that holds a bare shell in the worktree (the
//! server-restart case), and otherwise closes the dead session and opens a new one.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{json, Value};

use super::{CreatedWorktree, PaneAgent, PromptError, RootPaneError};

/// Rex's exit status for a target that does not exist (`session target … not found`).
const REX_NOT_FOUND: i32 = 3;

/// Prefix of the provisional workspace id a Rex worktree carries until its session exists.
pub(super) const PENDING_PREFIX: &str = "rex-pending:";

/// Whether `id` has the shape of a Rex session id. The dispatch record's host decides which
/// host owns a dispatch; this only keeps text that is not a session id (which Rex could
/// resolve as a session label) out of a Rex target argument.
fn is_session_id(id: &str) -> bool {
    id.starts_with("session:") || id.starts_with(PENDING_PREFIX)
}

/// What Rex reports about the agent in a dispatched block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RexAgent {
    /// The block, or its program, is gone, or only a shell is left in it.
    Gone,
    /// The agent's own report: `idle`, `working`, `done`, `blocked`, or `error`, with what a
    /// blocked agent waits for (`permission`, `question`, `auth`) when it said.
    Reported { state: String, kind: Option<String> },
    /// No current report, but a program other than a shell runs in the foreground.
    Running { process: String },
}

/// `git worktree add -b <branch> <path> <base>`, or `git worktree add <path> <branch>` when
/// the branch already exists (a relaunch after cleanup kept it), as Herdr's create does. The
/// workspace id is provisional until the session opens; the root pane is unknown until then.
pub fn create_worktree(
    project: &Path,
    path: &Path,
    branch: &str,
    base: Option<&str>,
) -> Result<CreatedWorktree, String> {
    if path.symlink_metadata().is_ok() {
        return Err(format!("{} already exists", path.display()));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    let existing = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    let mut command = Command::new("git");
    command.arg("-C").arg(project).args(["worktree", "add"]);
    if existing.status.success() {
        command.arg(path).arg(branch);
    } else {
        command.args(["-b", branch]).arg(path);
        if let Some(base) = base {
            command.arg(base);
        }
    }
    let output = command
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(CreatedWorktree {
        path: path.to_path_buf(),
        branch: branch.to_string(),
        workspace_id: format!("{PENDING_PREFIX}{}", path.display()),
        root_pane_id: String::new(),
    })
}

/// The `session.create` request: one window, one terminal block running `command` directly in
/// `cwd` (`shell: none`; the launcher re-runs itself in the login shell), all labelled `label`.
/// The block stays open after the command exits, so what an agent printed as it failed stays
/// readable until the session is closed.
///
/// `path` goes in front of the block's PATH. Rex starts a block from the server's own minimal
/// environment, and a login shell reads only the profile, so a program the user put on PATH in
/// an interactive startup file (`~/.zshrc`) would not be found. The PATH tsk itself runs with
/// stands in for the pane shell a Herdr launch inherits.
pub fn session_request(label: &str, cwd: &Path, command: &[String], path: &[String]) -> Value {
    json!({
        "label": label,
        "initial_windows": [{
            "window_label": label,
            "layout": {"block": {
                "flavor": "com.superlogical.terminal.shell",
                "label": label,
                "options": {
                    "shell": "none",
                    "cwd": cwd.to_string_lossy(),
                    "command": command,
                    "path_prepend": path,
                    "exit": {"on_completion": false},
                },
            }},
        }],
    })
}

/// The PATH entries of this process, for [`session_request`].
pub fn inherited_path() -> Vec<String> {
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .filter(|entry| !entry.as_os_str().is_empty())
                .map(|entry| entry.to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// The pane id tsk hands around for a Rex block: Rex addresses a block only within its
/// session, so both ride along as `<session>/<block>` (Rex ids hold no `/`).
pub fn pane_id(session: &str, block: &str) -> String {
    format!("{session}/{block}")
}

fn split_pane(pane: &str) -> Result<(&str, &str), String> {
    pane.split_once('/')
        .ok_or_else(|| format!("{pane} is not a Rex pane id"))
}

/// Open the task's session with the agent's block: `(session id, pane id)`.
pub fn open_session(
    program: &Path,
    label: &str,
    cwd: &Path,
    command: &[String],
) -> Result<(String, String), String> {
    let request = session_request(label, cwd, command, &inherited_path()).to_string();
    let output = rex(program, &["api", "call", "session.create", &request])?;
    let value = rex_json(output)?;
    let session = value
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or("rex session.create returned no session id")?;
    let Some(block) = value
        .pointer("/initial_windows/0/block_ids/0")
        .and_then(Value::as_str)
    else {
        // The session opened and runs the launcher: close it, or the agent runs in a worktree
        // the failed launch is about to remove.
        let closed = kill_session(program, session)
            .err()
            .map(|error| format!("; could not close it: {error}"))
            .unwrap_or_default();
        return Err(format!(
            "rex session.create returned no block for {session}{closed}"
        ));
    };
    Ok((session.to_string(), pane_id(session, block)))
}

/// The session's blocks, `None` when the session is gone.
fn session_blocks(program: &Path, session: &str) -> Result<Option<Vec<Value>>, String> {
    let output = rex(program, &["block", "ls", "-s", session, "--json"])?;
    if not_found(&output) {
        return Ok(None);
    }
    Ok(Some(
        rex_json(output)?
            .get("blocks")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    ))
}

fn label_of(block: &Value) -> Option<&str> {
    block.get("label").and_then(Value::as_str)
}

/// The dispatched agent's pane in `session`: the block labelled `t<n>-…`. A missing session, or
/// one whose agent block closed, is gone.
pub fn agent_block(program: &Path, session: &str) -> Result<String, RootPaneError> {
    if session.starts_with(PENDING_PREFIX) {
        return Err(RootPaneError::WorkspaceGone(format!(
            "rex session for {session} never opened"
        )));
    }
    if !is_session_id(session) {
        // Not a session id: a lookup by that text could match a Rex session's label.
        return Err(RootPaneError::Failed(format!(
            "{session} is not a Rex session"
        )));
    }
    let blocks = session_blocks(program, session)
        .map_err(RootPaneError::Failed)?
        .ok_or_else(|| RootPaneError::WorkspaceGone(format!("rex session {session} not found")))?;
    blocks
        .iter()
        .find(|block| label_of(block).is_some_and(is_agent_label))
        .and_then(|block| block.get("block_id").and_then(Value::as_str))
        .map(|block| pane_id(session, block))
        .ok_or_else(|| {
            RootPaneError::WorkspaceGone(format!("rex session {session} has no agent block"))
        })
}

/// `t<digits>-<profile>`: the label [`super::agent_name`] gives.
pub fn is_agent_label(label: &str) -> bool {
    label
        .strip_prefix('t')
        .and_then(|rest| rest.split_once('-'))
        .is_some_and(|(number, profile)| {
            !number.is_empty()
                && number.bytes().all(|byte| byte.is_ascii_digit())
                && !profile.is_empty()
        })
}

/// Ask Rex about the agent in `pane` (`<session>/<block>`), with the block's label.
pub fn agent(program: &Path, pane: &str) -> Result<(RexAgent, Option<String>), String> {
    let (session, block) = split_pane(pane)?;
    if !is_session_id(session) {
        return Err(format!("{session} is not a Rex session"));
    }
    let Some(blocks) = session_blocks(program, session)? else {
        return Ok((RexAgent::Gone, None));
    };
    let Some(found) = blocks
        .iter()
        .find(|entry| entry.get("block_id").and_then(Value::as_str) == Some(block))
    else {
        return Ok((RexAgent::Gone, None));
    };
    let label = label_of(found).map(str::to_string);
    let status = rex(
        program,
        &[
            "block",
            "call",
            "-s",
            session,
            "-b",
            block,
            "program_status",
        ],
    )?;
    if not_found(&status) {
        return Ok((RexAgent::Gone, label));
    }
    let status = rex_json(status)?;
    let process = rex(
        program,
        &["block", "call", "-s", session, "-b", block, "process"],
    )?;
    if not_found(&process) {
        return Ok((RexAgent::Gone, label));
    }
    Ok((classify(&status, &rex_json(process)?), label))
}

/// [`agent`] as the dispatch seam reads it, named by the block's label.
pub fn pane_agent(program: &Path, pane: &str) -> Result<PaneAgent, String> {
    Ok(match agent(program, pane)? {
        (RexAgent::Gone, _) => PaneAgent::Absent,
        (RexAgent::Reported { .. } | RexAgent::Running { .. }, name) => PaneAgent::Present { name },
    })
}

/// The liveness rule, from `program_status` and `process` results.
pub fn classify(status: &Value, process: &Value) -> RexAgent {
    let foreground = process
        .get("foreground")
        .filter(|value| !value.is_null())
        .or_else(|| process.get("child").filter(|value| !value.is_null()));
    let Some(foreground) = foreground else {
        return RexAgent::Gone;
    };
    let pid = foreground.get("pid").and_then(Value::as_i64);
    let name = foreground
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // A report counts only while the process that sent it is still the foreground one: the
    // system reuses pids, so the name is compared too.
    let current = status
        .get("records")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|record| {
            record.pointer("/owner/pid").and_then(Value::as_i64) == pid
                && record.pointer("/owner/name").and_then(Value::as_str) == Some(name)
        })
        // The root record (no id) is the program's own; a child record is one of its tasks.
        .min_by_key(|record| record.get("id").is_some());
    if let Some(record) = current {
        let state = record
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("idle")
            .to_string();
        let kind = record
            .get("kind")
            .and_then(Value::as_str)
            .map(str::to_string);
        return RexAgent::Reported { state, kind };
    }
    if is_shell(name) {
        RexAgent::Gone
    } else {
        RexAgent::Running {
            process: name.to_string(),
        }
    }
}

/// Whether the foreground program is a bare shell. `login` is not: Rex starts a block's
/// command under `/usr/bin/login`, which stays the foreground process group leader while the
/// agent runs as its child, so `login` in the foreground means the launch is still running.
fn is_shell(name: &str) -> bool {
    let name = name.trim_start_matches('-');
    let name = name.rsplit('/').next().unwrap_or(name);
    matches!(
        name,
        "sh" | "bash" | "zsh" | "fish" | "nu" | "dash" | "ksh" | "tcsh" | "csh"
    )
}

/// What a dispatched block holds right now, for a relaunch or a typed line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockProgram {
    /// A bare shell in the foreground, in `cwd` when Rex knows it.
    Shell { cwd: Option<String> },
    /// The block's program exited and nothing runs in it.
    Exited,
    /// Some other program runs in the foreground: the agent, or something it started.
    Busy { process: String },
}

/// The program in a `process` result.
pub fn block_program(process: &Value) -> BlockProgram {
    let foreground = process
        .get("foreground")
        .filter(|value| !value.is_null())
        .or_else(|| process.get("child").filter(|value| !value.is_null()));
    let Some(foreground) = foreground else {
        return BlockProgram::Exited;
    };
    let name = foreground
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if is_shell(name) {
        BlockProgram::Shell {
            cwd: foreground
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::to_string),
        }
    } else {
        BlockProgram::Busy {
            process: name.to_string(),
        }
    }
}

/// What runs in `pane` (`<session>/<block>`). A missing session or block is
/// [`RootPaneError::WorkspaceGone`].
pub fn pane_program(program: &Path, pane: &str) -> Result<BlockProgram, RootPaneError> {
    let (session, block) = split_pane(pane)?;
    if !is_session_id(session) {
        return Err(RootPaneError::Failed(format!(
            "{session} is not a Rex session"
        )));
    }
    let process = rex(
        program,
        &["block", "call", "-s", session, "-b", block, "process"],
    )?;
    if not_found(&process) {
        return Err(RootPaneError::WorkspaceGone(format!(
            "rex block {pane} not found"
        )));
    }
    Ok(block_program(&rex_json(process)?))
}

/// The pane a relaunch reuses in `session`: its agent block, when it holds a bare shell in
/// `worktree` (the agent is gone, as after a Rex server restart). A block whose program exited
/// closes with its session and reads as gone, so the relaunch opens a new one. Anything still
/// running there, or a shell elsewhere, refuses: nothing launches on a guess.
pub fn reusable_pane(
    program: &Path,
    session: &str,
    worktree: &Path,
) -> Result<String, RootPaneError> {
    let pane = agent_block(program, session)?;
    match pane_program(program, &pane)? {
        BlockProgram::Shell { cwd } => {
            let here = cwd.as_deref().is_some_and(|cwd| {
                let canonical =
                    |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
                canonical(Path::new(cwd)) == canonical(worktree)
            });
            if here {
                Ok(pane)
            } else {
                Err(RootPaneError::Failed(format!(
                    "the shell in rex session {session} is not in the worktree {}; close the \
                     session to relaunch",
                    worktree.display()
                )))
            }
        }
        BlockProgram::Exited => {
            kill_session(program, session).map_err(RootPaneError::Failed)?;
            Err(RootPaneError::WorkspaceGone(format!(
                "rex session {session} held an exited agent"
            )))
        }
        BlockProgram::Busy { process } => Err(RootPaneError::Failed(format!(
            "{process} still runs in rex session {session}"
        ))),
    }
}

/// Give the block in `pane` the label `label`.
pub fn label_block(program: &Path, pane: &str, label: &str) -> Result<(), String> {
    let (session, block) = split_pane(pane)?;
    let request = json!({"session_id": session, "block_id": block, "label": label}).to_string();
    rex_json(rex(
        program,
        &["api", "call", "session.set_block_label", &request],
    )?)
    .map(|_| ())
}

/// Type `line` and Enter into the bare shell in `pane`, checked again just before: a line typed
/// into anything else could reach a running program.
pub fn type_line(program: &Path, pane: &str, line: &str) -> Result<(), String> {
    match pane_program(program, pane).map_err(|error| error.to_string())? {
        BlockProgram::Shell { .. } => {}
        BlockProgram::Exited => return Err(format!("the shell in {pane} exited")),
        BlockProgram::Busy { process } => {
            return Err(format!("{process} runs in {pane}; nothing typed"))
        }
    }
    let (session, block) = split_pane(pane)?;
    let data = json!({"data": format!("{line}\r")}).to_string();
    rex_json(rex(
        program,
        &["block", "call", "-s", session, "-b", block, "write", &data],
    )?)
    .map(|_| ())
}

/// Submit `text` to the agent in `pane` as one message: a paste, then Enter. Only an agent
/// whose current report is not `blocked` receives it; one with no report (at a first-run
/// dialog, or a program that never reports) is not typed into, since Enter could answer a
/// prompt nobody saw.
pub fn prompt(program: &Path, pane: &str, text: &str) -> Result<(), PromptError> {
    match agent(program, pane).map_err(PromptError::Failed)? {
        (RexAgent::Gone, _) => {
            return Err(PromptError::Failed(format!("the agent in {pane} is gone")))
        }
        (RexAgent::Reported { state, .. }, _) if state == "blocked" => {
            return Err(PromptError::AgentBlocked)
        }
        (RexAgent::Reported { .. }, _) => {}
        (RexAgent::Running { process }, _) => {
            return Err(PromptError::Failed(format!(
                "{process} has not reported its status, so tsk cannot tell it is ready for input"
            )))
        }
    }
    let (session, block) = split_pane(pane).map_err(PromptError::Failed)?;
    let paste = json!({
        "session_id": session,
        "block_id": block,
        "event": {"type": "paste", "payload": {"text": text}},
    })
    .to_string();
    rex_json(rex(program, &["api", "call", "block.event", &paste]).map_err(PromptError::Failed)?)
        .map_err(PromptError::Failed)?;
    let enter = rex(program, &["send-key", "-s", session, "-b", block, "enter"])
        .map_err(PromptError::Failed)?;
    if enter.status.success() {
        Ok(())
    } else {
        Err(PromptError::Failed(rex_failure(&enter)))
    }
}

/// Whether `session` is still open. A session that never opened is not.
pub fn session_exists(program: &Path, session: &str) -> Result<bool, String> {
    if session.starts_with(PENDING_PREFIX) {
        return Ok(false);
    }
    if !is_session_id(session) {
        return Err(format!("{session} is not a Rex session"));
    }
    Ok(session_blocks(program, session)?.is_some())
}

/// Close a session tsk opened. A session already gone is closed.
pub fn kill_session(program: &Path, session: &str) -> Result<(), String> {
    if session.starts_with(PENDING_PREFIX) {
        return Ok(());
    }
    if !is_session_id(session) {
        return Err(format!("{session} is not a Rex session"));
    }
    let output = rex(program, &["session", "kill", session])?;
    if output.status.success() || not_found(&output) {
        Ok(())
    } else {
        Err(rex_failure(&output))
    }
}

fn rex(program: &Path, args: &[&str]) -> Result<Output, String> {
    Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("could not run rex: {error}"))
}

fn not_found(output: &Output) -> bool {
    output.status.code() == Some(REX_NOT_FOUND)
        && String::from_utf8_lossy(&output.stderr).contains("not found")
}

fn rex_failure(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim().trim_start_matches("Error: ");
    if detail.is_empty() {
        format!("rex failed with {}", output.status)
    } else {
        format!("rex: {detail}")
    }
}

fn rex_json(output: Output) -> Result<Value, String> {
    if !output.status.success() {
        return Err(rex_failure(&output));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("rex returned invalid JSON: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(name: &str, pid: i64) -> Value {
        json!({
            "last_exit": null,
            "child": {"pid": 10, "name": "zsh"},
            "foreground": {"pid": pid, "name": name},
        })
    }

    fn record(state: &str, pid: i64, name: &str) -> Value {
        json!({"state": state, "app": "claude-code", "owner": {"pid": pid, "name": name}})
    }

    #[test]
    fn host_kind_prefers_herdr_then_rex() {
        use super::super::HostKind;
        assert_eq!(
            HostKind::from_env(Some("1"), Some("session:a")),
            HostKind::Herdr
        );
        assert_eq!(HostKind::from_env(None, Some("session:a")), HostKind::Rex);
        assert_eq!(
            HostKind::from_env(Some("0"), Some("session:a")),
            HostKind::Rex
        );
        assert_eq!(HostKind::from_env(None, Some(" ")), HostKind::Standalone);
        assert_eq!(HostKind::from_env(None, None), HostKind::Standalone);
        assert!(HostKind::Rex.is_hosted());
    }

    #[test]
    fn a_current_report_decides() {
        let status = json!({"records": [{
            "state": "blocked", "kind": "permission",
            "owner": {"pid": 42, "name": "2.1.318"},
        }]});
        assert_eq!(
            classify(&status, &process("2.1.318", 42)),
            RexAgent::Reported {
                state: "blocked".into(),
                kind: Some("permission".into())
            }
        );
    }

    #[test]
    fn a_report_from_a_gone_process_does_not_count() {
        let status = json!({"records": [record("working", 42, "2.1.318")]});
        // The agent exited back to a shell (or Rex restarted the terminal).
        assert_eq!(classify(&status, &process("zsh", 77)), RexAgent::Gone);
        // A reused pid under another name is not the reporter.
        assert_eq!(
            classify(&status, &process("vim", 42)),
            RexAgent::Running {
                process: "vim".into()
            }
        );
    }

    #[test]
    fn no_report_falls_back_to_the_foreground_process() {
        let none = json!({"records": []});
        assert_eq!(
            classify(&none, &process("codex", 5)),
            RexAgent::Running {
                process: "codex".into()
            }
        );
        assert_eq!(classify(&none, &process("-zsh", 5)), RexAgent::Gone);
        // Seen live: an agent at its trust dialog reports nothing, under Rex's `login`.
        assert_eq!(
            classify(&none, &process("login", 5)),
            RexAgent::Running {
                process: "login".into()
            }
        );
        assert_eq!(
            classify(
                &none,
                &json!({"last_exit": {"code": 0}, "child": null, "foreground": null})
            ),
            RexAgent::Gone
        );
    }

    #[test]
    fn the_root_record_wins_over_a_child_task() {
        let status = json!({"records": [
            {"id": "build", "state": "working", "owner": {"pid": 1, "name": "pi"}},
            {"state": "idle", "owner": {"pid": 1, "name": "pi"}},
        ]});
        assert_eq!(
            classify(&status, &process("pi", 1)),
            RexAgent::Reported {
                state: "idle".into(),
                kind: None
            }
        );
    }

    #[test]
    fn only_session_shaped_ids_are_rex_records() {
        assert!(is_session_id("session:01a1"));
        assert!(is_session_id("rex-pending:/w/t12"));
        assert!(!is_session_id("w9"));
        assert!(!is_session_id("t12-claude"));
        assert!(matches!(
            agent_block(Path::new("rex"), "w9"),
            Err(RootPaneError::Failed(_))
        ));
        assert!(kill_session(Path::new("rex"), "w9").is_err());
    }

    #[test]
    fn agent_labels_are_task_and_profile() {
        assert!(is_agent_label("t12-claude"));
        assert!(is_agent_label("t1234-review-strict"));
        assert!(!is_agent_label("shell"));
        assert!(!is_agent_label("t-claude"));
        assert!(!is_agent_label("t12-"));
        assert!(!is_agent_label("tx2-claude"));
    }

    #[test]
    fn the_session_runs_the_launcher_directly_under_the_agent_label() {
        let request = session_request(
            "t12-claude",
            Path::new("/w/t12"),
            &["/bin/sh".into(), "/state/launchers/dispatch-x.sh".into()],
            &["/home/u/.local/bin".into()],
        );
        assert_eq!(request["label"], "t12-claude");
        let block = &request["initial_windows"][0]["layout"]["block"];
        assert_eq!(block["label"], "t12-claude");
        assert_eq!(block["options"]["shell"], "none");
        assert_eq!(block["options"]["cwd"], "/w/t12");
        assert_eq!(
            block["options"]["command"],
            json!(["/bin/sh", "/state/launchers/dispatch-x.sh"])
        );
        assert_eq!(
            block["options"]["path_prepend"],
            json!(["/home/u/.local/bin"])
        );
        // An agent that fails at once leaves its error on screen.
        assert_eq!(block["options"]["exit"], json!({"on_completion": false}));
    }

    #[test]
    fn a_block_holds_a_shell_an_exited_program_or_something_busy() {
        assert_eq!(
            block_program(&json!({"foreground": {"pid": 1, "name": "-zsh", "cwd": "/w/t12"}})),
            BlockProgram::Shell {
                cwd: Some("/w/t12".into())
            }
        );
        assert_eq!(
            block_program(&json!({"last_exit": {"code": 1}, "child": null, "foreground": null})),
            BlockProgram::Exited
        );
        assert_eq!(
            block_program(&process("login", 5)),
            BlockProgram::Busy {
                process: "login".into()
            }
        );
    }
}
