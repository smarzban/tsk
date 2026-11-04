//! The terminal host tsk dispatches into, and the host-specific calls that are not git's.
//!
//! [`HostKind`] is detected once per invocation and passed by value wherever dispatch,
//! cleanup, or a start needs to know whether a host can answer. Herdr is the only host today;
//! each host-specific call below matches on the kind, so a second host adds an arm here
//! instead of a flag through every caller.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

use super::{
    canonical_cleanup_path, herdr_checkout_path, herdr_error_code, herdr_json, same_path,
    AgentNaming, Rename,
};

/// Which terminal host this invocation runs inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKind {
    /// Inside Herdr (`HERDR_ENV=1`): dispatch can open workspaces and launch agents.
    Herdr,
    /// No host: dispatch refuses, a start is a plain status change, cleanup leaves
    /// workspaces alone.
    Standalone,
}

impl HostKind {
    /// The host of this process, from its environment. The one place tsk reads `HERDR_ENV`.
    pub fn detect() -> Self {
        if std::env::var("HERDR_ENV").as_deref() == Ok("1") {
            Self::Herdr
        } else {
            Self::Standalone
        }
    }

    /// Whether a host can answer: open workspaces, launch agents, report them.
    pub fn is_hosted(self) -> bool {
        self != Self::Standalone
    }

    /// Whether the workspace `workspace_id` is still open on `worktree`, a checkout already
    /// deleted. No host: never open.
    pub(super) fn workspace_open_on(self, workspace_id: &str, worktree: &Path) -> bool {
        match self {
            Self::Herdr => herdr_workspace_open_on(workspace_id, worktree),
            Self::Standalone => false,
        }
    }

    /// Whether a workspace `workspace_id` is open on one of `project`'s worktrees, and if so
    /// whether that checkout is `worktree`: `(exists, matches)`. No host: `(false, true)`.
    pub(super) fn worktree_workspace(
        self,
        project: &Path,
        workspace_id: &str,
        worktree: &Path,
    ) -> Result<(bool, bool), String> {
        match self {
            Self::Herdr => herdr_worktree_workspace(project, workspace_id, worktree),
            Self::Standalone => Ok((false, true)),
        }
    }

    /// One attempt to give the agent in `naming.pane_id` its name, bounded by `left`.
    pub(super) fn rename_agent(self, naming: &AgentNaming, left: Duration) -> Rename {
        match self {
            Self::Herdr => herdr_rename_agent(naming, left),
            Self::Standalone => Rename::Final(Err("no host to name the agent in".into())),
        }
    }
}

/// Whether the recorded workspace is still open on `worktree`, a checkout already deleted.
/// Herdr keeps a workspace's checkout path after the directory goes (and after a `git
/// worktree prune`), so only a workspace that still names this checkout reads as open: an id
/// that now names another checkout, or none, is never closed. A Herdr that cannot answer
/// leaves it open, for the close to fail and say why.
fn herdr_workspace_open_on(workspace_id: &str, worktree: &Path) -> bool {
    let Ok(output) = Command::new("herdr")
        .args(["workspace", "get", workspace_id])
        .output()
    else {
        return true;
    };
    if !output.status.success() {
        return herdr_error_code(&output.stderr).as_deref() != Some("workspace_not_found");
    }
    let value = serde_json::from_slice::<Value>(&output.stdout).unwrap_or(Value::Null);
    value
        .pointer("/result/workspace/worktree")
        .and_then(herdr_checkout_path)
        .and_then(|path| canonical_cleanup_path(Path::new(path)).ok())
        .is_some_and(|path| same_path(&path, worktree))
}

fn herdr_worktree_workspace(
    project: &Path,
    workspace_id: &str,
    worktree: &Path,
) -> Result<(bool, bool), String> {
    // Herdr refuses `worktree list` outside a Git work tree, whatever `--cwd` says.
    let listed = Command::new("herdr")
        .args(["worktree", "list", "--cwd"])
        .arg(project)
        .current_dir(project)
        .output()
        .map_err(|error| format!("could not run herdr: {error}"))?;
    let value = herdr_json(listed)?;
    let workspace = value
        .pointer("/result/worktrees")
        .and_then(Value::as_array)
        .and_then(|worktrees| {
            worktrees.iter().find(|entry| {
                entry.get("open_workspace_id").and_then(Value::as_str) == Some(workspace_id)
            })
        });
    Ok(match workspace {
        Some(entry) => {
            let matches = herdr_checkout_path(entry)
                .and_then(|path| canonical_cleanup_path(Path::new(path)).ok())
                .is_some_and(|path| same_path(&path, worktree));
            (true, matches)
        }
        None => (false, true),
    })
}

fn herdr_rename_agent(naming: &AgentNaming, left: Duration) -> Rename {
    let mut rename = Command::new("herdr");
    rename.args(["agent", "rename", &naming.pane_id, &naming.name]);
    let output = match crate::git_base::bounded_process_output(rename, "herdr", left) {
        Ok(output) => output,
        Err(error) => return Rename::Final(Err(error)),
    };
    let undetected = !output.status.success()
        && herdr_error_code(&output.stderr).as_deref() == Some("agent_not_found");
    let result = herdr_json(output).map(|_| ());
    match result {
        Err(error) if undetected => Rename::Undetected(error),
        result => Rename::Final(result),
    }
}

/// The checkout Herdr gives a new worktree for `branch`:
/// `<home>/.herdr/worktrees/<repo>/<directory>`. `None` when `project` has no name.
pub(super) fn herdr_default_worktree(home: &Path, project: &Path, branch: &str) -> Option<PathBuf> {
    project.file_name().map(|repo| {
        home.join(".herdr/worktrees")
            .join(repo)
            .join(herdr_worktree_directory(branch))
    })
}

/// The user's home, where Herdr keeps worktrees: `%USERPROFILE%` on Windows.
pub(super) fn herdr_home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// Directory Herdr gives a worktree for `branch`: every run of characters other than ASCII
/// letters and digits becomes one `-`, so `tsk/t1-café-東京` checks out in `tsk-t1-caf`.
pub(super) fn herdr_worktree_directory(branch: &str) -> String {
    branch
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}
