//! The cleanup card's words: each row's verdict and each option's sentence, built from what
//! the card inspected. The painter, the keys and the mouse all read the same option list, so
//! a line on the card is exactly what its key does.

use super::model::{CleanupPrompt, CleanupRow};
use super::selector::{CardChoice, CardOption, OptionCard, OptionSelector};
use crate::agents::MergeMethod;
use crate::dispatch::pr::{self, MergeRefusal, PrLookupError, PrState};

/// Past this many tasks an option line counts them instead of naming each one.
const NAMED_TASKS: usize = 6;

/// One of the card's choices, by its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupChoice {
    /// `m`: merge the open pull request first.
    Merge,
    /// `y`: mark done and tidy up.
    Clean,
    /// `n`: mark done, keep everything.
    Keep,
}

impl CardChoice for CleanupChoice {
    fn key(self) -> &'static str {
        match self {
            Self::Merge => "m",
            Self::Clean => "y",
            Self::Keep => "n",
        }
    }

    fn intent(self) -> crate::ui::input::BoardIntent {
        use crate::ui::input::BoardIntent;
        match self {
            Self::Merge => BoardIntent::MergeCleanup,
            Self::Clean => BoardIntent::ConfirmCleanup,
            Self::Keep => BoardIntent::KeepCleanup,
        }
    }
}

/// One option line: its key, what it does to the PR or branch and then locally, and whether
/// it is the card's likely choice (bold).
pub type CleanupOption = CardOption<CleanupChoice>;

/// Where one card row stands, which decides its verdict and which options touch it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowVerdict {
    /// Uncommitted work: nothing but `n` touches it.
    Uncommitted,
    /// The pull-request lookup is still running.
    CheckingPr,
    /// Merged and confirmed: `y` deletes the branch. The PR number when GitHub said so.
    Merged { pr: Option<u64> },
    /// An open pull request into the recorded base: `m` merges it.
    OpenPr { pr: u64 },
    /// The background merged check is still running.
    CheckingMerge,
    /// GitHub merged the PR, but the branch stays: the whole sentence why.
    MergedKept(String),
    /// Nothing confirms the merge either way: why.
    Unconfirmed(String),
    /// Not merged, with what is known about its pull request.
    NotMerged(Option<String>),
}

impl CleanupRow {
    /// The recorded base as GitHub names it (`main`), or the recorded name.
    pub fn base_name(&self) -> String {
        self.inspected
            .as_ref()
            .and_then(pr::record_base_branch)
            .unwrap_or_else(|| self.base.clone())
    }

    fn landed_pr(&self) -> Option<&pr::PullRequest> {
        match self.pr.landed.as_ref().map(|landed| &landed.lookup) {
            Some(Ok(Some(pr))) => Some(pr),
            _ => None,
        }
    }

    /// The number of the PR the lookup found, in any state.
    pub fn pr_number(&self) -> Option<u64> {
        self.landed_pr().map(|pr| pr.number)
    }

    /// An open PR into the recorded base, whatever the worktree holds.
    fn open_pr(&self) -> Option<u64> {
        let (landed, record) = (self.pr.landed.as_ref()?, self.inspected.as_ref()?);
        pr::mergeable(&landed.lookup, record)
            .ok()
            .map(|pr| pr.number)
    }

    pub fn verdict(&self) -> RowVerdict {
        if self.dirty {
            return RowVerdict::Uncommitted;
        }
        if self.pr.probe.is_some() {
            return RowVerdict::CheckingPr;
        }
        // Cleanup never deletes a branch another worktree has checked out, merged or not.
        if self.checked_out_elsewhere {
            return RowVerdict::Unconfirmed("branch checked out in another worktree".into());
        }
        if self.branch_deletable() {
            let pr = self
                .landed_pr()
                .filter(|pr| pr.state == PrState::Merged)
                .map(|pr| pr.number);
            return RowVerdict::Merged { pr };
        }
        if let Some(pr) = self.open_pr() {
            return RowVerdict::OpenPr { pr };
        }
        if self.checking() {
            return RowVerdict::CheckingMerge;
        }
        let base = self.base_name();
        if let Some(pr) = self.landed_pr().filter(|pr| pr.state == PrState::Merged) {
            if pr.base != base {
                return RowVerdict::MergedKept(format!(
                    "PR #{} merged into {}, not {base}: the branch is kept",
                    pr.number, pr.base
                ));
            }
            let tip = self
                .pr
                .landed
                .as_ref()
                .and_then(|landed| landed.tip.as_ref());
            if tip.is_some_and(|tip| *tip != pr.head_oid) {
                return RowVerdict::MergedKept(format!(
                    "PR #{} merged, but the branch has newer commits: the branch is kept",
                    pr.number
                ));
            }
        }
        if let Some(reason) = self.unconfirmed_reason() {
            return RowVerdict::Unconfirmed(reason);
        }
        RowVerdict::NotMerged(self.not_merged_cause())
    }

    fn unconfirmed_reason(&self) -> Option<String> {
        if self.no_recorded_base() {
            Some("no recorded base".into())
        } else if self.check_failed {
            Some(
                if self
                    .warning
                    .as_deref()
                    .is_some_and(|warning| warning.contains("timed out"))
                {
                    "the check timed out".into()
                } else {
                    "the check failed".into()
                },
            )
        } else if self.unreachable_remote.is_some() {
            Some("offline (base fetch failed)".into())
        } else if !self.base_available {
            Some("base no longer available".into())
        } else {
            None
        }
    }

    /// What the pull-request lookup says about an unmerged branch. Without `gh`, the only
    /// hint left is that a squash merge reads as unmerged.
    fn not_merged_cause(&self) -> Option<String> {
        let (landed, record) = (self.pr.landed.as_ref()?, self.inspected.as_ref()?);
        let number = self.landed_pr().map_or(0, |pr| pr.number);
        match pr::mergeable(&landed.lookup, record) {
            Ok(_) => None,
            Err(MergeRefusal::GhUnavailable(PrLookupError::Missing)) => {
                Some("squash-merged? delete it by hand".into())
            }
            Err(MergeRefusal::GhUnavailable(PrLookupError::NotLoggedIn)) => {
                Some("gh is not logged in (gh auth login)".into())
            }
            Err(MergeRefusal::GhUnavailable(PrLookupError::NoGithubRemote)) => {
                Some("no GitHub remote".into())
            }
            Err(MergeRefusal::GhUnavailable(error)) => Some(error.message()),
            Err(MergeRefusal::NoPr) => Some("no pull request".into()),
            Err(MergeRefusal::NotOpen(state)) => Some(format!("PR #{number} {}", state.name())),
            Err(MergeRefusal::BaseMismatch { pr_base, base }) => {
                Some(format!("PR #{number} targets {pr_base}, not {base}"))
            }
        }
    }

    /// `3 uncommitted files`, and the first two names.
    fn uncommitted(&self) -> (String, Option<String>) {
        let count = self.dirty_files.len();
        if count == 0 {
            return ("uncommitted changes".into(), None);
        }
        let noun = if count == 1 { "file" } else { "files" };
        let mut names = self
            .dirty_files
            .iter()
            .take(2)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        if count > 2 {
            names.push_str(", …");
        }
        (format!("{count} uncommitted {noun}"), Some(names))
    }

    /// The single card's verdict line.
    pub fn verdict_line(&self) -> String {
        let base = self.base_name();
        match self.verdict() {
            RowVerdict::Uncommitted => {
                let open = self
                    .open_pr()
                    .map(|pr| format!("PR #{pr} is still open · "))
                    .unwrap_or_default();
                let it = if self.dirty_files.len() == 1 {
                    "it"
                } else {
                    "them"
                };
                let (count, names) = self.uncommitted();
                let names = names.map(|names| format!(" ({names})")).unwrap_or_default();
                format!(
                    "{open}{count} in the worktree{names}: commit or discard {it} to merge or tidy up"
                )
            }
            RowVerdict::CheckingPr => "checking pull request…".into(),
            RowVerdict::Merged { pr: Some(pr) } => format!("PR #{pr} merged into {base} ✓"),
            RowVerdict::Merged { pr: None } => format!("Merged into {base} ✓"),
            RowVerdict::OpenPr { pr } => format!("PR #{pr} is still open:"),
            RowVerdict::CheckingMerge => format!("Checking merge into {base}…"),
            RowVerdict::MergedKept(text) => text,
            RowVerdict::Unconfirmed(reason) => {
                format!("Can't tell if it's merged: {reason} · the branch is kept")
            }
            RowVerdict::NotMerged(Some(cause)) => format!("Not merged into {base} · {cause}"),
            RowVerdict::NotMerged(None) => format!("Not merged into {base}"),
        }
    }

    /// A marked-set row: only where the task stands now.
    pub fn row_line(&self) -> String {
        let base = self.base_name();
        match self.verdict() {
            RowVerdict::Uncommitted => match self.uncommitted() {
                (count, Some(names)) => format!("{count} · {names}"),
                (count, None) => count,
            },
            RowVerdict::CheckingPr | RowVerdict::CheckingMerge => "checking…".into(),
            RowVerdict::Merged { pr: Some(pr) } => format!("PR #{pr} merged into {base} ✓"),
            RowVerdict::Merged { pr: None } => format!("merged into {base} ✓"),
            RowVerdict::OpenPr { pr } => format!("PR #{pr} is still open"),
            RowVerdict::MergedKept(text) => text,
            RowVerdict::Unconfirmed(reason) => format!("can't tell if merged: {reason}"),
            RowVerdict::NotMerged(Some(cause)) => format!("not merged · {cause}"),
            RowVerdict::NotMerged(None) => format!("not merged into {base}"),
        }
    }

    /// Why a row the card leaves alone stays as it was, for the finished card.
    pub fn unchanged_reason(&self) -> String {
        if let Some(reason) = &self.pr.merge_failed {
            return format!("merge failed: {reason}");
        }
        match self.verdict() {
            RowVerdict::Uncommitted => self.uncommitted().0,
            RowVerdict::Unconfirmed(reason) => format!("can't tell if merged: {reason}"),
            _ => "not merged".into(),
        }
    }
}

/// The `m` verb for the configured method.
pub fn merge_verb(method: MergeMethod) -> &'static str {
    match method {
        MergeMethod::Merge => "merge",
        MergeMethod::Squash => "squash-merge",
        MergeMethod::Rebase => "rebase-merge",
    }
}

/// `a`, `a and b`, `a, b and c`.
pub(super) fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The tasks a line touches: named, or counted past [`NAMED_TASKS`].
pub(super) fn tasks(names: &[String]) -> String {
    if names.len() > NAMED_TASKS {
        format!("{} tasks", names.len())
    } else {
        and_list(names)
    }
}

/// `T8's branch`, `T7's and T9's branches`, or `the branches of 7 tasks`.
fn possessive(names: &[String], one: &str, many: &str) -> String {
    match names.len() {
        1 => format!("{}'s {one}", names[0]),
        count if count > NAMED_TASKS => format!("the {many} of {count} tasks"),
        _ => {
            let owners = names
                .iter()
                .map(|name| format!("{name}'s"))
                .collect::<Vec<_>>();
            format!("{} {many}", and_list(&owners))
        }
    }
}

/// `remove worktree, branch and agent pane`.
fn remove_list(branch: bool, pane: bool) -> String {
    let mut items = vec!["worktree".to_string()];
    if branch {
        items.push("branch".into());
    }
    if pane {
        items.push("agent pane".into());
    }
    format!("remove {}", and_list(&items))
}

/// The marked set's rows sorted by what the options do to them.
#[derive(Default)]
struct Buckets {
    open: Vec<(String, u64)>,
    merged: Vec<String>,
    other: Vec<String>,
    dirty: Vec<String>,
    /// Targets without a row (no dispatch, worktree gone, can't inspect): `y` and `n` mark
    /// them done, `m` leaves them.
    extra: Vec<String>,
    /// A tidied row still has its agent pane.
    pane: bool,
    checking: bool,
}

impl CleanupPrompt {
    /// The card's options, in key order `m` · `y` · `n`. None while a check or merge runs,
    /// once a run started, or after `m` went nothing on.
    pub fn options(&self) -> Vec<CleanupOption> {
        if self.merging() || self.settled.is_some() {
            return Vec::new();
        }
        let mut options = match &self.bulk {
            None => match self.rows.first() {
                Some(row) => self.single_options(row),
                None => Vec::new(),
            },
            Some(_) => self.bulk_options(),
        };
        if options.len() == 1 {
            options[0].bold = true;
        }
        options
    }

    fn single_options(&self, row: &CleanupRow) -> Vec<CleanupOption> {
        let option = |choice, text: String, bold| CleanupOption { choice, text, bold };
        let pane = row.workspace_exists;
        let keep = |prefix: &str| {
            option(
                CleanupChoice::Keep,
                format!("{prefix}mark done · keep everything"),
                false,
            )
        };
        match row.verdict() {
            RowVerdict::Uncommitted => vec![keep("don't merge · ")],
            RowVerdict::CheckingPr => Vec::new(),
            RowVerdict::OpenPr { pr } => vec![
                option(
                    CleanupChoice::Merge,
                    format!(
                        "{} PR #{pr} to {} · mark done · {}",
                        merge_verb(self.method),
                        row.base_name(),
                        remove_list(true, pane)
                    ),
                    true,
                ),
                option(
                    CleanupChoice::Clean,
                    format!(
                        "don't merge · mark done · {} · keep the branch and the PR open",
                        remove_list(false, pane)
                    ),
                    false,
                ),
                keep("don't merge · "),
            ],
            RowVerdict::Merged { .. } => vec![
                option(
                    CleanupChoice::Clean,
                    format!("mark done · {}", remove_list(true, pane)),
                    true,
                ),
                keep(""),
            ],
            RowVerdict::CheckingMerge => vec![
                option(
                    CleanupChoice::Clean,
                    format!(
                        "mark done · {} · delete the branch if merged",
                        remove_list(false, pane)
                    ),
                    false,
                ),
                keep(""),
            ],
            RowVerdict::NotMerged(_) => vec![
                option(
                    CleanupChoice::Clean,
                    format!(
                        "mark done · {} · keep the branch (not merged)",
                        remove_list(false, pane)
                    ),
                    false,
                ),
                keep(""),
            ],
            RowVerdict::MergedKept(_) | RowVerdict::Unconfirmed(_) => vec![
                option(
                    CleanupChoice::Clean,
                    format!("mark done · {} · keep the branch", remove_list(false, pane)),
                    false,
                ),
                keep(""),
            ],
        }
    }

    fn buckets(&self) -> Buckets {
        let mut buckets = Buckets::default();
        for row in &self.rows {
            let name = format!("T{}", row.number);
            let verdict = row.verdict();
            if !matches!(verdict, RowVerdict::Uncommitted) && row.workspace_exists {
                buckets.pane = true;
            }
            match verdict {
                RowVerdict::Uncommitted => buckets.dirty.push(name),
                RowVerdict::CheckingPr | RowVerdict::CheckingMerge => buckets.checking = true,
                RowVerdict::OpenPr { pr } => buckets.open.push((name, pr)),
                RowVerdict::Merged { .. } => buckets.merged.push(name),
                RowVerdict::MergedKept(_)
                | RowVerdict::Unconfirmed(_)
                | RowVerdict::NotMerged(_) => buckets.other.push(name),
            }
        }
        if let Some(bulk) = &self.bulk {
            buckets.extra.extend(bulk.plain.iter().cloned());
            buckets
                .extra
                .extend(bulk.missing.iter().map(|(_, name, _)| name.clone()));
            buckets
                .extra
                .extend(bulk.refused.iter().map(|(name, _)| name.clone()));
        }
        buckets
    }

    fn bulk_options(&self) -> Vec<CleanupOption> {
        let b = self.buckets();
        if b.checking {
            return Vec::new();
        }
        let total = b.open.len() + b.merged.len() + b.other.len() + b.dirty.len() + b.extra.len();
        let done_phrase = |names: &[String]| {
            if names.len() == total && total > 1 {
                format!("mark all {total} done")
            } else {
                format!("mark {} done", tasks(names))
            }
        };
        let open_names = b
            .open
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        let mut options = Vec::new();
        if !b.open.is_empty() {
            let prs = if b.open.len() > NAMED_TASKS {
                format!("{} PRs", b.open.len())
            } else {
                and_list(
                    &b.open
                        .iter()
                        .map(|(name, pr)| format!("PR #{pr} ({name})"))
                        .collect::<Vec<_>>(),
                )
            };
            let done = [open_names.clone(), b.merged.clone()].concat();
            let one = done.len() == 1;
            let mut items = vec![if one { "worktree" } else { "worktrees" }.to_string()];
            items.push(if one { "branch" } else { "branches" }.into());
            if b.pane {
                items.push(if one { "agent pane" } else { "agent panes" }.into());
            }
            let owner = if one { "its" } else { "their" };
            let mut parts = vec![
                format!("{} {prs}", merge_verb(self.method)),
                done_phrase(&done),
                format!("remove {owner} {}", and_list(&items)),
            ];
            let unchanged = [b.other.clone(), b.dirty.clone(), b.extra.clone()].concat();
            if !unchanged.is_empty() {
                parts.push(format!("{} unchanged", tasks(&unchanged)));
            }
            options.push(CleanupOption {
                choice: CleanupChoice::Merge,
                text: parts.join(" · "),
                bold: true,
            });
        }
        let tidy = [open_names.clone(), b.merged.clone(), b.other.clone()].concat();
        if !tidy.is_empty() {
            let done = [tidy.clone(), b.extra.clone()].concat();
            let mut parts = Vec::new();
            if !b.open.is_empty() {
                parts.push("don't merge".to_string());
            }
            parts.push(done_phrase(&done));
            let one = tidy.len() == 1;
            let things = match (one, b.pane) {
                (true, true) => "worktree and agent pane",
                (true, false) => "worktree",
                (false, true) => "worktrees and agent panes",
                (false, false) => "worktrees",
            };
            parts.push(if tidy == done {
                format!("remove {} {things}", if one { "its" } else { "their" })
            } else if one {
                format!("remove {}'s {things}", tidy[0])
            } else if tidy.len() > NAMED_TASKS {
                format!("remove the {things} of {} tasks", tidy.len())
            } else {
                format!(
                    "remove {} {things}",
                    and_list(
                        &tidy
                            .iter()
                            .map(|name| format!("{name}'s"))
                            .collect::<Vec<_>>()
                    )
                )
            });
            if !b.merged.is_empty() {
                parts.push(format!(
                    "delete {}",
                    possessive(&b.merged, "branch", "branches")
                ));
            }
            let kept = [open_names, b.other.clone()].concat();
            if !kept.is_empty() {
                parts.push(format!("keep {}", possessive(&kept, "branch", "branches")));
            }
            if !b.dirty.is_empty() {
                parts.push(format!("{} unchanged", tasks(&b.dirty)));
            }
            options.push(CleanupOption {
                choice: CleanupChoice::Clean,
                text: parts.join(" · "),
                bold: b.open.is_empty() && !b.merged.is_empty(),
            });
        }
        let everyone = [tidy, b.dirty.clone(), b.extra.clone()].concat();
        options.push(CleanupOption {
            choice: CleanupChoice::Keep,
            text: format!("{} · keep everything", done_phrase(&everyone)),
            bold: false,
        });
        options
    }
}

impl OptionCard for CleanupPrompt {
    type Choice = CleanupChoice;

    fn options(&self) -> Vec<CleanupOption> {
        CleanupPrompt::options(self)
    }

    fn selector(&self) -> &OptionSelector<CleanupChoice> {
        &self.selector
    }

    fn selector_mut(&mut self) -> &mut OptionSelector<CleanupChoice> {
        &mut self.selector
    }

    fn scroll_follow(&mut self) -> (&mut usize, &mut bool) {
        (&mut self.scroll, &mut self.follow)
    }

    /// While a lookup still runs the card shows no option, yet `y` and `n` work as before it
    /// landed.
    fn accepts(&self, choice: CleanupChoice) -> bool {
        if self.merging() || self.settled.is_some() {
            return false;
        }
        let options = self.options();
        if options.is_empty() {
            return match choice {
                CleanupChoice::Merge => false,
                CleanupChoice::Clean => self.can_clean_any(),
                CleanupChoice::Keep => true,
            };
        }
        options.iter().any(|option| option.choice == choice)
    }
}
