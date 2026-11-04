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

/// Where dispatch checks out a new worktree for `branch`, on any host:
/// `<root>/<repo>/<directory>`. `None` when `project` has no name.
pub(super) fn worktree_path(root: &Path, project: &Path, branch: &str) -> Option<PathBuf> {
    project
        .file_name()
        .map(|repo| root.join(repo).join(herdr_worktree_directory(branch)))
}

/// The root new worktrees of `project` go under: `[dispatch] worktrees` in `config.toml`, or
/// `<state_dir>/worktrees`, made absolute. A root inside the project refuses: the checkout
/// would sit in the tree it was cut from.
pub(super) fn worktree_root(state_dir: &Path, project: &Path) -> Result<PathBuf, String> {
    let configured = crate::agents::DispatchSettings::load(state_dir)
        .map_err(|error| error.to_string())?
        .worktrees;
    let root = configured.unwrap_or_else(|| state_dir.join("worktrees"));
    let root = std::path::absolute(&root)
        .map_err(|error| format!("could not resolve {}: {error}", root.display()))?;
    let resolved = resolve_existing_prefix(&root);
    let inside = [project.to_path_buf(), resolve_existing_prefix(project)]
        .iter()
        .any(|project| path_within(&resolved, project) || path_within(&root, project));
    if inside {
        return Err(format!(
            "config.toml: [dispatch] worktrees {} is inside the project {}; pick a folder outside it",
            root.display(),
            project.display()
        ));
    }
    Ok(root)
}

/// `path` with its longest existing ancestor canonicalized, so symlinks and `..` resolve
/// before a containment check; the rest is appended as written.
fn resolve_existing_prefix(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut current = path;
    loop {
        if let Ok(canonical) = current.canonicalize() {
            return rest
                .iter()
                .rev()
                .fold(canonical, |acc, part: &&std::ffi::OsStr| acc.join(part));
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Whether `path` is `ancestor` or below it. Windows compares without case.
fn path_within(path: &Path, ancestor: &Path) -> bool {
    if cfg!(windows) {
        let lower = |path: &Path| PathBuf::from(path.to_string_lossy().to_lowercase());
        lower(path).starts_with(lower(ancestor))
    } else {
        path.starts_with(ancestor)
    }
}

/// Directory a worktree for `branch` checks out in, on any host (Herdr's own rule): every run
/// of characters other than ASCII letters and digits becomes one `-`, so `tsk/t1-café-東京`
/// checks out in `tsk-t1-caf`.
pub(super) fn herdr_worktree_directory(branch: &str) -> String {
    branch
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "tsk-host-{name}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("mkdir");
        root
    }

    #[test]
    fn worktrees_go_under_the_state_dir_unless_config_moves_them() {
        let root = temp_root("worktree-root");
        let (state, project) = (root.join("state"), root.join("app"));
        fs::create_dir_all(&state).expect("state");
        fs::create_dir_all(&project).expect("project");
        let default = worktree_root(&state, &project).expect("default root");
        assert_eq!(default, state.join("worktrees"));
        assert_eq!(
            worktree_path(&default, &project, "tsk/t12-fix-login-timeout"),
            Some(state.join("worktrees/app/tsk-t12-fix-login-timeout"))
        );
        assert!(!default.exists(), "resolving creates nothing");

        let elsewhere = root.join("elsewhere");
        let configure = |worktrees: &Path| {
            fs::write(
                state.join("config.toml"),
                format!("[dispatch]\nworktrees = '{}'\n", worktrees.display()),
            )
            .expect("config");
        };
        configure(&elsewhere);
        assert_eq!(worktree_root(&state, &project), Ok(elsewhere));

        // Inside the project refuses, also when reached through `..` or a symlink.
        for inside in [
            project.join("wt"),
            project.clone(),
            root.join("state/../app/wt"),
        ] {
            configure(&inside);
            let error = worktree_root(&state, &project).unwrap_err();
            assert!(error.contains("is inside the project"), "{error}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&project, root.join("link")).expect("symlink");
            configure(&root.join("link/wt"));
            assert!(worktree_root(&state, &project).is_err());
        }
        // A sibling whose name starts like the project's is outside it.
        configure(&root.join("app-worktrees"));
        assert!(worktree_root(&state, &project).is_ok());
        // A state dir inside the project puts its default root there too: refused.
        let nested = project.join(".tsk");
        fs::create_dir_all(&nested).expect("nested");
        assert!(worktree_root(&nested, &project).is_err());

        fs::write(state.join("config.toml"), "[dispatch]\nworktrees = 'rel'\n").expect("config");
        let error = worktree_root(&state, &project).unwrap_err();
        assert!(error.starts_with("config.toml: [dispatch] "), "{error}");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn a_relative_state_dir_gives_an_absolute_root() {
        let root =
            worktree_root(Path::new("relative-state"), Path::new("/nowhere/app")).expect("root");
        assert!(root.is_absolute(), "{}", root.display());
        assert!(root.ends_with("relative-state/worktrees"));
    }
}
