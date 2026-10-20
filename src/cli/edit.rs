//! Headless edits: title, notes, assignee, base, after, and who an open block or review
//! waits on.

use std::path::PathBuf;

use crate::agents::AgentProfiles;
use crate::cli::add::has_c0_control;
use crate::cli::parser::TaskAddress;
use crate::domain::{
    block_text, Assignee, BlockField, BlockKind, BlockOn, DomainError, DomainState, TaskScope,
};
use crate::store::{default_state_dir, TaskStore};
use uuid::Uuid;

/// Parsed field values after the argv boundary. `None` means leave the stored value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditFields {
    pub title: Option<String>,
    pub notes: Option<String>,
    /// Text added below the stored notes, after one blank line.
    pub append_notes: Option<String>,
    /// `None` leaves it unchanged, `Some(None)` clears it. A name is read by the name rule.
    pub assignee: Option<Option<String>>,
    /// `None` leaves it unchanged, `Some(None)` clears it.
    pub base: Option<Option<String>>,
    /// `None` leaves it unchanged; `Some` replaces the list (empty clears it).
    pub after: Option<Vec<u64>>,
    /// Who the open block or review waits on, as typed; `None` leaves it.
    pub on: Option<String>,
}

/// A successful edit, including an idempotent repeat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditResult {
    pub number: u64,
    pub title: String,
    /// What `--assignee` resolved to, for the line that names a person.
    pub assignee: Option<Assignee>,
}

/// An edit failure after parsing and before presenting output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    UnknownTask,
    SoftDeletedTask,
    EmptyTitle,
    InvalidTitle,
    UnknownBase(String),
    AgentConfig(String),
    /// An `--after` task is this task, not on the board, or already done.
    InvalidAfter(String),
    /// An `--after` task already runs after this one.
    AfterLoop(String),
    /// `--on` with no open block or review on the task.
    NotBlocked,
    /// `--on` names this task, or a task not on the board.
    InvalidBlocker,
    /// `--on` text is longer than the block text limit.
    TextTooLong,
    Store(String),
}

impl EditError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownTask => "unknown-task",
            Self::SoftDeletedTask => "soft-deleted-task",
            Self::EmptyTitle => "empty-title",
            Self::InvalidTitle => "invalid-title",
            Self::UnknownBase(_) => "unknown-base",
            Self::AgentConfig(_) => "agent-config",
            Self::InvalidAfter(_) => "invalid-after",
            Self::AfterLoop(_) => "after-loop",
            Self::NotBlocked => "not-blocked",
            Self::InvalidBlocker => "invalid-blocker",
            Self::TextTooLong => "text-too-long",
            Self::Store(_) => "store-error",
        }
    }
}

/// Replace the supplied title and/or notes, keeping scope and thread.
///
/// Whitespace-only notes clear the field. Repeating the stored values writes nothing; a
/// repeated append adds its text again.
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
    // The name rule: you, an agent profile, or any other name for a person. Broken profiles
    // refuse rather than guess, so a profile's name never becomes a person by accident.
    let assignee = match fields.assignee.as_ref() {
        Some(Some(name)) => {
            let profiles = AgentProfiles::load(&state_dir)
                .map_err(|error| EditError::AgentConfig(error.to_string()))?;
            Some(Some(
                profiles
                    .resolve_assignee(name)
                    .map_err(EditError::AgentConfig)?,
            ))
        }
        Some(None) => Some(None),
        None => None,
    };
    // Like `tsk status … --on`: a broken profile file reads as no profiles.
    let profiles = fields
        .on
        .as_ref()
        .map(|_| AgentProfiles::load(&state_dir).unwrap_or_default());
    let edit = Edit {
        title: fields.title,
        notes: fields.notes,
        append_notes: fields.append_notes,
        assignee,
        base: fields.base,
        after: fields.after,
        on: fields.on.zip(profiles),
    };
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
                    assignee: task.assigned(),
                    base: task.base.clone(),
                    after: task.after.clone(),
                    soft_deleted: task.soft_deleted,
                });
            Ok(apply(state, found, &edit))
        })
        .map_err(EditError::Store)?
}

/// The edit after the boundary resolved its names.
struct Edit {
    title: Option<String>,
    notes: Option<String>,
    append_notes: Option<String>,
    assignee: Option<Option<Assignee>>,
    base: Option<Option<String>>,
    after: Option<Vec<u64>>,
    /// The typed `--on` value and the profiles it is read against.
    on: Option<(String, AgentProfiles)>,
}

struct Found {
    id: Uuid,
    number: Option<u64>,
    title: String,
    notes: Option<String>,
    scope: TaskScope,
    thread: Option<String>,
    assignee: Option<Assignee>,
    base: Option<String>,
    after: Vec<u64>,
    soft_deleted: bool,
}

fn apply(
    state: &mut DomainState,
    found: Option<Found>,
    fields: &Edit,
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
    // Appended under the lock, to the notes as stored now, so a concurrent edit is kept.
    let next_notes = match (fields.append_notes.as_deref(), next_notes) {
        (Some(text), Some(notes)) if !notes.trim().is_empty() => {
            Some(format!("{}\n\n{text}", notes.trim_end()))
        }
        (Some(text), _) => Some(text.to_string()),
        (None, notes) => notes,
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
    let on = match fields.on.as_ref() {
        Some((value, profiles)) => match waiting_on(state, found.id, number, value, profiles) {
            Ok(on) => Some(on),
            Err(error) => return (Err(error), false),
        },
        None => None,
    };
    let changed_on = on.as_ref().is_some_and(|on| {
        state
            .get(found.id)
            .and_then(|task| task.block.as_ref())
            .is_some_and(|block| block.on != *on)
    });
    let assignee = fields.assignee.clone().flatten();
    if next_title == found.title
        && next_notes == found.notes
        && next_assignee == found.assignee
        && next_base == found.base
        && !changed_after
        && !changed_on
    {
        return (
            Ok(EditResult {
                number,
                title: found.title,
                assignee,
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
    let edited = edited.and_then(|()| match on {
        Some(on) if changed_on => state.set_on(found.id, on).map(|_| ()),
        _ => Ok(()),
    });
    match edited {
        Ok(()) => (
            Ok(EditResult {
                number,
                title: next_title,
                assignee,
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

/// Read `--on` for the task's open record by the name rule: on a block a task address names
/// another live task; on a review it is text. No open record refuses with `not-blocked`.
fn waiting_on(
    state: &DomainState,
    id: Uuid,
    number: u64,
    value: &str,
    profiles: &AgentProfiles,
) -> Result<BlockOn, EditError> {
    let kind = state
        .get(id)
        .and_then(|task| task.block.as_ref())
        .map(|block| block.kind)
        .ok_or(EditError::NotBlocked)?;
    match profiles.resolve_on(value, kind) {
        BlockOn::Task(task) => {
            let exists = state.tasks().iter().any(|other| {
                other.number == Some(task) && !other.soft_deleted && !other.is_notice()
            });
            if task == number || !exists || kind != BlockKind::Blocked {
                return Err(EditError::InvalidBlocker);
            }
            Ok(BlockOn::Task(task))
        }
        BlockOn::Other(text) => match block_text(Some(&text), BlockField::On) {
            Ok(Some(text)) => Ok(BlockOn::Other(text)),
            Ok(None) => Ok(BlockOn::You),
            Err(_) => Err(EditError::TextTooLong),
        },
        on => Ok(on),
    }
}

fn after_error(error: DomainError) -> EditError {
    match error {
        DomainError::AfterLoop { .. } => EditError::AfterLoop(error.to_string()),
        DomainError::UnknownId(_) => EditError::UnknownTask,
        other => EditError::InvalidAfter(other.to_string()),
    }
}
