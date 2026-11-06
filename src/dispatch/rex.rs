//! Rex calls behind the dispatch host (T411 spike).
//!
//! One Rex tab (window) per dispatched task, in the session tsk was run from. tsk creates the
//! worktree with `git worktree add` under the dispatch worktree root (`host::worktree_root`,
//! shared with Herdr), then `session.new_window` opens the tab with one block whose command is
//! the launcher, so no task text and no `env` value ever passes through a shell Rex typed into.
//! The tab and the block are both labelled with the agent name (`t<n>-<profile>`); the record
//! keeps `<session>/<window>`. A record from before tabs keeps a bare session id: the whole
//! session is the agent's, as it was then, and only such a record ever closes a session.
//!
//! Liveness reads the agent's block in the tab, matched by label pattern, never by tab label.
//! A missing session, tab, or block is gone. Otherwise the agent's own report
//! (`program_status`, OSC 7501) decides, cross-checked against the foreground process, because
//! a report outlives the process that sent it; with no current report the foreground process
//! alone decides. A shell in the foreground is gone: after a Rex server restart every terminal
//! comes back as a fresh shell with the agent gone.
//!
//! The agent's block stays open after its program exits (`exit.on_completion: false`), so an
//! agent that fails at once leaves its error on screen; with no process left it reads as gone.
//! A relaunch types the launch line into a block that holds a bare shell in the worktree (the
//! server-restart case). An exited agent's block is replaced by a new one in the same tab, so
//! the user's other blocks there are never touched. A closed agent block gets a new one in the
//! same tab when every block left there holds a bare shell or an exited program; any other
//! program there may be the agent under another label, so nothing launches. A closed tab gets
//! a new one in its session, and a closed session a new tab in the session tsk runs in.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{json, Value};

use super::{CreatedWorktree, PaneAgent, PromptError, Reuse, RootPaneError};

/// Rex's exit status for a target that does not exist (`session target … not found`).
const REX_NOT_FOUND: i32 = 3;

/// Prefix of the provisional workspace id a Rex worktree carries until its tab exists.
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
/// workspace id is provisional until the tab opens; the root pane is unknown until then.
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

/// The `session.new_window` request: a tab in `session`, labelled `label`, holding one
/// terminal block labelled `label` that runs `command` directly in `cwd` (`shell: none`; the
/// launcher re-runs itself in the login shell). It does not take focus from the pane tsk runs
/// in. The block stays open after the command exits, so what an agent printed as it failed
/// stays readable until the tab is closed.
///
/// `path` goes in front of the block's PATH. Rex starts a block from the server's own minimal
/// environment, and a login shell reads only the profile, so a program the user put on PATH in
/// an interactive startup file (`~/.zshrc`) would not be found. The PATH tsk itself runs with
/// stands in for the pane shell a Herdr launch inherits.
pub fn window_request(
    session: &str,
    label: &str,
    cwd: &Path,
    command: &[String],
    path: &[String],
) -> Value {
    json!({
        "session_id": session,
        "window_label": label,
        "focus": false,
        "layout": agent_layout(label, cwd, command, path),
    })
}

/// The layout node of the agent's block, for a new tab or a block replacing an exited one:
/// `command` runs directly in `cwd` (see [`window_request`]).
fn agent_layout(label: &str, cwd: &Path, command: &[String], path: &[String]) -> Value {
    json!({"block": {
        "flavor": "com.superlogical.terminal.shell",
        "label": label,
        "options": {
            "shell": "none",
            "cwd": cwd.to_string_lossy(),
            "command": command,
            "path_prepend": path,
            "exit": {"on_completion": false},
        },
    }})
}

/// The `session.new_split` request that opens the agent's block beside `anchor`, without
/// taking focus from whatever the user is doing in that session.
pub fn replacement_request(
    session: &str,
    anchor: &str,
    label: &str,
    cwd: &Path,
    command: &[String],
    path: &[String],
) -> Value {
    json!({
        "session_id": session,
        "anchor_id": anchor,
        "direction": "horizontal",
        "side": "after",
        "focus": false,
        "layout": agent_layout(label, cwd, command, path),
    })
}

/// Label a dead agent block carries while its replacement opens: not an agent label, so two
/// blocks never both read as the task's agent.
const EXITED_LABEL: &str = "exited agent";

/// Replace the exited agent's block in `pane` (`<session>/<block>`) with a new block beside it,
/// in the same tab, running `command` in `cwd`, labelled `label`: the tab and every other
/// block stay. Returns the new pane id, and a note when the dead block could not be
/// closed (the new agent runs either way).
///
/// The dead block gives up the agent label first, so a later lookup can only find the new
/// block. If Rex fails to open the new block, the label goes back and nothing is closed. If
/// Rex succeeds but its answer names no block, a new agent may run: the dead block keeps its
/// non-agent label, nothing is closed, and the launch fails.
pub fn replace_block(
    program: &Path,
    pane: &str,
    label: &str,
    cwd: &Path,
    command: &[String],
) -> Result<(String, Option<String>), String> {
    let (session, block) = split_pane(pane)?;
    if !is_session_id(session) || session.starts_with(PENDING_PREFIX) {
        return Err(format!("{session} is not a Rex session"));
    }
    let old_label = rex_json(rex(program, &["block", "ls", "-s", session, "--json"])?)?
        .get("blocks")
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks
                .iter()
                .find(|entry| entry.get("block_id").and_then(Value::as_str) == Some(block))
                .and_then(label_of)
                .map(str::to_string)
        })
        .ok_or_else(|| format!("rex block {pane} not found"))?;
    label_block(program, pane, EXITED_LABEL)?;
    let new_block = match open_beside(program, session, block, label, cwd, command) {
        Ok(block) => block,
        // Rex did not open the block: the dead block takes its label back, nothing closes.
        Err(Split::Refused(error)) => {
            let restored = label_block(program, pane, &old_label)
                .err()
                .map(|restore| format!("; could not label {pane} {old_label} again: {restore}"))
                .unwrap_or_default();
            return Err(format!("{error}{restored}"));
        }
        // Rex answered success, so a new agent block may run under the agent label even though
        // the answer does not say which: the dead block keeps its non-agent label, nothing
        // closes.
        Err(Split::Unreadable(error)) => return Err(error),
    };
    let close = json!({"session_id": session, "block_id": block}).to_string();
    let note = rex(program, &["api", "call", "block.close", &close])
        .and_then(rex_json)
        .err()
        .map(|error| format!("the exited agent's block {pane} stays open ({error})"));
    Ok((pane_id(session, &new_block), note))
}

/// Why [`open_beside`] has no new block to hand back.
enum Split {
    /// Rex did not open a block.
    Refused(String),
    /// Rex answered success without naming the block: one may run under the agent label.
    Unreadable(String),
}

/// Open the agent's block labelled `label` beside `anchor` in `session`, running `command` in
/// `cwd`: the new block id.
fn open_beside(
    program: &Path,
    session: &str,
    anchor: &str,
    label: &str,
    cwd: &Path,
    command: &[String],
) -> Result<String, Split> {
    let request =
        replacement_request(session, anchor, label, cwd, command, &inherited_path()).to_string();
    let output = match rex(program, &["api", "call", "session.new_split", &request]) {
        Ok(output) if output.status.success() => output,
        failed => {
            return Err(Split::Refused(
                failed.and_then(rex_json).err().unwrap_or_default(),
            ))
        }
    };
    rex_json(output)
        .ok()
        .and_then(|value| {
            value
                .pointer("/block_ids/0")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .ok_or_else(|| {
            Split::Unreadable(format!(
                "rex session.new_split gave no readable block in {session}; a new {label} block \
                 may have opened there, so check the session before relaunching"
            ))
        })
}

/// Open the agent's block labelled `label` in the session of `anchor` (`<session>/<block>`),
/// running `command` in `cwd`, after the agent's own block was closed: the new pane id. The
/// session and every block in it stay; nothing is relabelled or closed.
pub fn add_block(
    program: &Path,
    anchor: &str,
    label: &str,
    cwd: &Path,
    command: &[String],
) -> Result<String, String> {
    let (session, block) = split_pane(anchor)?;
    if !is_session_id(session) || session.starts_with(PENDING_PREFIX) {
        return Err(format!("{session} is not a Rex session"));
    }
    match open_beside(program, session, block, label, cwd, command) {
        Ok(new_block) => Ok(pane_id(session, &new_block)),
        Err(Split::Refused(error) | Split::Unreadable(error)) => Err(error),
    }
}

/// The PATH entries of this process, for [`window_request`].
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

/// Open the task's tab in `session` with the agent's block: `(tab id, pane id)`.
///
/// When Rex names the tab but no block, the tab is closed again, or the agent runs in a
/// worktree the failed launch is about to remove. When it names no tab, nothing can be closed
/// and the launch fails saying so.
pub fn open_tab(
    program: &Path,
    session: &str,
    label: &str,
    cwd: &Path,
    command: &[String],
) -> Result<(String, String), String> {
    if !session.starts_with("session:") || session.contains('/') {
        return Err(format!("{session} is not a Rex session"));
    }
    let request = window_request(session, label, cwd, command, &inherited_path()).to_string();
    let output = rex(program, &["api", "call", "session.new_window", &request])?;
    let value = rex_json(output)?;
    let Some(window) = value.get("window_id").and_then(Value::as_str) else {
        return Err(format!(
            "rex session.new_window gave no readable tab in {session}; a {label} tab may have \
             opened there, so check the session before dispatching again"
        ));
    };
    let tab = tab_id(session, window);
    let Some(block) = value.pointer("/block_ids/0").and_then(Value::as_str) else {
        let closed = close_window(program, session, window)
            .err()
            .map(|error| format!("; could not close it: {error}"))
            .unwrap_or_default();
        return Err(format!(
            "rex session.new_window returned no block for {tab}{closed}"
        ));
    };
    Ok((tab, pane_id(session, block)))
}

/// The session a new tab opens in: the recorded tab's session `near` while it is still open
/// (a relaunch after the tab was closed), else `here`, the session tsk runs in (`REX_SESSION`).
/// A record from before tabs owned its whole session, so its relaunch opens in `here`.
pub fn tab_session(
    program: &Path,
    near: Option<&str>,
    here: Option<&str>,
) -> Result<String, String> {
    let recorded = near
        .and_then(|near| workspace(near).ok())
        .filter(|at| at.window.is_some())
        .map(|at| at.session);
    if let Some(session) = recorded {
        if session_windows(program, session)?.is_some() {
            return Ok(session.to_string());
        }
    }
    here.filter(|here| !here.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| "no Rex session to open the tab in: run tsk in a Rex pane".to_string())
}

/// The recorded id of the tab `window` in `session`: `<session>/<window>`.
pub fn tab_id(session: &str, window: &str) -> String {
    format!("{session}/{window}")
}

/// A recorded Rex workspace: the agent's tab, or (a record from before tabs) a whole session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Workspace<'a> {
    pub session: &'a str,
    /// `None` for a record from before tabs: the whole session is the agent's.
    pub window: Option<&'a str>,
}

/// Parse a recorded workspace id, `<session>/<window>` or a bare `<session>`. Anything else
/// (text Rex could resolve as a label, a pane id) is refused.
pub fn workspace(id: &str) -> Result<Workspace<'_>, String> {
    let refused = || format!("{id} is not a Rex session or tab");
    if !id.starts_with("session:") {
        return Err(refused());
    }
    match id.split_once('/') {
        None => Ok(Workspace {
            session: id,
            window: None,
        }),
        Some((session, window)) if window.starts_with("window:") && !window.contains('/') => {
            Ok(Workspace {
                session,
                window: Some(window),
            })
        }
        Some(_) => Err(refused()),
    }
}

/// The ids of `session`'s windows, `None` when the session is gone.
fn session_windows(program: &Path, session: &str) -> Result<Option<Vec<String>>, String> {
    let output = rex(program, &["window", "ls", "-s", session, "--json"])?;
    if not_found(&output) {
        return Ok(None);
    }
    Ok(Some(
        rex_json(output)?
            .get("windows")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|window| window.get("window_id").and_then(Value::as_str))
            .map(str::to_string)
            .collect(),
    ))
}

/// The blocks of the recorded workspace: the tab's own, or a legacy record's whole session.
/// `None` when the session or the tab is gone.
fn workspace_blocks(program: &Path, at: Workspace) -> Result<Option<Vec<Value>>, String> {
    let Some(window) = at.window else {
        return session_blocks(program, at.session);
    };
    let Some(windows) = session_windows(program, at.session)? else {
        return Ok(None);
    };
    if !windows.iter().any(|id| id == window) {
        return Ok(None);
    }
    Ok(session_blocks(program, at.session)?.map(|blocks| {
        blocks
            .into_iter()
            .filter(|block| block.get("window_id").and_then(Value::as_str) == Some(window))
            .collect()
    }))
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

/// The dispatched agent's pane in the recorded tab (or legacy session) `workspace_id`. With `agent` (`t<n>-<assignee>`), the block
/// labelled exactly that, else one labelled for the same task under an earlier assignee
/// (`t<n>-…`, a reassigned task); without it, any `t<n>-<profile>` block. Another task's
/// block is never this task's agent.
///
/// The agent is gone, [`RootPaneError::WorkspaceGone`], when the session or tab is gone or
/// never opened, or when it is open with no such block and nothing in it could be the agent:
/// every block holds a bare shell or an exited program, or there is no block at all. A block running
/// anything else may be the agent under another label: [`RootPaneError::Failed`], naming it, so
/// nothing launches beside an agent that may still run there.
pub fn agent_block(
    program: &Path,
    workspace_id: &str,
    agent: Option<&str>,
) -> Result<String, RootPaneError> {
    if let Some(pane) = find_agent_block(program, workspace_id, agent)? {
        return Ok(pane);
    }
    match without_agent(program, workspace(workspace_id)?)? {
        Leftover::Empty | Leftover::Idle { .. } => Err(RootPaneError::WorkspaceGone(format!(
            "rex {} {workspace_id} has no {} block and nothing else runs there",
            noun(workspace_id),
            agent.unwrap_or("agent")
        ))),
        Leftover::Busy { block, process } => Err(busy_block(workspace_id, agent, &block, &process)),
    }
}

/// What a recorded workspace id names: `tab`, or `session` for a record from before tabs.
fn noun(workspace_id: &str) -> &'static str {
    if workspace_id.contains('/') {
        "tab"
    } else {
        "session"
    }
}

/// What an open tab (or legacy session) holds once its agent block is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Leftover {
    /// No block at all.
    Empty,
    /// Every block holds a bare shell or an exited program; `anchor` is the first one's pane.
    Idle { anchor: String },
    /// `block` (its label, else its id) runs `process`, which may be the agent renamed.
    Busy { block: String, process: String },
}

/// Ask every block of the tab (or legacy session) `at` what it runs, for one with no agent
/// block. A block that closes meanwhile is skipped; a tab that closes meanwhile is gone. A
/// block Rex cannot answer for refuses: it may be the agent.
fn without_agent(program: &Path, at: Workspace) -> Result<Leftover, RootPaneError> {
    let session = at.session;
    let blocks = workspace_blocks(program, at)
        .map_err(RootPaneError::Failed)?
        .ok_or_else(|| RootPaneError::WorkspaceGone(format!("rex {} not found", described(at))))?;
    let mut anchor = None;
    for entry in &blocks {
        let Some(block) = entry.get("block_id").and_then(Value::as_str) else {
            continue;
        };
        let pane = pane_id(session, block);
        match pane_process(program, &pane).map(|process| leftover_program(&process)) {
            Ok(BlockProgram::Shell { .. } | BlockProgram::Exited) => {
                anchor.get_or_insert(pane);
            }
            Ok(BlockProgram::Busy { process }) => {
                return Ok(Leftover::Busy {
                    block: label_of(entry)
                        .filter(|label| !label.is_empty())
                        .unwrap_or(block)
                        .to_string(),
                    process,
                })
            }
            Err(RootPaneError::WorkspaceGone(_)) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(anchor.map_or(Leftover::Empty, |anchor| Leftover::Idle { anchor }))
}

/// `rex tab <id>`, or `rex session <id>` for a record from before tabs.
fn described(at: Workspace) -> String {
    match at.window {
        Some(window) => format!("tab {}", tab_id(at.session, window)),
        None => format!("session {}", at.session),
    }
}

/// Why nothing launches in `workspace_id`: its agent block is gone, but `block` runs `process`.
fn busy_block(
    workspace_id: &str,
    agent: Option<&str>,
    block: &str,
    process: &str,
) -> RootPaneError {
    let whose = agent.map_or_else(|| "an agent".to_string(), |agent| format!("{agent}'s"));
    RootPaneError::Failed(format!(
        "rex {} {workspace_id} has no block labelled as {whose}, and its block {block} runs \
         {process}, which may be the agent under another label; quit it or close that block to \
         relaunch",
        noun(workspace_id)
    ))
}

/// [`agent_block`], with `None` for an open session that has no such block.
fn find_agent_block(
    program: &Path,
    workspace_id: &str,
    agent: Option<&str>,
) -> Result<Option<String>, RootPaneError> {
    if workspace_id.starts_with(PENDING_PREFIX) {
        return Err(RootPaneError::WorkspaceGone(format!(
            "rex tab for {workspace_id} never opened"
        )));
    }
    // Not a session or tab id: a lookup by that text could match a Rex label.
    let at = workspace(workspace_id)?;
    let session = at.session;
    let blocks = workspace_blocks(program, at)
        .map_err(RootPaneError::Failed)?
        .ok_or_else(|| RootPaneError::WorkspaceGone(format!("rex {} not found", described(at))))?;
    let labelled = |wanted: &dyn Fn(&str) -> bool| {
        blocks
            .iter()
            .find(|block| label_of(block).is_some_and(wanted))
            .and_then(|block| block.get("block_id").and_then(Value::as_str))
            .map(|block| pane_id(session, block))
    };
    let found = match agent {
        Some(agent) => {
            let task = agent.split_once('-').map(|(task, _)| format!("{task}-"));
            labelled(&|label| label == agent).or_else(|| {
                labelled(&|label| {
                    is_agent_label(label)
                        && task.as_deref().is_some_and(|task| label.starts_with(task))
                })
            })
        }
        None => labelled(&is_agent_label),
    };
    Ok(found)
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
    Ok(block_program(&pane_process(program, pane)?))
}

/// The `process` result for `pane`. A missing session or block is
/// [`RootPaneError::WorkspaceGone`].
fn pane_process(program: &Path, pane: &str) -> Result<Value, RootPaneError> {
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
    Ok(rex_json(process)?)
}

/// [`block_program`] for a block that is not labelled as the agent. A shell in the foreground
/// counts as bare only when everything between the block and it is Rex's `login` wrapper or
/// another shell, and the outermost shell is the login shell `login` started (`argv0` with a
/// leading `-`, when Rex reports it). A shell an agent spawned has the agent among its
/// ancestors, and the launcher's own shell, in the moment before it starts the agent, is not a
/// login shell: both are named instead.
pub fn leftover_program(process: &Value) -> BlockProgram {
    let program = block_program(process);
    let Some(foreground) = process.get("foreground").filter(|value| !value.is_null()) else {
        return program;
    };
    if !matches!(program, BlockProgram::Shell { .. }) {
        return program;
    }
    let name = |entry: &Value| {
        entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let pid = |entry: &Value| entry.get("pid").and_then(Value::as_i64);
    // Outermost first: Rex lists the foreground's ancestors inside the block, else its child.
    let mut chain: Vec<&Value> = process
        .get("ancestors")
        .and_then(Value::as_array)
        .map(|ancestors| ancestors.iter().collect())
        .unwrap_or_default();
    if chain.is_empty() {
        if let Some(child) = process
            .get("child")
            .filter(|child| !child.is_null() && pid(child) != pid(foreground))
        {
            chain.push(child);
        }
    }
    if let Some(other) = chain
        .iter()
        .find(|entry| name(entry) != "login" && !is_shell(&name(entry)))
    {
        return BlockProgram::Busy {
            process: name(other),
        };
    }
    let outer = chain
        .iter()
        .copied()
        .chain(std::iter::once(foreground))
        .find(|entry| is_shell(&name(entry)))
        .unwrap_or(foreground);
    let argv0 = outer.get("argv0").and_then(Value::as_str);
    if argv0.is_some_and(|argv0| !argv0.starts_with('-')) {
        return BlockProgram::Busy {
            process: name(outer),
        };
    }
    program
}

/// Where a relaunch runs the agent in the recorded tab (or legacy session) `workspace_id`: its
/// agent block when it holds a bare shell in `worktree` (the agent is gone, as after a Rex
/// server restart), or a new block beside it when its program exited. With the agent block
/// closed, a new block opens in the tab when every other block there holds a bare shell or an
/// exited program; a tab with no block left, or a closed one, is gone. The tab and the user's
/// other blocks stay. Anything still running in the agent
/// block, a shell elsewhere, or any other block running something refuses: nothing launches
/// on a guess.
pub fn reusable_pane(
    program: &Path,
    workspace_id: &str,
    agent: Option<&str>,
    worktree: &Path,
) -> Result<Reuse, RootPaneError> {
    let Some(pane) = find_agent_block(program, workspace_id, agent)? else {
        return match without_agent(program, workspace(workspace_id)?)? {
            Leftover::Empty => Err(RootPaneError::WorkspaceGone(format!(
                "rex {} {workspace_id} has no blocks",
                noun(workspace_id)
            ))),
            Leftover::Idle { anchor } => Ok(Reuse::Add(anchor)),
            Leftover::Busy { block, process } => {
                Err(busy_block(workspace_id, agent, &block, &process))
            }
        };
    };
    match pane_program(program, &pane)? {
        BlockProgram::Shell { cwd } => {
            let here = cwd.as_deref().is_some_and(|cwd| {
                let canonical =
                    |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
                canonical(Path::new(cwd)) == canonical(worktree)
            });
            if here {
                Ok(Reuse::Type(pane))
            } else {
                Err(RootPaneError::Failed(format!(
                    "the shell in rex {} {workspace_id} is not in the worktree {}; close the \
                     {} to relaunch",
                    noun(workspace_id),
                    worktree.display(),
                    noun(workspace_id)
                )))
            }
        }
        BlockProgram::Exited => Ok(Reuse::Replace(pane)),
        // The foreground process is Rex's `login` wrapper while the agent runs: name the agent.
        BlockProgram::Busy { .. } => Err(RootPaneError::Failed(format!(
            "{} still runs in rex {} {workspace_id}",
            agent.unwrap_or("the agent"),
            noun(workspace_id)
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

/// Submit `text` to the agent in `pane` as one message: a paste, then Enter. Only the block
/// still labelled `name` receives it, read in the same check as its status, right before the
/// send; and only an agent whose current report is not `blocked`. One with no report (at a
/// first-run dialog, or a program that never reports) is not typed into, since Enter could
/// answer a prompt nobody saw.
pub fn prompt(program: &Path, pane: &str, name: &str, text: &str) -> Result<(), PromptError> {
    let (found, label) = agent(program, pane).map_err(PromptError::Failed)?;
    if !matches!(found, RexAgent::Gone) && label.as_deref() != Some(name) {
        return Err(PromptError::NotInPane);
    }
    match found {
        RexAgent::Gone => return Err(PromptError::Failed(format!("the agent in {pane} is gone"))),
        RexAgent::Reported { state, .. } if state == "blocked" => {
            return Err(PromptError::AgentBlocked)
        }
        RexAgent::Reported { .. } => {}
        RexAgent::Running { process } => {
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

/// Whether the recorded tab (or legacy session) is still open. One that never opened is not.
pub fn workspace_exists(program: &Path, workspace_id: &str) -> Result<bool, String> {
    if workspace_id.starts_with(PENDING_PREFIX) {
        return Ok(false);
    }
    let at = workspace(workspace_id)?;
    match at.window {
        Some(window) => Ok(session_windows(program, at.session)?
            .is_some_and(|windows| windows.iter().any(|id| id == window))),
        None => Ok(session_blocks(program, at.session)?.is_some()),
    }
}

/// Close the recorded workspace: the agent's tab, never the session around it. Only a record
/// from before tabs, whose whole session tsk opened, closes a session. One already gone is
/// closed.
pub fn close_workspace(program: &Path, workspace_id: &str) -> Result<(), String> {
    if workspace_id.starts_with(PENDING_PREFIX) {
        return Ok(());
    }
    let at = workspace(workspace_id)?;
    match at.window {
        Some(window) => close_window(program, at.session, window),
        None => kill_session(program, at.session),
    }
}

/// Close one tab of `session`. A tab or session already gone is closed.
fn close_window(program: &Path, session: &str, window: &str) -> Result<(), String> {
    let request = json!({"session_id": session, "window_id": window}).to_string();
    let output = rex(program, &["api", "call", "session.close_window", &request])?;
    if output.status.success() || not_found(&output) {
        Ok(())
    } else {
        Err(rex_failure(&output))
    }
}

/// Close a whole session a dispatch from before tabs opened. A session already gone is
/// closed. A tab id is refused: a tab lives in a session that is not tsk's to close.
fn kill_session(program: &Path, session: &str) -> Result<(), String> {
    if !session.starts_with("session:") || session.contains('/') {
        return Err(format!("{session} is not a Rex session tsk opened"));
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
            agent_block(Path::new("rex"), "w9", None),
            Err(RootPaneError::Failed(_))
        ));
        assert!(kill_session(Path::new("rex"), "w9").is_err());
        assert!(close_workspace(Path::new("rex"), "w9").is_err());
    }

    #[test]
    fn a_tab_id_never_reaches_a_session_kill() {
        // The stub would answer any call; a tab id must be refused before Rex runs at all.
        let missing = Path::new("/nonexistent/rex");
        let error = kill_session(missing, "session:01a1/window:01b2").expect_err("refused");
        assert!(error.contains("not a Rex session tsk opened"), "{error}");
        assert_eq!(
            workspace("session:01a1/window:01b2"),
            Ok(Workspace {
                session: "session:01a1",
                window: Some("window:01b2"),
            })
        );
        assert_eq!(
            workspace("session:01a1"),
            Ok(Workspace {
                session: "session:01a1",
                window: None,
            })
        );
        for refused in [
            "session:01a1/block:01b2",
            "session:a/window:b/c",
            "t12-claude",
            "w9",
        ] {
            assert!(workspace(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn a_shell_over_another_program_is_not_a_bare_shell() {
        let login = json!({"pid": 3, "name": "login"});
        let bare = json!({"pid": 9, "name": "zsh", "argv0": "-zsh", "cwd": "/w"});
        // A bare shell as Rex reports it: `login` started a login shell.
        assert!(matches!(
            leftover_program(&json!({"child": login, "ancestors": [login], "foreground": bare})),
            BlockProgram::Shell { .. }
        ));
        // The block's own program is that shell, or Rex says nothing more.
        assert!(matches!(
            leftover_program(&json!({"child": bare, "foreground": bare})),
            BlockProgram::Shell { .. }
        ));
        assert!(matches!(
            leftover_program(&json!({"child": null, "foreground": bare})),
            BlockProgram::Shell { .. }
        ));
        // A user's nested shell is still bare.
        let nested = json!({"pid": 10, "name": "bash", "argv0": "bash"});
        assert!(matches!(
            leftover_program(&json!({
                "child": login, "ancestors": [login, bare], "foreground": nested,
            })),
            BlockProgram::Shell { .. }
        ));
        // A shell an agent spawned: the agent is named.
        let claude = json!({"pid": 4, "name": "claude"});
        let spawned = json!({"pid": 11, "name": "zsh", "argv0": "zsh"});
        assert_eq!(
            leftover_program(&json!({
                "child": login, "ancestors": [login, claude], "foreground": spawned,
            })),
            BlockProgram::Busy {
                process: "claude".into()
            }
        );
        assert_eq!(
            leftover_program(&json!({"child": claude, "foreground": spawned})),
            BlockProgram::Busy {
                process: "claude".into()
            }
        );
        // The launcher's login shell before it starts the agent: not a login-started shell.
        let launcher = json!({"pid": 12, "name": "zsh", "argv0": "/bin/zsh"});
        assert_eq!(
            leftover_program(
                &json!({"child": login, "ancestors": [login], "foreground": launcher})
            ),
            BlockProgram::Busy {
                process: "zsh".into()
            }
        );
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
    fn the_tab_runs_the_launcher_directly_under_the_agent_label_without_taking_focus() {
        let request = window_request(
            "session:here",
            "t12-claude",
            Path::new("/w/t12"),
            &["/bin/sh".into(), "/state/launchers/dispatch-x.sh".into()],
            &["/home/u/.local/bin".into()],
        );
        assert_eq!(request["session_id"], "session:here");
        assert_eq!(request["window_label"], "t12-claude");
        assert_eq!(request["focus"], false);
        let block = &request["layout"]["block"];
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
