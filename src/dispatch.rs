//! Dispatch a task to its configured agent profile through Herdr.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;
use uuid::Uuid;

use crate::agents::{AgentProfiles, RenderContext};
use crate::domain::{Dispatch, DomainState, HumanStatus, Task, TaskScope};

/// Cleanup metadata queries (full untracked-file status, merge-base ancestry) read a real
/// checkout rather than a quick plumbing ref, so git_base's 250ms metadata deadline false-times
/// out on an ordinarily slow repo. Bounded finitely so an unusually stuck repo still converges
/// instead of hanging a `tsk clean`.
const CLEANUP_QUERY_TIMEOUT: Duration = Duration::from_secs(10);

/// Dedicated timeout for cleanup's worktree listings, full untracked-file status and
/// ancestry checks, which can exceed the short board metadata deadline.
fn cleanup_query(project: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    crate::git_base::git_process_output_timeout(project, args, CLEANUP_QUERY_TIMEOUT)
}

fn cleanup_inspection_error(operation: &str, error: String) -> String {
    if error == "git timed out" {
        format!(
            "Git {operation} timed out after {}s; cleanup refused before removal",
            CLEANUP_QUERY_TIMEOUT.as_secs()
        )
    } else {
        error
    }
}

pub const NO_ASSIGNEE: &str = "no agent assigned, use tsk edit T<n> --assignee <name>";
/// The board's wording of the same refusal: there `@` assigns, and with no profile it opens nothing.
pub const BOARD_NO_ASSIGNEE: &str = "no agent assigned: press @ or add a profile to config.toml";
pub const NOT_IN_HERDR: &str = "dispatch works inside herdr for now";
pub const NEEDS_GIT_PROJECT: &str = "dispatch needs a project in a git repo";
pub const UNSUPPORTED_PLATFORM: &str = "dispatch on Windows needs Windows PowerShell";
/// Said once per Windows dispatch whose repository leaves Git's long-path support off.
pub const LONG_PATHS_OFF: &str = "core.longpaths is off, so deep paths in the worktree may fail; enable it with git config --global core.longpaths true";

/// Which shell a host launches agents through and whose filesystem rules cleanup meets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPlatform {
    /// One `$SHELL -lc` command line typed into the pane.
    Unix,
    /// A per-dispatch Windows PowerShell launcher script, run with `powershell -File`.
    Windows,
}

impl HostPlatform {
    pub fn native() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanupInspection {
    /// The bounded fetch failed: why, for the caller to show.
    pub warning: Option<String>,
    /// The recorded base's remote could not be fetched (outside the fetch window), so the
    /// refs on disk may predate a force-push or reset: merged status is unconfirmed.
    pub unreachable_remote: Option<String>,
    /// Separate from ancestry: a vanished base must not block worktree removal.
    pub base_available: bool,
    pub worktree_exists: bool,
    pub dirty: bool,
    pub branch_merged: bool,
    pub workspace_exists: bool,
    /// The recorded path is a non-root worktree registered to the project, and any Herdr
    /// workspace selected by id names that same checkout.
    pub target_matches: bool,
}

/// Longest a board `y` waits for its card's background merged check: the bounded fetch
/// plus the bounded ancestry query the check runs after it.
pub const MERGE_CHECK_TIMEOUT: Duration = Duration::from_secs(
    crate::git_base::NETWORK_TIMEOUT.as_secs() + CLEANUP_QUERY_TIMEOUT.as_secs(),
);

/// Which refs a cleanup trusts for the branch's merged status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupRefs {
    /// Fetch the recorded base's remote first (the CLI).
    Fetch,
    /// The refs on disk now. The board's card already refreshed them off the event loop.
    Cached,
    /// The card's refresh never finished: remove a clean worktree, never the branch.
    Unconfirmed,
    /// The card's refresh could not fetch the recorded base: the cached refs may be stale,
    /// so remove a clean worktree, never the branch.
    Offline,
}

/// The base-dependent half of a cleanup inspection, recomputed after a background fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeVerdict {
    pub branch_merged: bool,
    pub base_available: bool,
    pub warning: Option<String>,
    /// False when the ancestry check itself errored or timed out: nothing was confirmed, so a
    /// cleanup must keep the branch even if the refs on disk later read as merged.
    pub confirmed: bool,
    /// The fetch before the ancestry check failed: the named remote's refs on disk may be
    /// stale, so the merge is unconfirmed and cleanup keeps the branch.
    pub unreachable_remote: Option<String>,
}

/// A board cleanup card's merged check running off the event loop. The host completes it
/// once; the board polls it on idle ticks and never waits on it except through `y`.
#[derive(Debug, Clone, Default)]
pub struct MergeCheck(std::sync::Arc<std::sync::Mutex<Option<MergeVerdict>>>);

/// Identity, not content: two handles are equal when they watch the same check.
impl PartialEq for MergeCheck {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for MergeCheck {}

impl MergeCheck {
    pub fn complete(&self, verdict: MergeVerdict) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(verdict);
        }
    }

    pub fn take(&self) -> Option<MergeVerdict> {
        self.0.try_lock().ok().and_then(|mut slot| slot.take())
    }
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

/// Why a branch was retained, independently of the worktree cleanup outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchRetentionReason {
    NotMerged,
    BaseUnavailable,
    CheckedOutElsewhere,
    LatestTipNotMerged,
    Advanced,
    NoRecordedBase,
    MissingWorktree,
    BranchUnavailable,
    /// The worktree is already removed; a slow ancestry check must not block on it, but
    /// deleting the branch without a confirmed merge is never safe either.
    AncestryCheckTimedOut,
    WorktreeListingTimedOut,
    /// A board `y` queued behind the background merged check outlived its bound: without a
    /// completed check the merge is unconfirmed, so the branch stays.
    MergeCheckUnfinished,
    /// The recorded base's remote could not be fetched: cached refs may predate a
    /// force-push or reset that dropped the task's commits, so the merge is unconfirmed.
    RemoteUnreachable,
}

impl BranchRetentionReason {
    /// A few words for the status row; the card and the CLI carry [`Self::message`].
    pub fn short(self) -> &'static str {
        match self {
            Self::NotMerged | Self::LatestTipNotMerged => "not merged",
            Self::BaseUnavailable => "base unavailable",
            Self::CheckedOutElsewhere => "checked out elsewhere",
            Self::Advanced => "branch changed",
            Self::NoRecordedBase => "no recorded base",
            Self::MissingWorktree => "worktree already gone",
            Self::BranchUnavailable => "branch gone",
            Self::AncestryCheckTimedOut | Self::WorktreeListingTimedOut => "check timed out",
            Self::MergeCheckUnfinished => "merge unconfirmed",
            Self::RemoteUnreachable => "offline",
        }
    }

    pub fn message(self, base: Option<&str>, remote: Option<&str>) -> String {
        match self {
            Self::NotMerged => format!("not merged into {}; squash-merged? delete by hand", base.unwrap_or("recorded base")),
            Self::BaseUnavailable => "base no longer available; branch retained".into(),
            Self::CheckedOutElsewhere => "branch checked out in another worktree".into(),
            Self::LatestTipNotMerged => "latest branch tip is no longer merged into the recorded base; branch or base changed since inspection".into(),
            Self::Advanced => "branch changed during cleanup".into(),
            Self::NoRecordedBase => "no recorded base; branch retained".into(),
            Self::MissingWorktree => "worktree already missing; branch retained".into(),
            Self::BranchUnavailable => "branch no longer available".into(),
            Self::AncestryCheckTimedOut => {
                "ancestry check timed out after the worktree was removed; branch retained".into()
            }
            Self::WorktreeListingTimedOut => "worktree listing timed out; branch retained".into(),
            Self::MergeCheckUnfinished => {
                "merged check did not finish; branch retained".into()
            }
            Self::RemoteUnreachable => format!(
                "could not reach {} to confirm the merge",
                remote.unwrap_or("the remote")
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchDeletion {
    Removed,
    Kept(BranchRetentionReason),
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
    pub warning: Option<String>,
    pub branch_reason: Option<BranchRetentionReason>,
    /// The remote a [`BranchRetentionReason::RemoteUnreachable`] names.
    pub remote: Option<String>,
    pub number: u64,
    pub title: String,
    pub worktree_path: String,
    pub branch_name: String,
    pub base: Option<String>,
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
    /// Board only: the task's dispatch is no longer the one its cleanup card inspected
    /// (another board or the CLI cleaned or relaunched it), so nothing is touched.
    DispatchChanged,
    /// Windows refused the removal because a process still holds files in the worktree.
    FilesInUse,
    /// Windows refused the removal because a path in the worktree exceeds MAX_PATH.
    PathTooLong,
    Herdr(String),
    Store(String),
}

impl CleanupError {
    /// A few words for the status row; the card carries the full message.
    pub fn short(&self) -> &'static str {
        match self {
            Self::UnknownTask => "task gone",
            Self::NotDispatched => "no dispatch",
            Self::AlreadyCleaned => "already cleaned",
            Self::DirtyWorktree => "uncommitted changes",
            Self::WorktreeMismatch => "worktree mismatch",
            Self::DispatchChanged => "dispatch changed",
            Self::FilesInUse => "files in use",
            Self::PathTooLong => "path too long",
            Self::Herdr(_) => "removal failed",
            Self::Store(_) => "save failed",
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownTask => "unknown-task",
            Self::NotDispatched => "not-dispatched",
            Self::AlreadyCleaned => "already-cleaned",
            Self::DirtyWorktree => "dirty-worktree",
            Self::WorktreeMismatch => "worktree-mismatch",
            Self::DispatchChanged => "dispatch-changed",
            Self::FilesInUse => "files-in-use",
            Self::PathTooLong => "path-too-long",
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
            Self::DispatchChanged => write!(formatter, "changed since the card opened"),
            Self::FilesInUse => write!(
                formatter,
                "kept: files in use (close what is running in the worktree and retry)"
            ),
            Self::PathTooLong => write!(
                formatter,
                "kept: path too long (run git config --global core.longpaths true and retry)"
            ),
            Self::Herdr(reason) | Self::Store(reason) => write!(formatter, "{reason}"),
        }
    }
}

impl std::error::Error for CleanupError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchResult {
    /// Fetch failures retain cached refs and are visible to the caller.
    pub warning: Option<String>,
    pub number: u64,
    pub title: String,
    pub assignee: String,
    pub record: Dispatch,
    /// The Herdr agent name to apply once the record is saved, or `None` when a relaunch found
    /// an agent already running in the reused pane.
    pub naming: Option<AgentNaming>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentNaming {
    pub pane_id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchError {
    UnknownTask,
    NoAssignee,
    NotInHerdr,
    UnsupportedPlatform,
    NeedsGitProject,
    DoneTask,
    ArchivedTask,
    SoftDeletedTask,
    AlreadyDispatched(String),
    UnknownAgent(String),
    UnknownBase(String),
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
            Self::UnsupportedPlatform => "unsupported-platform",
            Self::NeedsGitProject => "needs-git-project",
            Self::DoneTask => "done-task",
            Self::ArchivedTask => "archived-task",
            Self::SoftDeletedTask => "soft-deleted-task",
            Self::AlreadyDispatched(_) => "already-dispatched",
            Self::UnknownAgent(_) => "unknown-agent",
            Self::UnknownBase(_) => "unknown-base",
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
            Self::UnsupportedPlatform => write!(formatter, "{UNSUPPORTED_PLATFORM}"),
            Self::NeedsGitProject => write!(formatter, "{NEEDS_GIT_PROJECT}"),
            Self::DoneTask => write!(formatter, "completed tasks cannot be dispatched"),
            Self::ArchivedTask => write!(formatter, "archived tasks cannot be dispatched"),
            Self::SoftDeletedTask => write!(formatter, "deleted tasks cannot be dispatched"),
            Self::AlreadyDispatched(path) => write!(
                formatter,
                "already dispatched in {path}, use --again to relaunch"
            ),
            Self::UnknownAgent(name) => write!(formatter, "unknown agent {name}"),
            Self::UnknownBase(reason)
            | Self::AgentConfig(reason)
            | Self::Herdr(reason)
            | Self::Store(reason) => {
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
    fn resolve_base_choice(
        &mut self,
        project: &Path,
        explicit: Option<&str>,
    ) -> Result<crate::git_base::ResolvedBase, String> {
        let reference = match explicit {
            Some(base) => base.to_string(),
            None => self.resolve_base(project)?,
        };
        Ok(crate::git_base::ResolvedBase {
            full_ref: format!("refs/heads/{reference}"),
            reference,
            commit: None,
            remote: None,
            warning: None,
        })
    }
    /// Whether a first dispatch must not take `branch`: a local or remote-tracking branch of that
    /// name, or a registered worktree in the directory Herdr would derive from it, exists.
    fn branch_taken(&mut self, _project: &Path, _branch: &str) -> Result<bool, String> {
        Ok(false)
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
    /// Same inspection from the refs already on disk: the board's cleanup card opens on this
    /// without waiting for the network.
    fn inspect_cleanup_cached(
        &mut self,
        project: &Path,
        dispatch: &Dispatch,
        in_herdr: bool,
    ) -> Result<CleanupInspection, String> {
        self.inspect_cleanup(project, dispatch, in_herdr)
    }
    /// Start the recorded base's fetch and ancestry recheck off the event loop. `None` means
    /// the cached verdict is already current (no remote, or fetched inside the fetch window).
    fn begin_merge_check(&mut self, _project: &Path, _dispatch: &Dispatch) -> Option<MergeCheck> {
        None
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
    /// Recheck the latest tip after worktree removal, then delete only that exact tip.
    fn delete_merged_branch(
        &mut self,
        project: &Path,
        branch: &str,
        _base_ref: &str,
    ) -> Result<BranchDeletion, String> {
        self.delete_branch(project, branch)?;
        Ok(BranchDeletion::Removed)
    }
    /// Run a board cleanup's host work off the event loop, filling `job`: the board polls it
    /// and applies each row's outcome as it lands. This default runs inline.
    fn begin_cleanup(&mut self, job: CleanupJob, plan: Vec<CleanupPlanRow>, in_herdr: bool)
    where
        Self: Sized,
    {
        run_cleanup_job(&job, &plan, in_herdr, self);
    }
    fn root_pane(&mut self, workspace_id: &str) -> Result<String, String>;
    fn run_in_pane(&mut self, pane_id: &str, command: &str) -> Result<(), String>;
    /// The platform whose launch line and cleanup rules apply. Test hosts default to Unix.
    fn platform(&self) -> HostPlatform {
        HostPlatform::Unix
    }
    /// Store a Windows launcher script for the dispatch in `workspace_id` and return its path.
    /// It lives under the state dir, never inside the worktree, so it is never committed.
    fn write_launcher(&mut self, _workspace_id: &str, _script: &str) -> Result<PathBuf, String> {
        Err("PowerShell launchers are not supported".into())
    }
    /// Delete a launcher the agent never ran (a launched one deletes itself). Best effort.
    fn remove_launcher(&mut self, _workspace_id: &str) {}
    /// Whether Git may create paths longer than Windows' MAX_PATH in `project`'s checkouts.
    fn long_paths_enabled(&mut self, _project: &Path) -> Result<bool, String> {
        Ok(true)
    }
    /// The pause before cleanup checks a worktree again that files still in use blocked: a
    /// closed workspace's agent may take a moment to exit.
    fn wait_before_retry(&mut self) {}
    /// Close a Herdr workspace without touching its checkout (Windows cleanup closes before it
    /// checks and removes).
    fn close_herdr_workspace(&mut self, _workspace_id: &str) -> Result<(), String> {
        Err("Herdr workspace close is not supported".into())
    }
    /// Why removing `worktree` now would fail partway on Windows, if it would.
    fn removal_blocker(
        &mut self,
        _project: &Path,
        _worktree: &Path,
    ) -> Result<Option<RemovalBlock>, String> {
        Ok(None)
    }
    /// Whether Herdr currently detects an agent in `pane_id`.
    fn pane_has_agent(&mut self, _pane_id: &str) -> Result<bool, String> {
        Err("agent detection is not supported".into())
    }
    /// Launch each task in order, off the event loop where the host can. Outcomes land on the
    /// returned batch one by one; nothing is recorded on any task here. The default runs every
    /// launch before returning (test hosts); the system host runs them on its own thread.
    fn begin_launches(&mut self, jobs: Vec<EligibleDispatch>) -> LaunchBatch
    where
        Self: Sized,
    {
        let batch = LaunchBatch::new(jobs.len());
        for job in jobs {
            let outcome = launch_bulk_job(&job, self);
            batch.land(job, outcome);
        }
        batch
    }
    /// Check which `projects` are git repositories, off the event loop where the host can. The
    /// default answers before returning (test hosts).
    fn begin_git_checks(&mut self, projects: Vec<PathBuf>) -> GitChecks
    where
        Self: Sized,
    {
        let checks = GitChecks::default();
        checks.finish(check_git_projects(projects, self));
        checks
    }
}

/// The outcomes of a bulk launch, landing in launch order while the launches run.
#[derive(Debug, Clone)]
pub struct LaunchBatch(std::sync::Arc<std::sync::Mutex<LaunchBatchState>>);

type LandedLaunch = (EligibleDispatch, Result<Launched, DispatchError>);

#[derive(Debug, Default)]
struct LaunchBatchState {
    total: usize,
    landed: std::collections::VecDeque<LandedLaunch>,
    taken: usize,
}

impl LaunchBatch {
    pub fn new(total: usize) -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(
            LaunchBatchState {
                total,
                ..LaunchBatchState::default()
            },
        )))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, LaunchBatchState> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn land(&self, job: EligibleDispatch, outcome: Result<Launched, DispatchError>) {
        self.state().landed.push_back((job, outcome));
    }

    /// Every outcome landed since the last take, oldest first.
    pub fn take_landed(&self) -> Vec<LandedLaunch> {
        let mut state = self.state();
        let landed = state.landed.drain(..).collect::<Vec<_>>();
        state.taken += landed.len();
        landed
    }

    /// Outcomes already handed to the board.
    pub fn taken(&self) -> usize {
        self.state().taken
    }

    /// Every launch has landed and been taken.
    pub fn finished(&self) -> bool {
        let state = self.state();
        state.taken >= state.total && state.landed.is_empty()
    }
}

/// How long dispatch waits for Herdr to detect the launched agent before leaving it unnamed.
const AGENT_DETECTION_TIMEOUT: Duration = Duration::from_secs(3);
const AGENT_DETECTION_POLL: Duration = Duration::from_millis(100);
/// How long cleanup waits before its one retry of a removal refused by files in use.
const REMOVAL_RETRY_DELAY: Duration = Duration::from_secs(2);

/// The real host: git and Herdr processes. Windows launchers go under `state_dir`'s
/// `launchers` directory (the default state dir when unset).
#[derive(Debug, Default, Clone)]
pub struct SystemDispatchHost {
    state_dir: Option<PathBuf>,
}

impl SystemDispatchHost {
    pub fn in_state_dir(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            state_dir: Some(state_dir.into()),
        }
    }

    fn launcher_path(&self, workspace_id: &str) -> PathBuf {
        self.state_dir
            .clone()
            .unwrap_or_else(crate::store::default_state_dir)
            .join("launchers")
            .join(launcher_file_name(workspace_id))
    }
}

/// Windows' MAX_PATH, in UTF-16 units, including the terminating null.
const MAX_PATH: usize = 260;

/// Whether any path under `root` is too long for Git to delete without `core.longpaths`.
fn has_long_path(root: &Path) -> Result<bool, String> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory)
            .map_err(|error| format!("could not read {}: {error}", directory.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            if path.to_string_lossy().encode_utf16().count() >= MAX_PATH {
                return Ok(true);
            }
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                pending.push(path);
            }
        }
    }
    Ok(false)
}

/// Windows refuses to rename a directory while any process holds a handle inside it without
/// delete sharing (an open file, a running program, a shell's working directory): the same
/// handles that would stop a removal halfway. Rename the checkout aside and straight back.
fn worktree_in_use(worktree: &Path) -> Result<bool, String> {
    const ACCESS_DENIED: i32 = 5;
    const SHARING_VIOLATION: i32 = 32;
    let mut aside = worktree.as_os_str().to_owned();
    aside.push(".tsk-cleanup-check");
    let aside = PathBuf::from(aside);
    match std::fs::rename(worktree, &aside) {
        Ok(()) => std::fs::rename(&aside, worktree)
            .map(|()| false)
            .map_err(|error| {
                format!(
                    "could not move {} back from {}: {error}",
                    worktree.display(),
                    aside.display()
                )
            }),
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(ACCESS_DENIED | SHARING_VIOLATION)
            ) =>
        {
            Ok(true)
        }
        Err(error) => Err(format!("could not check {}: {error}", worktree.display())),
    }
}

/// One launcher per Herdr workspace: a relaunch in the same workspace replaces it, and cleanup
/// finds it from the dispatch record alone.
fn launcher_file_name(workspace_id: &str) -> String {
    let safe = workspace_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("dispatch-{safe}.ps1")
}

/// The pane line that runs a Windows launcher. It reads the same in PowerShell and cmd.
pub fn powershell_launch_line(launcher: &Path) -> String {
    format!(
        "powershell -NoProfile -ExecutionPolicy Bypass -File \"{}\"",
        launcher.display()
    )
}

impl DispatchHost for SystemDispatchHost {
    fn is_git_repo(&mut self, project: &Path) -> Result<bool, String> {
        crate::git_base::git_process_output(project, &["rev-parse", "--show-toplevel"])
            .map(|output| output.status.success())
    }

    fn resolve_base_choice(
        &mut self,
        project: &Path,
        explicit: Option<&str>,
    ) -> Result<crate::git_base::ResolvedBase, String> {
        crate::git_base::resolve(project, explicit)
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

    fn branch_taken(&mut self, project: &Path, branch: &str) -> Result<bool, String> {
        // Herdr keeps worktrees under the user's home: `%USERPROFILE%` on Windows.
        let home =
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
        system_branch_taken(project, branch, home.as_deref())
    }

    fn inspect_cleanup(
        &mut self,
        project: &Path,
        dispatch: &Dispatch,
        in_herdr: bool,
    ) -> Result<CleanupInspection, String> {
        system_inspect_cleanup(project, dispatch, in_herdr, true)
    }

    fn inspect_cleanup_cached(
        &mut self,
        project: &Path,
        dispatch: &Dispatch,
        in_herdr: bool,
    ) -> Result<CleanupInspection, String> {
        system_inspect_cleanup(project, dispatch, in_herdr, false)
    }

    fn begin_merge_check(&mut self, project: &Path, dispatch: &Dispatch) -> Option<MergeCheck> {
        let remote = dispatch.base_remote.clone()?;
        if crate::git_base::fetch_is_fresh(project, &remote) {
            return None;
        }
        let check = MergeCheck::default();
        let job = check.clone();
        let (project, dispatch) = (project.to_path_buf(), dispatch.clone());
        std::thread::spawn(move || {
            let fetched = crate::git_base::fetch_remote(&project, &remote);
            let failure = fetched.err().map(|reason| (remote, reason));
            job.complete(match merge_verdict(&project, &dispatch, failure) {
                Ok(verdict) => verdict,
                Err(reason) => MergeVerdict {
                    branch_merged: false,
                    base_available: false,
                    warning: Some(reason),
                    confirmed: false,
                    unreachable_remote: None,
                },
            });
        });
        Some(check)
    }
    fn remove_herdr_worktree(&mut self, workspace_id: &str) -> Result<(), String> {
        let output = Command::new("herdr")
            .args(["worktree", "remove", "--workspace", workspace_id])
            .output()
            .map_err(|error| format!("could not run herdr: {error}"))?;
        herdr_json(output).map(|_| ())
    }

    fn remove_git_worktree(&mut self, project: &Path, worktree: &Path) -> Result<(), String> {
        let worktree = worktree
            .to_str()
            .ok_or_else(|| "worktree path is not UTF-8".to_string())?;
        let output = crate::git_base::git_process_output_timeout(
            project,
            &["worktree", "remove", worktree],
            Duration::from_secs(5),
        )?;
        if output.status.success() {
            Ok(())
        } else {
            Err(command_failure("git worktree remove", &output))
        }
    }

    fn delete_merged_branch(
        &mut self,
        project: &Path,
        branch: &str,
        base_ref: &str,
    ) -> Result<BranchDeletion, String> {
        let reference = format!("refs/heads/{branch}");
        let listed = match cleanup_query(project, &["worktree", "list", "--porcelain"]) {
            Ok(output) => output,
            Err(error) if error == "git timed out" => {
                return Ok(BranchDeletion::Kept(
                    BranchRetentionReason::WorktreeListingTimedOut,
                ));
            }
            Err(error) => return Err(error),
        };
        if !listed.status.success() {
            return Err(command_failure("git worktree list", &listed));
        }
        if String::from_utf8_lossy(&listed.stdout)
            .lines()
            .any(|line| line == format!("branch {reference}"))
        {
            return Ok(BranchDeletion::Kept(
                BranchRetentionReason::CheckedOutElsewhere,
            ));
        }
        let tip =
            crate::git_base::git_process_output(project, &["rev-parse", "--verify", &reference])?;
        if !tip.status.success() {
            return Ok(BranchDeletion::Kept(
                BranchRetentionReason::BranchUnavailable,
            ));
        }
        let tip = String::from_utf8(tip.stdout).map_err(|error| error.to_string())?;
        let tip = tip.trim();
        if !base_ref_exists(project, base_ref)? {
            return Ok(BranchDeletion::Kept(BranchRetentionReason::BaseUnavailable));
        }
        // The worktree is already gone by the time this runs, so a timeout here must retain
        // the branch rather than surface a bare process error or a false merged/not-merged
        // reason: cleanup_query's longer deadline keeps this rare, but it must still resolve
        // to an honest, dedicated retention reason instead of an opaque failure.
        let merged = match cleanup_query(project, &["merge-base", "--is-ancestor", tip, base_ref]) {
            Ok(output) => output,
            Err(error) if error == "git timed out" => {
                return Ok(BranchDeletion::Kept(
                    BranchRetentionReason::AncestryCheckTimedOut,
                ));
            }
            Err(error) => return Err(error),
        };
        if merged.status.code() == Some(1) {
            return Ok(BranchDeletion::Kept(
                BranchRetentionReason::LatestTipNotMerged,
            ));
        }
        if !merged.status.success() {
            return Err(command_failure("git merge-base", &merged));
        }
        // update-ref compares the exact checked OID atomically. Git's -d instead
        // consults this checkout's stale HEAD for an untracked dispatch branch.
        let deleted = crate::git_base::git_process_output_timeout(
            project,
            &["update-ref", "--no-deref", "-d", &reference, tip],
            Duration::from_secs(5),
        )?;
        if deleted.status.success() {
            return Ok(BranchDeletion::Removed);
        }
        let current =
            crate::git_base::git_process_output(project, &["rev-parse", "--verify", &reference])?;
        if current.status.success() && String::from_utf8_lossy(&current.stdout).trim() != tip {
            return Ok(BranchDeletion::Kept(BranchRetentionReason::Advanced));
        }
        Err(command_failure("git update-ref", &deleted))
    }

    fn begin_cleanup(&mut self, job: CleanupJob, plan: Vec<CleanupPlanRow>, in_herdr: bool) {
        let mut host = self.clone();
        std::thread::spawn(move || run_cleanup_job(&job, &plan, in_herdr, &mut host));
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

    fn pane_has_agent(&mut self, pane_id: &str) -> Result<bool, String> {
        let output = Command::new("herdr")
            .args(["agent", "get", pane_id])
            .output()
            .map_err(|error| format!("could not run herdr: {error}"))?;
        if !output.status.success()
            && herdr_error_code(&output.stderr).as_deref() == Some("agent_not_found")
        {
            return Ok(false);
        }
        herdr_json(output).map(|_| true)
    }

    fn platform(&self) -> HostPlatform {
        HostPlatform::native()
    }

    fn write_launcher(&mut self, workspace_id: &str, script: &str) -> Result<PathBuf, String> {
        let path = self.launcher_path(workspace_id);
        let directory = path.parent().expect("launcher has a directory");
        std::fs::create_dir_all(directory)
            .and_then(|()| std::fs::write(&path, script))
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
        Ok(path)
    }

    fn remove_launcher(&mut self, workspace_id: &str) {
        let _ = std::fs::remove_file(self.launcher_path(workspace_id));
    }

    fn long_paths_enabled(&mut self, project: &Path) -> Result<bool, String> {
        let output = crate::git_base::git_process_output(
            project,
            &["config", "--type=bool", "--get", "core.longpaths"],
        )?;
        match output.status.code() {
            Some(0) => Ok(String::from_utf8_lossy(&output.stdout).trim() == "true"),
            Some(1) => Ok(false),
            _ => Err(command_failure("git config", &output)),
        }
    }

    fn wait_before_retry(&mut self) {
        std::thread::sleep(REMOVAL_RETRY_DELAY);
    }

    fn close_herdr_workspace(&mut self, workspace_id: &str) -> Result<(), String> {
        let output = Command::new("herdr")
            .args(["workspace", "close", workspace_id])
            .output()
            .map_err(|error| format!("could not run herdr: {error}"))?;
        herdr_json(output).map(|_| ())
    }

    fn removal_blocker(
        &mut self,
        project: &Path,
        worktree: &Path,
    ) -> Result<Option<RemovalBlock>, String> {
        if !cfg!(windows) {
            return Ok(None);
        }
        if !self.long_paths_enabled(project).unwrap_or(false) && has_long_path(worktree)? {
            return Ok(Some(RemovalBlock::PathTooLong));
        }
        worktree_in_use(worktree).map(|in_use| in_use.then_some(RemovalBlock::FilesInUse))
    }

    fn begin_launches(&mut self, jobs: Vec<EligibleDispatch>) -> LaunchBatch {
        let host = self.clone();
        spawn_launches(jobs, move || host)
    }

    fn begin_git_checks(&mut self, projects: Vec<PathBuf>) -> GitChecks {
        let checks = GitChecks::default();
        let finishing = checks.clone();
        let mut host = self.clone();
        std::thread::spawn(move || {
            finishing.finish(check_git_projects(projects, &mut host));
        });
        checks
    }
}

/// Name the dispatched agent on a detached thread so neither the board nor the save waits on
/// Herdr's detection. Best effort: every failure leaves the agent unnamed. A CLI process must
/// join the handle before exiting, or the thread dies with it.
pub fn spawn_agent_naming(naming: AgentNaming) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let _ = name_agent_when_detected(&naming);
    })
}

fn name_agent_when_detected(naming: &AgentNaming) -> Result<(), String> {
    // Herdr detects the agent shortly after the launch line runs; until then the pane has no
    // agent and rename answers `agent_not_found`. Any other refusal, such as
    // `agent_name_taken` by an agent elsewhere, is final: the agent stays unnamed.
    let deadline = Instant::now() + AGENT_DETECTION_TIMEOUT;
    loop {
        let output = Command::new("herdr")
            .args(["agent", "rename", &naming.pane_id, &naming.name])
            .output()
            .map_err(|error| format!("could not run herdr: {error}"))?;
        let undetected = !output.status.success()
            && herdr_error_code(&output.stderr).as_deref() == Some("agent_not_found");
        if !undetected || Instant::now() >= deadline {
            return herdr_json(output).map(|_| ());
        }
        std::thread::sleep(AGENT_DETECTION_POLL);
    }
}

fn herdr_error_code(stderr: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(stderr).ok()?;
    value
        .pointer("/error/code")
        .and_then(Value::as_str)
        .map(str::to_string)
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

/// Whether two canonical cleanup paths name the same checkout. Windows compares them
/// case-insensitively and ignores separator style, a trailing separator, and the `\\?\`
/// prefix `canonicalize` adds, since Herdr, git, and the record each spell paths differently.
fn same_path(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        normalize_windows_path(&left.to_string_lossy())
            == normalize_windows_path(&right.to_string_lossy())
    } else {
        left == right
    }
}

/// One spelling for a Windows path: `\` separators, no verbatim prefix, no trailing separator
/// (except on a drive root), lowercase.
fn normalize_windows_path(path: &str) -> String {
    let path = path.replace('/', "\\");
    let path = if let Some(unc) = path.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{unc}")
    } else if let Some(local) = path.strip_prefix("\\\\?\\") {
        local.to_string()
    } else {
        path
    };
    let trimmed = path.trim_end_matches('\\');
    let path = if trimmed.ends_with(':') {
        format!("{trimmed}\\")
    } else {
        trimmed.to_string()
    };
    path.to_lowercase()
}

/// Canonical paths of every worktree git lists for the project. Prunable entries whose
/// directories are already gone (a leftover from another tool) can neither canonicalize
/// nor be the recorded target, so they are skipped instead of failing the whole listing:
/// one stale entry must not disable cleanup offers for unrelated tasks.
fn git_worktree_paths(project: &Path) -> Result<Vec<PathBuf>, String> {
    let listed = cleanup_query(project, &["worktree", "list", "--porcelain"])
        .map_err(|error| cleanup_inspection_error("worktree listing", error))?;
    if !listed.status.success() {
        return Err(command_failure("git worktree list", &listed));
    }
    Ok(String::from_utf8_lossy(&listed.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .filter_map(|path| canonical_cleanup_path(Path::new(path)).ok())
        .collect())
}

/// Cleanup inspection for the real host. `fetch` refreshes the recorded base's remote first
/// (through the shared fetch window); the board's card opens with it off.
fn system_inspect_cleanup(
    project: &Path,
    dispatch: &Dispatch,
    in_herdr: bool,
    fetch: bool,
) -> Result<CleanupInspection, String> {
    let worktree = Path::new(&dispatch.worktree);
    let project_path = canonical_cleanup_path(project)?;
    // A worktree deleted by hand may already be pruned from git's list, so a missing
    // directory converges to cleaned before the registration gate can refuse it. The
    // project root itself is never a removal target, present or not.
    let worktree_path = match canonical_cleanup_path(worktree) {
        Ok(path) if !same_path(&path, &project_path) => path,
        _ => {
            return Ok(CleanupInspection {
                worktree_exists: worktree.exists(),
                ..CleanupInspection::default()
            })
        }
    };
    if !worktree.exists() {
        return Ok(CleanupInspection {
            target_matches: true,
            ..CleanupInspection::default()
        });
    }
    let registered = git_worktree_paths(project)?
        .iter()
        .any(|listed| same_path(listed, &worktree_path));
    if !registered {
        return Ok(CleanupInspection {
            worktree_exists: true,
            ..CleanupInspection::default()
        });
    }
    // Full untracked status on a real checkout, not a quick plumbing ref: give it cleanup's
    // longer deadline rather than git_base's 250ms metadata default. A timeout here returns
    // Err before any worktree or branch is touched below, so the refusal is always safe.
    let status = cleanup_query(
        worktree,
        &["status", "--porcelain", "--untracked-files=all"],
    )
    .map_err(|error| cleanup_inspection_error("status", error))?;
    if !status.status.success() {
        return Err(command_failure("git status", &status));
    }
    let fetch_failure = if fetch {
        dispatch.base_remote.clone().and_then(|remote| {
            crate::git_base::fetch_remote(project, &remote)
                .err()
                .map(|reason| (remote, reason))
        })
    } else {
        None
    };
    let MergeVerdict {
        branch_merged,
        base_available,
        warning,
        unreachable_remote,
        ..
    } = merge_verdict(project, dispatch, fetch_failure)?;
    let (workspace_exists, workspace_matches) = if in_herdr {
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
                    entry.get("open_workspace_id").and_then(Value::as_str)
                        == Some(dispatch.herdr_workspace_id.as_str())
                })
            });
        match workspace {
            Some(entry) => {
                let matches = herdr_checkout_path(entry)
                    .and_then(|path| canonical_cleanup_path(Path::new(path)).ok())
                    .is_some_and(|path| same_path(&path, &worktree_path));
                (true, matches)
            }
            None => (false, true),
        }
    } else {
        (false, true)
    };
    Ok(CleanupInspection {
        warning,
        unreachable_remote,
        base_available,
        worktree_exists: true,
        dirty: !status.stdout.is_empty(),
        branch_merged,
        workspace_exists,
        target_matches: workspace_matches,
    })
}

/// Ancestry against the recorded base from the refs on disk now. `fetch_failure` is the
/// remote whose preceding fetch failed and why: the refs on disk then confirm nothing.
fn merge_verdict(
    project: &Path,
    dispatch: &Dispatch,
    fetch_failure: Option<(String, String)>,
) -> Result<MergeVerdict, String> {
    let mut base_available = false;
    let branch_merged = if let Some(base_ref) = dispatch.base_ref.as_deref() {
        if base_ref_exists(project, base_ref)? {
            base_available = true;
            // Same deadline reasoning as the status read: this still runs before any
            // worktree or branch mutation, so a timeout here refuses safely too.
            let merged = cleanup_query(
                project,
                &[
                    "merge-base",
                    "--is-ancestor",
                    &format!("refs/heads/{}", dispatch.branch),
                    base_ref,
                ],
            )
            .map_err(|error| cleanup_inspection_error("ancestry check", error))?;
            match merged.status.code() {
                Some(0) => true,
                Some(1) => false,
                _ => return Err(command_failure("git merge-base", &merged)),
            }
        } else {
            // Fetch may have pruned the base, or a local base was deleted.
            // Removing a clean worktree is still safe, deleting its branch is not.
            false
        }
    } else {
        false
    };
    let (unreachable_remote, warning) = match fetch_failure {
        Some((remote, reason)) => {
            let warning = if base_available {
                format!("{reason}; merged status not confirmed, branch kept")
            } else {
                format!("fetch failed: {reason}; merged status unavailable because recorded base no longer available")
            };
            (Some(remote), Some(warning))
        }
        None => (None, None),
    };
    Ok(MergeVerdict {
        branch_merged,
        base_available,
        warning,
        confirmed: true,
        unreachable_remote,
    })
}

/// The recorded `base_ref`, verified verbatim. Missing bases are a retention reason, not a
/// failure to clean the worktree.
fn base_ref_exists(project: &Path, base_ref: &str) -> Result<bool, String> {
    let output = crate::git_base::git_process_output(
        project,
        &["show-ref", "--quiet", "--verify", base_ref],
    )?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(command_failure("git show-ref", &output)),
    }
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
        path: PathBuf::from(without_trailing_separator(&string("/worktree/path")?)),
        branch: string("/worktree/branch")?,
        workspace_id: string("/workspace/workspace_id")?,
        root_pane_id: string("/root_pane/pane_id")?,
    })
}

/// Herdr on Windows reports checkout paths with a trailing `\\`; the record and `{worktree}`
/// carry the path without it. A bare root keeps its separator.
fn without_trailing_separator(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() || trimmed.ends_with(':') {
        path.to_string()
    } else {
        trimmed.to_string()
    }
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
        let detail = herdr_error_detail(&output.stderr)
            .unwrap_or_else(|| String::from_utf8_lossy(&output.stderr).trim().to_string());
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

/// Herdr prints failures as a JSON error envelope (`{"error":{"code","message"}}`).
/// Surface its message instead of the raw document so status lines stay readable;
/// plain-text or unparseable stderr passes through verbatim.
fn herdr_error_detail(stderr: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(stderr).ok()?;
    let error = value.get("error")?;
    if let Some(message) = error
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
    {
        return Some(format!("herdr: {message}"));
    }
    error
        .get("code")
        .and_then(Value::as_str)
        .map(|code| format!("herdr: {code}"))
}

/// The board card's preview: cached refs only, so it opens without a network round trip.
pub fn inspect_cleanup_cached_with_host(
    state: &DomainState,
    id: Uuid,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Result<CleanupPreview, CleanupError> {
    let plan = cleanup_plan(state, id, CleanupRefs::Cached)?;
    inspect_planned(&plan, in_herdr, host)
}

/// One dispatch a cleanup will remove, captured from the domain so the host work can run
/// without it (on the board, off the event loop).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupPlanRow {
    pub task_id: Uuid,
    pub number: u64,
    pub title: String,
    pub project: PathBuf,
    pub record: Dispatch,
    pub refs: CleanupRefs,
}

/// Capture the task's live dispatch for cleanup, refusing what cleanup never touches.
pub fn cleanup_plan(
    state: &DomainState,
    id: Uuid,
    refs: CleanupRefs,
) -> Result<CleanupPlanRow, CleanupError> {
    let task = state.get(id).ok_or(CleanupError::UnknownTask)?;
    let number = task.number.ok_or(CleanupError::UnknownTask)?;
    let record = task.dispatch.clone().ok_or(CleanupError::NotDispatched)?;
    if record.cleaned {
        return Err(CleanupError::AlreadyCleaned);
    }
    let project = match &task.scope {
        TaskScope::Project { path } => PathBuf::from(path),
        TaskScope::Global => return Err(CleanupError::NotDispatched),
    };
    Ok(CleanupPlanRow {
        task_id: id,
        number,
        title: task.title.clone(),
        project,
        record,
        refs,
    })
}

fn inspect_planned(
    plan: &CleanupPlanRow,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Result<CleanupPreview, CleanupError> {
    let (project, record) = (&plan.project, &plan.record);
    let mut inspection = if plan.refs != CleanupRefs::Fetch {
        host.inspect_cleanup_cached(project, record, in_herdr)
    } else {
        host.inspect_cleanup(project, record, in_herdr)
    }
    .map_err(CleanupError::Herdr)?;
    if !inspection.target_matches {
        return Err(CleanupError::WorktreeMismatch);
    }
    // Records dispatched before exact base tracking have no `base_ref`. They may still be
    // cleaned, but the branch is always retained because no safe ancestry target is known.
    if record.base_ref.is_none() {
        inspection.branch_merged = false;
        inspection.base_available = false;
    }
    Ok(CleanupPreview {
        number: plan.number,
        title: plan.title.clone(),
        project: project.clone(),
        record: record.clone(),
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
    let plan = cleanup_plan(state, id, CleanupRefs::Fetch)?;
    let result = clean_planned_with_host(&plan, in_herdr, host)?;
    state
        .record_dispatch_cleaned(id)
        .map_err(|error| CleanupError::Store(error.to_string()))?;
    Ok(result)
}

/// The host half of a cleanup: inspect, then remove the worktree and, when confirmed merged,
/// the branch. Touches no domain state; the caller records the dispatch cleaned on success.
pub fn clean_planned_with_host(
    plan: &CleanupPlanRow,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Result<CleanupResult, CleanupError> {
    let refs = plan.refs;
    let preview = inspect_planned(plan, in_herdr, host)?;
    if preview.inspection.dirty {
        return Err(CleanupError::DirtyWorktree);
    }

    let (worktree, deletion, workspace_removed) = if preview.inspection.worktree_exists {
        let workspace_removed = in_herdr && preview.inspection.workspace_exists;
        let worktree = Path::new(&preview.record.worktree);
        if host.platform() == HostPlatform::Windows {
            remove_windows_worktree(host, &preview, workspace_removed)?;
        } else if workspace_removed {
            host.remove_herdr_worktree(&preview.record.herdr_workspace_id)
                .map_err(CleanupError::Herdr)?;
        } else {
            host.remove_git_worktree(&preview.project, worktree)
                .map_err(CleanupError::Herdr)?;
        }
        let deletion = if preview.record.base_ref.is_none() {
            BranchDeletion::Kept(BranchRetentionReason::NoRecordedBase)
        } else if refs == CleanupRefs::Unconfirmed {
            BranchDeletion::Kept(BranchRetentionReason::MergeCheckUnfinished)
        } else if refs == CleanupRefs::Offline || preview.inspection.unreachable_remote.is_some() {
            BranchDeletion::Kept(BranchRetentionReason::RemoteUnreachable)
        } else if !preview.inspection.base_available {
            BranchDeletion::Kept(BranchRetentionReason::BaseUnavailable)
        } else if preview.inspection.branch_merged {
            let base = preview.record.base_ref.as_deref().expect("recorded base");
            host.delete_merged_branch(&preview.project, &preview.record.branch, base)
                .map_err(CleanupError::Herdr)?
        } else {
            BranchDeletion::Kept(BranchRetentionReason::NotMerged)
        };
        (WorktreeCleanup::Removed, deletion, workspace_removed)
    } else {
        (
            WorktreeCleanup::Missing,
            BranchDeletion::Kept(BranchRetentionReason::MissingWorktree),
            false,
        )
    };
    if host.platform() == HostPlatform::Windows {
        host.remove_launcher(&preview.record.herdr_workspace_id);
    }
    let (branch, branch_reason) = match deletion {
        BranchDeletion::Removed => (BranchCleanup::Removed, None),
        BranchDeletion::Kept(reason) => (BranchCleanup::Kept, Some(reason)),
    };
    let remote = (branch_reason == Some(BranchRetentionReason::RemoteUnreachable))
        .then(|| {
            preview
                .inspection
                .unreachable_remote
                .clone()
                .or_else(|| preview.record.base_remote.clone())
        })
        .flatten();
    Ok(CleanupResult {
        warning: preview.inspection.warning,
        branch_reason,
        remote,
        number: preview.number,
        title: preview.title,
        worktree_path: preview.record.worktree,
        branch_name: preview.record.branch,
        base: preview.record.base,
        workspace_id: preview.record.herdr_workspace_id,
        worktree,
        branch,
        workspace_removed,
    })
}

/// Why Windows would refuse to delete a worktree right now. Git deletes a worktree file by
/// file and unregisters it on the way, so a removal that fails midway leaves an unregistered,
/// half-deleted directory no later cleanup may touch: these are checked before anything is
/// deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalBlock {
    /// A process holds a file or directory in the worktree open.
    FilesInUse,
    /// A path in the worktree exceeds MAX_PATH while Git's `core.longpaths` is off.
    PathTooLong,
}

/// Windows cleanup: close the workspace first (that ends its agent), make sure nothing still
/// holds the checkout (one brief retry while the agent exits), then let git remove it.
fn remove_windows_worktree(
    host: &mut impl DispatchHost,
    preview: &CleanupPreview,
    close_workspace: bool,
) -> Result<(), CleanupError> {
    let (project, worktree) = (&preview.project, Path::new(&preview.record.worktree));
    if close_workspace {
        host.close_herdr_workspace(&preview.record.herdr_workspace_id)
            .map_err(CleanupError::Herdr)?;
    }
    let mut blocked = host
        .removal_blocker(project, worktree)
        .map_err(CleanupError::Herdr)?;
    if blocked == Some(RemovalBlock::FilesInUse) {
        host.wait_before_retry();
        blocked = host
            .removal_blocker(project, worktree)
            .map_err(CleanupError::Herdr)?;
    }
    match blocked {
        Some(RemovalBlock::FilesInUse) => return Err(CleanupError::FilesInUse),
        Some(RemovalBlock::PathTooLong) => return Err(CleanupError::PathTooLong),
        None => {}
    }
    // Something could still open a file between the check and the removal: name the cause.
    host.remove_git_worktree(project, worktree)
        .map_err(|error| match removal_failure(&error) {
            Some(RemovalBlock::FilesInUse) => CleanupError::FilesInUse,
            Some(RemovalBlock::PathTooLong) => CleanupError::PathTooLong,
            None => CleanupError::Herdr(error),
        })
}

/// Why git on Windows failed to remove a worktree, read from its error text.
fn removal_failure(error: &str) -> Option<RemovalBlock> {
    let error = error.to_ascii_lowercase();
    let any = |needles: &[&str]| needles.iter().any(|needle| error.contains(needle));
    if any(&[
        "filename too long",
        "file name too long",
        "path too long",
        "filename or extension is too long",
        "os error 206",
    ]) {
        Some(RemovalBlock::PathTooLong)
    } else if any(&[
        // Git for Windows reports a sharing violation while deleting as EINVAL or EACCES.
        "failed to delete",
        "being used by another process",
        "sharing violation",
        "os error 32",
        "permission denied",
        "access is denied",
        "directory not empty",
        "resource busy",
    ]) {
        Some(RemovalBlock::FilesInUse)
    } else {
        None
    }
}

/// Where one row of a background cleanup stands. A handful per card, so the outcome is
/// held inline rather than boxed.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupSlot {
    Queued,
    Running,
    Done(Result<CleanupResult, CleanupError>),
}

#[derive(Debug, Default)]
struct CleanupJobState {
    rows: Vec<CleanupSlot>,
    /// Rows the board withdrew because their task's dispatch changed: never touched.
    cancelled: Vec<bool>,
    /// Every row's git and Herdr work has landed.
    settled: bool,
}

/// A board cleanup's host work running off the event loop, one slot per planned row. The
/// worker fills it; the board polls it each frame and never waits on it.
#[derive(Debug, Clone, Default)]
pub struct CleanupJob {
    state: std::sync::Arc<std::sync::Mutex<CleanupJobState>>,
    /// The state dir whose `tsk.json` the worker rereads right before each row's host work.
    store: Option<PathBuf>,
}

/// Identity, not content: two handles are equal when they watch the same job.
impl PartialEq for CleanupJob {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.state, &other.state)
    }
}

impl Eq for CleanupJob {}

impl CleanupJob {
    pub fn new(rows: usize) -> Self {
        let job = Self::default();
        if let Ok(mut state) = job.state.lock() {
            state.rows = vec![CleanupSlot::Queued; rows];
            state.cancelled = vec![false; rows];
            state.settled = rows == 0;
        }
        job
    }

    /// Bind each row to its task's dispatch as `tsk.json` in `state_dir` records it: the
    /// worker rereads the store right before a row's host work and skips a row whose dispatch
    /// another board or process relaunched or cleaned meanwhile.
    pub fn bound_to_store(mut self, state_dir: &Path) -> Self {
        self.store = Some(state_dir.to_path_buf());
        self
    }

    /// Withdraw row `index` before the worker reaches it: it lands as `dispatch-changed`.
    pub fn cancel(&self, index: usize) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(flag) = state.cancelled.get_mut(index) {
                *flag = true;
            }
        }
    }

    /// Whether row `index` still names its task's current dispatch: not withdrawn by the
    /// board, and (when bound) still the record on disk.
    pub fn row_current(&self, index: usize, row: &CleanupPlanRow) -> bool {
        let cancelled = self
            .state
            .lock()
            .map(|state| state.cancelled.get(index).copied().unwrap_or(true))
            .unwrap_or(true);
        if cancelled {
            return false;
        }
        let Some(dir) = &self.store else {
            return true;
        };
        crate::store::TaskStore::new(dir).load().is_ok_and(|state| {
            state
                .get(row.task_id)
                .and_then(|task| task.dispatch.as_ref())
                == Some(&row.record)
        })
    }

    fn set(&self, index: usize, slot: CleanupSlot) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(row) = state.rows.get_mut(index) {
                *row = slot;
            }
        }
    }

    pub fn start(&self, index: usize) {
        self.set(index, CleanupSlot::Running);
    }

    pub fn finish(&self, index: usize, result: Result<CleanupResult, CleanupError>) {
        self.set(index, CleanupSlot::Done(result));
    }

    pub fn settle(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.settled = true;
        }
    }

    /// Every row's slot and whether the job settled; `None` while the worker holds the lock.
    pub fn snapshot(&self) -> Option<(Vec<CleanupSlot>, bool)> {
        self.state
            .try_lock()
            .ok()
            .map(|state| (state.rows.clone(), state.settled))
    }
}

/// Run every planned row in order; a refusal on one never stops the others.
pub fn run_cleanup_job(
    job: &CleanupJob,
    plan: &[CleanupPlanRow],
    in_herdr: bool,
    host: &mut impl DispatchHost,
) {
    for (index, row) in plan.iter().enumerate() {
        run_cleanup_row(job, index, row, in_herdr, host);
    }
    job.settle();
}

/// One row of [`run_cleanup_job`].
pub fn run_cleanup_row(
    job: &CleanupJob,
    index: usize,
    row: &CleanupPlanRow,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) {
    job.start(index);
    // Checked right before the host work, not only when `y` planned the row: the board
    // stays interactive meanwhile, so the task may have been relaunched since.
    let result = if job.row_current(index, row) {
        clean_planned_with_host(row, in_herdr, host)
    } else {
        Err(CleanupError::DispatchChanged)
    };
    job.finish(index, result);
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
    run_with_host_base(state, id, profiles, again, in_herdr, None, host)
}

/// One-off base overrides do not edit the task's saved preference or an existing dispatch.
#[allow(clippy::too_many_arguments)]
pub fn run_with_host_base(
    state: &mut DomainState,
    id: Uuid,
    profiles: &AgentProfiles,
    again: bool,
    in_herdr: bool,
    base_override: Option<&str>,
    host: &mut impl DispatchHost,
) -> Result<DispatchResult, DispatchError> {
    let eligible = check_with_host(state, id, profiles, again, in_herdr, host)?;
    let launched = launch_with_host(&eligible, base_override, host)?;
    commit_launch(state, eligible, launched)
}

/// A task that passed every dispatch check, with what its launch needs. Owns its data so a
/// launch can run off the board's event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EligibleDispatch {
    pub id: Uuid,
    pub number: u64,
    pub assignee: String,
    task: Task,
    profile: crate::agents::AgentProfile,
    project: PathBuf,
    again: bool,
}

impl EligibleDispatch {
    /// The task's explicit dispatch base, `None` for the repository's remote default.
    pub fn base(&self) -> Option<&str> {
        self.task.base.as_deref()
    }

    /// The task's repository.
    pub fn project(&self) -> &Path {
        &self.project
    }

    /// Whether a human changed `current`'s status (or archived, deleted, restored it) since this
    /// check, judged from its history rather than the value: blocking and unblocking again
    /// counts. History rewritten under the snapshot (an undo) counts too.
    pub fn status_touched_since(&self, current: &Task) -> bool {
        use crate::domain::TaskEventKind as Kind;
        let before = &self.task.history;
        let Some(added) = current
            .history
            .get(before.len()..)
            .filter(|_| current.history.starts_with(before))
        else {
            return true;
        };
        added.iter().any(|event| {
            matches!(
                event.kind,
                Kind::StatusSet
                    | Kind::Completed
                    | Kind::Reopened
                    | Kind::SoftDeleted
                    | Kind::Restored
                    | Kind::Archived
                    | Kind::Unarchived
            )
        })
    }
}

/// What one launch produced before it is recorded on the task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launched {
    pub record: Dispatch,
    pub naming: Option<AgentNaming>,
    pub warning: Option<String>,
}

/// Every refusal a dispatch can give before any worktree or workspace exists. The only host
/// call is the local git-repository check.
pub fn check_with_host(
    state: &DomainState,
    id: Uuid,
    profiles: &AgentProfiles,
    again: bool,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Result<EligibleDispatch, DispatchError> {
    check_inner(state, id, profiles, again, in_herdr, &mut |project| {
        host.is_git_repo(project) == Ok(true)
    })
}

/// The same checks from task state alone, leaving out the git-repository check: the bulk card
/// runs that one off the event loop ([`DispatchHost::begin_git_checks`]), and each bulk launch
/// repeats it before creating anything ([`launch_bulk_job`]).
pub fn check_task(
    state: &DomainState,
    id: Uuid,
    profiles: &AgentProfiles,
    in_herdr: bool,
) -> Result<EligibleDispatch, DispatchError> {
    check_inner(state, id, profiles, false, in_herdr, &mut |_| true)
}

fn check_inner(
    state: &DomainState,
    id: Uuid,
    profiles: &AgentProfiles,
    again: bool,
    in_herdr: bool,
    is_git_repo: &mut dyn FnMut(&Path) -> bool,
) -> Result<EligibleDispatch, DispatchError> {
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
        TaskScope::Project { path } => PathBuf::from(path),
        TaskScope::Global => return Err(DispatchError::NeedsGitProject),
    };
    if !is_git_repo(&project) {
        return Err(DispatchError::NeedsGitProject);
    }
    let profile = profiles
        .get(&assignee)
        .ok_or_else(|| DispatchError::UnknownAgent(assignee.clone()))?
        .clone();
    if let Some(existing) = &task.dispatch {
        if !again {
            return Err(DispatchError::AlreadyDispatched(existing.worktree.clone()));
        }
    }
    Ok(EligibleDispatch {
        id,
        number,
        assignee,
        task,
        profile,
        project,
        again,
    })
}

/// Create or reuse the worktree and launch the agent in its pane. Touches no task state, so
/// it can run on another thread; [`commit_launch`] records the result.
pub fn launch_with_host(
    eligible: &EligibleDispatch,
    base_override: Option<&str>,
    host: &mut impl DispatchHost,
) -> Result<Launched, DispatchError> {
    let EligibleDispatch {
        number,
        assignee,
        task,
        profile,
        project,
        ..
    } = eligible;
    let number = *number;
    let project = project.as_path();
    let platform = host.platform();
    // Checked before anything is created; an unanswered check stays quiet rather than nag.
    let long_paths_off =
        platform == HostPlatform::Windows && host.long_paths_enabled(project) == Ok(false);
    let mut warning = None;
    let (worktree, branch, base, base_ref, base_commit, base_remote, workspace_id, pane_id) =
        if let Some(existing) = task.dispatch.as_ref().filter(|_| eligible.again) {
            if existing.cleaned {
                let label = workspace_label(number, &task.title);
                // Herdr's create command deliberately handles both cases: it creates a missing
                // branch from the recorded base, or checks out an existing retained branch.
                let recreated = host
                    .create_worktree(
                        project,
                        &existing.branch,
                        existing.base_commit.as_deref().or(existing.base.as_deref()),
                        &label,
                    )
                    .map_err(DispatchError::Herdr)?;
                (
                    recreated.path.to_string_lossy().into_owned(),
                    recreated.branch,
                    existing.base.clone(),
                    existing.base_ref.clone(),
                    existing.base_commit.clone(),
                    existing.base_remote.clone(),
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
                    existing.base_ref.clone(),
                    existing.base_commit.clone(),
                    existing.base_remote.clone(),
                    existing.herdr_workspace_id.clone(),
                    pane,
                )
            }
        } else {
            let names = DispatchNames::new(number, &task.title);
            let label = names.label.clone();
            let explicit = base_override.or(task.base.as_deref());
            let choice = host
                .resolve_base_choice(project, explicit)
                .map_err(|reason| {
                    if explicit.is_some() {
                        DispatchError::UnknownBase(reason)
                    } else {
                        DispatchError::Herdr(reason)
                    }
                })?;
            let base = choice.reference;
            warning = choice.warning;
            let requested_branch = free_branch(host, project, &names, number)?;
            let created = host
                .create_worktree(
                    project,
                    &requested_branch,
                    Some(choice.commit.as_deref().unwrap_or(&base)),
                    &label,
                )
                .map_err(DispatchError::Herdr)?;
            (
                created.path.to_string_lossy().into_owned(),
                created.branch,
                Some(base),
                Some(choice.full_ref),
                choice.commit,
                choice.remote,
                created.workspace_id,
                created.root_pane_id,
            )
        };

    // A relaunch reuses the pane; never rename an agent that is still running there, it may be
    // the previous launch under another assignee. An unanswered check counts as occupied.
    let name_launch = task.dispatch.is_none() || host.pane_has_agent(&pane_id) == Ok(false);
    let steps = rendered_steps(task);
    let short_base = base
        .as_deref()
        .map(|base| crate::git_base::short_name_for_remote(base, base_remote.as_deref()))
        .unwrap_or_default();
    let rendered = profile.render(&RenderContext {
        number,
        title: &task.title,
        notes: task.notes.as_deref().unwrap_or_default(),
        steps: &steps,
        worktree: &worktree,
        branch: &branch,
        base: &short_base,
    });
    let line = match platform {
        HostPlatform::Unix => rendered.command.clone(),
        HostPlatform::Windows => {
            let launcher = host
                .write_launcher(&workspace_id, &rendered.powershell_script())
                .map_err(DispatchError::Herdr)?;
            powershell_launch_line(&launcher)
        }
    };
    if let Err(error) = host.run_in_pane(&pane_id, &line) {
        if platform == HostPlatform::Windows {
            host.remove_launcher(&workspace_id);
        }
        return Err(DispatchError::Herdr(error));
    }
    if long_paths_off {
        warning = Some(match warning {
            Some(earlier) => format!("{earlier}; {LONG_PATHS_OFF}"),
            None => LONG_PATHS_OFF.to_string(),
        });
    }

    let record = Dispatch {
        argv: rendered.argv,
        worktree,
        branch,
        base,
        base_ref,
        base_commit,
        base_remote,
        herdr_workspace_id: workspace_id,
        at: SystemTime::now(),
        cleaned: false,
    };
    Ok(Launched {
        record,
        naming: name_launch.then(|| AgentNaming {
            pane_id,
            name: agent_name(number, assignee),
        }),
        warning,
    })
}

/// One bulk launch: confirm the repository first (the card checked it off the event loop, but
/// nothing on disk is frozen), then launch exactly as a single dispatch does.
pub fn launch_bulk_job(
    job: &EligibleDispatch,
    host: &mut impl DispatchHost,
) -> Result<Launched, DispatchError> {
    #[cfg(test)]
    thread_probe::record(&job.project);
    if host.is_git_repo(&job.project) != Ok(true) {
        return Err(DispatchError::NeedsGitProject);
    }
    launch_with_host(job, None, host)
}

/// Run `jobs` in order on one new thread with the host `make_host` builds there, landing each
/// outcome on the returned batch. Launches in one repository then share its fetch window. A
/// panicking launch still lands, as a failure, or the board would wait on it forever.
pub fn spawn_launches<H: DispatchHost>(
    jobs: Vec<EligibleDispatch>,
    make_host: impl FnOnce() -> H + Send + 'static,
) -> LaunchBatch {
    let batch = LaunchBatch::new(jobs.len());
    let landing = batch.clone();
    std::thread::spawn(move || {
        let mut host = make_host();
        for job in jobs {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                launch_bulk_job(&job, &mut host)
            }))
            .unwrap_or_else(|_| Err(DispatchError::Herdr("launch failed unexpectedly".into())));
            landing.land(job, outcome);
        }
    });
    batch
}

/// Which of a bulk card's repositories are git repositories, filled in off the event loop.
#[derive(Debug, Clone, Default)]
pub struct GitChecks(
    std::sync::Arc<std::sync::Mutex<Option<std::collections::HashMap<PathBuf, bool>>>>,
);

impl PartialEq for GitChecks {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for GitChecks {}

impl GitChecks {
    pub fn finish(&self, results: std::collections::HashMap<PathBuf, bool>) {
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(results);
    }

    /// The results once every repository was checked.
    pub fn take(&self) -> Option<std::collections::HashMap<PathBuf, bool>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

fn check_git_projects(
    projects: Vec<PathBuf>,
    host: &mut impl DispatchHost,
) -> std::collections::HashMap<PathBuf, bool> {
    projects
        .into_iter()
        .map(|project| {
            #[cfg(test)]
            thread_probe::record(&project);
            let git = host.is_git_repo(&project) == Ok(true);
            (project, git)
        })
        .collect()
}

/// Which thread ran each bulk git check and launch, by project path, so tests can prove the
/// system host's work leaves the board thread.
#[cfg(test)]
pub(crate) mod thread_probe {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::thread::ThreadId;

    static RUNS: Mutex<Vec<(PathBuf, ThreadId)>> = Mutex::new(Vec::new());

    pub(crate) fn record(project: &Path) {
        RUNS.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((project.to_path_buf(), std::thread::current().id()));
    }

    /// The threads that ran work for `project` (read by the bulk dispatch tests).
    pub(crate) fn threads(project: &Path) -> Vec<ThreadId> {
        RUNS.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|(path, _)| path == project)
            .map(|(_, thread)| *thread)
            .collect()
    }
}

/// Record a launch on its task: the dispatch record and `started`, in one domain change.
pub fn commit_launch(
    state: &mut DomainState,
    eligible: EligibleDispatch,
    launched: Launched,
) -> Result<DispatchResult, DispatchError> {
    commit_launch_with_status(state, eligible, launched, true)
}

/// [`commit_launch`], starting the task only when `start` is set.
pub fn commit_launch_with_status(
    state: &mut DomainState,
    eligible: EligibleDispatch,
    launched: Launched,
    start: bool,
) -> Result<DispatchResult, DispatchError> {
    state
        .record_dispatch_with_status(eligible.id, launched.record.clone(), start)
        .map_err(|error| DispatchError::Store(error.to_string()))?;
    Ok(DispatchResult {
        warning: launched.warning,
        number: eligible.number,
        title: eligible.task.title,
        naming: launched.naming,
        assignee: eligible.assignee,
        record: launched.record,
    })
}

/// Refuse dispatch where the launch cannot run: on Windows the launcher needs the stock
/// Windows PowerShell. Checked at the board and CLI boundaries, before any worktree or
/// workspace is created.
pub fn ensure_platform_supported() -> Result<(), DispatchError> {
    #[cfg(windows)]
    if crate::cli::update::windows_powershell_path().is_err() {
        return Err(DispatchError::UnsupportedPlatform);
    }
    Ok(())
}

pub fn running_inside_herdr() -> bool {
    std::env::var("HERDR_ENV").as_deref() == Ok("1")
}

/// Herdr agent name for a dispatched task, e.g. `t105-claude`. Assignees are lowercase
/// thread-style names; Herdr refuses dots and names over 32 characters, so dots become hyphens
/// and the assignee part is truncated to fit.
pub fn agent_name(number: u64, assignee: &str) -> String {
    const MAX: usize = 32;
    let prefix = format!("t{number}-");
    let room = MAX.saturating_sub(prefix.len());
    let profile: String = assignee.replace('.', "-").chars().take(room).collect();
    let name = format!("{prefix}{}", profile.trim_end_matches('-'));
    name.trim_end_matches('-').to_string()
}

fn rendered_steps(task: &Task) -> String {
    task.steps
        .iter()
        .map(|step| format!("[{}] {}", if step.done { 'x' } else { ' ' }, step.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Longest slug a dispatch name carries, before any `-2` collision suffix.
const SLUG_MAX: usize = 30;

/// Branch, worktree directory, and Herdr workspace label for a first dispatch, cut from one rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchNames {
    /// Git-safe title component, possibly empty (a title of only symbols).
    pub slug: String,
    /// Herdr workspace label: `T<n>` plus the title words the slug kept, `…` when cut.
    pub label: String,
}

impl DispatchNames {
    pub fn new(number: u64, title: &str) -> Self {
        let (slug, words) = cut_title(title);
        let label = if words.is_empty() {
            format!("T{number}")
        } else {
            format!("T{number} {words}")
        };
        Self { slug, label }
    }

    /// `tsk/t<n>-<slug>`, or `tsk/t<n>` for an empty slug; `attempt` above 1 appends `-<attempt>`.
    pub fn branch(&self, number: u64, attempt: u32) -> String {
        let mut branch = format!("tsk/t{number}");
        if !self.slug.is_empty() {
            branch.push('-');
            branch.push_str(&self.slug);
        }
        if attempt > 1 {
            branch.push_str(&format!("-{attempt}"));
        }
        branch
    }
}

/// Label for a dispatch whose branch already exists (a cleaned record recreated by `--again`).
pub fn workspace_label(number: u64, title: &str) -> String {
    DispatchNames::new(number, title).label
}

/// Cuts the title into a slug and a label from one word list. Words are runs of alphanumeric
/// characters (any script); everything else separates them and becomes one `-`. Whole words are
/// kept while the lowercased slug stays within `SLUG_MAX` characters; a first word longer than
/// that is hard-cut at a character boundary. The label is the original title up to the end of
/// the last kept character, whitespace collapsed, with `…` only when something was cut.
fn cut_title(title: &str) -> (String, String) {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut slug = String::new();
    let mut slug_chars = 0;
    // Byte offset in `title` just past the last kept character.
    let mut kept_end = 0;
    let mut cut = false;
    for (start, word) in alphanumeric_words(&title) {
        let lower: String = word.chars().flat_map(char::to_lowercase).collect();
        let joined = slug_chars + usize::from(slug_chars > 0) + lower.chars().count();
        if joined <= SLUG_MAX {
            if slug_chars > 0 {
                slug.push('-');
            }
            slug.push_str(&lower);
            slug_chars = joined;
            kept_end = start + word.len();
            continue;
        }
        if slug_chars == 0 {
            for (offset, character) in word.char_indices() {
                let lowered: String = character.to_lowercase().collect();
                let width = lowered.chars().count();
                if slug_chars + width > SLUG_MAX {
                    break;
                }
                slug.push_str(&lowered);
                slug_chars += width;
                kept_end = start + offset + character.len_utf8();
            }
        }
        cut = true;
        break;
    }
    let label = if cut {
        format!("{}…", title[..kept_end].trim_end())
    } else {
        title
    };
    (slug, label)
}

/// Maximal runs of alphanumeric characters with their byte offsets.
fn alphanumeric_words(text: &str) -> Vec<(usize, &str)> {
    let mut words = Vec::new();
    let mut start = None;
    for (offset, character) in text.char_indices() {
        match (character.is_alphanumeric(), start) {
            (true, None) => start = Some(offset),
            (false, Some(begin)) => {
                words.push((begin, &text[begin..offset]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(begin) = start {
        words.push((begin, &text[begin..]));
    }
    words
}

/// Directory Herdr gives a worktree for `branch`: every run of characters other than ASCII
/// letters and digits becomes one `-`, so `tsk/t1-café-東京` checks out in `tsk-t1-caf`.
fn herdr_worktree_directory(branch: &str) -> String {
    branch
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Real-host name check: a local or remote-tracking branch, a registered worktree in the
/// directory Herdr derives from `branch`, or any entry already at Herdr's default checkout path
/// (`~/.herdr/worktrees/<repo>/<directory>`), registered or not.
fn system_branch_taken(project: &Path, branch: &str, home: Option<&Path>) -> Result<bool, String> {
    let local = format!("refs/heads/{branch}");
    let remote = format!("refs/remotes/*/{branch}");
    let output = crate::git_base::git_process_output_timeout(
        project,
        &["for-each-ref", "--format=%(refname)", &local, &remote],
        Duration::from_secs(5),
    )?;
    if !output.status.success() {
        return Err(command_failure("git for-each-ref", &output));
    }
    if !String::from_utf8_lossy(&output.stdout).trim().is_empty() {
        return Ok(true);
    }
    let output = crate::git_base::git_process_output_timeout(
        project,
        &["worktree", "list", "--porcelain"],
        Duration::from_secs(5),
    )?;
    if !output.status.success() {
        return Err(command_failure("git worktree list", &output));
    }
    let directory = herdr_worktree_directory(branch);
    if let (Some(home), Some(repo)) = (home, project.file_name()) {
        let default = home.join(".herdr/worktrees").join(repo).join(&directory);
        if default.symlink_metadata().is_ok() {
            return Ok(true);
        }
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .any(|path| Path::new(path).file_name() == Some(directory.as_ref())))
}

/// Collision suffixes tried before dispatch gives up (`-2` through this).
const MAX_NAME_ATTEMPT: u32 = 99;

/// First of `tsk/t<n>-<slug>`, `…-2`, `…-3` that no branch or worktree holds. A failed check
/// refuses: guessing could hand the agent someone else's branch.
fn free_branch(
    host: &mut impl DispatchHost,
    project: &Path,
    names: &DispatchNames,
    number: u64,
) -> Result<String, DispatchError> {
    for attempt in 1..=MAX_NAME_ATTEMPT {
        let branch = names.branch(number, attempt);
        if !host
            .branch_taken(project, &branch)
            .map_err(DispatchError::Herdr)?
        {
            return Ok(branch);
        }
    }
    Err(DispatchError::Herdr(format!(
        "no free branch name: {} through -{MAX_NAME_ATTEMPT} exist",
        names.branch(number, 1)
    )))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::domain::{ProvenanceOrigin, TaskEventKind, UndoEntry};
    use crate::ui::board::{apply_intent, BoardModel};
    use crate::ui::input::BoardIntent;

    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// A process exit status carrying `code`, built the way each platform encodes it.
    fn exit_code(code: u8) -> std::process::ExitStatus {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(i32::from(code) << 8)
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(u32::from(code))
        }
    }

    #[derive(Default)]
    struct FakeHost {
        git: bool,
        taken: Vec<String>,
        fail_taken: Option<String>,
        created_labels: Vec<String>,
        fail_create: Option<String>,
        fail_run: Option<String>,
        pane_agent: Option<bool>,
        agent_checks: usize,
        creates: usize,
        roots: usize,
        runs: Vec<(String, String)>,
        created_bases: Vec<Option<String>>,
        inspected_bases: Vec<Option<String>>,
        cleanup: Option<CleanupInspection>,
        removed_herdr: usize,
        removed_git: usize,
        deleted_branches: usize,
        windows: bool,
        launchers: Vec<(String, String)>,
        removed_launchers: Vec<String>,
        long_paths: Option<bool>,
        /// Errors the next Herdr or git worktree removals return, oldest first.
        herdr_remove_errors: Vec<String>,
        git_remove_errors: Vec<String>,
        retry_waits: usize,
        /// What the next Windows removal checks find, oldest first; then nothing.
        blockers: Vec<Option<RemovalBlock>>,
        blocker_checks: usize,
        closed: usize,
    }

    impl DispatchHost for FakeHost {
        fn is_git_repo(&mut self, _: &Path) -> Result<bool, String> {
            Ok(self.git)
        }

        fn resolve_base(&mut self, _: &Path) -> Result<String, String> {
            Ok("main".into())
        }

        fn branch_taken(&mut self, _: &Path, branch: &str) -> Result<bool, String> {
            if let Some(error) = self.fail_taken.clone() {
                return Err(error);
            }
            Ok(self.taken.iter().any(|taken| taken == branch))
        }

        fn create_worktree(
            &mut self,
            _: &Path,
            branch: &str,
            base: Option<&str>,
            label: &str,
        ) -> Result<CreatedWorktree, String> {
            self.creates += 1;
            self.created_bases.push(base.map(str::to_string));
            self.created_labels.push(label.into());
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
            match self.herdr_remove_errors.is_empty() {
                true => Ok(()),
                false => Err(self.herdr_remove_errors.remove(0)),
            }
        }

        fn remove_git_worktree(&mut self, _: &Path, _: &Path) -> Result<(), String> {
            self.removed_git += 1;
            match self.git_remove_errors.is_empty() {
                true => Ok(()),
                false => Err(self.git_remove_errors.remove(0)),
            }
        }

        fn platform(&self) -> HostPlatform {
            if self.windows {
                HostPlatform::Windows
            } else {
                HostPlatform::Unix
            }
        }

        fn write_launcher(&mut self, workspace_id: &str, script: &str) -> Result<PathBuf, String> {
            self.launchers.push((workspace_id.into(), script.into()));
            Ok(PathBuf::from(format!(
                "C:\\Users\\Some One\\tsk\\launchers\\{}",
                launcher_file_name(workspace_id)
            )))
        }

        fn remove_launcher(&mut self, workspace_id: &str) {
            self.removed_launchers.push(workspace_id.into());
        }

        fn long_paths_enabled(&mut self, _: &Path) -> Result<bool, String> {
            self.long_paths.ok_or_else(|| "git config failed".into())
        }

        fn wait_before_retry(&mut self) {
            self.retry_waits += 1;
        }

        fn close_herdr_workspace(&mut self, _: &str) -> Result<(), String> {
            self.closed += 1;
            Ok(())
        }

        fn removal_blocker(&mut self, _: &Path, _: &Path) -> Result<Option<RemovalBlock>, String> {
            self.blocker_checks += 1;
            Ok(match self.blockers.is_empty() {
                true => None,
                false => self.blockers.remove(0),
            })
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

        fn pane_has_agent(&mut self, _: &str) -> Result<bool, String> {
            self.agent_checks += 1;
            self.pane_agent.ok_or_else(|| "herdr unreachable".into())
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
            path.join("config.toml"),
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
                        .edit_with_assignee_and_base(
                            id,
                            "Ship Dispatch!!!",
                            Some("notes".into()),
                            TaskScope::Global,
                            None,
                            Some("implementer".into()),
                            None,
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
    fn git_worktree_listing_survives_prunable_entries_whose_directories_are_gone() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("tsk-dispatch-prunable-{nanos}-{seq}"));
        let repo = root.join("repo");
        fs::create_dir_all(&repo).expect("repo dir");
        struct Guard(PathBuf);
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _guard = Guard(root.clone());

        let git = |args: &[&str]| {
            Command::new("git")
                .args(["-C"])
                .arg(&repo)
                .args(args)
                .output()
                .expect("run git")
        };
        assert!(git(&["init", "-q"]).status.success());
        fs::write(repo.join("README"), "init\n").expect("write readme");
        assert!(git(&["add", "."]).status.success());
        assert!(git(&[
            "-c",
            "user.email=tsk@example.com",
            "-c",
            "user.name=tsk",
            "commit",
            "-qm",
            "init",
        ])
        .status
        .success());
        let stale = root.join("stale").join("wt");
        assert!(
            git(&["worktree", "add", "--detach", stale.to_str().expect("utf8")])
                .status
                .success()
        );
        // Delete the worktree's whole parent tree without pruning it: git keeps listing
        // the entry as prunable with no resolvable path or parent, and that must not fail
        // the listing for every other worktree.
        fs::remove_dir_all(root.join("stale")).expect("remove stale tree");

        let listed = git_worktree_paths(&repo).expect("worktree paths");
        let canonical_repo = repo.canonicalize().expect("canonical repo");
        assert!(
            listed.contains(&canonical_repo),
            "listing must keep resolvable worktrees: {listed:?}"
        );
    }

    #[test]
    fn successful_herdr_command_may_have_empty_stdout() {
        let output = Output {
            status: exit_code(0),
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        assert_eq!(herdr_json(output).expect("empty success"), Value::Null);
    }

    #[test]
    fn herdr_error_envelope_surfaces_its_message() {
        let output = Output {
            status: exit_code(1),
            stdout: Vec::new(),
            stderr: br#"{"error":{"code":"linked_worktree_source","message":"New and open worktree actions start from the repo parent workspace."},"id":"cli:worktree:create"}"#.to_vec(),
        };
        assert_eq!(
            herdr_json(output).unwrap_err(),
            "herdr: New and open worktree actions start from the repo parent workspace."
        );
    }

    #[test]
    fn herdr_error_envelope_without_message_falls_back_to_code() {
        let output = Output {
            status: exit_code(1),
            stdout: Vec::new(),
            stderr: br#"{"error":{"code":"workspace_gone"},"id":"cli:worktree:remove"}"#.to_vec(),
        };
        assert_eq!(herdr_json(output).unwrap_err(), "herdr: workspace_gone");
    }

    #[test]
    fn plain_text_herdr_failure_stays_verbatim() {
        let output = Output {
            status: exit_code(1),
            stdout: Vec::new(),
            stderr: b"herdr: socket not found\n".to_vec(),
        };
        assert_eq!(herdr_json(output).unwrap_err(), "herdr: socket not found");
    }

    #[test]
    fn herdr_failure_without_detail_reports_exit_status() {
        let output = Output {
            status: exit_code(1),
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        assert_eq!(
            herdr_json(output).unwrap_err(),
            format!("herdr exited with {}", exit_code(1))
        );
        assert!(herdr_json(Output {
            status: exit_code(1),
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
        .unwrap_err()
        .ends_with(": 1"));
    }

    #[test]
    fn platform_gate_passes_where_the_launch_can_run() {
        assert_eq!(ensure_platform_supported(), Ok(()));
        assert_eq!(
            DispatchError::UnsupportedPlatform.to_string(),
            "dispatch on Windows needs Windows PowerShell"
        );
    }

    fn windows_host() -> FakeHost {
        FakeHost {
            git: true,
            windows: true,
            long_paths: Some(true),
            ..FakeHost::default()
        }
    }

    /// Decode every `TskText '…'` value of a launcher, in order.
    fn launcher_values(script: &str) -> Vec<String> {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        script
            .split("(TskText '")
            .skip(1)
            .map(|rest| {
                let encoded = &rest[..rest.find('\'').expect("closing quote")];
                String::from_utf8(STANDARD.decode(encoded).expect("base64")).expect("utf-8")
            })
            .collect()
    }

    #[test]
    fn windows_dispatch_runs_a_state_dir_launcher_carrying_the_argv() {
        let (path, profiles) = profiles();
        let (mut state, id) = task();
        let mut host = windows_host();
        let result =
            run_with_host(&mut state, id, &profiles, false, true, &mut host).expect("dispatch");

        assert_eq!(host.launchers.len(), 1, "one launcher per launch");
        let (workspace, script) = &host.launchers[0];
        assert_eq!(workspace, "w9");
        assert!(
            script.is_ascii(),
            "task text is embedded encoded, never as PowerShell"
        );
        assert_eq!(launcher_values(script), result.record.argv);
        assert_eq!(
            host.runs,
            vec![(
                "w9:p1".to_string(),
                "powershell -NoProfile -ExecutionPolicy Bypass -File \
                 \"C:\\Users\\Some One\\tsk\\launchers\\dispatch-w9.ps1\""
                    .to_string()
            )]
        );
        assert_eq!(result.warning, None, "long paths on: nothing to say");
        assert!(host.removed_launchers.is_empty());
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn a_failed_windows_launch_deletes_its_launcher_and_records_nothing() {
        let (path, profiles) = profiles();
        let (mut state, id) = task();
        let before = state.clone();
        let mut host = FakeHost {
            fail_run: Some("pane gone".into()),
            ..windows_host()
        };
        let error = run_with_host(&mut state, id, &profiles, false, true, &mut host)
            .expect_err("launch fails");
        assert_eq!(error, DispatchError::Herdr("pane gone".into()));
        assert_eq!(host.removed_launchers, vec!["w9".to_string()]);
        assert_eq!(
            serde_json::to_value(&state).expect("state"),
            serde_json::to_value(&before).expect("before")
        );
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn windows_dispatch_says_once_when_long_paths_are_off() {
        let (path, profiles) = profiles();
        for (long_paths, expected) in [
            (Some(false), Some(LONG_PATHS_OFF)),
            (Some(true), None),
            // An unanswered check stays quiet.
            (None, None),
        ] {
            let (mut state, id) = task();
            let mut host = FakeHost {
                long_paths,
                ..windows_host()
            };
            let result =
                run_with_host(&mut state, id, &profiles, false, true, &mut host).expect("dispatch");
            assert_eq!(result.warning.as_deref(), expected, "{long_paths:?}");
        }
        // Unix never asks: a host whose answer would be "off" stays silent there.
        let (mut state, id) = task();
        let mut host = FakeHost {
            git: true,
            long_paths: Some(false),
            ..FakeHost::default()
        };
        let result =
            run_with_host(&mut state, id, &profiles, false, true, &mut host).expect("dispatch");
        assert_eq!(result.warning, None);
        assert!(host.launchers.is_empty(), "Unix types the $SHELL line");
        assert!(host.runs[0].1.starts_with("$SHELL -lc "));
        fs::remove_dir_all(path).expect("cleanup");
    }

    fn clean_inspection() -> CleanupInspection {
        CleanupInspection {
            unreachable_remote: None,
            warning: None,
            base_available: true,
            worktree_exists: true,
            dirty: false,
            branch_merged: true,
            workspace_exists: true,
            target_matches: true,
        }
    }

    /// Git for Windows' text when another process holds a file it deletes (seen live).
    const IN_USE: &str =
        "error: failed to delete 'C:/Users/DevBoxWin/.herdr/worktrees/app/tsk-t1-x': \
        Invalid argument";

    #[test]
    fn windows_cleanup_closes_the_workspace_and_keeps_everything_while_files_stay_in_use() {
        let (path, profiles) = profiles();
        let (mut state, id) = task();
        run_with_host(&mut state, id, &profiles, false, true, &mut windows_host())
            .expect("dispatch");
        let before = state.clone();

        let mut host = FakeHost {
            cleanup: Some(clean_inspection()),
            blockers: vec![
                Some(RemovalBlock::FilesInUse),
                Some(RemovalBlock::FilesInUse),
            ],
            ..windows_host()
        };
        let error = clean_with_host(&mut state, id, true, &mut host).expect_err("kept");
        assert_eq!(error, CleanupError::FilesInUse);
        assert_eq!(error.code(), "files-in-use");
        assert_eq!(
            error.to_string(),
            "kept: files in use (close what is running in the worktree and retry)"
        );
        assert_eq!(host.closed, 1, "closing the workspace ends its agent first");
        assert_eq!(
            (host.blocker_checks, host.retry_waits),
            (2, 1),
            "one brief retry"
        );
        assert_eq!(
            (host.removed_herdr, host.removed_git, host.deleted_branches),
            (0, 0, 0),
            "nothing is deleted while files are in use"
        );
        assert!(host.removed_launchers.is_empty());
        assert_eq!(
            serde_json::to_value(&state).expect("state"),
            serde_json::to_value(&before).expect("before"),
            "the record stays live, never marked cleaned"
        );

        // Once whatever held the files exits, the same cleanup completes; the workspace is
        // already closed.
        let mut host = FakeHost {
            cleanup: Some(CleanupInspection {
                workspace_exists: false,
                ..clean_inspection()
            }),
            ..windows_host()
        };
        let result = clean_with_host(&mut state, id, true, &mut host).expect("cleaned");
        assert_eq!(result.worktree, WorktreeCleanup::Removed);
        assert_eq!(
            (host.closed, host.removed_git, host.removed_herdr),
            (0, 1, 0)
        );
        assert_eq!(host.removed_launchers, vec!["w9".to_string()]);
        assert!(state
            .get(id)
            .and_then(|task| task.dispatch.as_ref())
            .is_some_and(|dispatch| dispatch.cleaned));
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn windows_cleanup_removes_once_the_closed_agent_has_exited() {
        let (path, profiles) = profiles();
        let (mut state, id) = task();
        run_with_host(&mut state, id, &profiles, false, true, &mut windows_host())
            .expect("dispatch");
        let mut host = FakeHost {
            cleanup: Some(clean_inspection()),
            blockers: vec![Some(RemovalBlock::FilesInUse), None],
            ..windows_host()
        };
        let result = clean_with_host(&mut state, id, true, &mut host).expect("cleaned");
        assert_eq!(result.worktree, WorktreeCleanup::Removed);
        assert_eq!(result.branch, BranchCleanup::Removed);
        assert!(result.workspace_removed);
        assert_eq!(
            (
                host.closed,
                host.retry_waits,
                host.removed_git,
                host.removed_herdr
            ),
            (1, 1, 1, 0)
        );
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn windows_cleanup_names_long_paths_and_a_removal_that_lost_a_race() {
        let (path, profiles) = profiles();
        let (mut state, id) = task();
        run_with_host(&mut state, id, &profiles, false, true, &mut windows_host())
            .expect("dispatch");
        let before = serde_json::to_value(&state).expect("before");

        let mut host = FakeHost {
            cleanup: Some(clean_inspection()),
            blockers: vec![Some(RemovalBlock::PathTooLong)],
            ..windows_host()
        };
        let error = clean_with_host(&mut state, id, true, &mut host).expect_err("kept");
        assert_eq!(error, CleanupError::PathTooLong);
        assert_eq!(error.code(), "path-too-long");
        assert_eq!(
            error.to_string(),
            "kept: path too long (run git config --global core.longpaths true and retry)"
        );
        assert_eq!(
            host.retry_waits, 0,
            "a long path does not go away by waiting"
        );
        assert_eq!(host.removed_git, 0);

        // A file opened between the check and git's removal still reads as files in use.
        let mut host = FakeHost {
            cleanup: Some(clean_inspection()),
            git_remove_errors: vec![IN_USE.into()],
            ..windows_host()
        };
        let error = clean_with_host(&mut state, id, true, &mut host).expect_err("kept");
        assert_eq!(error, CleanupError::FilesInUse);
        assert_eq!(serde_json::to_value(&state).expect("state"), before);

        // Elsewhere Herdr removes the checkout itself and its failures pass through verbatim.
        let mut host = FakeHost {
            git: true,
            cleanup: Some(clean_inspection()),
            herdr_remove_errors: vec![IN_USE.into()],
            ..FakeHost::default()
        };
        let error = clean_with_host(&mut state, id, true, &mut host).expect_err("failed");
        assert_eq!(error, CleanupError::Herdr(IN_USE.into()));
        assert_eq!((host.closed, host.blocker_checks), (0, 0));
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn git_removal_failures_name_their_windows_cause() {
        for (text, expected) in [
            (IN_USE, Some(RemovalBlock::FilesInUse)),
            (
                "error: failed to delete 'C:/w': Permission denied",
                Some(RemovalBlock::FilesInUse),
            ),
            (
                "The process cannot access the file because it is being used by another \
                 process. (os error 32)",
                Some(RemovalBlock::FilesInUse),
            ),
            (
                "fatal: cannot remove: Filename too long",
                Some(RemovalBlock::PathTooLong),
            ),
            ("fatal: 'C:/w' is not a working tree", None),
        ] {
            assert_eq!(removal_failure(text), expected, "{text}");
        }
    }

    /// The real checks against a real directory: free, then held open by this process.
    #[cfg(windows)]
    #[test]
    fn a_held_file_blocks_the_windows_removal_check_and_a_free_tree_passes() {
        let root = std::env::temp_dir().join(format!(
            "tsk-removal-check-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let worktree = root.join("tsk-t1-x");
        fs::create_dir_all(worktree.join("deep")).expect("mkdir");
        fs::write(worktree.join("deep").join("file.txt"), "x").expect("file");
        assert_eq!(worktree_in_use(&worktree), Ok(false));
        assert!(
            worktree.join("deep").join("file.txt").exists(),
            "moved back"
        );
        // Read sharing only, as a running program or a writer holds its files; Rust's default
        // shares delete too, which Windows lets a rename through.
        let held = {
            use std::os::windows::fs::OpenOptionsExt;
            fs::OpenOptions::new()
                .read(true)
                .share_mode(1) // FILE_SHARE_READ
                .open(worktree.join("deep").join("file.txt"))
                .expect("hold")
        };
        assert_eq!(worktree_in_use(&worktree), Ok(true));
        drop(held);
        assert_eq!(worktree_in_use(&worktree), Ok(false));

        assert_eq!(has_long_path(&worktree), Ok(false));
        let mut deep = worktree.clone();
        while deep.to_string_lossy().encode_utf16().count() < MAX_PATH {
            deep.push("a-directory-name-of-some-length");
        }
        fs::create_dir_all(&deep).expect("long path");
        assert_eq!(has_long_path(&worktree), Ok(true));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn windows_paths_match_across_herdr_git_and_canonical_spellings() {
        let canonical =
            normalize_windows_path(r"\\?\C:\Users\Some One\.herdr\worktrees\app\tsk-t1-x");
        for spelling in [
            r"C:\Users\Some One\.herdr\worktrees\app\tsk-t1-x\",
            "C:/Users/Some One/.herdr/worktrees/app/tsk-t1-x",
            r"c:\users\some one\.HERDR\worktrees\APP\tsk-t1-x",
        ] {
            assert_eq!(normalize_windows_path(spelling), canonical, "{spelling}");
        }
        assert_ne!(
            normalize_windows_path(r"C:\Users\Some One\.herdr\worktrees\app\tsk-t1-y"),
            canonical
        );
        assert_eq!(
            normalize_windows_path(r"\\?\UNC\server\share\repo"),
            normalize_windows_path(r"\\server\share\repo\")
        );
        assert_eq!(
            normalize_windows_path(r"C:\"),
            normalize_windows_path("c:/")
        );
        assert_eq!(normalize_windows_path(r"C:\"), r"c:\");
    }

    #[test]
    fn herdr_checkout_paths_lose_a_trailing_separator() {
        assert_eq!(
            without_trailing_separator(r"C:\Users\Some One\.herdr\worktrees\app\tsk-t1-x\"),
            r"C:\Users\Some One\.herdr\worktrees\app\tsk-t1-x"
        );
        assert_eq!(
            without_trailing_separator("/tmp/worktree/"),
            "/tmp/worktree"
        );
        assert_eq!(without_trailing_separator("/tmp/worktree"), "/tmp/worktree");
        assert_eq!(without_trailing_separator(r"C:\"), r"C:\");
        assert_eq!(without_trailing_separator("/"), "/");
    }

    #[test]
    fn launcher_files_are_named_from_the_workspace_id_alone() {
        assert_eq!(launcher_file_name("w7T"), "dispatch-w7T.ps1");
        assert_eq!(launcher_file_name(r"..\w:1"), "dispatch-___w_1.ps1");
        assert_eq!(
            powershell_launch_line(Path::new(
                r"C:\Users\Some One\tsk\launchers\dispatch-w7T.ps1"
            )),
            "powershell -NoProfile -ExecutionPolicy Bypass -File \
             \"C:\\Users\\Some One\\tsk\\launchers\\dispatch-w7T.ps1\""
        );
    }

    #[test]
    fn refusal_codes_are_stable() {
        let cases = [
            (DispatchError::UnknownTask, "unknown-task"),
            (DispatchError::SoftDeletedTask, "soft-deleted-task"),
            (DispatchError::NoAssignee, "no-assignee"),
            (DispatchError::NotInHerdr, "not-in-herdr"),
            (DispatchError::UnsupportedPlatform, "unsupported-platform"),
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
    fn launch_names_the_herdr_agent_after_the_task_and_profile() {
        let (path, profiles) = profiles();
        let (mut state, id) = task();
        let number = state.get(id).and_then(|task| task.number).expect("number");
        let mut host = FakeHost {
            git: true,
            ..FakeHost::default()
        };
        let result =
            run_with_host(&mut state, id, &profiles, false, true, &mut host).expect("dispatch");
        assert_eq!(
            result.naming,
            Some(AgentNaming {
                pane_id: "w9:p1".into(),
                name: format!("t{number}-implementer"),
            })
        );
        assert_eq!(
            host.agent_checks, 0,
            "a fresh pane needs no occupancy check"
        );
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn again_names_only_a_pane_that_had_no_running_agent() {
        let (path, profiles) = profiles();
        let (mut state, id) = task();
        let number = state.get(id).and_then(|task| task.number).expect("number");
        let mut first = FakeHost {
            git: true,
            ..FakeHost::default()
        };
        run_with_host(&mut state, id, &profiles, false, true, &mut first).expect("first");

        let mut empty_pane = FakeHost {
            git: true,
            pane_agent: Some(false),
            ..FakeHost::default()
        };
        let relaunch =
            run_with_host(&mut state, id, &profiles, true, true, &mut empty_pane).expect("again");
        assert_eq!(
            relaunch.naming.map(|naming| naming.name),
            Some(format!("t{number}-implementer"))
        );

        for pane_agent in [Some(true), None] {
            let mut occupied = FakeHost {
                git: true,
                pane_agent,
                ..FakeHost::default()
            };
            let relaunch =
                run_with_host(&mut state, id, &profiles, true, true, &mut occupied).expect("again");
            assert_eq!(occupied.agent_checks, 1);
            assert_eq!(occupied.runs.len(), 1, "the relaunch itself still runs");
            assert_eq!(relaunch.naming, None, "{pane_agent:?}");
        }
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn agent_name_is_herdr_legal_for_every_profile_name() {
        assert_eq!(agent_name(105, "claude"), "t105-claude");
        assert_eq!(agent_name(12, "review.strict"), "t12-review-strict");
        let long = "a".repeat(32);
        let name = agent_name(1234, &long);
        assert_eq!(name.len(), 32);
        assert_eq!(name, format!("t1234-{}", "a".repeat(26)));
        let cut_at_separator = format!("{}.b", "a".repeat(26));
        assert_eq!(
            agent_name(1234, &cut_at_separator),
            format!("t1234-{}", "a".repeat(26))
        );
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
                    unreachable_remote: None,
                    warning: None,
                    base_available: true,
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
                    unreachable_remote: None,
                    warning: None,
                    base_available: true,
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
                    unreachable_remote: None,
                    warning: None,
                    base_available: true,
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
                    unreachable_remote: None,
                    warning: None,
                    base_available: true,
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
                unreachable_remote: None,
                warning: None,
                base_available: true,
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

        // Records from before exact base tracking: no base at all, or a short base without
        // `base_ref`. Both clean the worktree but keep the branch, even when reported merged.
        for base in [None, Some("main")] {
            let (mut legacy, legacy_id) = task();
            legacy
                .record_dispatch(
                    legacy_id,
                    Dispatch {
                        argv: vec!["agent".into()],
                        worktree: "/tmp/worktree".into(),
                        branch: "tsk/t1-legacy".into(),
                        base: base.map(str::to_string),
                        base_ref: None,
                        base_commit: None,
                        base_remote: None,
                        herdr_workspace_id: "w9".into(),
                        at: SystemTime::now(),
                        cleaned: false,
                    },
                )
                .expect("legacy dispatch");
            let mut cleanup = FakeHost {
                cleanup: Some(CleanupInspection {
                    unreachable_remote: None,
                    warning: None,
                    base_available: true,
                    worktree_exists: true,
                    dirty: false,
                    branch_merged: true,
                    workspace_exists: true,
                    target_matches: true,
                }),
                ..FakeHost::default()
            };
            let result =
                clean_with_host(&mut legacy, legacy_id, true, &mut cleanup).expect("clean");
            assert_eq!(cleanup.inspected_bases, vec![base.map(str::to_string)]);
            assert_eq!(result.worktree, WorktreeCleanup::Removed, "base {base:?}");
            assert_eq!(result.branch, BranchCleanup::Kept, "base {base:?}");
            assert_eq!(
                result.branch_reason,
                Some(BranchRetentionReason::NoRecordedBase),
                "base {base:?}"
            );
            assert_eq!(cleanup.deleted_branches, 0, "base {base:?}");
            let shown = crate::cli::presenter::cleaned(result.clone(), false).stdout;
            assert!(
                shown.contains("(kept, no recorded base; branch retained)"),
                "base {base:?}: {shown}"
            );
        }
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn explicit_base_and_one_off_override_leave_task_preference_unchanged() {
        let (path, profiles) = profiles();
        let (mut state, id) = task();
        state.set_base(id, Some("saved-base".into())).unwrap();
        let mut launch = FakeHost {
            git: true,
            ..FakeHost::default()
        };
        let dispatched = run_with_host_base(
            &mut state,
            id,
            &profiles,
            false,
            true,
            Some("override"),
            &mut launch,
        )
        .unwrap();
        assert_eq!(dispatched.record.base.as_deref(), Some("override"));
        assert_eq!(state.get(id).unwrap().base.as_deref(), Some("saved-base"));
        assert_eq!(launch.created_bases, vec![Some("override".into())]);
        let mut again = FakeHost {
            git: true,
            ..FakeHost::default()
        };
        let relaunched = run_with_host_base(
            &mut state,
            id,
            &profiles,
            true,
            true,
            Some("different"),
            &mut again,
        )
        .unwrap();
        assert_eq!(relaunched.record.base.as_deref(), Some("override"));
        assert!(again.created_bases.is_empty());
        fs::remove_dir_all(path).unwrap();
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
                    unreachable_remote: None,
                    warning: None,
                    base_available: true,
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
            base_ref: None,
            base_commit: None,
            base_remote: None,
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
        assert_eq!(CleanupError::DispatchChanged.code(), "dispatch-changed");
        assert_eq!(CleanupError::Herdr("failed".into()).code(), "herdr-failed");
    }

    fn names(title: &str) -> (String, String) {
        let names = DispatchNames::new(7, title);
        (names.branch(7, 1), names.label)
    }

    #[test]
    fn dispatch_names_keep_short_titles_whole() {
        assert_eq!(
            names("  Hello, WORLD!! "),
            ("tsk/t7-hello-world".into(), "T7 Hello, WORLD!!".into())
        );
    }

    #[test]
    fn dispatch_names_cut_long_titles_at_a_word_boundary_within_30() {
        assert_eq!(
            names("up/down is reveresd in project selector"),
            (
                "tsk/t7-up-down-is-reveresd-in-project".into(),
                "T7 up/down is reveresd in project…".into()
            )
        );
        assert_eq!(
            names("Shorter dispatch branch, worktree, and workspace names"),
            (
                "tsk/t7-shorter-dispatch-branch".into(),
                "T7 Shorter dispatch branch…".into()
            )
        );
    }

    #[test]
    fn dispatch_names_split_words_on_every_separator_not_only_whitespace() {
        assert_eq!(
            names("fix parser/lexer_tokenizer_overflow bug"),
            (
                "tsk/t7-fix-parser-lexer-tokenizer".into(),
                "T7 fix parser/lexer_tokenizer…".into()
            )
        );
    }

    #[test]
    fn dispatch_names_keep_a_slug_of_exactly_30_without_ellipsis() {
        let thirty = "abcdefghij abcdefghi abcdefghi";
        assert_eq!(cut_title(thirty).0.chars().count(), 30);
        assert_eq!(
            names(thirty),
            (
                "tsk/t7-abcdefghij-abcdefghi-abcdefghi".into(),
                "T7 abcdefghij abcdefghi abcdefghi".into()
            )
        );
        assert_eq!(names(&format!("{thirty} x")).1, format!("T7 {thirty}…"));
    }

    #[test]
    fn dispatch_names_let_symbol_only_tokens_cost_nothing() {
        assert_eq!(
            names("abcdefghij abcdefghi abcdefghi 🚀"),
            (
                "tsk/t7-abcdefghij-abcdefghi-abcdefghi".into(),
                "T7 abcdefghij abcdefghi abcdefghi 🚀".into()
            )
        );
        assert_eq!(
            names("Bump deps (serde, tokio, clap) — CI"),
            (
                "tsk/t7-bump-deps-serde-tokio-clap-ci".into(),
                "T7 Bump deps (serde, tokio, clap) — CI".into()
            )
        );
        let rockets = "🚀".repeat(31);
        assert_eq!(
            names(&format!("{rockets} fix")),
            ("tsk/t7-fix".into(), format!("T7 {rockets} fix"))
        );
    }

    #[test]
    fn dispatch_names_hard_cut_a_first_word_longer_than_30_in_slug_and_label() {
        let (branch, label) = names(&format!("{} tail", "a".repeat(40)));
        assert_eq!(branch, format!("tsk/t7-{}", "a".repeat(30)));
        assert_eq!(label, format!("T7 {}…", "a".repeat(30)));
        // The label cut lands where the slug cut does, past leading symbols.
        let (branch, label) = names(&format!("!!!{}", "a".repeat(31)));
        assert_eq!(branch, format!("tsk/t7-{}", "a".repeat(30)));
        assert_eq!(label, format!("T7 !!!{}…", "a".repeat(30)));
    }

    #[test]
    fn dispatch_names_cut_multibyte_words_at_character_boundaries() {
        let (branch, label) = names(&"é".repeat(40));
        assert_eq!(branch, format!("tsk/t7-{}", "é".repeat(30)));
        assert_eq!(label, format!("T7 {}…", "é".repeat(30)));
        // `İ` lowercases to two characters; the cut never splits that pair.
        let (branch, label) = names(&"İ".repeat(20));
        assert_eq!(branch, format!("tsk/t7-{}", "i\u{307}".repeat(15)));
        assert_eq!(label, format!("T7 {}…", "İ".repeat(15)));
    }

    #[test]
    fn dispatch_names_drop_the_slug_for_a_symbols_only_title() {
        assert_eq!(names("!!! ???"), ("tsk/t7".into(), "T7 !!! ???".into()));
        assert_eq!(names("🚀"), ("tsk/t7".into(), "T7 🚀".into()));
        let long = "🚀 ".repeat(40);
        assert_eq!(
            names(&long),
            ("tsk/t7".into(), format!("T7 {}", long.trim()))
        );
        assert_eq!(DispatchNames::new(7, "!!!").branch(7, 2), "tsk/t7-2");
    }

    #[test]
    fn dispatch_names_keep_unicode_letters_and_digits() {
        assert_eq!(
            names("Café crème 🚀 launch"),
            (
                "tsk/t7-café-crème-launch".into(),
                "T7 Café crème 🚀 launch".into()
            )
        );
        assert_eq!(
            names("東京 タワー２"),
            ("tsk/t7-東京-タワー２".into(), "T7 東京 タワー２".into())
        );
        let long =
            "修复项目 选择器上下 方向颠倒问题 以及其他 一些很长的标题文字 还有更多内容在这里";
        let (branch, label) = names(long);
        assert_eq!(branch, "tsk/t7-修复项目-选择器上下-方向颠倒问题-以及其他");
        assert_eq!(label, "T7 修复项目 选择器上下 方向颠倒问题 以及其他…");
    }

    #[test]
    fn system_host_treats_local_remote_and_worktree_names_as_taken() {
        let root = std::env::temp_dir().join(format!(
            "tsk-dispatch-taken-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let project = root.join("repo");
        fs::create_dir_all(&project).expect("mkdir");
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(&project)
                .args(args)
                .output()
                .expect("run git");
            assert!(output.status.success(), "{args:?}: {output:?}");
        };
        git(&["init", "-q"]);
        git(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ]);
        git(&["branch", "tsk/t1-local"]);
        git(&["update-ref", "refs/remotes/origin/tsk/t2-remote", "HEAD"]);
        let worktree = root.join("tsk-t3-dir");
        git(&[
            "worktree",
            "add",
            "-q",
            "-b",
            "elsewhere",
            worktree.to_str().unwrap(),
        ]);

        // Left behind at Herdr's default checkout path with no registration and no branch.
        let home = root.join("home");
        fs::create_dir_all(home.join(".herdr/worktrees/repo/tsk-t4-leftover")).expect("mkdir");

        for (branch, taken) in [
            ("tsk/t1-local", true),
            ("tsk/t2-remote", true),
            ("tsk/t3-dir", true),
            ("tsk/t4-leftover", true),
            ("tsk/t1", false),
            ("tsk/t1-local-2", false),
            ("tsk/t4-leftover-2", false),
        ] {
            assert_eq!(
                system_branch_taken(&project, branch, Some(&home)),
                Ok(taken),
                "{branch}"
            );
        }
        assert!(system_branch_taken(&root, "tsk/t1", Some(&home)).is_err());

        // Herdr checks a Unicode branch out in an ASCII directory, which can collide.
        assert_eq!(
            herdr_worktree_directory("tsk/t6-café-東京-x"),
            "tsk-t6-caf-x"
        );
        fs::create_dir_all(home.join(".herdr/worktrees/repo/tsk-t6-caf-x")).expect("mkdir");
        assert_eq!(
            system_branch_taken(&project, "tsk/t6-café-東京-x", Some(&home)),
            Ok(true)
        );

        // Unicode slugs stay valid branch names.
        let branch = DispatchNames::new(5, "Café 東京 İstanbul — ünïcode").branch(5, 2);
        git(&["check-ref-format", "--branch", &branch]);
        git(&["branch", &branch]);
        assert_eq!(system_branch_taken(&project, &branch, None), Ok(true));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn first_dispatch_hands_the_cut_label_and_suffixed_branch_to_herdr_and_the_agent() {
        let (path, profiles) = profiles();
        let (mut state, id) = task();
        let title = "Shorter dispatch branch, worktree, and workspace names";
        state
            .edit(
                id,
                title,
                None,
                state.get(id).expect("task").scope.clone(),
                None,
            )
            .expect("retitle");
        let mut host = FakeHost {
            git: true,
            taken: vec!["tsk/t1-shorter-dispatch-branch".into()],
            ..FakeHost::default()
        };
        let result =
            run_with_host(&mut state, id, &profiles, false, true, &mut host).expect("dispatch");
        let branch = "tsk/t1-shorter-dispatch-branch-2";
        assert_eq!(result.record.branch, branch);
        assert_eq!(host.created_labels, ["T1 Shorter dispatch branch…"]);
        assert!(result.record.argv.iter().any(|arg| arg == branch));
        assert!(host.runs[0].1.contains(branch));
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn again_keeps_recorded_names_after_a_retitle_and_never_rechecks_them() {
        let (path, profiles) = profiles();
        let (mut state, id) = task();
        let mut first = FakeHost {
            git: true,
            taken: vec!["tsk/t1-ship-dispatch".into()],
            ..FakeHost::default()
        };
        run_with_host(&mut state, id, &profiles, false, true, &mut first).expect("dispatch");
        let recorded = "tsk/t1-ship-dispatch-2";
        let scope = state.get(id).expect("task").scope.clone();
        state
            .edit(
                id,
                "A much longer title that the slug has to cut",
                None,
                scope,
                None,
            )
            .expect("retitle");
        // Any name probe on a relaunch would fail it: the recorded branch is reused as-is.
        let probe_fails = || FakeHost {
            git: true,
            fail_taken: Some("name probe must not run on --again".into()),
            ..FakeHost::default()
        };

        let mut live = probe_fails();
        let result =
            run_with_host(&mut state, id, &profiles, true, true, &mut live).expect("again live");
        assert_eq!(result.record.branch, recorded);
        assert_eq!(live.creates, 0);

        let mut clean = FakeHost {
            cleanup: Some(CleanupInspection {
                unreachable_remote: None,
                warning: None,
                base_available: true,
                worktree_exists: false,
                dirty: false,
                branch_merged: false,
                workspace_exists: false,
                target_matches: true,
            }),
            ..FakeHost::default()
        };
        clean_with_host(&mut state, id, true, &mut clean).expect("clean");
        let mut recreate = probe_fails();
        recreate.taken = vec![recorded.into()];
        let result = run_with_host(&mut state, id, &profiles, true, true, &mut recreate)
            .expect("again cleaned");
        assert_eq!(result.record.branch, recorded);
        assert_eq!(
            recreate.created_labels,
            ["T1 A much longer title that the…"]
        );
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn first_dispatch_appends_a_suffix_past_taken_branch_names() {
        let (path, profiles) = profiles();
        let (mut state, id) = task();
        let mut host = FakeHost {
            git: true,
            taken: vec![
                "tsk/t1-ship-dispatch".into(),
                "tsk/t1-ship-dispatch-2".into(),
            ],
            ..FakeHost::default()
        };
        let result =
            run_with_host(&mut state, id, &profiles, false, true, &mut host).expect("dispatch");
        assert_eq!(result.record.branch, "tsk/t1-ship-dispatch-3");
        assert_eq!(host.created_labels, ["T1 Ship Dispatch!!!"]);

        let (mut state, id) = task();
        let mut host = FakeHost {
            git: true,
            fail_taken: Some("git timed out".into()),
            ..FakeHost::default()
        };
        let error = run_with_host(&mut state, id, &profiles, false, true, &mut host)
            .expect_err("unanswered name check refuses");
        assert!(matches!(error, DispatchError::Herdr(_)), "{error}");
        assert_eq!(host.creates, 0);
        assert!(state.get(id).expect("task").dispatch.is_none());
        fs::remove_dir_all(path).expect("cleanup");
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
            base_ref: None,
            base_commit: None,
            base_remote: None,
            herdr_workspace_id: "w9".into(),
            at: SystemTime::now(),
            cleaned: false,
        };
        let inspection = SystemDispatchHost::default()
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
        let inspection = SystemDispatchHost::default()
            .inspect_cleanup(&project, &root_record, false)
            .expect("inspect root");
        assert!(!inspection.target_matches);

        let _ = std::fs::remove_dir_all(&root);
    }
}
