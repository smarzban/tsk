//! A task's optional due date: a calendar day with no time and no time zone.
//!
//! Typed as `YYYY-MM-DD` or with a one-digit month or day (`2026-8-8`), and stored padded
//! (`2026-08-08`). Only a date that exists is stored, so the text orders the same way the
//! days do.

use std::time::SystemTime;

use uuid::Uuid;

use super::{record_event, DomainError, DomainState, EditedField, EventDetail, TaskEventKind};

/// Why a due date was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DueError {
    /// Not `YYYY-M-D`: a year other than four digits, a month or day other than one or two
    /// digits, a non-digit, or a missing dash.
    Malformed,
    /// Shaped like a date, but no such day exists (`2026-02-30`, `2026-13-01`, year `0000`).
    NoSuchDay,
}

impl std::fmt::Display for DueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DueError::Malformed => write!(f, "a due date is a day written YYYY-MM-DD"),
            DueError::NoSuchDay => write!(f, "that day does not exist in the calendar"),
        }
    }
}

impl std::error::Error for DueError {}

/// Check that `text` is `YYYY-MM-DD`, or the same with a one-digit month or day, and names a
/// real day (Gregorian, year 0001 to 9999). Returns the day padded to `YYYY-MM-DD`.
pub fn parse_due(text: &str) -> Result<String, DueError> {
    let mut parts = text.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(DueError::Malformed);
    };
    let digits = |part: &str, widths: std::ops::RangeInclusive<usize>| -> Option<u32> {
        (widths.contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| part.parse().expect("these bytes are ASCII digits"))
    };
    let (Some(year), Some(month), Some(day)) = (
        digits(year, 4..=4),
        digits(month, 1..=2),
        digits(day, 1..=2),
    ) else {
        return Err(DueError::Malformed);
    };
    if year == 0 || !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
        return Err(DueError::NoSuchDay);
    }
    Ok(format!("{year:04}-{month:02}-{day:02}"))
}

/// A due day as stored: `None` stays `None`, and a day is checked and padded.
fn stored_due(due: Option<String>) -> Result<Option<String>, DomainError> {
    due.as_deref()
        .map(parse_due)
        .transpose()
        .map_err(DomainError::InvalidDue)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn due_detail() -> Option<EventDetail> {
    Some(EventDetail {
        fields: vec![EditedField::Due],
        ..EventDetail::default()
    })
}

impl DomainState {
    /// Give a task created in this same mutation its due date. Part of the creation, so no
    /// event of its own. A malformed date is refused.
    pub fn set_due_on_create(&mut self, id: Uuid, due: Option<String>) -> Result<(), DomainError> {
        let due = stored_due(due)?;
        self.task_mut(id)?.due = due;
        Ok(())
    }

    /// Set or clear one task's due date as an edit, with no undo entry (the CLI's
    /// `edit --due` and `--clear-due`). Returns whether the task changed.
    pub fn edit_due(&mut self, id: Uuid, due: Option<String>) -> Result<bool, DomainError> {
        let due = stored_due(due)?;
        let task = self.get(id).ok_or(DomainError::UnknownId(id))?;
        if task.soft_deleted {
            return Err(DomainError::SoftDeleted(id));
        }
        if task.due == due {
            return Ok(false);
        }
        let task = self.task_mut(id)?;
        task.due = due;
        record_event(
            task,
            TaskEventKind::Edited,
            SystemTime::now(),
            None,
            due_detail(),
        );
        Ok(true)
    }

    /// Stage a due day inside a task-page save that already recorded its `edited` event: the
    /// day joins that event's fields, so one save stays one revision. A malformed day is
    /// refused before anything changes.
    pub fn stage_due_in_edit(&mut self, id: Uuid, due: Option<String>) -> Result<(), DomainError> {
        let due = stored_due(due)?;
        let task = self.task_mut(id)?;
        if task.due == due {
            return Ok(());
        }
        task.due = due;
        if let Some(detail) = task
            .history
            .iter_mut()
            .rev()
            .find(|event| event.kind == TaskEventKind::Edited)
            .and_then(|event| event.detail.as_mut())
        {
            if !detail.fields.contains(&EditedField::Due) {
                detail.fields.push(EditedField::Due);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ProvenanceOrigin, TaskScope};

    /// A staged day joins the save's `edited` event and pushes no undo entry; a day that does
    /// not exist is refused before anything changes.
    #[test]
    fn a_staged_due_day_joins_the_edited_event_and_is_not_undoable() {
        let mut state = DomainState::new();
        let id = state
            .create(
                "Dated",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create");
        let depth = state.undo_len();
        // The save's own edit names only the title; staging the day adds Due to it.
        state
            .edit(id, "Renamed", None, TaskScope::Global, None)
            .expect("edit title");
        let events = state.get(id).expect("task").history.len();
        state
            .stage_due_in_edit(id, Some("2026-11-30".into()))
            .expect("stage");
        let task = state.get(id).expect("task");
        assert_eq!(task.due.as_deref(), Some("2026-11-30"));
        assert_eq!(task.history.len(), events, "no event of its own");
        assert_eq!(
            task.history
                .last()
                .and_then(|event| event.detail.as_ref())
                .map(|d| d.fields.clone()),
            Some(vec![EditedField::Title, EditedField::Due])
        );
        assert_eq!(state.undo_len(), depth);
        assert!(matches!(
            state.stage_due_in_edit(id, Some("2026-13-01".into())),
            Err(DomainError::InvalidDue(DueError::NoSuchDay))
        ));
    }
}
