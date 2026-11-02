//! Headless `tsk check`: set one check of a task's open review round.
//!
//! Saves through `DomainState::set_check`, like the board's `Enter` on a check, and adds no
//! undo entry, matching the board.

use std::path::PathBuf;

use crate::cli::parser::TaskAddress;
use crate::domain::{CheckState, DomainError, DomainState};
use crate::store::{default_state_dir, TaskStore};

/// A set check: the task and the check's 1-based position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub number: u64,
    pub position: usize,
    pub state: CheckState,
}

/// A check failure after parsing and before presenting output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    UnknownTask,
    SoftDeletedTask,
    NotInReview,
    UnknownCheck,
    Store(String),
}

impl CheckError {
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

/// Set check `position` (1-based) of the task's open review round to `state`. Setting the
/// state it already has saves nothing and succeeds. Refusals persist nothing.
pub fn run(
    target: TaskAddress,
    position: usize,
    state: CheckState,
    state_dir: Option<PathBuf>,
) -> Result<CheckResult, CheckError> {
    let store = TaskStore::new(state_dir.unwrap_or_else(default_state_dir));
    store
        .locked_transition_if_changed(|domain| {
            let outcome = apply(domain, target, position, state);
            let changed = outcome.as_ref().is_ok_and(|(_, changed)| *changed);
            Ok((outcome.map(|(result, _)| result), changed))
        })
        .map_err(CheckError::Store)?
}

fn apply(
    domain: &mut DomainState,
    target: TaskAddress,
    position: usize,
    state: CheckState,
) -> Result<(CheckResult, bool), CheckError> {
    let task = domain
        .tasks()
        .iter()
        .find(|task| target.matches(task))
        .ok_or(CheckError::UnknownTask)?;
    if task.soft_deleted {
        return Err(CheckError::SoftDeletedTask);
    }
    let (id, number) = (task.id, task.number.unwrap_or_default());
    let index = position.checked_sub(1).ok_or(CheckError::UnknownCheck)?;
    let changed = domain
        .set_check(id, index, state)
        .map_err(map_domain_error)?;
    Ok((
        CheckResult {
            number,
            position,
            state,
        },
        changed,
    ))
}

fn map_domain_error(error: DomainError) -> CheckError {
    match error {
        DomainError::UnknownId(_) => CheckError::UnknownTask,
        DomainError::NotInReview(_) => CheckError::NotInReview,
        DomainError::UnknownCheck(_) => CheckError::UnknownCheck,
        other => CheckError::Store(other.to_string()),
    }
}
