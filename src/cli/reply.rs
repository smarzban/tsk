//! Headless replies to a task's open block.

use std::path::PathBuf;

use uuid::Uuid;

use crate::cli::parser::TaskAddress;
use crate::domain::{actor_from_env, DomainError, DomainState};
use crate::store::{default_state_dir, TaskStore};

/// A stored reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyResult {
    pub number: u64,
    pub title: String,
    pub by: String,
}

/// A reply failure after parsing and before presenting output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyError {
    UnknownTask,
    SoftDeletedTask,
    NotBlocked,
    EmptyReply,
    TextTooLong,
    Store(String),
}

impl ReplyError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownTask => "unknown-task",
            Self::SoftDeletedTask => "soft-deleted-task",
            Self::NotBlocked => "not-blocked",
            Self::EmptyReply => "empty-reply",
            Self::TextTooLong => "text-too-long",
            Self::Store(_) => "store-error",
        }
    }
}

/// Add a reply to the task's open block. The author is the dispatched agent named by
/// `TSK_AGENT`, or `you`. Not idempotent: each run adds a reply.
pub fn run(
    target: TaskAddress,
    text: &str,
    state_dir: Option<PathBuf>,
) -> Result<ReplyResult, ReplyError> {
    let store = TaskStore::new(state_dir.unwrap_or_else(default_state_dir));
    let by = actor_from_env();
    store
        .locked_transition_if_changed(|state: &mut DomainState| {
            let found = state
                .tasks()
                .iter()
                .find(|task| target.matches(task))
                .map(|task| (task.id, task.number, task.title.clone(), task.soft_deleted));
            Ok(apply(state, found, text, &by))
        })
        .map_err(ReplyError::Store)?
}

fn apply(
    state: &mut DomainState,
    found: Option<(Uuid, Option<u64>, String, bool)>,
    text: &str,
    by: &str,
) -> (Result<ReplyResult, ReplyError>, bool) {
    let Some((id, Some(number), title, soft_deleted)) = found else {
        return (Err(ReplyError::UnknownTask), false);
    };
    if soft_deleted {
        return (Err(ReplyError::SoftDeletedTask), false);
    }
    match state.reply(id, text, by) {
        Ok(_) => (
            Ok(ReplyResult {
                number,
                title,
                by: by.to_string(),
            }),
            true,
        ),
        Err(DomainError::NotBlocked(_)) => (Err(ReplyError::NotBlocked), false),
        Err(DomainError::EmptyReply) => (Err(ReplyError::EmptyReply), false),
        Err(DomainError::TextTooLong(_)) => (Err(ReplyError::TextTooLong), false),
        Err(DomainError::UnknownId(_)) => (Err(ReplyError::UnknownTask), false),
        Err(other) => (Err(ReplyError::Store(other.to_string())), false),
    }
}
