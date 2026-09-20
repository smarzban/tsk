//! Dispatch a task to its configured agent profile through Herdr.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

use serde_json::Value;
use uuid::Uuid;

use crate::agents::{AgentProfiles, RenderContext};
use crate::domain::{Dispatch, DomainState, HumanStatus, Task, TaskScope};

pub const NO_ASSIGNEE: &str = "no agent assigned, use !a name";
pub const NOT_IN_HERDR: &str = "dispatch works inside herdr for now";
pub const NEEDS_GIT_PROJECT: &str = "dispatch needs a project in a git repo";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupInspection {
    pub worktree_exists: bool,
    pub dirty: bool,
    pub branch_merged: bool,
    pub workspace_exists: bool,
    /// The recorded path is a non-root worktree registered to the project, and any Herdr
    /// workspace selected by id names that same checkout.
    pub target_matches: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeCleanup {
    Removed,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchCleanup {
    Removed,
    Kept,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupPreview {
    pub number: u64,
    pub title: String,
    pub project: PathBuf,
    pub record: Dispatch,
    pub inspection: CleanupInspection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupResult {
    pub number: u64,
    pub title: String,
    pub worktree_path: String,
    pub branch_name: String,
    pub workspace_id: String,
    pub worktree: WorktreeCleanup,
    pub branch: BranchCleanup,
    pub workspace_removed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupError {
    UnknownTask,
    NotDispatched,
    AlreadyCleaned,
    DirtyWorktree,
    WorktreeMismatch,
    Herdr(String),
    Store(String),
}

impl CleanupError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownTask => "unknown-task",
            Self::NotDispatched => "not-dispatched",
            Self::AlreadyCleaned => "already-cleaned",
            Self::DirtyWorktree => "dirty-worktree",
            Self::WorktreeMismatch => "worktree-mismatch",
            Self::Herdr(_) => "herdr-failed",
            Self::Store(_) => "store-error",
        }
    }
}

impl std::fmt::Display for CleanupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTask => write!(formatter, "task is not on the board"),
            Self::NotDispatched => write!(formatter, "task has no dispatch to clean"),
            Self::AlreadyCleaned => write!(formatter, "dispatch is already cleaned"),
            Self::DirtyWorktree => write!(formatter, "worktree has uncommitted changes"),
            Self::WorktreeMismatch => {
                write!(
                    formatter,
                    "recorded worktree does not match the project or workspace"
                )
            }
            Self::Herdr(reason) | Self::Store(reason) => write!(formatter, "{reason}"),
        }
    }
}

impl std::error::Error for CleanupError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchResult {
    pub number: u64,
    pub title: String,
    pub assignee: String,
    pub record: Dispatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchError {
    UnknownTask,
    NoAssignee,
    NotInHerdr,
    NeedsGitProject,
    DoneTask,
    ArchivedTask,
    SoftDeletedTask,
    AlreadyDispatched(String),
    UnknownAgent(String),
    AgentConfig(String),
    Herdr(String),
    Store(String),
}

impl DispatchError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownTask => "unknown-task",
            Self::NoAssignee => "no-assignee",
            Self::NotInHerdr => "not-in-herdr",
            Self::NeedsGitProject => "needs-git-project",
            Self::DoneTask => "done-task",
            Self::ArchivedTask => "archived-task",
            Self::SoftDeletedTask => "soft-deleted-task",
            Self::AlreadyDispatched(_) => "already-dispatched",
            Self::UnknownAgent(_) => "unknown-agent",
            Self::AgentConfig(_) => "agent-config",
            Self::Herdr(_) => "herdr-failed",
            Self::Store(_) => "store-error",
        }
    }
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTask => write!(formatter, "task is not on the board"),
            Self::NoAssignee => write!(formatter, "{NO_ASSIGNEE}"),
            Self::NotInHerdr => write!(formatter, "{NOT_IN_HERDR}"),
            Self::NeedsGitProject => write!(formatter, "{NEEDS_GIT_PROJECT}"),
            Self::DoneTask => write!(formatter, "completed tasks cannot be dispatched"),
            Self::ArchivedTask => write!(formatter, "archived tasks cannot be dispatched"),
            Self::SoftDeletedTask => write!(formatter, "deleted tasks cannot be dispatched"),
            Self::AlreadyDispatched(path) => write!(
                formatter,
                "already dispatched in {path}, use --again to relaunch"
            ),
            Self::UnknownAgent(name) => write!(formatter, "unknown agent {name}"),
            Self::AgentConfig(reason) | Self::Herdr(reason) | Self::Store(reason) => {
                write!(formatter, "{reason}")
            }
        }
    }
}

impl std::error::Error for DispatchError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedWorktree {
    pub path: PathBuf,
    pub branch: String,
    pub workspace_id: String,
    pub root_pane_id: String,
}

/// Process seam for git and Herdr. Tests implement this without spawning either binary.
pub trait DispatchHost {
    fn is_git_repo(&mut self, project: &Path) -> Result<bool, String>;
    fn resolve_base(&mut self, _project: &Path) -> Result<String, String> {
        Err("dispatch base resolution is not supported".into())
    }
    fn create_worktree(
        &mut self,
        project: &Path,
        branch: &str,
        base: Option<&str>,
        label: &str,
    ) -> Result<CreatedWorktree, String>;
    fn inspect_cleanup(
        &mut self,
        _project: &Path,
        _dispatch: &Dispatch,
        _in_herdr: bool,
    ) -> Result<CleanupInspection, String> {
        Err("cleanup inspection is not supported".into())
    }
    fn remove_herdr_worktree(&mut self, _workspace_id: &str) -> Result<(), String> {
        Err("Herdr worktree removal is not supported".into())
    }
    fn remove_git_worktree(&mut self, _project: &Path, _worktree: &Path) -> Result<(), String> {
        Err("git worktree removal is not supported".into())
    }
    fn delete_branch(&mut self, _project: &Path, _branch: &str) -> Result<(), String> {
        Err("git branch removal is not supported".into())
    }
    fn root_pane(&mut self, workspace_id: &str) -> Result<String, String>;
    fn run_in_pane(&mut self, pane_id: &str, command: &str) -> Result<(), String>;
}

#[derive(Debug, Default)]
pub struct SystemDispatchHost;

impl DispatchHost for SystemDispatchHost {
    fn is_git_repo(&mut self, project: &Path) -> Result<bool, String> {
        match Command::new("git")
            .args(["-C"])
            .arg(project)
            .args(["rev-parse", "--show-toplevel"])
            .output()
        {
            Ok(output) => Ok(output.status.success()),
            Err(error) => Err(format!("could not run git: {error}")),
        }
    }

    fn resolve_base(&mut self, project: &Path) -> Result<String, String> {
        let symbolic = Command::new("git")
            .args(["-C"])
            .arg(project)
            .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
            .output()
            .map_err(|error| format!("could not run git: {error}"))?;
        if symbolic.status.success() {
            return String::from_utf8(symbolic.stdout)
                .map(|base| base.trim().to_string())
                .map_err(|error| format!("git returned an invalid base: {error}"));
        }
        let detached = Command::new("git")
            .args(["-C"])
            .arg(project)
            .args(["rev-parse", "HEAD"])
            .output()
            .map_err(|error| format!("could not run git: {error}"))?;
        if !detached.status.success() {
            return Err(command_failure("git rev-parse HEAD", &detached));
        }
        String::from_utf8(detached.stdout)
            .map(|base| base.trim().to_string())
            .map_err(|error| format!("git returned an invalid base: {error}"))
    }

    fn create_worktree(
        &mut self,
        project: &Path,
        branch: &str,
        base: Option<&str>,
        label: &str,
    ) -> Result<CreatedWorktree, String> {
        let mut command = Command::new("herdr");
        command
            .args(["worktree", "create", "--cwd"])
            .arg(project)
            .args(["--branch", branch]);
        if let Some(base) = base {
            command.args(["--base", base]);
        }
        let output = command
            .args(["--label", label])
            .output()
            .map_err(|error| format!("could not run herdr: {error}"))?;
        created_worktree_from_value(herdr_json(output)?)
    }

    fn inspect_cleanup(
        &mut self,
        project: &Path,
        dispatch: &Dispatch,
        in_herdr: bool,
    ) -> Result<CleanupInspection, String> {
        let worktree = Path::new(&dispatch.worktree);
        let project_path = canonical_cleanup_path(project)?;
        let Ok(worktree_path) = canonical_cleanup_path(worktree) else {
            return Ok(CleanupInspection {
                worktree_exists: worktree.exists(),
                dirty: false,
                branch_merged: false,
                workspace_exists: false,
                target_matches: false,
            });
        };
        // A worktree deleted by hand may already be pruned from git's list, so a missing
        // directory converges to cleaned before the registration gate can refuse it. The
        // project root itself is never a removal target, present or not.
        if project_path == worktree_path {
            return Ok(CleanupInspection {
                worktree_exists: worktree.exists(),
                dirty: false,
                branch_merged: false,
                workspace_exists: false,
                target_matches: false,
            });
        }
        if !worktree.exists() {
            return Ok(CleanupInspection {
                worktree_exists: false,
                dirty: false,
                branch_merged: false,
                workspace_exists: false,
                target_matches: true,
            });
        }
        let registered = git_worktree_paths(project)?
            .iter()
            .any(|listed| listed == &worktree_path);
        if !registered {
            return Ok(CleanupInspection {
                worktree_exists: true,
                dirty: false,
                branch_merged: false,
                workspace_exists: false,
                target_matches: false,
            });
        }
        let status = Command::new("git")
            .args(["-C"])
            .arg(worktree)
            .args(["status", "--porcelain", "--untracked-files=all"])
            .output()
            .map_err(|error| format!("could not run git: {error}"))?;
        if !status.status.success() {
            return Err(command_failure("git status", &status));
        }
        let branch_merged = if let Some(base) = dispatch.base.as_deref() {
            let merged = Command::new("git")
                .args(["-C"])
                .arg(project)
                .args(["merge-base", "--is-ancestor", &dispatch.branch, base])
                .output()
                .map_err(|error| format!("could not run git: {error}"))?;
            match merged.status.code() {
                Some(0) => true,
                Some(1) => false,
                _ => return Err(command_failure("git merge-base", &merged)),
            }
        } else {
            false
        };
        let (workspace_exists, workspace_matches) = if in_herdr {
            let listed = Command::new("herdr")
                .args(["worktree", "list", "--cwd"])
                .arg(project)
                .output()
                .map_err(|error| format!("could not run herdr: {error}"))?;
            let value = herdr_json(listed)?;
            let workspace = value
                .pointer("/result/worktrees")
                .and_then(Value::as_array)
                .and_then(|worktrees| {
                    worktrees.iter().find(|entry| {
                        entry.get("open_workspace_id").and_then(Value::as_str)
                            == Some(dispatch.herdr_workspace_id.as_str())
                    })
                });
            match workspace {
                Some(entry) => {
                    let matches = herdr_checkout_path(entry)
                        .and_then(|path| canonical_cleanup_path(Path::new(path)).ok())
                        .is_some_and(|path| path == worktree_path);
                    (true, matches)
                }
                None => (false, true),
            }
        } else {
            (false, true)
        };
        Ok(CleanupInspection {
            worktree_exists: true,
            dirty: !status.stdout.is_empty(),
            branch_merged,
            workspace_exists,
            target_matches: workspace_matches,
        })
    }

    fn remove_herdr_worktree(&mut self, workspace_id: &str) -> Result<(), String> {
        let output = Command::new("herdr")
            .args(["worktree", "remove", "--workspace", workspace_id])
            .output()
            .map_err(|error| format!("could not run herdr: {error}"))?;
        herdr_json(output).map(|_| ())
    }

    fn remove_git_worktree(&mut self, project: &Path, worktree: &Path) -> Result<(), String> {
        let output = Command::new("git")
            .args(["-C"])
            .arg(project)
            .args(["worktree", "remove"])
            .arg(worktree)
            .output()
            .map_err(|error| format!("could not run git: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(command_failure("git worktree remove", &output))
        }
    }

    fn delete_branch(&mut self, project: &Path, branch: &str) -> Result<(), String> {
        let output = Command::new("git")
            .args(["-C"])
            .arg(project)
            .args(["branch", "-d", branch])
            .output()
            .map_err(|error| format!("could not run git: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(command_failure("git branch -d", &output))
        }
    }

    fn root_pane(&mut self, workspace_id: &str) -> Result<String, String> {
        let output = Command::new("herdr")
            .args(["pane", "list", "--workspace", workspace_id])
            .output()
            .map_err(|error| format!("could not run herdr: {error}"))?;
        let value = herdr_json(output)?;
        value
            .pointer("/result/panes/0/pane_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("herdr workspace {workspace_id} has no root pane"))
    }

    fn run_in_pane(&mut self, pane_id: &str, command: &str) -> Result<(), String> {
        let output = Command::new("herdr")
            .args(["pane", "run", pane_id])
            .arg(command)
            .output()
            .map_err(|error| format!("could not run herdr: {error}"))?;
        herdr_json(output).map(|_| ())
    }
}

fn canonical_cleanup_path(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("recorded worktree path is not absolute".into());
    }
    if path.exists() {
        return path
            .canonicalize()
            .map_err(|error| format!("could not resolve {}: {error}", path.display()));
    }
    let parent = path
        .parent()
        .ok_or_else(|| "recorded worktree has no parent".to_string())?
        .canonicalize()
        .map_err(|error| format!("could not resolve {}: {error}", path.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| "recorded worktree has no name".to_string())?;
    Ok(parent.join(name))
}

fn git_worktree_paths(project: &Path) -> Result<Vec<PathBuf>, String> {
    let listed = Command::new("git")
        .args(["-C"])
        .arg(project)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    if !listed.status.success() {
        return Err(command_failure("git worktree list", &listed));
    }
    String::from_utf8_lossy(&listed.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(|path| canonical_cleanup_path(Path::new(path)))
        .collect()
}

fn herdr_checkout_path(entry: &Value) -> Option<&str> {
    ["path", "checkout_path", "source_checkout_path"]
        .into_iter()
        .find_map(|field| entry.get(field).and_then(Value::as_str))
}

fn created_worktree_from_value(value: Value) -> Result<CreatedWorktree, String> {
    let result = value
        .get("result")
        .ok_or_else(|| "herdr returned no result".to_string())?;
    let string = |pointer: &str| {
        result
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("herdr response is missing {pointer}"))
    };
    Ok(CreatedWorktree {
        path: PathBuf::from(string("/worktree/path")?),
        branch: string("/worktree/branch")?,
        workspace_id: string("/workspace/workspace_id")?,
        root_pane_id: string("/root_pane/pane_id")?,
    })
}

fn command_failure(name: &str, output: &Output) -> String {
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if detail.is_empty() {
        format!("{name} exited with {}", output.status)
    } else {
        detail
    }
}

fn herdr_json(output: Output) -> Result<Value, String> {
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if detail.is_empty() {
            format!("herdr exited with {}", output.status)
        } else {
            detail
        });
    }
    if output.stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("herdr returned invalid JSON: {error}"))
}

/// Inspect a retained dispatch without mutating host or domain state.
pub fn inspect_cleanup_with_host(
    state: &DomainState,
    id: Uuid,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Result<CleanupPreview, CleanupError> {
    let task = state.get(id).cloned().ok_or(CleanupError::UnknownTask)?;
    let number = task.number.ok_or(CleanupError::UnknownTask)?;
    let record = task.dispatch.clone().ok_or(CleanupError::NotDispatched)?;
    if record.cleaned {
        return Err(CleanupError::AlreadyCleaned);
    }
    let project = match task.scope {
        TaskScope::Project { path } => PathBuf::from(path),
        TaskScope::Global => return Err(CleanupError::NotDispatched),
    };
    let mut inspection = host
        .inspect_cleanup(&project, &record, in_herdr)
        .map_err(CleanupError::Herdr)?;
    if !inspection.target_matches {
        return Err(CleanupError::WorktreeMismatch);
    }
    // Legacy v6 dispatch records have no creation base. They may still be cleaned, but the
    // branch is always retained because no safe ancestry target is known.
    if record.base.is_none() {
        inspection.branch_merged = false;
    }
    Ok(CleanupPreview {
        number,
        title: task.title,
        project,
        record,
        inspection,
    })
}

/// Remove a dispatched worktree under the shared cleanup guardrails.
///
/// Domain state changes only after all requested host work succeeds. A missing worktree is a
/// successful convergence case and retains its branch.
pub fn clean_with_host(
    state: &mut DomainState,
    id: Uuid,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Result<CleanupResult, CleanupError> {
    let preview = inspect_cleanup_with_host(state, id, in_herdr, host)?;
    if preview.inspection.dirty {
        return Err(CleanupError::DirtyWorktree);
    }

    let (worktree, branch, workspace_removed) = if preview.inspection.worktree_exists {
        let workspace_removed = in_herdr && preview.inspection.workspace_exists;
        if workspace_removed {
            host.remove_herdr_worktree(&preview.record.herdr_workspace_id)
                .map_err(CleanupError::Herdr)?;
        } else {
            host.remove_git_worktree(&preview.project, Path::new(&preview.record.worktree))
                .map_err(CleanupError::Herdr)?;
        }
        let branch = if preview.inspection.branch_merged {
            host.delete_branch(&preview.project, &preview.record.branch)
                .map_err(CleanupError::Herdr)?;
            BranchCleanup::Removed
        } else {
            BranchCleanup::Kept
        };
        (WorktreeCleanup::Removed, branch, workspace_removed)
    } else {
        (WorktreeCleanup::Missing, BranchCleanup::Kept, false)
    };

    state
        .record_dispatch_cleaned(id)
        .map_err(|error| CleanupError::Store(error.to_string()))?;
    Ok(CleanupResult {
        number: preview.number,
        title: preview.title,
        worktree_path: preview.record.worktree,
        branch_name: preview.record.branch,
        workspace_id: preview.record.herdr_workspace_id,
        worktree,
        branch,
        workspace_removed,
    })
}

/// Launch the cursor task. Domain state changes only after every host command succeeds.
pub fn run_with_host(
    state: &mut DomainState,
    id: Uuid,
    profiles: &AgentProfiles,
    again: bool,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Result<DispatchResult, DispatchError> {
    let task = state.get(id).cloned().ok_or(DispatchError::UnknownTask)?;
    let number = task.number.ok_or(DispatchError::UnknownTask)?;
    let assignee = task.assignee.clone().ok_or(DispatchError::NoAssignee)?;
    if !in_herdr {
        return Err(DispatchError::NotInHerdr);
    }
    if task.soft_deleted {
        return Err(DispatchError::SoftDeletedTask);
    }
    if task.archived
        || matches!(
            &task.scope,
            TaskScope::Project { path } if state.is_project_archived(path)
        )
    {
        return Err(DispatchError::ArchivedTask);
    }
    if task.status == HumanStatus::Done {
        return Err(DispatchError::DoneTask);
    }
    let project = match &task.scope {
        TaskScope::Project { path } => Path::new(path),
        TaskScope::Global => return Err(DispatchError::NeedsGitProject),
    };
    match host.is_git_repo(project) {
        Ok(true) => {}
        Ok(false) | Err(_) => return Err(DispatchError::NeedsGitProject),
    }
    let profile = profiles
        .get(&assignee)
        .ok_or_else(|| DispatchError::UnknownAgent(assignee.clone()))?;

    let (worktree, branch, base, workspace_id, pane_id) = if let Some(existing) = &task.dispatch {
        if !again {
            return Err(DispatchError::AlreadyDispatched(existing.worktree.clone()));
        }
        if existing.cleaned {
            let label = format!("T{number} {}", task.title);
            // Herdr's create command deliberately handles both cases: it creates a missing
            // branch from the recorded base, or checks out an existing retained branch.
            let recreated = host
                .create_worktree(project, &existing.branch, existing.base.as_deref(), &label)
                .map_err(DispatchError::Herdr)?;
            (
                recreated.path.to_string_lossy().into_owned(),
                recreated.branch,
                existing.base.clone(),
                recreated.workspace_id,
                recreated.root_pane_id,
            )
        } else {
            let pane = host
                .root_pane(&existing.herdr_workspace_id)
                .map_err(DispatchError::Herdr)?;
            (
                existing.worktree.clone(),
                existing.branch.clone(),
                existing.base.clone(),
                existing.herdr_workspace_id.clone(),
                pane,
            )
        }
    } else {
        let requested_branch = format!("tsk/t{number}-{}", slug(&task.title));
        let label = format!("T{number} {}", task.title);
        let base = host.resolve_base(project).map_err(DispatchError::Herdr)?;
        let created = host
            .create_worktree(project, &requested_branch, Some(&base), &label)
            .map_err(DispatchError::Herdr)?;
        (
            created.path.to_string_lossy().into_owned(),
            created.branch,
            Some(base),
            created.workspace_id,
            created.root_pane_id,
        )
    };

    let steps = rendered_steps(&task);
    let rendered = profile.render(&RenderContext {
        number,
        title: &task.title,
        notes: task.notes.as_deref().unwrap_or_default(),
        steps: &steps,
        worktree: &worktree,
        branch: &branch,
    });
    host.run_in_pane(&pane_id, &rendered.command)
        .map_err(DispatchError::Herdr)?;

    let record = Dispatch {
        argv: rendered.argv,
        worktree,
        branch,
        base,
        herdr_workspace_id: workspace_id,
        at: SystemTime::now(),
        cleaned: false,
    };
    state
        .record_dispatch(id, record.clone())
        .map_err(|error| DispatchError::Store(error.to_string()))?;
    Ok(DispatchResult {
        number,
        title: task.title,
        assignee,
        record,
    })
}

pub fn running_inside_herdr() -> bool {
    std::env::var("HERDR_ENV").as_deref() == Ok("1")
}

fn rendered_steps(task: &Task) -> String {
    task.steps
        .iter()
        .map(|step| format!("[{}] {}", if step.done { 'x' } else { ' ' }, step.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Git-safe, bounded title component. An all-punctuation title falls back to `task`.
pub fn slug(title: &str) -> String {
    const MAX: usize = 40;
    let mut result = String::new();
    let mut separator = false;
    for character in title.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            if separator && !result.is_empty() && result.len() < MAX {
                result.push('-');
            }
            separator = false;
            if result.len() < MAX {
                result.push(character);
            }
        } else {
            separator = true;
        }
        if result.len() >= MAX {
            break;
        }
    }
    while result.ends_with('-') {
        result.pop();
    }
    if result.is_empty() {
        "task".into()
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::process::ExitStatusExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::domain::{ProvenanceOrigin, TaskEventKind, UndoEntry};
    use crate::ui::board::{apply_intent, BoardModel};
    use crate::ui::input::BoardIntent;

    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    #[derive(Default)]
    struct FakeHost {
        git: bool,
        fail_create: Option<String>,
        fail_run: Option<String>,
        creates: usize,
        roots: usize,
        runs: Vec<(String, String)>,
        created_bases: Vec<Option<String>>,
        inspected_bases: Vec<Option<String>>,
        cleanup: Option<CleanupInspection>,
        removed_herdr: usize,
        removed_git: usize,
        deleted_branches: usize,
    }

    impl DispatchHost for FakeHost {
        fn is_git_repo(&mut self, _: &Path) -> Result<bool, String> {
            Ok(self.git)
        }

        fn resolve_base(&mut self, _: &Path) -> Result<String, String> {
            Ok("main".into())
        }

        fn create_worktree(
            &mut self,
            _: &Path,
            branch: &str,
            base: Option<&str>,
            _: &str,
        ) -> Result<CreatedWorktree, String> {
            self.creates += 1;
            self.created_bases.push(base.map(str::to_string));
            if let Some(error) = self.fail_create.clone() {
                return Err(error);
            }
            Ok(CreatedWorktree {
                path: "/tmp/worktree".into(),
                branch: branch.into(),
                workspace_id: "w9".into(),
                root_pane_id: "w9:p1".into(),
            })
        }

        fn inspect_cleanup(
            &mut self,
            _: &Path,
            dispatch: &Dispatch,
            _: bool,
        ) -> Result<CleanupInspection, String> {
            self.inspected_bases.push(dispatch.base.clone());
            self.cleanup
                .clone()
                .ok_or_else(|| "cleanup inspection not configured".into())
        }

        fn remove_herdr_worktree(&mut self, _: &str) -> Result<(), String> {
            self.removed_herdr += 1;
            Ok(())
        }

        fn remove_git_worktree(&mut self, _: &Path, _: &Path) -> Result<(), String> {
            self.removed_git += 1;
            Ok(())
        }

        fn delete_branch(&mut self, _: &Path, _: &str) -> Result<(), String> {
            self.deleted_branches += 1;
            Ok(())
        }

        fn root_pane(&mut self, _: &str) -> Result<String, String> {
            self.roots += 1;
            Ok("w9:p1".into())
        }

        fn run_in_pane(&mut self, pane: &str, command: &str) -> Result<(), String> {
            self.runs.push((pane.into(), command.into()));
            if let Some(error) = self.fail_run.clone() {
                Err(error)
            } else {
                Ok(())
            }
        }
    }

    fn profiles() -> (PathBuf, AgentProfiles) {
        let path = std::env::temp_dir().join(format!(
            "tsk-dispatch-profiles-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("mkdir");
        fs::write(
            path.join("agents.toml"),
            "[agent.implementer]\ncommand = [\"runner\", \"{worktree}\", \"{branch}\", \"{prompt}\"]\n",
        )
        .expect("agents");
        let loaded = AgentProfiles::load(&path).expect("profiles");
        (path, loaded)
    }

    fn task() -> (DomainState, Uuid) {
        let mut state = DomainState::new();
        let id = state
            .create_assigned(
                "Ship Dispatch!!!",
                Some("notes".into()),
                TaskScope::Project {
                    path: "/repos/app".into(),
                },
                ProvenanceOrigin::Manual,
                None,
                Some("implementer".into()),
            )
            .expect("task");
        state.assign_numbers_for_persistence();
        (state, id)
    }

    #[test]
    fn every_precondition_refuses_before_host_launch_and_without_mutation() {
        let (path, profiles) = profiles();
        let (base, id) = task();
        let cases = [
            ("no-assignee", DispatchError::NoAssignee),
            ("not-herdr", DispatchError::NotInHerdr),
            ("desk", DispatchError::NeedsGitProject),
            ("not-git", DispatchError::NeedsGitProject),
            ("done", DispatchError::DoneTask),
            ("archived", DispatchError::ArchivedTask),
            ("archived-project", DispatchError::ArchivedTask),
            ("soft-deleted", DispatchError::SoftDeletedTask),
            (
                "unknown-profile",
                DispatchError::UnknownAgent("missing".into()),
            ),
        ];
        for (name, expected) in cases {
            let mut state = base.clone();
            let mut host = FakeHost {
                git: name != "not-git",
                ..FakeHost::default()
            };
            let in_herdr = name != "not-herdr";
            match name {
                "no-assignee" => {
                    state.assign(id, None).expect("unassign");
                }
                "desk" => {
                    state
                        .edit_with_assignee(
                            id,
                            "Ship Dispatch!!!",
                            Some("notes".into()),
                            TaskScope::Global,
                            None,
                            Some("implementer".into()),
                        )
                        .expect("move to desk");
                }
                "done" => state.set_status(id, HumanStatus::Done).expect("done"),
                "archived" => {
                    state.archive_task(id).expect("archive");
                }
                "archived-project" => {
                    state
                        .archive_project("/repos/app")
                        .expect("archive project");
                }
                "soft-deleted" => {
                    state.soft_delete(id).expect("delete");
                }
                "unknown-profile" => {
                    state
                        .assign(id, Some("missing".into()))
                        .expect("assign missing");
                }
                _ => {}
            }
            let before = state.clone();
            assert_eq!(
                run_with_host(&mut state, id, &profiles, false, in_herdr, &mut host),
                Err(expected),
                "{name}"
            );
            assert_eq!(
                serde_json::to_value(&state).expect("state json"),
                serde_json::to_value(&before).expect("before json"),
                "{name} must not mutate"
            );
            assert_eq!(host.creates, 0, "{name} must not create");
            assert!(host.runs.is_empty(), "{name} must not launch");
        }
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn successful_herdr_command_may_have_empty_stdout() {
        let output = Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        assert_eq!(herdr_json(output).expect("empty success"), Value::Null);
    }

    #[test]
    fn refusal_codes_are_stable() {
        let cases = [
            (DispatchError::UnknownTask, "unknown-task"),
            (DispatchError::SoftDeletedTask, "soft-deleted-task"),
            (DispatchError::NoAssignee, "no-assignee"),
            (DispatchError::NotInHerdr, "not-in-herdr"),
            (DispatchError::NeedsGitProject, "needs-git-project"),
            (DispatchError::DoneTask, "done-task"),
            (DispatchError::ArchivedTask, "archived-task"),
            (
                DispatchError::AlreadyDispatched("/tmp/worktree".into()),
                "already-dispatched",
            ),
            (
                DispatchError::UnknownAgent("missing".into()),
                "unknown-agent",
            ),
            (
                DispatchError::AgentConfig("bad config".into()),
                "agent-config",
            ),
            (DispatchError::Herdr("failed".into()), "herdr-failed"),
            (DispatchError::Store("failed".into()), "store-error"),
        ];
        for (error, code) in cases {
            assert_eq!(error.code(), code);
        }
    }

    #[test]
    fn host_failures_persist_nothing() {
        let (path, profiles) = profiles();
        for (label, create, run) in [
            ("create", Some("create failed".into()), None),
            ("run", None, Some("pane failed".into())),
        ] {
            let (mut state, id) = task();
            let before = state.clone();
            let mut host = FakeHost {
                git: true,
                fail_create: create,
                fail_run: run,
                ..FakeHost::default()
            };
            let error = run_with_host(&mut state, id, &profiles, false, true, &mut host)
                .expect_err("host failure");
            assert!(matches!(error, DispatchError::Herdr(_)), "{label}: {error}");
            assert_eq!(
                serde_json::to_value(&state).expect("state json"),
                serde_json::to_value(&before).expect("before json"),
                "{label}"
            );
        }
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn launch_records_dispatch_and_started_as_one_non_undoable_mutation() {
        let (path, profiles) = profiles();
        let (mut state, id) = task();
        let undo_before: Option<UndoEntry> = state.last_undo().cloned();
        let revision = state.get(id).expect("task").revision;
        let mut host = FakeHost {
            git: true,
            ..FakeHost::default()
        };

        let result =
            run_with_host(&mut state, id, &profiles, false, true, &mut host).expect("dispatch");
        let task = state.get(id).expect("task");
        assert_eq!(task.status, HumanStatus::Started);
        assert_eq!(task.dispatch.as_ref(), Some(&result.record));
        assert_ne!(task.revision, revision);
        assert_eq!(
            task.history.last().map(|event| event.kind),
            Some(TaskEventKind::Dispatched)
        );
        assert_eq!(state.last_undo(), undo_before.as_ref());
        let retained = task.dispatch.clone();
        state
            .set_status(id, HumanStatus::Blocked)
            .expect("ordinary status change");
        assert_eq!(state.get(id).expect("task").dispatch, retained);
        assert_eq!(host.creates, 1);
        assert_eq!(host.runs.len(), 1);
        assert!(host.runs[0].1.contains("/tmp/worktree"));
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn already_dispatched_refuses_and_again_reuses_workspace_without_creating() {
        let (path, profiles) = profiles();
        let (mut state, id) = task();
        let mut first_host = FakeHost {
            git: true,
            ..FakeHost::default()
        };
        run_with_host(&mut state, id, &profiles, false, true, &mut first_host).expect("first");
        let before = state.clone();
        let mut refused_host = FakeHost {
            git: true,
            ..FakeHost::default()
        };
        assert_eq!(
            run_with_host(&mut state, id, &profiles, false, true, &mut refused_host),
            Err(DispatchError::AlreadyDispatched("/tmp/worktree".into()))
        );
        assert_eq!(
            serde_json::to_value(&state).expect("state json"),
            serde_json::to_value(&before).expect("before json")
        );

        let mut again_host = FakeHost {
            git: true,
            ..FakeHost::default()
        };
        run_with_host(&mut state, id, &profiles, true, true, &mut again_host).expect("again");
        assert_eq!(again_host.creates, 0);
        assert_eq!(again_host.roots, 1);
        assert_eq!(again_host.runs.len(), 1);
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn board_dispatch_uses_only_the_cursor_and_clears_a_marked_set() {
        let (path, profiles) = profiles();
        let (mut state, first) = task();
        let second = state
            .create_assigned(
                "second",
                None,
                TaskScope::Project {
                    path: "/repos/app".into(),
                },
                ProvenanceOrigin::Manual,
                None,
                Some("implementer".into()),
            )
            .expect("second");
        state.assign_numbers_for_persistence();
        let mut model = BoardModel::from_domain(&state, Some(PathBuf::from("/repos/app")));
        let second_index = model
            .visible_ids()
            .iter()
            .position(|id| *id == second)
            .expect("second visible");
        apply_intent(&mut state, &mut model, BoardIntent::ToggleMarkMode, None).expect("mark mode");
        apply_intent(
            &mut state,
            &mut model,
            BoardIntent::SelectIndex(second_index),
            None,
        )
        .expect("select second");
        apply_intent(&mut state, &mut model, BoardIntent::MarkToggle, None).expect("mark second");
        let first_index = model
            .visible_ids()
            .iter()
            .position(|id| *id == first)
            .expect("first visible");
        apply_intent(
            &mut state,
            &mut model,
            BoardIntent::SelectIndex(first_index),
            None,
        )
        .expect("select first");

        let mut host = FakeHost {
            git: true,
            ..FakeHost::default()
        };
        crate::app::dispatch_task_with_host(
            &mut state, &mut model, first, &profiles, false, true, &mut host,
        )
        .expect("dispatch cursor");

        assert!(state.get(first).expect("first").dispatch.is_some());
        assert!(state.get(second).expect("second").dispatch.is_none());
        assert!(model.marked_ids().is_empty());
        assert!(!model.mark_mode_active());
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn board_dispatch_never_retargets_after_a_refresh_moves_the_cursor() {
        let (path, profiles) = profiles();
        let (mut state, first) = task();
        let second = state
            .create_assigned(
                "second",
                None,
                TaskScope::Project {
                    path: "/repos/app".into(),
                },
                ProvenanceOrigin::Manual,
                None,
                Some("implementer".into()),
            )
            .expect("second");
        state.assign_numbers_for_persistence();
        let mut model = BoardModel::from_domain(&state, Some(PathBuf::from("/repos/app")));
        assert_eq!(model.selected_id(), Some(first));
        state.soft_delete(first).expect("concurrent deletion");
        model.sync_from_domain(&state);
        assert_eq!(
            model.selected_id(),
            Some(second),
            "refresh moved the cursor"
        );
        let mut host = FakeHost {
            git: true,
            ..FakeHost::default()
        };

        assert_eq!(
            crate::app::dispatch_task_with_host(
                &mut state, &mut model, first, &profiles, false, true, &mut host,
            ),
            Err(DispatchError::SoftDeletedTask)
        );
        assert!(state.get(second).expect("second").dispatch.is_none());
        assert_eq!(host.creates, 0);
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn cleanup_guardrails_cover_dirty_unmerged_merged_and_missing_worktrees() {
        let (path, profiles) = profiles();
        for (label, inspection, expected, herdr_removes, branch_deletes) in [
            (
                "dirty",
                CleanupInspection {
                    worktree_exists: true,
                    dirty: true,
                    branch_merged: false,
                    workspace_exists: true,
                    target_matches: true,
                },
                Err(CleanupError::DirtyWorktree),
                0,
                0,
            ),
            (
                "unmerged",
                CleanupInspection {
                    worktree_exists: true,
                    dirty: false,
                    branch_merged: false,
                    workspace_exists: true,
                    target_matches: true,
                },
                Ok((WorktreeCleanup::Removed, BranchCleanup::Kept)),
                1,
                0,
            ),
            (
                "merged",
                CleanupInspection {
                    worktree_exists: true,
                    dirty: false,
                    branch_merged: true,
                    workspace_exists: true,
                    target_matches: true,
                },
                Ok((WorktreeCleanup::Removed, BranchCleanup::Removed)),
                1,
                1,
            ),
            (
                "missing",
                CleanupInspection {
                    worktree_exists: false,
                    dirty: false,
                    branch_merged: false,
                    workspace_exists: false,
                    target_matches: true,
                },
                Ok((WorktreeCleanup::Missing, BranchCleanup::Kept)),
                0,
                0,
            ),
        ] {
            let (mut state, id) = task();
            let mut launch = FakeHost {
                git: true,
                ..FakeHost::default()
            };
            run_with_host(&mut state, id, &profiles, false, true, &mut launch).expect("dispatch");
            let before = state.clone();
            let mut host = FakeHost {
                cleanup: Some(inspection),
                ..FakeHost::default()
            };
            let actual = clean_with_host(&mut state, id, true, &mut host)
                .map(|result| (result.worktree, result.branch));
            assert_eq!(actual, expected, "{label}");
            assert_eq!(host.removed_herdr, herdr_removes, "{label}");
            assert_eq!(host.deleted_branches, branch_deletes, "{label}");
            if label == "dirty" {
                assert_eq!(
                    serde_json::to_value(&state).unwrap(),
                    serde_json::to_value(&before).unwrap(),
                    "dirty cleanup must mutate nothing"
                );
            } else {
                let task = state.get(id).expect("task");
                assert!(task.dispatch.as_ref().expect("dispatch").cleaned, "{label}");
                assert_eq!(
                    task.history.last().map(|event| event.kind),
                    Some(TaskEventKind::Cleaned),
                    "{label}"
                );
                assert_eq!(
                    clean_with_host(&mut state, id, true, &mut host),
                    Err(CleanupError::AlreadyCleaned),
                    "{label} repeat"
                );
            }
        }
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn cleanup_refuses_a_mismatched_removal_target_without_host_or_domain_mutation() {
        let (path, profiles) = profiles();
        let (mut state, id) = task();
        let mut launch = FakeHost {
            git: true,
            ..FakeHost::default()
        };
        run_with_host(&mut state, id, &profiles, false, true, &mut launch).expect("dispatch");
        let before = state.clone();
        let mut host = FakeHost {
            cleanup: Some(CleanupInspection {
                worktree_exists: true,
                dirty: false,
                branch_merged: true,
                workspace_exists: true,
                target_matches: false,
            }),
            ..FakeHost::default()
        };

        assert_eq!(
            clean_with_host(&mut state, id, true, &mut host),
            Err(CleanupError::WorktreeMismatch)
        );
        assert_eq!(host.removed_herdr, 0);
        assert_eq!(host.removed_git, 0);
        assert_eq!(host.deleted_branches, 0);
        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn dispatch_records_and_reuses_the_creation_base_while_legacy_cleanup_keeps_the_branch() {
        let (path, profiles) = profiles();
        let (mut state, id) = task();
        let mut launch = FakeHost {
            git: true,
            ..FakeHost::default()
        };
        let dispatched =
            run_with_host(&mut state, id, &profiles, false, true, &mut launch).expect("dispatch");
        assert_eq!(dispatched.record.base.as_deref(), Some("main"));
        assert_eq!(launch.created_bases, vec![Some("main".into())]);

        let (mut legacy, legacy_id) = task();
        legacy
            .record_dispatch(
                legacy_id,
                Dispatch {
                    argv: vec!["agent".into()],
                    worktree: "/tmp/worktree".into(),
                    branch: "tsk/t1-legacy".into(),
                    base: None,
                    herdr_workspace_id: "w9".into(),
                    at: SystemTime::now(),
                    cleaned: false,
                },
            )
            .expect("legacy dispatch");
        let mut cleanup = FakeHost {
            cleanup: Some(CleanupInspection {
                worktree_exists: true,
                dirty: false,
                branch_merged: true,
                workspace_exists: true,
                target_matches: true,
            }),
            ..FakeHost::default()
        };
        let result = clean_with_host(&mut legacy, legacy_id, true, &mut cleanup).expect("clean");
        assert_eq!(cleanup.inspected_bases, vec![None]);
        assert_eq!(result.branch, BranchCleanup::Kept);
        assert_eq!(cleanup.deleted_branches, 0);
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn cleaned_dispatch_again_recreates_removed_and_retained_branches() {
        let (path, profiles) = profiles();
        for branch_exists in [false, true] {
            let (mut state, id) = task();
            let mut launch = FakeHost {
                git: true,
                ..FakeHost::default()
            };
            run_with_host(&mut state, id, &profiles, false, true, &mut launch).expect("dispatch");
            let mut clean = FakeHost {
                cleanup: Some(CleanupInspection {
                    worktree_exists: false,
                    dirty: false,
                    branch_merged: false,
                    workspace_exists: false,
                    target_matches: true,
                }),
                ..FakeHost::default()
            };
            clean_with_host(&mut state, id, true, &mut clean).expect("clean");

            let mut again = FakeHost {
                git: true,
                ..FakeHost::default()
            };
            let result = run_with_host(&mut state, id, &profiles, true, true, &mut again)
                .expect("recreate cleaned dispatch");
            assert!(!result.record.cleaned);
            assert_eq!(again.creates, 1, "existing branch: {branch_exists}");
            assert_eq!(again.roots, 0);
        }
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn cleaned_marker_is_optional_and_store_format_stays_v6() {
        assert_eq!(crate::domain::STORE_FORMAT_VERSION, 6);
        let record = Dispatch {
            argv: vec!["agent".into()],
            worktree: "/tmp/worktree".into(),
            branch: "tsk/t1-task".into(),
            base: None,
            herdr_workspace_id: "w1".into(),
            at: SystemTime::now(),
            cleaned: false,
        };
        let value = serde_json::to_value(&record).unwrap();
        assert!(value.get("base").is_none());
        assert!(value.get("cleaned").is_none());
        let mut cleaned = record;
        cleaned.cleaned = true;
        assert_eq!(serde_json::to_value(cleaned).unwrap()["cleaned"], true);
    }

    #[test]
    fn cleanup_refusal_codes_are_stable() {
        assert_eq!(CleanupError::NotDispatched.code(), "not-dispatched");
        assert_eq!(CleanupError::AlreadyCleaned.code(), "already-cleaned");
        assert_eq!(CleanupError::DirtyWorktree.code(), "dirty-worktree");
        assert_eq!(CleanupError::WorktreeMismatch.code(), "worktree-mismatch");
        assert_eq!(CleanupError::Herdr("failed".into()).code(), "herdr-failed");
    }

    #[test]
    fn slug_is_lowercase_bounded_and_trimmed() {
        assert_eq!(slug("  Hello, WORLD!! "), "hello-world");
        assert_eq!(slug("!!!"), "task");
        let value = slug(&"A".repeat(100));
        assert_eq!(value.len(), 40);
        assert!(value.bytes().all(|byte| byte.is_ascii_lowercase()));
    }

    #[test]
    fn system_inspection_converges_a_pruned_missing_worktree_before_the_registration_gate() {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "tsk-dispatch-prune-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let project = root.join("repo");
        std::fs::create_dir_all(&project).expect("mkdir");
        let git = |dir: &Path, args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .expect("run git");
            assert!(output.status.success(), "{args:?}: {output:?}");
        };
        git(&project, &["init", "-q"]);
        git(
            &project,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        let worktree = root.join("repo-t1");
        git(
            &project,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "tsk/t1-x",
                worktree.to_str().unwrap(),
            ],
        );
        // Deleted by hand and pruned: git no longer lists it.
        std::fs::remove_dir_all(&worktree).expect("remove worktree");
        git(&project, &["worktree", "prune"]);

        let record = Dispatch {
            argv: vec!["agent".into()],
            worktree: worktree.to_string_lossy().into_owned(),
            branch: "tsk/t1-x".into(),
            base: None,
            herdr_workspace_id: "w9".into(),
            at: SystemTime::now(),
            cleaned: false,
        };
        let inspection = SystemDispatchHost
            .inspect_cleanup(&project, &record, false)
            .expect("inspect");
        assert!(!inspection.worktree_exists);
        assert!(
            inspection.target_matches,
            "a missing worktree converges, it is not a mismatch"
        );

        // The project root is never a removal target, whatever git lists.
        let root_record = Dispatch {
            worktree: project.to_string_lossy().into_owned(),
            ..record
        };
        let inspection = SystemDispatchHost
            .inspect_cleanup(&project, &root_record, false)
            .expect("inspect root");
        assert!(!inspection.target_matches);

        let _ = std::fs::remove_dir_all(&root);
    }
}
