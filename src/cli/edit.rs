//! Headless title and notes edits.

use std::path::PathBuf;

use crate::agents::AgentProfiles;
use crate::cli::add::has_c0_control;
use crate::cli::parser::TaskAddress;
use crate::domain::{DomainError, DomainState, TaskScope};
use crate::store::{default_state_dir, TaskStore};
use uuid::Uuid;

/// Parsed field values after the argv boundary. `None` means leave the stored value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditFields {
    pub title: Option<String>,
    pub notes: Option<String>,
    /// `None` leaves it unchanged, `Some(None)` clears it.
    pub assignee: Option<Option<String>>,
    /// `None` leaves it unchanged, `Some(None)` clears it.
    pub base: Option<Option<String>>,
    /// `None` leaves it unchanged; `Some` replaces the list (empty clears it).
    pub after: Option<Vec<u64>>,
}

/// A successful edit, including an idempotent repeat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditResult {
    pub number: u64,
    pub title: String,
}

/// An edit failure after parsing and before presenting output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    UnknownTask,
    SoftDeletedTask,
    EmptyTitle,
    InvalidTitle,
    UnknownAgent(String),
    UnknownBase(String),
    AgentConfig(String),
    /// An `--after` task is this task, not on the board, or already done.
    InvalidAfter(String),
    /// An `--after` task already runs after this one.
    AfterLoop(String),
    Store(String),
}

impl EditError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownTask => "unknown-task",
            Self::SoftDeletedTask => "soft-deleted-task",
            Self::EmptyTitle => "empty-title",
            Self::InvalidTitle => "invalid-title",
            Self::UnknownAgent(_) => "unknown-agent",
            Self::UnknownBase(_) => "unknown-base",
            Self::AgentConfig(_) => "agent-config",
            Self::InvalidAfter(_) => "invalid-after",
            Self::AfterLoop(_) => "after-loop",
            Self::Store(_) => "store-error",
        }
    }
}

/// Replace the supplied title and/or notes, keeping scope and thread.
///
/// Whitespace-only notes clear the field. Repeating the stored values writes nothing.
pub fn run(
    target: TaskAddress,
    fields: EditFields,
    state_dir: Option<PathBuf>,
) -> Result<EditResult, EditError> {
    if let Some(title) = fields.title.as_deref() {
        if has_c0_control(title) {
            return Err(EditError::InvalidTitle);
        }
        if title.trim().is_empty() {
            return Err(EditError::EmptyTitle);
        }
    }

    let state_dir = state_dir.unwrap_or_else(default_state_dir);
    let assignee = match fields.assignee.as_ref().and_then(|value| value.as_deref()) {
        Some(name) => {
            let profiles = AgentProfiles::load(&state_dir)
                .map_err(|error| EditError::AgentConfig(error.to_string()))?;
            Some(
                profiles
                    .resolve_name(name)
                    .map_err(EditError::UnknownAgent)?,
            )
        }
        None => None,
    };
    let mut fields = fields;
    if fields.assignee.as_ref().is_some_and(Option::is_some) {
        fields.assignee = Some(assignee);
    }
    let store = TaskStore::new(state_dir);
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
                    notes: task.notes.clone(),
                    scope: task.scope.clone(),
                    thread: task.thread.clone(),
                    assignee: task.assignee.clone(),
                    base: task.base.clone(),
                    after: task.after.clone(),
                    soft_deleted: task.soft_deleted,
                });
            Ok(apply(state, found, &fields))
        })
        .map_err(EditError::Store)?
}

struct Found {
    id: Uuid,
    number: Option<u64>,
    title: String,
    notes: Option<String>,
    scope: TaskScope,
    thread: Option<String>,
    assignee: Option<String>,
    base: Option<String>,
    after: Vec<u64>,
    soft_deleted: bool,
}

fn apply(
    state: &mut DomainState,
    found: Option<Found>,
    fields: &EditFields,
) -> (Result<EditResult, EditError>, bool) {
    let Some(found) = found else {
        return (Err(EditError::UnknownTask), false);
    };
    let Some(number) = found.number else {
        return (Err(EditError::UnknownTask), false);
    };
    if found.soft_deleted {
        return (Err(EditError::SoftDeletedTask), false);
    }
    let next_title = fields
        .title
        .as_deref()
        .map(str::trim)
        .unwrap_or(found.title.as_str())
        .to_string();
    let next_notes = match fields.notes.as_deref() {
        Some(value) if value.trim().is_empty() => None,
        Some(value) => Some(value.to_string()),
        None => found.notes.clone(),
    };
    let next_assignee = fields
        .assignee
        .clone()
        .unwrap_or_else(|| found.assignee.clone());
    let next_base = fields.base.clone().unwrap_or_else(|| found.base.clone());
    if let Some(base) = fields.base.as_ref().and_then(|value| value.as_deref()) {
        if let Err(error) = crate::git_base::validate_task_base(&found.scope, base, true) {
            return (Err(EditError::UnknownBase(error)), false);
        }
    }
    let next_after = fields.after.clone().unwrap_or_else(|| found.after.clone());
    let changed_after = next_after != found.after;
    if changed_after {
        if let Err(error) = state.check_after(found.id, &next_after) {
            return (Err(after_error(error)), false);
        }
    }
    if next_title == found.title
        && next_notes == found.notes
        && next_assignee == found.assignee
        && next_base == found.base
        && !changed_after
    {
        return (
            Ok(EditResult {
                number,
                title: found.title,
            }),
            false,
        );
    }
    let fields_changed = next_title != found.title
        || next_notes != found.notes
        || next_assignee != found.assignee
        || next_base != found.base;
    let edited = if fields_changed {
        state.edit_with_assignee_and_base(
            found.id,
            &next_title,
            next_notes,
            found.scope,
            found.thread,
            next_assignee,
            next_base,
        )
    } else {
        Ok(())
    };
    let edited = edited.and_then(|()| {
        if changed_after {
            state.edit_after(found.id, &next_after).map(|_| ())
        } else {
            Ok(())
        }
    });
    match edited {
        Ok(()) => (
            Ok(EditResult {
                number,
                title: next_title,
            }),
            true,
        ),
        Err(DomainError::EmptyTitle) => (Err(EditError::EmptyTitle), false),
        Err(DomainError::UnknownId(_)) => (Err(EditError::UnknownTask), false),
        Err(
            error @ (DomainError::AfterSelf(_)
            | DomainError::AfterUnknown(_)
            | DomainError::AfterDone(_)
            | DomainError::AfterLoop { .. }),
        ) => (Err(after_error(error)), false),
        Err(other) => (Err(EditError::Store(other.to_string())), false),
    }
}

fn after_error(error: DomainError) -> EditError {
    match error {
        DomainError::AfterLoop { .. } => EditError::AfterLoop(error.to_string()),
        DomainError::UnknownId(_) => EditError::UnknownTask,
        other => EditError::InvalidAfter(other.to_string()),
    }
}
