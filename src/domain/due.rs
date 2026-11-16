//! A task's optional due date: a calendar day with no time and no time zone.
//!
//! Stored as the `YYYY-MM-DD` text the user typed. Only a date that exists is stored, so the
//! text orders the same way the days do.

use std::time::SystemTime;

use uuid::Uuid;

use super::{
    record_event, DomainError, DomainState, EditedField, EventDetail, TaskEventKind, UndoEntry,
};

/// Why a due date was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DueError {
    /// Not `YYYY-MM-DD`: wrong length, a non-digit, or a missing dash.
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

/// Check that `text` is exactly `YYYY-MM-DD` and names a real day (Gregorian, year 0001 to 9999).
pub fn parse_due(text: &str) -> Result<&str, DueError> {
    let bytes = text.as_bytes();
    let shaped = bytes.len() == 10
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            _ => byte.is_ascii_digit(),
        });
    if !shaped {
        return Err(DueError::Malformed);
    }
    let number = |range: std::ops::Range<usize>| -> u32 {
        text[range].parse().expect("these bytes are ASCII digits")
    };
    let (year, month, day) = (number(0..4), number(5..7), number(8..10));
    if year == 0 || !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
        return Err(DueError::NoSuchDay);
    }
    Ok(text)
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
        if let Some(text) = due.as_deref() {
            parse_due(text).map_err(DomainError::InvalidDue)?;
        }
        self.task_mut(id)?.due = due;
        Ok(())
    }

    /// Set or clear one task's due date as an edit, with no undo entry (the CLI's
    /// `edit --due` and `--clear-due`). Returns whether the task changed.
    pub fn edit_due(&mut self, id: Uuid, due: Option<String>) -> Result<bool, DomainError> {
        if let Some(text) = due.as_deref() {
            parse_due(text).map_err(DomainError::InvalidDue)?;
        }
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
        if let Some(text) = due.as_deref() {
            parse_due(text).map_err(DomainError::InvalidDue)?;
        }
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

    /// Run a task-page save and make its steps and due day undoable as one entry: `Steps`,
    /// `SetDue`, or a batch of both, each expecting the revision the save left.
    pub fn with_steps_and_due_undo<T>(
        &mut self,
        id: Uuid,
        change: impl FnOnce(&mut Self) -> Result<T, DomainError>,
    ) -> Result<T, DomainError> {
        let task = self.task_mut(id)?;
        let previous_steps = task.steps.clone();
        let previous_due = task.due.clone();
        let value = change(self)?;
        let task = self.task_mut(id)?;
        let expected_revision = task.revision;
        let mut entries = Vec::new();
        if task.due != previous_due {
            entries.push(UndoEntry::SetDue {
                id,
                previous: previous_due,
                expected_revision,
            });
        }
        if task.steps != previous_steps {
            entries.push(UndoEntry::Steps {
                id,
                previous: previous_steps,
                expected_revision,
            });
        }
        match entries.len() {
            0 => {}
            1 => self
                .undo_stack
                .push(entries.pop().expect("one save undo entry")),
            _ => self.undo_stack.push(UndoEntry::Batch { entries }),
        }
        Ok(value)
    }

    /// Undo's half of a board due edit: put back an earlier day.
    pub(crate) fn restore_due(&mut self, id: Uuid, due: Option<String>) -> Result<(), DomainError> {
        let task = self.task_mut(id)?;
        task.due = due;
        record_event(
            task,
            TaskEventKind::Edited,
            SystemTime::now(),
            None,
            due_detail(),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ProvenanceOrigin, TaskScope};

    fn dated_task(state: &mut DomainState) -> Uuid {
        let id = state
            .create(
                "Dated",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create");
        state.add_step(id, "first").expect("step");
        state.edit_due(id, Some("2026-11-01".into())).expect("due");
        id
    }

    /// A page save that changes the steps and the due day is one undo entry, and undoing it
    /// puts both back.
    #[test]
    fn a_save_changing_steps_and_due_undoes_as_one_entry() {
        let mut state = DomainState::new();
        let id = dated_task(&mut state);
        let depth = state.undo_len();
        state
            .with_steps_and_due_undo(id, |state| {
                state.add_step(id, "second")?;
                state.stage_due_in_edit(id, Some("2026-11-30".into()))
            })
            .expect("save");
        assert_eq!(state.undo_len(), depth + 1);
        assert!(matches!(
            state.last_undo(),
            Some(UndoEntry::Batch { entries }) if entries.len() == 2
        ));

        // The entry survives the wire format, as `set_due` in a saved undo stack.
        let wire = serde_json::to_value(&state).expect("serialize");
        assert!(wire.to_string().contains("\"set_due\""), "{wire}");
        let mut state: DomainState = serde_json::from_value(wire).expect("deserialize");

        state.undo().expect("undo");
        let task = state.get(id).expect("task");
        assert_eq!(task.due.as_deref(), Some("2026-11-01"));
        assert_eq!(task.steps.len(), 1);
        assert_eq!(state.undo_len(), depth);
    }

    /// A due-only save is a lone `set_due` entry; a save that leaves the day alone adds none.
    #[test]
    fn a_due_only_save_is_a_set_due_entry_and_an_unchanged_day_adds_none() {
        let mut state = DomainState::new();
        let id = dated_task(&mut state);
        let depth = state.undo_len();
        state
            .with_steps_and_due_undo(id, |state| {
                state.stage_due_in_edit(id, Some("2026-11-01".into()))
            })
            .expect("unchanged");
        assert_eq!(state.undo_len(), depth);
        state
            .with_steps_and_due_undo(id, |state| state.stage_due_in_edit(id, None))
            .expect("clear");
        assert!(matches!(
            state.last_undo(),
            Some(UndoEntry::SetDue { previous: Some(day), .. }) if day == "2026-11-01"
        ));
        assert!(matches!(
            state.stage_due_in_edit(id, Some("2026-13-01".into())),
            Err(DomainError::InvalidDue(DueError::NoSuchDay))
        ));
    }
}
