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
pub const UNSUPPORTED_PLATFORM: &str = "dispatch needs herdr on macOS or Linux";

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
    DeletionDeclined,
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
            Self::DeletionDeclined => "deletion declined",
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
            Self::DeletionDeclined => "branch deletion declined by host".into(),
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
    /// The worktree's git-ignored entries, parked in the cleanup trash before removal. The
    /// caller deletes it once the cleaned marker is saved ([`purge_trash`]).
    pub trash: Option<PathBuf>,
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
            reference,
            full_ref: None,
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
        _base: &str,
    ) -> Result<bool, String> {
        self.delete_branch(project, branch)?;
        Ok(true)
    }
    /// Compatibility seam: existing hosts can keep their bool deletion implementation.
    fn delete_merged_branch_with_reason(
        &mut self,
        project: &Path,
        branch: &str,
        base: &str,
    ) -> Result<BranchDeletion, String> {
        self.delete_merged_branch(project, branch, base)
            .map(|removed| {
                if removed {
                    BranchDeletion::Removed
                } else {
                    BranchDeletion::Kept(BranchRetentionReason::DeletionDeclined)
                }
            })
    }
    /// Rename the worktree's git-ignored entries (build output) into the cleanup trash, so the
    /// removal below deletes only tracked files and git keeps its own dirty-worktree refusal.
    /// `None` leaves the worktree as is and removal takes the slow path.
    /// An `Err` means parking failed and what had moved could not all go back: the stash is
    /// kept (its path is in the message) and the cleanup refuses before removal.
    fn stash_ignored(&mut self, _worktree: &Path) -> Result<Option<IgnoredStash>, String> {
        Ok(None)
    }
    /// Put a stash back after its worktree's removal failed. `Err` names where the entries
    /// that could not go back are kept.
    fn restore_ignored(&mut self, _stash: IgnoredStash) -> Result<(), String> {
        Ok(())
    }
    /// Run a board cleanup's host work off the event loop, filling `job`: the board polls it
    /// and applies each row's outcome as it lands. This default runs inline.
    fn begin_cleanup(&mut self, job: CleanupJob, plan: Vec<CleanupPlanRow>, in_herdr: bool)
    where
        Self: Sized,
    {
        for trash in run_cleanup_job(&job, &plan, in_herdr, self) {
            purge_trash(&trash);
        }
    }
    fn root_pane(&mut self, workspace_id: &str) -> Result<String, String>;
    fn run_in_pane(&mut self, pane_id: &str, command: &str) -> Result<(), String>;
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

    pub fn total(&self) -> usize {
        self.state().total
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

#[derive(Debug, Default)]
pub struct SystemDispatchHost;

impl DispatchHost for SystemDispatchHost {
    fn is_git_repo(&mut self, project: &Path) -> Result<bool, String> {
        crate::git_base::git_process_output(project, &["rev-parse", "--show-toplevel"])
            .map(|output| output.status.success())
    }

    fn resolve_base(&mut self, project: &Path) -> Result<String, String> {
        crate::git_base::resolve(project, None).map(|base| base.reference)
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
        let home = std::env::var_os("HOME").map(PathBuf::from);
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
        let remote = cleanup_base_remote(project, dispatch)?;
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

    fn delete_branch(&mut self, project: &Path, branch: &str) -> Result<(), String> {
        let output = crate::git_base::git_process_output_timeout(
            project,
            &["branch", "-d", branch],
            Duration::from_secs(5),
        )?;
        if output.status.success() {
            Ok(())
        } else {
            Err(command_failure("git branch -d", &output))
        }
    }

    fn delete_merged_branch(
        &mut self,
        project: &Path,
        branch: &str,
        base: &str,
    ) -> Result<bool, String> {
        self.delete_merged_branch_with_reason(project, branch, base)
            .map(|result| result == BranchDeletion::Removed)
    }

    fn delete_merged_branch_with_reason(
        &mut self,
        project: &Path,
        branch: &str,
        base: &str,
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
        let Some(exact_base) = cleanup_base_ref(project, base)? else {
            return Ok(BranchDeletion::Kept(BranchRetentionReason::BaseUnavailable));
        };
        // The worktree is already gone by the time this runs, so a timeout here must retain
        // the branch rather than surface a bare process error or a false merged/not-merged
        // reason: cleanup_query's longer deadline keeps this rare, but it must still resolve
        // to an honest, dedicated retention reason instead of an opaque failure.
        let merged =
            match cleanup_query(project, &["merge-base", "--is-ancestor", tip, &exact_base]) {
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

    fn stash_ignored(&mut self, worktree: &Path) -> Result<Option<IgnoredStash>, String> {
        match trash_root() {
            Some(root) => stash_ignored_entries(worktree, &root),
            None => Ok(None),
        }
    }

    fn restore_ignored(&mut self, stash: IgnoredStash) -> Result<(), String> {
        restore_or_keep(&stash.dir, &stash.worktree, &stash.entries)
    }

    fn begin_cleanup(&mut self, job: CleanupJob, plan: Vec<CleanupPlanRow>, in_herdr: bool) {
        std::thread::spawn(move || {
            // Every git and Herdr step lands first; the parked build output goes last.
            for trash in run_cleanup_job(&job, &plan, in_herdr, &mut SystemDispatchHost) {
                purge_trash(&trash);
            }
        });
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

    fn begin_launches(&mut self, jobs: Vec<EligibleDispatch>) -> LaunchBatch {
        spawn_launches(jobs, || SystemDispatchHost)
    }

    fn begin_git_checks(&mut self, projects: Vec<PathBuf>) -> GitChecks {
        let checks = GitChecks::default();
        let finishing = checks.clone();
        std::thread::spawn(move || {
            finishing.finish(check_git_projects(projects, &mut SystemDispatchHost));
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
    let Ok(worktree_path) = canonical_cleanup_path(worktree) else {
        return Ok(CleanupInspection {
            unreachable_remote: None,
            warning: None,
            base_available: false,
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
            unreachable_remote: None,
            warning: None,
            base_available: false,
            worktree_exists: worktree.exists(),
            dirty: false,
            branch_merged: false,
            workspace_exists: false,
            target_matches: false,
        });
    }
    if !worktree.exists() {
        return Ok(CleanupInspection {
            unreachable_remote: None,
            warning: None,
            base_available: false,
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
            unreachable_remote: None,
            warning: None,
            base_available: false,
            worktree_exists: true,
            dirty: false,
            branch_merged: false,
            workspace_exists: false,
            target_matches: false,
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
        cleanup_base_remote(project, dispatch).and_then(|remote| {
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

/// The remote whose fetch can change the recorded base. A new fully qualified record never
/// borrows another namespace, even if a remote with the same prefix is configured later.
fn cleanup_base_remote(project: &Path, dispatch: &Dispatch) -> Option<String> {
    let base = dispatch.base.as_deref().or(dispatch.base_ref.as_deref())?;
    dispatch.base_remote.clone().or_else(|| {
        if let Some(exact) = dispatch.base_ref.as_deref() {
            exact
                .strip_prefix("refs/remotes/")
                .and_then(|_| crate::git_base::remote_for_ref(project, exact))
        } else {
            crate::git_base::remote_for_ref(project, base)
        }
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
    let branch_merged = if let Some(base) =
        dispatch.base.as_deref().or(dispatch.base_ref.as_deref())
    {
        let exact_base = cleanup_base_ref(project, dispatch.base_ref.as_deref().unwrap_or(base))?;
        if let Some(exact_base) = exact_base {
            base_available = true;
            // Same deadline reasoning as the status read: this still runs before any
            // worktree or branch mutation, so a timeout here refuses safely too.
            let merged = cleanup_query(
                project,
                &[
                    "merge-base",
                    "--is-ancestor",
                    &format!("refs/heads/{}", dispatch.branch),
                    &exact_base,
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

/// Missing bases are a retention reason, not a failure to clean the worktree.
/// Exact refs are verified verbatim; only records without one use legacy inference.
fn cleanup_base_ref(project: &Path, base: &str) -> Result<Option<String>, String> {
    if base.starts_with("refs/heads/") || base.starts_with("refs/remotes/") {
        let output = crate::git_base::git_process_output(
            project,
            &["show-ref", "--quiet", "--verify", base],
        )?;
        return match output.status.code() {
            Some(0) => Ok(Some(base.to_string())),
            Some(1) => Ok(None),
            _ => Err(command_failure("git show-ref", &output)),
        };
    }
    match crate::git_base::recorded_branch_ref(project, base) {
        Ok(reference) => Ok(Some(reference)),
        Err(reason) if reason == format!("recorded base {base} is unavailable") => Ok(None),
        Err(reason) => Err(reason),
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

/// Inspect a retained dispatch without mutating host or domain state.
pub fn inspect_cleanup_with_host(
    state: &DomainState,
    id: Uuid,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Result<CleanupPreview, CleanupError> {
    inspect_cleanup_refs(state, id, in_herdr, false, host)
}

/// The board card's preview: cached refs only, so it opens without a network round trip.
pub fn inspect_cleanup_cached_with_host(
    state: &DomainState,
    id: Uuid,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Result<CleanupPreview, CleanupError> {
    inspect_cleanup_refs(state, id, in_herdr, true, host)
}

fn inspect_cleanup_refs(
    state: &DomainState,
    id: Uuid,
    in_herdr: bool,
    cached: bool,
    host: &mut impl DispatchHost,
) -> Result<CleanupPreview, CleanupError> {
    let plan = cleanup_plan(state, id, CleanupRefs::Cached)?;
    inspect_planned(&plan, in_herdr, cached, host)
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
    cached: bool,
    host: &mut impl DispatchHost,
) -> Result<CleanupPreview, CleanupError> {
    let (project, record) = (&plan.project, &plan.record);
    let mut inspection = if cached {
        host.inspect_cleanup_cached(project, record, in_herdr)
    } else {
        host.inspect_cleanup(project, record, in_herdr)
    }
    .map_err(CleanupError::Herdr)?;
    if !inspection.target_matches {
        return Err(CleanupError::WorktreeMismatch);
    }
    // Legacy v6 dispatch records have no creation base. They may still be cleaned, but the
    // branch is always retained because no safe ancestry target is known.
    if record.base.is_none() && record.base_ref.is_none() {
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
    clean_with_host_refs(state, id, in_herdr, CleanupRefs::Fetch, host)
}

/// [`clean_with_host`] with an explicit ref policy; the board never fetches on its own thread.
pub fn clean_with_host_refs(
    state: &mut DomainState,
    id: Uuid,
    in_herdr: bool,
    refs: CleanupRefs,
    host: &mut impl DispatchHost,
) -> Result<CleanupResult, CleanupError> {
    let plan = cleanup_plan(state, id, refs)?;
    let result = clean_planned_with_host(&plan, in_herdr, host)?;
    if let Err(error) = state.record_dispatch_cleaned(id) {
        if let Some(trash) = &result.trash {
            purge_trash(trash);
        }
        return Err(CleanupError::Store(error.to_string()));
    }
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
    let preview = inspect_planned(plan, in_herdr, refs != CleanupRefs::Fetch, host)?;
    if preview.inspection.dirty {
        return Err(CleanupError::DirtyWorktree);
    }

    let mut trash = None;
    let (worktree, deletion, workspace_removed) = if preview.inspection.worktree_exists {
        let workspace_removed = in_herdr && preview.inspection.workspace_exists;
        let path = Path::new(&preview.record.worktree);
        // Park the build output first: removal then deletes only tracked files, in
        // milliseconds, and git's own refusal of modified or untracked files still applies.
        let stash = host.stash_ignored(path).map_err(CleanupError::Herdr)?;
        let removed = if workspace_removed {
            host.remove_herdr_worktree(&preview.record.herdr_workspace_id)
        } else {
            host.remove_git_worktree(&preview.project, path)
        };
        if let Err(error) = removed {
            let kept = stash.map_or(Ok(()), |stash| host.restore_ignored(stash));
            return Err(CleanupError::Herdr(match kept {
                Ok(()) => error,
                Err(kept) => format!("{error}; {kept}"),
            }));
        }
        trash = stash.map(IgnoredStash::into_trash);
        let deletion = if preview.record.base.is_none() && preview.record.base_ref.is_none() {
            BranchDeletion::Kept(BranchRetentionReason::NoRecordedBase)
        } else if refs == CleanupRefs::Unconfirmed {
            BranchDeletion::Kept(BranchRetentionReason::MergeCheckUnfinished)
        } else if refs == CleanupRefs::Offline || preview.inspection.unreachable_remote.is_some() {
            BranchDeletion::Kept(BranchRetentionReason::RemoteUnreachable)
        } else if !preview.inspection.base_available {
            BranchDeletion::Kept(BranchRetentionReason::BaseUnavailable)
        } else if preview.inspection.branch_merged {
            // Never resolve a fully qualified saved ref back through a short name.
            let base = preview
                .record
                .base_ref
                .as_deref()
                .or(preview.record.base.as_deref())
                .expect("recorded base");
            match host.delete_merged_branch_with_reason(
                &preview.project,
                &preview.record.branch,
                base,
            ) {
                Ok(deletion) => deletion,
                Err(error) => {
                    // The worktree is gone for good: its parked build output goes with it.
                    if let Some(trash) = &trash {
                        purge_trash(trash);
                    }
                    return Err(CleanupError::Herdr(error));
                }
            }
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
                .or_else(|| cleanup_base_remote(&preview.project, &preview.record))
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
        base: preview.record.base.or(preview.record.base_ref),
        workspace_id: preview.record.herdr_workspace_id,
        worktree,
        branch,
        workspace_removed,
        trash,
    })
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
    /// Every row's git and Herdr work has landed (trash deletion may still run).
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

/// Run every planned row in order; a refusal on one never stops the others. Returns the
/// parked trash of the rows that were removed, for the caller to delete last.
pub fn run_cleanup_job(
    job: &CleanupJob,
    plan: &[CleanupPlanRow],
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Vec<PathBuf> {
    let mut trash = Vec::new();
    for (index, row) in plan.iter().enumerate() {
        trash.extend(run_cleanup_row(job, index, row, in_herdr, host));
    }
    job.settle();
    trash
}

/// One row of [`run_cleanup_job`]: returns its parked trash, if it was removed.
pub fn run_cleanup_row(
    job: &CleanupJob,
    index: usize,
    row: &CleanupPlanRow,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Option<PathBuf> {
    job.start(index);
    // Checked right before the host work, not only when `y` planned the row: the board
    // stays interactive meanwhile, so the task may have been relaunched since.
    let result = if job.row_current(index, row) {
        clean_planned_with_host(row, in_herdr, host)
    } else {
        Err(CleanupError::DispatchChanged)
    };
    let trash = result.as_ref().ok().and_then(|result| result.trash.clone());
    job.finish(index, result);
    trash
}

/// Cleanup trash lives here under the state dir: on the same volume as a typical worktree,
/// so parking build output is one rename, and in one place for the sweep on board open.
pub const TRASH_DIR: &str = "cleanup-trash";
const STASH_ORIGIN: &str = "origin";
const STASH_MANIFEST: &str = "manifest";
const STASH_ITEMS: &str = "items";
/// Written once the worktree is removed: the sweep then deletes, never restores.
const STASH_REMOVED: &str = "removed";
/// Written when entries could not go back: the stash is never deleted, only reported.
pub const STASH_KEEP: &str = "keep";
/// A stash younger than this may belong to a cleanup still running in another process.
const TRASH_SWEEP_AGE: Duration = Duration::from_secs(120);

static TRASH_ROOT: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);
static STASH_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Park cleanup trash under `state_dir`. Without this, cleanup removes worktrees in place.
pub fn remember_trash_in(state_dir: &Path) {
    if let Ok(mut slot) = TRASH_ROOT.lock() {
        *slot = Some(state_dir.join(TRASH_DIR));
    }
}

fn trash_root() -> Option<PathBuf> {
    TRASH_ROOT.lock().ok().and_then(|slot| slot.clone())
}

/// A worktree's git-ignored entries, renamed into the cleanup trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoredStash {
    pub dir: PathBuf,
    pub worktree: PathBuf,
    /// Paths relative to the worktree, mirrored under the stash's `items`.
    pub entries: Vec<PathBuf>,
}

impl IgnoredStash {
    /// The worktree is removed: the stash is now only trash to delete.
    pub fn into_trash(self) -> PathBuf {
        let _ = std::fs::write(self.dir.join(STASH_REMOVED), b"");
        self.dir
    }
}

/// Rename every git-ignored entry of `worktree` into a fresh stash under `root`. `Ok(None)`
/// (no ignored entries, a listing error, a rename across volumes after everything moved back)
/// means removal deletes in place. `Err` means a rollback could not put everything back: the
/// stash is kept and named, and the cleanup must refuse.
pub fn stash_ignored_entries(worktree: &Path, root: &Path) -> Result<Option<IgnoredStash>, String> {
    let Some(listed) = cleanup_query(
        worktree,
        &[
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
        ],
    )
    .ok()
    .filter(|output| output.status.success()) else {
        return Ok(None);
    };
    let mut entries = Vec::new();
    for raw in listed.stdout.split(|byte| *byte == 0) {
        if raw.is_empty() {
            continue;
        }
        let Ok(text) = std::str::from_utf8(raw) else {
            return Ok(None);
        };
        let text = text.trim_end_matches('/');
        let relative = PathBuf::from(text);
        let plain = relative
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)));
        if text.is_empty() || text.contains('\n') || !plain {
            return Ok(None);
        }
        entries.push(relative);
    }
    let Some(origin) = worktree.to_str().filter(|_| !entries.is_empty()) else {
        return Ok(None);
    };
    let name = format!(
        "{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default(),
        STASH_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let dir = root.join(name);
    if std::fs::create_dir_all(dir.join(STASH_ITEMS)).is_err() {
        return Ok(None);
    }
    let manifest = entries
        .iter()
        .map(|entry| entry.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    if std::fs::write(dir.join(STASH_ORIGIN), origin).is_err()
        || std::fs::write(dir.join(STASH_MANIFEST), manifest).is_err()
    {
        let _ = std::fs::remove_dir_all(&dir);
        return Ok(None);
    }
    let mut moved = Vec::new();
    for entry in entries {
        let target = dir.join(STASH_ITEMS).join(&entry);
        let renamed = target
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::rename(worktree.join(&entry), &target));
        if renamed.is_err() {
            // Nothing is deleted on this path: either everything went back, or the stash is
            // kept and the cleanup refuses.
            restore_or_keep(&dir, worktree, &moved)?;
            return Ok(None);
        }
        moved.push(entry);
    }
    Ok(Some(IgnoredStash {
        dir,
        worktree: worktree.to_path_buf(),
        entries: moved,
    }))
}

/// Put a stash back, dropping it only once nothing in it is left to lose. Otherwise the stash
/// is marked kept (the sweep never deletes it) and the error names where it is.
fn restore_or_keep(dir: &Path, worktree: &Path, entries: &[PathBuf]) -> Result<(), String> {
    if restore_stash(dir, worktree, entries) {
        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    } else {
        let _ = std::fs::write(
            dir.join(STASH_KEEP),
            format!("could not put these back into {}\n", worktree.display()),
        );
        Err(format!(
            "ignored files that could not go back are kept in {}",
            dir.join(STASH_ITEMS).display()
        ))
    }
}

/// Rename a stash's entries back into its worktree. True when nothing is left in the stash.
/// An entry whose destination exists (a build recreated it) stays in the stash rather than
/// replace or merge, and no rename ever goes through a symlinked directory: restoration
/// lands inside the worktree or not at all.
fn restore_stash(dir: &Path, worktree: &Path, entries: &[PathBuf]) -> bool {
    let real_dir = |path: &Path| {
        std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir() && !meta.is_symlink())
    };
    if !real_dir(worktree) {
        return entries
            .iter()
            .all(|entry| std::fs::symlink_metadata(dir.join(STASH_ITEMS).join(entry)).is_err());
    }
    let mut complete = true;
    for entry in entries {
        let from = dir.join(STASH_ITEMS).join(entry);
        if std::fs::symlink_metadata(&from).is_err() {
            continue;
        }
        if !confined_parent(worktree, entry) {
            complete = false;
            continue;
        }
        let to = worktree.join(entry);
        if std::fs::symlink_metadata(&to).is_ok() || std::fs::rename(&from, &to).is_err() {
            complete = false;
        }
    }
    complete
}

/// Make sure every directory between `worktree` and `entry` is a real directory, never a
/// symlink, creating missing ones one component at a time. False refuses the restore.
fn confined_parent(worktree: &Path, entry: &Path) -> bool {
    let Some(parent) = entry.parent() else {
        return true;
    };
    let mut path = worktree.to_path_buf();
    for part in parent.components() {
        let std::path::Component::Normal(part) = part else {
            return false;
        };
        path.push(part);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() && !meta.is_symlink() => {}
            Ok(_) => return false,
            Err(_) => {
                if std::fs::create_dir(&path).is_err() {
                    return false;
                }
            }
        }
    }
    true
}

/// Stashes the sweep left in place because they hold files it could not put back.
pub fn kept_stashes(state_dir: &Path) -> Vec<PathBuf> {
    let Ok(children) = std::fs::read_dir(state_dir.join(TRASH_DIR)) else {
        return Vec::new();
    };
    let mut kept = children
        .flatten()
        .map(|child| child.path())
        .filter(|dir| std::fs::symlink_metadata(dir.join(STASH_KEEP)).is_ok())
        .collect::<Vec<_>>();
    kept.sort();
    kept
}

/// Delete parked trash. Best effort: what survives is swept on the next board open.
pub fn purge_trash(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

/// Empty trash a quit or crash left mid-delete, on a detached thread. A stash whose worktree
/// was never removed (the process died between parking and removal) goes back first, and is
/// kept, never deleted, when it cannot all go back. The handle returns every kept stash.
pub fn spawn_trash_sweep(state_dir: &Path) -> std::thread::JoinHandle<Vec<PathBuf>> {
    let state_dir = state_dir.to_path_buf();
    std::thread::spawn(move || {
        sweep_trash(&state_dir.join(TRASH_DIR), TRASH_SWEEP_AGE);
        kept_stashes(&state_dir)
    })
}

fn sweep_trash(root: &Path, min_age: Duration) {
    let Ok(children) = std::fs::read_dir(root) else {
        return;
    };
    for child in children.flatten() {
        let dir = child.path();
        let Ok(metadata) = std::fs::symlink_metadata(&dir) else {
            continue;
        };
        if !metadata.is_dir() {
            let _ = std::fs::remove_file(&dir);
            continue;
        }
        if std::fs::symlink_metadata(dir.join(STASH_KEEP)).is_ok() {
            continue;
        }
        let young = metadata
            .modified()
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_none_or(|age| age < min_age);
        if young {
            continue;
        }
        if std::fs::symlink_metadata(dir.join(STASH_REMOVED)).is_ok() {
            purge_trash(&dir);
            continue;
        }
        let manifest = std::fs::read_to_string(dir.join(STASH_MANIFEST)).unwrap_or_default();
        let entries = manifest.lines().map(PathBuf::from).collect::<Vec<_>>();
        let origin = std::fs::read_to_string(dir.join(STASH_ORIGIN))
            .ok()
            .map(PathBuf::from);
        match origin {
            Some(origin) => {
                let _ = restore_or_keep(&dir, &origin, &entries);
            }
            // No record of where it came from: nothing can be put back, so keep it.
            None => {
                let _ = std::fs::write(dir.join(STASH_KEEP), "origin unknown\n");
            }
        }
    }
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
                choice.full_ref,
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
    host.run_in_pane(&pane_id, &rendered.command)
        .map_err(DispatchError::Herdr)?;

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

    /// The threads that ran work for `project` (read by the unix-only bulk dispatch tests).
    #[cfg(unix)]
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

/// Refuse dispatch where the rendered launch, a POSIX `$SHELL -lc` line, cannot run. Checked at
/// the board and CLI boundaries, before any worktree or workspace is created.
pub fn ensure_platform_supported() -> Result<(), DispatchError> {
    if cfg!(windows) {
        Err(DispatchError::UnsupportedPlatform)
    } else {
        Ok(())
    }
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

    /// A throwaway repo with one linked worktree holding a tracked file, an ignored
    /// `target/` (nested build output) and an ignored file inside a tracked directory.
    struct StashRepo {
        root: PathBuf,
        repo: PathBuf,
        worktree: PathBuf,
        trash: PathBuf,
    }

    impl Drop for StashRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    impl StashRepo {
        fn new(label: &str) -> Self {
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "tsk-dispatch-stash-{label}-{}-{seq}",
                std::process::id()
            ));
            let repo = root.join("repo");
            fs::create_dir_all(&repo).expect("repo dir");
            let this = Self {
                worktree: root.join("wt"),
                trash: root.join("state").join(TRASH_DIR),
                repo,
                root,
            };
            this.git(&["init", "-q"]);
            fs::write(this.repo.join(".gitignore"), "target/\n*.log\n").expect("ignore");
            fs::create_dir_all(this.repo.join("src")).expect("src");
            fs::write(this.repo.join("src/lib.rs"), "// tracked\n").expect("tracked");
            this.git(&["add", "."]);
            this.git(&[
                "-c",
                "user.email=tsk@example.com",
                "-c",
                "user.name=tsk",
                "commit",
                "-qm",
                "init",
            ]);
            this.git(&[
                "worktree",
                "add",
                "-q",
                "-b",
                "tsk/t1",
                this.worktree.to_str().expect("utf8"),
            ]);
            fs::create_dir_all(this.worktree.join("target/debug/deps")).expect("target");
            fs::write(this.worktree.join("target/debug/deps/big"), vec![0u8; 4096])
                .expect("build output");
            fs::write(this.worktree.join("src/build.log"), "log\n").expect("ignored log");
            this
        }

        fn git(&self, args: &[&str]) -> Output {
            let output = Command::new("git")
                .arg("-C")
                .arg(&self.repo)
                .args(args)
                .output()
                .expect("run git");
            assert!(output.status.success(), "git {args:?}: {output:?}");
            output
        }

        fn remove_worktree(&self) -> Result<(), String> {
            SystemDispatchHost.remove_git_worktree(&self.repo, &self.worktree)
        }
    }

    #[test]
    fn stashing_ignored_entries_leaves_only_tracked_files_and_git_still_removes_cleanly() {
        let repo = StashRepo::new("removed");
        let stash = stash_ignored_entries(&repo.worktree, &repo.trash)
            .expect("no rollback")
            .expect("stash");
        let mut entries = stash.entries.clone();
        entries.sort();
        assert_eq!(
            entries,
            vec![PathBuf::from("src/build.log"), PathBuf::from("target")]
        );
        assert!(!repo.worktree.join("target").exists());
        assert!(
            repo.worktree.join("src/lib.rs").exists(),
            "tracked files stay"
        );
        assert!(stash.dir.join("items/target/debug/deps/big").exists());
        // git's own removal (no --force) sees a clean checkout and succeeds.
        repo.remove_worktree().expect("plain git worktree remove");
        assert!(!repo.worktree.exists());
        let trash = stash.into_trash();
        assert!(trash.join("removed").exists());
        purge_trash(&trash);
        assert!(!trash.exists());
    }

    #[test]
    fn a_refused_removal_puts_the_stash_back() {
        let repo = StashRepo::new("refused");
        // Uncommitted work: git refuses the removal and nothing may be lost.
        fs::write(repo.worktree.join("notes.txt"), "draft\n").expect("untracked");
        let stash = stash_ignored_entries(&repo.worktree, &repo.trash)
            .expect("no rollback")
            .expect("stash");
        assert!(
            repo.remove_worktree().is_err(),
            "git still guards dirty work"
        );
        SystemDispatchHost
            .restore_ignored(stash.clone())
            .expect("everything goes back");
        assert!(repo.worktree.join("target/debug/deps/big").exists());
        assert!(repo.worktree.join("src/build.log").exists());
        assert!(repo.worktree.join("notes.txt").exists());
        assert!(!stash.dir.exists(), "an emptied stash is dropped");
    }

    #[test]
    fn a_worktree_without_ignored_entries_takes_the_plain_path() {
        let repo = StashRepo::new("plain");
        fs::remove_dir_all(repo.worktree.join("target")).expect("no target");
        fs::remove_file(repo.worktree.join("src/build.log")).expect("no log");
        assert_eq!(stash_ignored_entries(&repo.worktree, &repo.trash), Ok(None));
        assert!(!repo.trash.exists() || fs::read_dir(&repo.trash).unwrap().next().is_none());
    }

    #[test]
    fn the_sweep_empties_left_over_trash_and_returns_a_stash_whose_worktree_survived() {
        let repo = StashRepo::new("sweep");
        // A quit mid-delete: the worktree is gone, its parked output never got deleted.
        let removed = stash_ignored_entries(&repo.worktree, &repo.trash)
            .expect("no rollback")
            .expect("stash");
        let removed_dir = removed.dir.clone();
        let _ = removed.into_trash();
        // A crash between parking and removal: the worktree survived without its output.
        fs::create_dir_all(repo.worktree.join("target")).expect("rebuild");
        fs::write(repo.worktree.join("target/again"), "x").expect("build");
        let survived = stash_ignored_entries(&repo.worktree, &repo.trash)
            .expect("no rollback")
            .expect("stash");
        assert!(!repo.worktree.join("target").exists());
        fs::write(repo.trash.join("stray"), "x").expect("stray file");

        sweep_trash(&repo.trash, Duration::from_secs(3600));
        assert!(
            removed_dir.exists(),
            "a young stash may belong to a running cleanup"
        );

        sweep_trash(&repo.trash, Duration::ZERO);
        assert!(!removed_dir.exists());
        assert!(!survived.dir.exists());
        assert!(
            repo.worktree.join("target/again").exists(),
            "a stash whose removal never happened goes back"
        );
        assert_eq!(fs::read_dir(&repo.trash).unwrap().count(), 0);
    }

    #[test]
    fn a_restore_that_meets_a_rebuilt_entry_keeps_the_stash_and_the_sweep_never_deletes_it() {
        let repo = StashRepo::new("collision");
        let stash = stash_ignored_entries(&repo.worktree, &repo.trash)
            .expect("no rollback")
            .expect("stash");
        // A build recreates target/ while removal is refused.
        fs::create_dir_all(repo.worktree.join("target")).expect("rebuild");
        fs::write(repo.worktree.join("target/new"), "new").expect("new output");
        let kept = SystemDispatchHost
            .restore_ignored(stash.clone())
            .expect_err("target/ could not go back");
        assert!(kept.contains(&stash.dir.display().to_string()), "{kept}");
        assert!(stash.dir.join("items/target/debug/deps/big").exists());
        assert!(stash.dir.join(STASH_KEEP).exists());
        assert!(
            repo.worktree.join("src/build.log").exists(),
            "the rest went back"
        );
        assert_eq!(fs::read(repo.worktree.join("target/new")).unwrap(), b"new");

        sweep_trash(&repo.trash, Duration::ZERO);
        assert!(
            stash.dir.join("items/target/debug/deps/big").exists(),
            "a kept stash outlives every sweep"
        );
        assert_eq!(kept_stashes(repo.trash.parent().unwrap()), vec![stash.dir]);
    }

    #[test]
    fn the_sweep_keeps_an_unremoved_stash_it_cannot_put_back() {
        let repo = StashRepo::new("sweep-collision");
        // A crash between parking and removal, then a rebuild before the next open.
        let stash = stash_ignored_entries(&repo.worktree, &repo.trash)
            .expect("no rollback")
            .expect("stash");
        fs::create_dir_all(repo.worktree.join("target")).expect("rebuild");
        sweep_trash(&repo.trash, Duration::ZERO);
        assert!(stash.dir.join("items/target/debug/deps/big").exists());
        assert!(stash.dir.join(STASH_KEEP).exists());
        assert!(repo.worktree.join("src/build.log").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_rename_that_fails_midway_puts_back_what_moved_and_removes_in_place() {
        use std::os::unix::fs::PermissionsExt;
        let repo = StashRepo::new("rename-fails");
        // `zz/` is read-only: its ignored file cannot be renamed out after target/ moved.
        fs::create_dir_all(repo.worktree.join("zz")).expect("zz");
        fs::write(repo.worktree.join("zz/late.log"), "late").expect("late");
        fs::write(repo.worktree.join("zz/.keep"), "").expect("keep");
        repo.git(&["-C", repo.worktree.to_str().unwrap(), "add", "zz/.keep"]);
        let mode = |mode| fs::Permissions::from_mode(mode);
        fs::set_permissions(repo.worktree.join("zz"), mode(0o555)).expect("read-only");
        let stashed = stash_ignored_entries(&repo.worktree, &repo.trash);
        fs::set_permissions(repo.worktree.join("zz"), mode(0o755)).expect("writable");
        assert_eq!(stashed, Ok(None));
        assert!(repo.worktree.join("target/debug/deps/big").exists());
        assert!(repo.worktree.join("zz/late.log").exists());
        assert_eq!(
            kept_stashes(repo.trash.parent().unwrap()),
            Vec::<PathBuf>::new()
        );
    }

    #[cfg(unix)]
    #[test]
    fn restoration_never_follows_a_symlinked_parent_out_of_the_worktree() {
        let repo = StashRepo::new("symlink");
        let stash = stash_ignored_entries(&repo.worktree, &repo.trash)
            .expect("no rollback")
            .expect("stash");
        // The checkout swaps src/ for a link to a directory outside the worktree.
        let outside = repo.root.join("outside");
        fs::create_dir_all(&outside).expect("outside");
        fs::remove_dir_all(repo.worktree.join("src")).expect("drop src");
        std::os::unix::fs::symlink(&outside, repo.worktree.join("src")).expect("link");
        let kept = SystemDispatchHost
            .restore_ignored(stash.clone())
            .expect_err("src/build.log must not land outside");
        assert!(kept.contains("kept in"), "{kept}");
        assert!(!outside.join("build.log").exists());
        assert!(stash.dir.join("items/src/build.log").exists());
        assert!(stash.dir.join(STASH_KEEP).exists());
        assert!(repo.worktree.join("target/debug/deps/big").exists());

        // The sweep is no way around it either.
        sweep_trash(&repo.trash, Duration::ZERO);
        assert!(!outside.join("build.log").exists());
        assert!(stash.dir.join("items/src/build.log").exists());
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
    fn platform_gate_refuses_only_windows() {
        let gate = ensure_platform_supported();
        if cfg!(windows) {
            assert_eq!(gate, Err(DispatchError::UnsupportedPlatform));
            assert_eq!(
                gate.unwrap_err().to_string(),
                "dispatch needs herdr on macOS or Linux"
            );
        } else {
            assert_eq!(gate, Ok(()));
        }
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

        let (mut legacy, legacy_id) = task();
        legacy
            .record_dispatch(
                legacy_id,
                Dispatch {
                    argv: vec!["agent".into()],
                    worktree: "/tmp/worktree".into(),
                    branch: "tsk/t1-legacy".into(),
                    base: None,
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
        let result = clean_with_host(&mut legacy, legacy_id, true, &mut cleanup).expect("clean");
        assert_eq!(cleanup.inspected_bases, vec![None]);
        assert_eq!(result.branch, BranchCleanup::Kept);
        assert_eq!(cleanup.deleted_branches, 0);
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
