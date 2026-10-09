//! Headless human-status changes.

use std::path::PathBuf;

use crate::cli::parser::{BlockFlags, TaskAddress};
use crate::domain::{
    actor_from_env, BlockDraft, BlockField, BlockOn, BlockPatch, DomainError, DomainState,
    HumanStatus,
};
use crate::store::{default_state_dir, TaskStore};
use uuid::Uuid;

/// A successful status change, including an idempotent repeat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusResult {
    pub number: u64,
    pub title: String,
    pub status: HumanStatus,
}

/// A status failure after parsing and before presenting output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusError {
    UnknownTask,
    SoftDeletedTask,
    /// A block text is longer than the cap.
    TextTooLong(BlockField),
    /// `--on` names the task itself or a task that is not on the board.
    InvalidBlocker,
    Store(String),
}

impl StatusError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownTask => "unknown-task",
            Self::SoftDeletedTask => "soft-deleted-task",
            Self::TextTooLong(_) => "text-too-long",
            Self::InvalidBlocker => "invalid-blocker",
            Self::Store(_) => "store-error",
        }
    }
}

/// Set one task's human status. Repeating the same status is idempotent.
///
/// With `blocked`, the block flags open a block, or edit the task's open block in place.
/// Soft-deleted and unknown tasks refuse the same way as archive.
pub fn run(
    target: TaskAddress,
    status: HumanStatus,
    block: BlockFlags,
    state_dir: Option<PathBuf>,
) -> Result<StatusResult, StatusError> {
    let store = TaskStore::new(state_dir.unwrap_or_else(default_state_dir));
    let actor = actor_from_env();
    store
        .locked_transition_if_changed(|state: &mut DomainState| {
            let found = state
                .tasks()
                .iter()
                .find(|task| target.matches(task))
                .map(|task| Found {
                    id: task.id,
                    number: task.number,
                    title: task.title.clone(),
                    current: task.status,
                    soft_deleted: task.soft_deleted,
                    has_block: task.block.is_some(),
                });
            Ok(apply(state, found, status, &block, &actor))
        })
        .map_err(StatusError::Store)?
}

struct Found {
    id: Uuid,
    number: Option<u64>,
    title: String,
    current: HumanStatus,
    soft_deleted: bool,
    has_block: bool,
}

fn apply(
    state: &mut DomainState,
    found: Option<Found>,
    status: HumanStatus,
    flags: &BlockFlags,
    actor: &str,
) -> (Result<StatusResult, StatusError>, bool) {
    let Some(found) = found else {
        return (Err(StatusError::UnknownTask), false);
    };
    let Some(number) = found.number else {
        return (Err(StatusError::UnknownTask), false);
    };
    if found.soft_deleted {
        return (Err(StatusError::SoftDeletedTask), false);
    }
    let result = StatusResult {
        number,
        title: found.title,
        status,
    };
    let outcome = if status == HumanStatus::Blocked && !flags.is_empty() {
        let draft = match block_draft(state, flags, number) {
            Ok(draft) => draft,
            Err(error) => return (Err(error), false),
        };
        if found.has_block {
            state.edit_block(found.id, patch_from(flags, draft))
        } else {
            state.block(found.id, draft, actor).map(|()| true)
        }
    } else if found.current == status {
        Ok(false)
    } else {
        state.set_status_by(found.id, status, actor).map(|()| true)
    };
    match outcome {
        Ok(changed) => (Ok(result), changed),
        Err(DomainError::UnknownId(_)) => (Err(StatusError::UnknownTask), false),
        Err(other) => (Err(StatusError::Store(other.to_string())), false),
    }
}

/// Validate the flags into a draft. `--on` must name another task on the board.
fn block_draft(
    state: &DomainState,
    flags: &BlockFlags,
    own_number: u64,
) -> Result<BlockDraft, StatusError> {
    let on = flags
        .on
        .as_deref()
        .map(BlockOn::parse_input)
        .unwrap_or_default();
    if let BlockOn::Task(number) = on {
        let exists = state
            .tasks()
            .iter()
            .any(|task| task.number == Some(number) && !task.soft_deleted && !task.is_notice());
        if number == own_number || !exists {
            return Err(StatusError::InvalidBlocker);
        }
    }
    BlockDraft::from_input(
        flags.why.as_deref(),
        flags.needs.as_deref(),
        &flags.options,
        on,
    )
    .map_err(StatusError::TextTooLong)
}

/// Re-blocking replaces only the fields that were given.
fn patch_from(flags: &BlockFlags, draft: BlockDraft) -> BlockPatch {
    BlockPatch {
        why: flags.why.is_some().then_some(draft.why),
        needs: flags.needs.is_some().then_some(draft.needs),
        options: (!flags.options.is_empty()).then_some(draft.options),
        on: flags.on.is_some().then_some(draft.on),
    }
}
