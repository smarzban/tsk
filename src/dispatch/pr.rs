//! A dispatched task's pull request, through the GitHub CLI (`gh`): whether one is open,
//! merging it from the cleanup card or `--merge`, and confirming a squash or rebase merge so
//! cleanup may delete a branch that git ancestry alone reads as unmerged.
//!
//! tsk stores nothing about pull requests: `gh` finds the PR from the dispatch's branch name
//! every time. Every call goes through the [`DispatchHost`] seam, so tests use a fake `gh`.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

use super::{BranchDeletion, BranchRetentionReason, DispatchHost};
use crate::agents::MergeMethod;
use crate::domain::Dispatch;

/// Longest a `gh pr view` may take before the card or cleanup treats gh as unavailable.
const GH_VIEW_TIMEOUT: Duration = Duration::from_secs(20);
/// Longest a `gh pr merge` may take. GitHub answers a merge in seconds; past this the
/// merge is reported failed and nothing is cleaned, whatever GitHub did meanwhile.
const GH_MERGE_TIMEOUT: Duration = Duration::from_secs(60);

/// Longest one row of a board `m` can take: the lookup, the merge, the lookup confirming it,
/// and the remote-branch check. A quit waits this long per merging row, then for cleanup.
pub const MERGE_ROW_BOUND: Duration = Duration::from_secs(
    2 * GH_VIEW_TIMEOUT.as_secs()
        + GH_MERGE_TIMEOUT.as_secs()
        + crate::git_base::NETWORK_TIMEOUT.as_secs(),
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Merged,
    Closed,
}

impl PrState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Merged => "merged",
            Self::Closed => "closed",
        }
    }
}

/// The pull request whose head is a dispatch's branch, as `gh pr view <branch>` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
    pub state: PrState,
    /// The commit GitHub has as the PR's head: a merged PR's head is what it merged.
    pub head_oid: String,
    /// The branch the PR targets, without a remote (`main`).
    pub base: String,
}

/// Why `gh` could not say anything about a branch's pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrLookupError {
    /// No `gh` on PATH (`gh.exe` on Windows).
    Missing,
    NotLoggedIn,
    Failed(String),
}

impl PrLookupError {
    pub fn message(&self) -> String {
        match self {
            Self::Missing => "gh not found on PATH".into(),
            Self::NotLoggedIn => "gh is not logged in (run gh auth login)".into(),
            Self::Failed(reason) => format!("gh failed: {reason}"),
        }
    }
}

/// What `gh pr view <branch>` found: a PR, no PR for the branch, or no answer.
pub type PrLookup = Result<Option<PullRequest>, PrLookupError>;

/// Why a task's pull request cannot be merged from tsk. Each has a stable CLI code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeRefusal {
    GhUnavailable(PrLookupError),
    NoPr,
    NotOpen(PrState),
    /// The PR targets another branch than the dispatch's recorded base.
    BaseMismatch {
        pr_base: String,
        base: String,
    },
}

impl MergeRefusal {
    pub fn code(&self) -> &'static str {
        match self {
            Self::GhUnavailable(_) => "gh-unavailable",
            Self::NoPr => "no-pr",
            Self::NotOpen(_) => "pr-not-open",
            Self::BaseMismatch { .. } => "pr-base-mismatch",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::GhUnavailable(error) => error.message(),
            Self::NoPr => "no pull request for the branch".into(),
            Self::NotOpen(PrState::Merged) => {
                "pull request is already merged; tsk clean (without --merge) cleans it".into()
            }
            Self::NotOpen(state) => format!("pull request is {}", state.name()),
            Self::BaseMismatch { pr_base, base } => {
                format!("pull request targets {pr_base}, not {base}")
            }
        }
    }
}

/// The recorded base as GitHub names a PR's base branch: `refs/remotes/origin/main` and
/// `origin/main` are both `main`.
pub fn record_base_branch(record: &Dispatch) -> Option<String> {
    let base = record.base_ref.as_deref().or(record.base.as_deref())?;
    let remote = record.base_remote.as_deref().or_else(|| {
        // An older record without the remote: a `refs/remotes/<remote>/` ref still names it.
        base.strip_prefix("refs/remotes/")
            .and_then(|rest| rest.split_once('/'))
            .map(|(remote, _)| remote)
    });
    Some(crate::git_base::short_name_for_remote(base, remote))
}

/// The open PR `m` and `--merge` would merge: open, and targeting the recorded base.
pub fn mergeable<'a>(
    lookup: &'a PrLookup,
    record: &Dispatch,
) -> Result<&'a PullRequest, MergeRefusal> {
    let pr = match lookup {
        Err(error) => return Err(MergeRefusal::GhUnavailable(error.clone())),
        Ok(None) => return Err(MergeRefusal::NoPr),
        Ok(Some(pr)) => pr,
    };
    if pr.state != PrState::Open {
        return Err(MergeRefusal::NotOpen(pr.state));
    }
    let base = record_base_branch(record).unwrap_or_default();
    if pr.base != base {
        return Err(MergeRefusal::BaseMismatch {
            pr_base: pr.base.clone(),
            base,
        });
    }
    Ok(pr)
}

/// A branch whose ancestry reads unmerged: delete it only when GitHub says its PR merged into
/// the recorded base and the PR's head is exactly the branch's tip (a squash or rebase merge
/// leaves no ancestry). Without an answer from gh, this is today's `not merged`. A PR merged
/// into another branch keeps the branch, and the second value names that branch.
pub fn pr_merged_deletion(
    host: &mut impl DispatchHost,
    project: &Path,
    record: &Dispatch,
) -> Result<(BranchDeletion, Option<String>), String> {
    let branch = record.branch.as_str();
    let kept = |reason| Ok((BranchDeletion::Kept(reason), None));
    let Ok(Some(pr)) = host.pull_request(project, branch) else {
        return kept(BranchRetentionReason::NotMerged);
    };
    if pr.state != PrState::Merged {
        return kept(BranchRetentionReason::NotMerged);
    }
    if record_base_branch(record).as_deref() != Some(pr.base.as_str()) {
        return Ok((
            BranchDeletion::Kept(BranchRetentionReason::PrMergedIntoOtherBase),
            Some(pr.base),
        ));
    }
    match host.branch_tip(project, branch) {
        Ok(Some(tip)) if tip == pr.head_oid => host
            .delete_branch_at_tip(project, branch, &tip)
            .map(|deletion| (deletion, None)),
        Ok(Some(_)) => kept(BranchRetentionReason::PrMergedNewerCommits),
        Ok(None) => kept(BranchRetentionReason::BranchUnavailable),
        Err(_) => kept(BranchRetentionReason::NotMerged),
    }
}

/// One task's PR to merge: captured from the domain so the merge can run off the event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeRow {
    pub task_id: uuid::Uuid,
    pub number: u64,
    pub project: std::path::PathBuf,
    pub record: Dispatch,
}

/// A merged PR: which one, how, and whether the method's merge left its branch on the remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    pub pr: PullRequest,
    pub method: MergeMethod,
    /// `Some(true)`: the remote branch is still there (tsk never deletes it); `None`: unknown.
    pub remote_branch_left: Option<bool>,
}

impl MergeOutcome {
    /// The status-row and CLI note about the remote branch, when it was left.
    pub fn remote_note(&self, record: &Dispatch) -> Option<String> {
        (self.remote_branch_left == Some(true)).then(|| {
            format!(
                "remote branch {}/{} left in place",
                record.base_remote.as_deref().unwrap_or("origin"),
                record.branch
            )
        })
    }
}

/// Why a merge did not happen. Nothing was changed on GitHub, the task or the worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeError {
    Refused(MergeRefusal),
    /// The local branch is gone.
    BranchUnavailable,
    /// GitHub's PR head is not the local branch tip: unpushed commits, or commits pushed
    /// from elsewhere. Merging would merge something other than what is checked out.
    HeadMismatch,
    /// `gh pr merge` refused or failed: its reason, on one line.
    Failed(String),
    /// `gh pr merge` succeeded, but GitHub does not show the PR merged (a merge queue, or
    /// auto-merge waiting on checks): its state now. Nothing local changes until it is.
    NotMergedYet(String),
}

impl MergeError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Refused(refusal) => refusal.code(),
            Self::BranchUnavailable
            | Self::HeadMismatch
            | Self::Failed(_)
            | Self::NotMergedYet(_) => "merge-failed",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Refused(refusal) => refusal.message(),
            Self::BranchUnavailable => "branch no longer available".into(),
            Self::HeadMismatch => {
                "pull request head is not the local branch tip; push or pull first".into()
            }
            Self::Failed(reason) => reason.clone(),
            Self::NotMergedYet(state) => format!("queued or not merged yet: {state}"),
        }
    }
}

/// Merge one task's PR: recheck it is open against the recorded base and that its head is the
/// local tip, then `gh pr merge <branch> --<method> --match-head-commit <tip>`. Never
/// `--admin`, `--auto` or `--delete-branch`: GitHub's protections apply, and the local branch
/// is cleanup's to delete under its own tip check.
pub fn merge_row(
    host: &mut impl DispatchHost,
    row: &MergeRow,
    method: MergeMethod,
) -> Result<MergeOutcome, MergeError> {
    let (project, record) = (row.project.as_path(), &row.record);
    let lookup = host.pull_request(project, &record.branch);
    let pr = mergeable(&lookup, record)
        .map_err(MergeError::Refused)?
        .clone();
    let tip = match host.branch_tip(project, &record.branch) {
        Ok(Some(tip)) => tip,
        Ok(None) => return Err(MergeError::BranchUnavailable),
        Err(reason) => return Err(MergeError::Failed(reason)),
    };
    if tip != pr.head_oid {
        return Err(MergeError::HeadMismatch);
    }
    host.merge_pull_request(project, &record.branch, method, &tip)
        .map_err(MergeError::Failed)?;
    // A successful gh is not proof: a merge queue or auto-merge accepts the request and
    // merges later, or never. Only a PR GitHub shows merged goes on to done and cleanup.
    let pr = match host.pull_request(project, &record.branch) {
        Ok(Some(merged)) if merged.state == PrState::Merged => merged,
        Ok(Some(other)) => return Err(MergeError::NotMergedYet(other.state.name().into())),
        Ok(None) => return Err(MergeError::NotMergedYet("no pull request".into())),
        Err(error) => return Err(MergeError::NotMergedYet(error.message())),
    };
    let remote_branch_left = record.base_remote.as_deref().and_then(|remote| {
        host.remote_branch_exists(project, remote, &record.branch)
            .ok()
    });
    Ok(MergeOutcome {
        pr,
        method,
        remote_branch_left,
    })
}

/// A cleanup card's PR lookup, with the branch tip it was compared against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrProbeResult {
    pub lookup: PrLookup,
    pub tip: Option<String>,
}

impl PrProbeResult {
    /// GitHub merged this exact tip into `base` (the recorded base, as GitHub names it):
    /// cleanup deletes the branch though ancestry says unmerged.
    pub fn merged_at_tip(&self, base: &str) -> bool {
        matches!(
            (&self.lookup, &self.tip),
            (Ok(Some(pr)), Some(tip))
                if pr.state == PrState::Merged && &pr.head_oid == tip && pr.base == base
        )
    }
}

/// A cleanup card's PR lookup running off the event loop; the board polls it on idle ticks.
#[derive(Debug, Clone, Default)]
pub struct PrProbe(std::sync::Arc<std::sync::Mutex<Option<PrProbeResult>>>);

/// Identity, not content: two handles are equal when they watch the same lookup.
impl PartialEq for PrProbe {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for PrProbe {}

impl PrProbe {
    pub fn complete(&self, result: PrProbeResult) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(result);
        }
    }

    pub fn take(&self) -> Option<PrProbeResult> {
        self.0.try_lock().ok().and_then(|mut slot| slot.take())
    }
}

/// Look up the PR and the branch tip, for a cleanup card.
pub fn probe(host: &mut impl DispatchHost, project: &Path, branch: &str) -> PrProbeResult {
    PrProbeResult {
        lookup: host.pull_request(project, branch),
        tip: host.branch_tip(project, branch).ok().flatten(),
    }
}

#[derive(Debug, Default)]
struct MergeJobState {
    results: Vec<Option<Result<MergeOutcome, MergeError>>>,
    settled: bool,
}

/// A board `m`'s merges running off the event loop, one slot per row, in order.
#[derive(Debug, Clone, Default)]
pub struct MergeJob(std::sync::Arc<std::sync::Mutex<MergeJobState>>);

impl PartialEq for MergeJob {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for MergeJob {}

impl MergeJob {
    pub fn new(rows: usize) -> Self {
        let job = Self::default();
        if let Ok(mut state) = job.0.lock() {
            state.results = vec![None; rows];
            state.settled = rows == 0;
        }
        job
    }

    pub fn finish(&self, index: usize, result: Result<MergeOutcome, MergeError>) {
        if let Ok(mut state) = self.0.lock() {
            if let Some(slot) = state.results.get_mut(index) {
                *slot = Some(result);
            }
        }
    }

    pub fn settle(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.settled = true;
        }
    }

    /// Every row's result once all landed; `None` while merges run or the worker holds the lock.
    pub fn results(&self) -> Option<Vec<Result<MergeOutcome, MergeError>>> {
        let state = self.0.try_lock().ok()?;
        if !state.settled {
            return None;
        }
        state.results.iter().cloned().collect()
    }
}

/// Merge every row in order; one failure never stops the others.
pub fn run_merge_job(
    job: &MergeJob,
    rows: &[MergeRow],
    method: MergeMethod,
    host: &mut impl DispatchHost,
) {
    for (index, row) in rows.iter().enumerate() {
        job.finish(index, merge_row(host, row, method));
    }
    job.settle();
}

// --- The real `gh` ----------------------------------------------------------------------

fn gh(
    program: &Path,
    project: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(project)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1");
    crate::git_base::bounded_process_output(command, "gh", timeout)
}

/// The first non-empty line gh wrote, for a one-line reason.
fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no output")
        .to_string()
}

fn not_logged_in(code: Option<i32>, stderr: &str) -> bool {
    // gh exits 4 when authentication is required.
    code == Some(4) || stderr.contains("gh auth login")
}

/// Whether `program` can run: a path that exists, or a name on PATH (`gh.exe` on Windows).
/// Answered without spawning anything, so a card without gh opens exactly as before.
pub fn gh_available(program: &Path) -> bool {
    if program.components().count() > 1 {
        return program.is_file();
    }
    let names = if cfg!(windows) && program.extension().is_none() {
        vec![program.with_extension("exe")]
    } else {
        vec![program.to_path_buf()]
    };
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| names.iter().any(|name| dir.join(name).is_file()))
    })
}

pub(super) fn system_pull_request(program: &Path, project: &Path, branch: &str) -> PrLookup {
    if !gh_available(program) {
        return Err(PrLookupError::Missing);
    }
    let output = match gh(
        program,
        project,
        &[
            "pr",
            "view",
            branch,
            "--json",
            "number,url,state,headRefOid,baseRefName",
        ],
        GH_VIEW_TIMEOUT,
    ) {
        Ok(output) => output,
        Err(reason) if reason.starts_with("could not run gh") => {
            return Err(PrLookupError::Missing)
        }
        Err(reason) => return Err(PrLookupError::Failed(reason)),
    };
    pr_view_outcome(output.status.code(), &output.stdout, &output.stderr)
}

/// `gh pr view --json` output, read. Split out so the parsing is tested without gh.
pub fn pr_view_outcome(code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> PrLookup {
    if code != Some(0) {
        let text = String::from_utf8_lossy(stderr);
        if text.contains("no pull requests found") {
            return Ok(None);
        }
        if not_logged_in(code, &text) {
            return Err(PrLookupError::NotLoggedIn);
        }
        return Err(PrLookupError::Failed(first_line(stderr)));
    }
    let value: Value = serde_json::from_slice(stdout)
        .map_err(|error| PrLookupError::Failed(format!("unreadable gh output: {error}")))?;
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
    let state = match text("state").as_deref() {
        Some("OPEN") => PrState::Open,
        Some("MERGED") => PrState::Merged,
        Some("CLOSED") => PrState::Closed,
        other => {
            return Err(PrLookupError::Failed(format!(
                "unknown pull request state {}",
                other.unwrap_or("(none)")
            )))
        }
    };
    let missing = |key: &str| PrLookupError::Failed(format!("gh output has no {key}"));
    Ok(Some(PullRequest {
        number: value
            .get("number")
            .and_then(Value::as_u64)
            .ok_or_else(|| missing("number"))?,
        url: text("url").unwrap_or_default(),
        state,
        head_oid: text("headRefOid").ok_or_else(|| missing("headRefOid"))?,
        base: text("baseRefName").ok_or_else(|| missing("baseRefName"))?,
    }))
}

/// `gh pr merge`'s arguments: the branch, never a PR number; the configured method; and the
/// exact head GitHub must still have. No `--admin`, `--auto` or `--delete-branch`, ever.
pub fn merge_args(branch: &str, method: MergeMethod, head: &str) -> Vec<String> {
    vec![
        "pr".into(),
        "merge".into(),
        branch.into(),
        format!("--{}", method.name()),
        "--match-head-commit".into(),
        head.into(),
    ]
}

pub(super) fn system_merge_pull_request(
    program: &Path,
    project: &Path,
    branch: &str,
    method: MergeMethod,
    head: &str,
) -> Result<(), String> {
    let args = merge_args(branch, method, head);
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = gh(program, project, &args, GH_MERGE_TIMEOUT).map_err(|reason| {
        if reason.starts_with("could not run gh") {
            PrLookupError::Missing.message()
        } else {
            reason
        }
    })?;
    if output.status.success() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&output.stderr);
    if not_logged_in(output.status.code(), &text) {
        return Err(PrLookupError::NotLoggedIn.message());
    }
    Err(first_line(&output.stderr))
}

/// Whether `branch` is still on `remote`, asked over the network under the fetch bound.
pub(super) fn system_remote_branch_exists(
    project: &Path,
    remote: &str,
    branch: &str,
) -> Result<bool, String> {
    let output = crate::git_base::git_process_output_timeout(
        project,
        &[
            "ls-remote",
            "--exit-code",
            "--heads",
            remote,
            &format!("refs/heads/{branch}"),
        ],
        crate::git_base::NETWORK_TIMEOUT,
    )?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(2) => Ok(false),
        _ => Err(first_line(&output.stderr)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_merge_method_passes_its_own_flag_and_never_a_bypass() {
        for (method, flag) in [
            (MergeMethod::Merge, "--merge"),
            (MergeMethod::Squash, "--squash"),
            (MergeMethod::Rebase, "--rebase"),
        ] {
            let args = merge_args("tsk/t1-x", method, "abc");
            assert_eq!(
                args,
                [
                    "pr",
                    "merge",
                    "tsk/t1-x",
                    flag,
                    "--match-head-commit",
                    "abc"
                ]
            );
            for forbidden in ["--admin", "--auto", "--delete-branch", "-d"] {
                assert!(!args.iter().any(|arg| arg == forbidden), "{forbidden}");
            }
        }
    }

    #[test]
    fn pr_view_output_reads_each_state_and_the_failures() {
        let json = br#"{"number":7,"url":"https://github.com/o/r/pull/7","state":"MERGED","headRefOid":"abc","baseRefName":"main"}"#;
        let pr = pr_view_outcome(Some(0), json, b"").unwrap().unwrap();
        assert_eq!(pr.state, PrState::Merged);
        assert_eq!(
            (pr.number, pr.head_oid.as_str(), pr.base.as_str()),
            (7, "abc", "main")
        );
        assert_eq!(
            pr_view_outcome(Some(1), b"", b"no pull requests found for branch \"x\"\n"),
            Ok(None)
        );
        assert_eq!(
            pr_view_outcome(
                Some(4),
                b"",
                b"To get started with GitHub CLI, please run:  gh auth login\n"
            ),
            Err(PrLookupError::NotLoggedIn)
        );
        assert_eq!(
            pr_view_outcome(Some(1), b"", b"\nGraphQL: something broke\nmore\n"),
            Err(PrLookupError::Failed("GraphQL: something broke".into()))
        );
    }

    fn record(base_ref: &str, remote: Option<&str>) -> Dispatch {
        Dispatch {
            argv: Vec::new(),
            worktree: "/tmp/w".into(),
            branch: "tsk/t1-x".into(),
            base: Some("origin/main".into()),
            base_ref: Some(base_ref.into()),
            base_commit: None,
            base_remote: remote.map(str::to_string),
            herdr_workspace_id: "w1".into(),
            at: std::time::SystemTime::UNIX_EPOCH,
            cleaned: false,
        }
    }

    #[test]
    fn the_recorded_base_compares_as_the_branch_github_names() {
        assert_eq!(
            record_base_branch(&record("refs/remotes/origin/dispatch", Some("origin"))).as_deref(),
            Some("dispatch")
        );
        assert_eq!(
            record_base_branch(&record("refs/remotes/upstream/main", None)).as_deref(),
            Some("main")
        );
        assert_eq!(
            record_base_branch(&record("refs/heads/feature/x", None)).as_deref(),
            Some("feature/x")
        );
    }

    #[test]
    fn only_an_open_pr_into_the_recorded_base_is_mergeable() {
        let record = record("refs/remotes/origin/main", Some("origin"));
        let pr = |state, base: &str| {
            Ok(Some(PullRequest {
                number: 1,
                url: String::new(),
                state,
                head_oid: "abc".into(),
                base: base.into(),
            }))
        };
        assert!(mergeable(&pr(PrState::Open, "main"), &record).is_ok());
        assert_eq!(
            mergeable(&pr(PrState::Open, "dev"), &record)
                .unwrap_err()
                .code(),
            "pr-base-mismatch"
        );
        assert_eq!(
            mergeable(&pr(PrState::Closed, "main"), &record).unwrap_err(),
            MergeRefusal::NotOpen(PrState::Closed)
        );
        assert_eq!(
            mergeable(&pr(PrState::Merged, "main"), &record)
                .unwrap_err()
                .code(),
            "pr-not-open"
        );
        assert_eq!(
            mergeable(&Ok(None), &record).unwrap_err(),
            MergeRefusal::NoPr
        );
        assert_eq!(
            mergeable(&Err(PrLookupError::Missing), &record)
                .unwrap_err()
                .code(),
            "gh-unavailable"
        );
    }
}
