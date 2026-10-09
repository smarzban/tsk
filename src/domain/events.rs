//! Provenance origin and append-only task event history.

use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// How a task was created. Allowed set includes at least these three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceOrigin {
    Manual,
    Capture,
    Selection,
}

/// Kind of domain mutation recorded on a task's event history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEventKind {
    Created,
    Edited,
    StatusSet,
    Completed,
    Reopened,
    SoftDeleted,
    Restored,
    Archived,
    Unarchived,
    StepAdded,
    StepChecked,
    StepUnchecked,
    StepRenamed,
    StepRemoved,
    Assigned,
    BaseSet,
    Dispatched,
    Cleaned,
    BlockEdited,
    Replied,
    ReplyEdited,
    ReplyDeleted,
    ReviewEdited,
    CheckSet,
}

/// One append-only history record on a task: what happened, when, who did it, and what
/// changed. Events from a v9 or older store carry no `by` and no `detail`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskEvent {
    pub kind: TaskEventKind,
    #[serde(with = "super::time_serde")]
    pub at: SystemTime,
    /// `you`, or the agent profile named by `TSK_AGENT` that made the change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// What changed, for the kinds that carry it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<EventDetail>,
}

impl TaskEvent {
    /// An event with no author or detail, as an older store recorded it.
    pub fn new(kind: TaskEventKind, at: SystemTime) -> Self {
        Self {
            kind,
            at,
            by: None,
            detail: None,
        }
    }
}

/// The task field an `edited` event changed. Only which field, never the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditedField {
    Title,
    Notes,
    Thread,
    Project,
}

/// What a cleanup did with the dispatch's worktree and branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupOutcome {
    /// The worktree and its merged branch were removed.
    Removed,
    /// The worktree was removed and the branch kept.
    BranchKept,
    /// The worktree was already gone; the record converged to cleaned.
    Missing,
}

/// Per-kind detail of one event. Every field is optional, so a kind fills only its own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventDetail {
    /// Status before and after a status change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<super::HumanStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<super::HumanStatus>,
    /// The assignee an `assigned` event set; absent on an `assigned` event means unassigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// The base a `base_set` event set (absent: cleared to the default), or the ref a dispatch
    /// started from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// The dispatch branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The short commit a dispatch started from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    /// A dispatch that replaced an earlier one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub relaunch: bool,
    /// What a cleanup did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<CleanupOutcome>,
    /// The fields an `edited` event changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<EditedField>,
}

impl EventDetail {
    /// A status change from `from` to `to`.
    pub fn status(from: super::HumanStatus, to: super::HumanStatus) -> Self {
        Self {
            from: Some(from),
            to: Some(to),
            ..Self::default()
        }
    }
}

thread_local! {
    static ACTOR: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Who this thread's domain mutations are recorded as: `you` unless a caller is acting as an
/// agent through [`acting_as`].
pub fn current_actor() -> String {
    ACTOR
        .with(|actor| actor.borrow().clone())
        .unwrap_or_else(|| super::OWNER.to_string())
}

/// Run `work` with this thread's mutations recorded as made by `by`. The CLI wraps every verb in
/// it with [`super::actor_from_env`]; the board never does, so its changes are always `you`.
pub fn acting_as<T>(by: &str, work: impl FnOnce() -> T) -> T {
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            ACTOR.with(|actor| *actor.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(ACTOR.with(|actor| actor.borrow_mut().replace(by.to_string())));
    work()
}
