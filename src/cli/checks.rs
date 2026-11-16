//! Headless check marking: set one check of a task's open review round by its position.

use std::path::PathBuf;

use uuid::Uuid;

use crate::cli::parser::TaskAddress;
use crate::domain::{CheckState, DomainError, DomainState};
use crate::store::{default_state_dir, TaskStore};

/// The `checks` action parsed from argv.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksAction {
    /// Set the check at `position` (1-based, in `tsk list <task> --json` order) to `state`.
    Set { position: usize, state: CheckState },
}

/// A checks failure after parsing and before presenting output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChecksError {
    UnknownTask,
    SoftDeletedTask,
    NotInReview,
    UnknownCheck,
    Store(String),
}

impl ChecksError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownTask => "unknown-task",
            Self::SoftDeletedTask => "soft-deleted-task",
            Self::NotInReview => "not-in-review",
            Self::UnknownCheck => "unknown-check",
            Self::Store(_) => "store-error",
        }
    }
}

/// A check after a successful `set`, whether or not its state changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecksResult {
    pub position: usize,
    pub text: String,
    pub state: CheckState,
}

/// Apply one checks action under the store lock. A refusal, or a check already in the asked
/// state, persists nothing.
pub fn run(
    task_address: TaskAddress,
    action: ChecksAction,
    state_dir: Option<PathBuf>,
) -> Result<ChecksResult, ChecksError> {
    let store = TaskStore::new(state_dir.unwrap_or_else(default_state_dir));
    store
        .locked_transition_if_changed(|domain| {
            let task_id = domain
                .tasks()
                .iter()
                .find(|task| task_address.matches(task))
                .map(|task| task.id);
            let outcome = task_id.map_or(Err(ChecksError::UnknownTask), |task_id| {
                apply(domain, task_id, action)
            });
            let changed = outcome.as_ref().is_ok_and(|(_, changed)| *changed);
            Ok((outcome.map(|(result, _)| result), changed))
        })
        .map_err(ChecksError::Store)?
}

fn apply(
    domain: &mut DomainState,
    task_id: Uuid,
    action: ChecksAction,
) -> Result<(ChecksResult, bool), ChecksError> {
    let ChecksAction::Set { position, state } = action;
    let task = domain.get(task_id).ok_or(ChecksError::UnknownTask)?;
    if task.soft_deleted {
        return Err(ChecksError::SoftDeletedTask);
    }
    // The parser refuses 0, so `position - 1` is the check's index.
    let index = position - 1;
    let changed = domain
        .set_check(task_id, index, state)
        .map_err(|error| match error {
            DomainError::NotInReview(_) => ChecksError::NotInReview,
            DomainError::UnknownCheck(_) => ChecksError::UnknownCheck,
            DomainError::UnknownId(_) => ChecksError::UnknownTask,
            other => ChecksError::Store(other.to_string()),
        })?;
    let text = domain
        .get(task_id)
        .and_then(|task| task.block.as_ref())
        .and_then(|block| block.checks.get(index))
        .map(|check| check.text.clone())
        .expect("a check that was just set exists");
    Ok((
        ChecksResult {
            position,
            text,
            state,
        },
        changed,
    ))
}
