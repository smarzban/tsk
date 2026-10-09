//! `after`: a task runs after other tasks, named by their store-global numbers.
//!
//! A task waits while any prerequisite is not done. When the last one becomes done, a waiting
//! task in `ready` starts; the board and CLI take [`DomainState::take_completed`] before they
//! save and start what it released, so the start lands in the same save as the done.

use std::collections::BTreeSet;
use std::time::SystemTime;

use uuid::Uuid;

use super::{
    current_actor, record_event, sync_block_with_status, DomainError, DomainState, EditedField,
    EventDetail, HumanStatus, Task, TaskEventKind, UndoEntry,
};

fn after_detail() -> Option<EventDetail> {
    Some(EventDetail {
        fields: vec![EditedField::After],
        ..EventDetail::default()
    })
}

/// The status as the board and CLI print it.
pub fn status_word(status: HumanStatus) -> &'static str {
    match status {
        HumanStatus::Open => "open",
        HumanStatus::Ready => "ready",
        HumanStatus::Started => "started",
        HumanStatus::Blocked => "blocked",
        HumanStatus::Review => "review",
        HumanStatus::Done => "done",
    }
}

/// `numbers` without repeats, first position kept.
fn dedupe(numbers: &[u64]) -> Vec<u64> {
    let mut seen = BTreeSet::new();
    numbers
        .iter()
        .copied()
        .filter(|number| seen.insert(*number))
        .collect()
}

impl DomainState {
    /// The live (not deleted, not a notice) task carrying `number`.
    pub fn task_by_number(&self, number: u64) -> Option<&Task> {
        self.tasks
            .iter()
            .find(|task| task.number == Some(number) && !task.soft_deleted && !task.is_notice())
    }

    /// The prerequisites `task` still waits on, in its own order: live tasks not yet done.
    /// A number no live task carries any more waits on nothing.
    pub fn waiting_on(&self, task: &Task) -> Vec<u64> {
        task.after
            .iter()
            .copied()
            .filter(|number| {
                self.task_by_number(*number)
                    .is_some_and(|prerequisite| prerequisite.status != HumanStatus::Done)
            })
            .collect()
    }

    /// `T3 runs after T2 (started), T4 (open)`: what `task` still waits on, or `None` when it
    /// waits on nothing.
    pub fn waiting_text(&self, task: &Task) -> Option<String> {
        let waiting = self.waiting_on(task);
        if waiting.is_empty() {
            return None;
        }
        let list = waiting
            .iter()
            .map(|number| {
                let status = self
                    .task_by_number(*number)
                    .map_or(HumanStatus::Open, |task| task.status);
                format!("T{number} ({})", status_word(status))
            })
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!(
            "T{} runs after {list}",
            task.number.unwrap_or_default()
        ))
    }

    /// The live tasks that run after `number`, by number: the read-only `before` side.
    pub fn before(&self, number: u64) -> Vec<u64> {
        let mut numbers: Vec<u64> = self
            .tasks
            .iter()
            .filter(|task| !task.soft_deleted && !task.is_notice() && task.after.contains(&number))
            .filter_map(|task| task.number)
            .collect();
        numbers.sort_unstable();
        numbers
    }

    /// Whether `id` may run after every number in `after`: each names a live task other than
    /// itself, not done (one it already runs after may stay), and none already runs after it,
    /// directly or through other tasks.
    pub fn check_after(&self, id: Uuid, after: &[u64]) -> Result<(), DomainError> {
        let task = self.get(id).ok_or(DomainError::UnknownId(id))?;
        for &number in after {
            if task.number == Some(number) {
                return Err(DomainError::AfterSelf(number));
            }
            let prerequisite = self
                .task_by_number(number)
                .ok_or(DomainError::AfterUnknown(number))?;
            if prerequisite.status == HumanStatus::Done && !task.after.contains(&number) {
                return Err(DomainError::AfterDone(number));
            }
            if let Some(own) = task.number {
                if self.runs_after(number, own) {
                    return Err(DomainError::AfterLoop {
                        waiting: number,
                        prerequisite: own,
                    });
                }
            }
        }
        Ok(())
    }

    /// Whether task `from` runs after task `target`, following `after` links transitively.
    fn runs_after(&self, from: u64, target: u64) -> bool {
        let mut seen = BTreeSet::new();
        let mut stack = vec![from];
        while let Some(number) = stack.pop() {
            if number == target {
                return true;
            }
            if !seen.insert(number) {
                continue;
            }
            if let Some(task) = self.task_by_number(number) {
                stack.extend(task.after.iter().copied());
            }
        }
        false
    }

    /// Set one task's prerequisites and make the change undoable.
    pub fn set_after(&mut self, id: Uuid, after: &[u64]) -> Result<bool, DomainError> {
        self.set_after_batch(&[id], after)
    }

    /// Set an ordered task set's prerequisites to the same list as one atomic, undoable action.
    /// Every task is checked against the links the earlier ones gain, so the set cannot close a
    /// loop through itself.
    pub fn set_after_batch(&mut self, ids: &[Uuid], after: &[u64]) -> Result<bool, DomainError> {
        let ids = self.prevalidate_batch_ids(ids)?;
        let after = dedupe(after);
        let changes = ids.into_iter().map(|id| (id, after.clone())).collect();
        self.apply_after_changes(changes)
    }

    /// Chain `ids` in order: each runs after the one before it, on top of what it already runs
    /// after. One atomic, undoable action. Fewer than two tasks change nothing.
    pub fn chain_after(&mut self, ids: &[Uuid]) -> Result<bool, DomainError> {
        let ids = self.prevalidate_batch_ids(ids)?;
        let mut changes = Vec::new();
        for pair in ids.windows(2) {
            let previous = self
                .get(pair[0])
                .and_then(|task| task.number)
                .ok_or(DomainError::UnknownId(pair[0]))?;
            let task = self.get(pair[1]).ok_or(DomainError::UnknownId(pair[1]))?;
            let mut after = task.after.clone();
            if !after.contains(&previous) {
                after.push(previous);
            }
            changes.push((pair[1], after));
        }
        self.apply_after_changes(changes)
    }

    /// Check every change against the links the earlier ones made, then apply them as one undo
    /// step. Nothing changes when any is refused.
    fn apply_after_changes(&mut self, changes: Vec<(Uuid, Vec<u64>)>) -> Result<bool, DomainError> {
        let mut work = self.clone();
        let mut changed = Vec::new();
        for (id, after) in changes {
            let current = work.get(id).ok_or(DomainError::UnknownId(id))?;
            if current.soft_deleted {
                return Err(DomainError::SoftDeleted(id));
            }
            if current.after == after {
                continue;
            }
            work.check_after(id, &after)?;
            work.task_mut(id)?.after.clone_from(&after);
            changed.push((id, after));
        }
        if changed.is_empty() {
            return Ok(false);
        }
        let at = SystemTime::now();
        let mut entries = Vec::with_capacity(changed.len());
        for (id, after) in changed {
            let task = self.task_mut(id)?;
            let previous = std::mem::replace(&mut task.after, after);
            record_event(task, TaskEventKind::Edited, at, None, after_detail());
            entries.push(UndoEntry::SetAfter {
                id,
                previous,
                expected_revision: task.revision,
            });
        }
        self.undo_stack.push(if entries.len() == 1 {
            entries.pop().expect("one after undo")
        } else {
            UndoEntry::Batch { entries }
        });
        Ok(true)
    }

    /// Whether a task not created yet may run after every number in `after`: each names a live
    /// task that is not done. Nothing runs after a new task, so it cannot close a loop.
    pub fn check_new_after(&self, after: &[u64]) -> Result<(), DomainError> {
        for &number in after {
            let prerequisite = self
                .task_by_number(number)
                .ok_or(DomainError::AfterUnknown(number))?;
            if prerequisite.status == HumanStatus::Done {
                return Err(DomainError::AfterDone(number));
            }
        }
        Ok(())
    }

    /// Give a task created in this same mutation its prerequisites, checked by
    /// [`Self::check_new_after`]. Part of the creation, so no event of its own.
    pub fn set_after_on_create(&mut self, id: Uuid, after: &[u64]) -> Result<(), DomainError> {
        let after = dedupe(after);
        self.check_new_after(&after)?;
        self.task_mut(id)?.after = after;
        Ok(())
    }

    /// Set one task's prerequisites as an edit, with no undo entry (the CLI's `edit --after`).
    pub fn edit_after(&mut self, id: Uuid, after: &[u64]) -> Result<bool, DomainError> {
        let after = dedupe(after);
        let task = self.get(id).ok_or(DomainError::UnknownId(id))?;
        if task.soft_deleted {
            return Err(DomainError::SoftDeleted(id));
        }
        if task.after == after {
            return Ok(false);
        }
        self.check_after(id, &after)?;
        let task = self.task_mut(id)?;
        task.after = after;
        record_event(
            task,
            TaskEventKind::Edited,
            SystemTime::now(),
            None,
            after_detail(),
        );
        Ok(true)
    }

    /// Fold an `after` change into the edit just recorded on `id`: same revision, and its
    /// `edited` event names the field. Checked first by [`Self::check_after`].
    pub fn stage_after_in_edit(&mut self, id: Uuid, after: &[u64]) -> Result<(), DomainError> {
        let after = dedupe(after);
        if self.get(id).ok_or(DomainError::UnknownId(id))?.after == after {
            return Ok(());
        }
        self.check_after(id, &after)?;
        let task = self.task_mut(id)?;
        task.after = after;
        if let Some(detail) = task
            .history
            .iter_mut()
            .rev()
            .find(|event| event.kind == TaskEventKind::Edited)
            .and_then(|event| event.detail.as_mut())
        {
            if !detail.fields.contains(&EditedField::After) {
                detail.fields.push(EditedField::After);
            }
        }
        Ok(())
    }

    /// Undo's half of [`Self::set_after`]: put back an earlier list.
    pub(crate) fn restore_after(&mut self, id: Uuid, after: Vec<u64>) -> Result<(), DomainError> {
        let task = self.task_mut(id)?;
        task.after = after;
        record_event(
            task,
            TaskEventKind::Edited,
            SystemTime::now(),
            None,
            after_detail(),
        );
        Ok(())
    }

    /// Drop the just-deleted `ids` from every other task's `after`, returning the undo entries
    /// that put the links back. A task left with nothing to wait on does not start.
    pub(super) fn unlink_deleted(&mut self, ids: &[Uuid], at: SystemTime) -> Vec<UndoEntry> {
        let numbers: BTreeSet<u64> = ids
            .iter()
            .filter_map(|id| self.get(*id).and_then(|task| task.number))
            .collect();
        let mut entries = Vec::new();
        if numbers.is_empty() {
            return entries;
        }
        for task in &mut self.tasks {
            if ids.contains(&task.id) || !task.after.iter().any(|n| numbers.contains(n)) {
                continue;
            }
            let previous = task.after.clone();
            task.after.retain(|number| !numbers.contains(number));
            record_event(task, TaskEventKind::Edited, at, None, after_detail());
            entries.push(UndoEntry::SetAfter {
                id: task.id,
                previous,
                expected_revision: task.revision,
            });
        }
        entries
    }

    /// Before a delete: the `ready` tasks it would leave with nothing to wait on, each with the
    /// deleted prerequisite it waited on. They stay ready; the board says so.
    pub fn unwaited_by_delete(&self, ids: &[Uuid]) -> Vec<(u64, u64)> {
        let numbers: BTreeSet<u64> = ids
            .iter()
            .filter_map(|id| self.get(*id).and_then(|task| task.number))
            .collect();
        self.tasks
            .iter()
            .filter(|task| {
                !ids.contains(&task.id)
                    && !task.soft_deleted
                    && task.status == HumanStatus::Ready
                    && task.after.iter().any(|n| numbers.contains(n))
            })
            .filter_map(|task| {
                let left: Vec<u64> = task
                    .after
                    .iter()
                    .copied()
                    .filter(|n| !numbers.contains(n))
                    .collect();
                let waits = left.iter().any(|n| {
                    self.task_by_number(*n)
                        .is_some_and(|prerequisite| prerequisite.status != HumanStatus::Done)
                });
                let gone = task.after.iter().copied().find(|n| numbers.contains(n))?;
                (!waits && !self.waiting_on(task).is_empty()).then_some((task.number?, gone))
            })
            .collect()
    }

    /// Trash purge: drop the removed tasks' numbers from every task left. Bookkeeping, not a
    /// user action, so undo entries that expected the old revision follow it.
    pub(super) fn unlink_removed(&mut self, ids: &BTreeSet<Uuid>) {
        let numbers: BTreeSet<u64> = self
            .tasks
            .iter()
            .filter(|task| ids.contains(&task.id))
            .filter_map(|task| task.number)
            .collect();
        if numbers.is_empty() {
            return;
        }
        let at = SystemTime::now();
        let mut moved = Vec::new();
        for task in &mut self.tasks {
            if ids.contains(&task.id) || !task.after.iter().any(|n| numbers.contains(n)) {
                continue;
            }
            let before = task.revision;
            task.after.retain(|number| !numbers.contains(number));
            record_event(task, TaskEventKind::Edited, at, None, after_detail());
            moved.push((task.id, before, task.revision));
        }
        for (id, before, after) in moved {
            for entry in &mut self.undo_stack {
                entry.retarget(id, before, after);
            }
        }
    }

    /// The tasks this process completed since the last take, oldest first.
    pub fn take_completed(&mut self) -> Vec<Uuid> {
        std::mem::take(&mut self.completed)
    }

    /// The `ready` tasks whose last prerequisite is among `done` (and still done), each with the
    /// prerequisite that released it, in board order by number.
    pub fn released_by(&self, done: &[Uuid]) -> Vec<(Uuid, u64)> {
        let numbers: Vec<u64> = done
            .iter()
            .filter_map(|id| self.get(*id))
            .filter(|task| task.status == HumanStatus::Done && !task.soft_deleted)
            .filter_map(|task| task.number)
            .collect();
        let mut released: Vec<(Uuid, u64, u64)> = self
            .tasks
            .iter()
            .filter(|task| {
                task.status == HumanStatus::Ready
                    && !task.soft_deleted
                    && !task.archived
                    && !task.is_notice()
                    && self.waiting_on(task).is_empty()
            })
            .filter_map(|task| {
                let by = numbers
                    .iter()
                    .rev()
                    .copied()
                    .find(|number| task.after.contains(number))?;
                Some((task.id, task.number.unwrap_or(u64::MAX), by))
            })
            .collect();
        released.sort_by_key(|(_, number, _)| *number);
        released.into_iter().map(|(id, _, by)| (id, by)).collect()
    }

    /// Start a task its prerequisite `after` released. When the top undo entry completed that
    /// prerequisite, undoing it also puts this task back.
    pub fn start_released(&mut self, id: Uuid, after: u64) -> Result<(), DomainError> {
        let by = current_actor();
        let at = SystemTime::now();
        let task = self.task_mut(id)?;
        let from = task.status;
        task.status = HumanStatus::Started;
        sync_block_with_status(task, at, &by);
        record_event(
            task,
            TaskEventKind::StatusSet,
            at,
            Some(&by),
            Some(EventDetail {
                after: Some(after),
                ..EventDetail::status(from, HumanStatus::Started)
            }),
        );
        let revision = task.revision;
        self.graft_released_undo(id, from, revision, after);
        Ok(())
    }

    /// A released task started by a dispatch just recorded: name the prerequisite on that
    /// event and make the start part of the done's undo, as [`Self::start_released`] does.
    pub fn note_released_dispatch(
        &mut self,
        id: Uuid,
        after: u64,
        previous: HumanStatus,
    ) -> Result<(), DomainError> {
        let task = self.task_mut(id)?;
        if let Some(event) = task
            .history
            .last_mut()
            .filter(|event| event.kind == TaskEventKind::Dispatched)
        {
            event.detail.get_or_insert_with(EventDetail::default).after = Some(after);
        }
        let revision = task.revision;
        self.graft_released_undo(id, previous, revision, after);
        Ok(())
    }

    fn graft_released_undo(&mut self, id: Uuid, previous: HumanStatus, revision: Uuid, after: u64) {
        let Some(prerequisite) = self.task_by_number(after).map(|task| task.id) else {
            return;
        };
        let Some(top) = self.undo_stack.last_mut() else {
            return;
        };
        let completes = top
            .leaf_entries()
            .into_iter()
            .any(|leaf| matches!(leaf, UndoEntry::Complete { id, .. } if *id == prerequisite));
        if !completes {
            return;
        }
        let start = UndoEntry::Start {
            id,
            previous,
            expected_revision: revision,
        };
        match top {
            UndoEntry::Batch { entries } => entries.push(start),
            other => {
                let first = other.clone();
                *other = UndoEntry::Batch {
                    entries: vec![first, start],
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ProvenanceOrigin, TaskScope};

    fn project(path: &str) -> TaskScope {
        TaskScope::Project { path: path.into() }
    }

    /// `count` numbered tasks, alternating between two projects, then the desk.
    fn board(count: usize) -> (DomainState, Vec<Uuid>) {
        let mut state = DomainState::new();
        let ids: Vec<Uuid> = (0..count)
            .map(|index| {
                let scope = match index % 3 {
                    0 => project("/repos/app"),
                    1 => project("/repos/api"),
                    _ => TaskScope::Global,
                };
                state
                    .create(
                        format!("task {index}"),
                        None,
                        scope,
                        ProvenanceOrigin::Manual,
                        None,
                    )
                    .expect("create")
            })
            .collect();
        state.assign_numbers_for_persistence();
        (state, ids)
    }

    fn after(state: &DomainState, id: Uuid) -> Vec<u64> {
        state.get(id).expect("task").after.clone()
    }

    #[test]
    fn a_task_runs_after_tasks_in_any_project_and_waits_until_they_are_done() {
        let (mut state, ids) = board(3);
        assert!(state.set_after(ids[2], &[1, 2, 1]).expect("set"));
        assert_eq!(after(&state, ids[2]), vec![1, 2], "repeats collapse");
        let waiting = state.get(ids[2]).expect("task").clone();
        assert_eq!(state.waiting_on(&waiting), vec![1, 2]);
        assert_eq!(state.before(1), vec![3]);
        state.set_status(ids[0], HumanStatus::Done).expect("done");
        let waiting = state.get(ids[2]).expect("task").clone();
        assert_eq!(state.waiting_on(&waiting), vec![2]);
        assert!(!state.set_after(ids[2], &[1, 2]).expect("same list"));
    }

    #[test]
    fn refuses_itself_unknown_done_and_loops() {
        let (mut state, ids) = board(4);
        assert_eq!(
            state.set_after(ids[0], &[1]),
            Err(DomainError::AfterSelf(1))
        );
        assert_eq!(
            state.set_after(ids[0], &[9]),
            Err(DomainError::AfterUnknown(9))
        );
        state.set_status(ids[3], HumanStatus::Done).expect("done");
        assert_eq!(
            state.set_after(ids[0], &[4]),
            Err(DomainError::AfterDone(4))
        );
        state.set_after(ids[1], &[1]).expect("2 after 1");
        state.set_after(ids[2], &[2]).expect("3 after 2");
        let error = state.set_after(ids[0], &[3]).expect_err("loop through T2");
        assert_eq!(
            error,
            DomainError::AfterLoop {
                waiting: 3,
                prerequisite: 1
            }
        );
        assert_eq!(error.to_string(), "T3 already runs after T1");
        assert!(
            after(&state, ids[0]).is_empty(),
            "a refusal changes nothing"
        );
    }

    #[test]
    fn a_done_prerequisite_already_listed_may_stay() {
        let (mut state, ids) = board(3);
        state.set_after(ids[2], &[1]).expect("set");
        state.set_status(ids[0], HumanStatus::Done).expect("done");
        assert!(state
            .set_after(ids[2], &[1, 2])
            .expect("keeps the done one"));
    }

    #[test]
    fn chain_in_order_links_each_to_the_one_before_as_one_undo() {
        let (mut state, ids) = board(4);
        state.set_after(ids[3], &[1]).expect("existing link");
        let order = [ids[2], ids[0], ids[3]];
        assert!(state.chain_after(&order).expect("chain"));
        assert_eq!(after(&state, ids[0]), vec![3]);
        assert_eq!(after(&state, ids[3]), vec![1], "already after T1");
        state.undo().expect("undo");
        assert!(after(&state, ids[0]).is_empty());
        assert_eq!(after(&state, ids[3]), vec![1]);
        state.undo().expect("undo the earlier set");
        assert!(after(&state, ids[3]).is_empty());
    }

    #[test]
    fn a_chain_that_would_close_a_loop_changes_nothing() {
        let (mut state, ids) = board(3);
        state.set_after(ids[0], &[3]).expect("1 after 3");
        let error = state
            .chain_after(&[ids[0], ids[1], ids[2]])
            .expect_err("3 would run after 2 after 1 after 3");
        assert_eq!(
            error,
            DomainError::AfterLoop {
                waiting: 2,
                prerequisite: 3
            }
        );
        assert!(after(&state, ids[1]).is_empty());
    }

    #[test]
    fn a_set_over_marked_tasks_cannot_loop_through_itself() {
        let (mut state, ids) = board(3);
        let error = state
            .set_after_batch(&[ids[0], ids[1]], &[2])
            .expect_err("T2 cannot run after itself");
        assert_eq!(error, DomainError::AfterSelf(2));
        assert!(after(&state, ids[0]).is_empty(), "nothing applied");
        assert!(state.set_after_batch(&[ids[0], ids[1]], &[3]).expect("set"));
        state.undo().expect("one undo");
        assert!(after(&state, ids[0]).is_empty() && after(&state, ids[1]).is_empty());
    }

    #[test]
    fn deleting_a_prerequisite_unlinks_it_and_undo_restores_the_links() {
        let (mut state, ids) = board(4);
        state.set_after(ids[2], &[1]).expect("set");
        state.set_after(ids[3], &[1, 2]).expect("set");
        state.set_status(ids[2], HumanStatus::Ready).expect("ready");
        assert_eq!(state.unwaited_by_delete(&[ids[0]]), vec![(3, 1)]);
        state.soft_delete(ids[0]).expect("delete");
        assert!(after(&state, ids[2]).is_empty());
        assert_eq!(after(&state, ids[3]), vec![2]);
        assert_eq!(
            state.get(ids[2]).expect("task").status,
            HumanStatus::Ready,
            "no auto-start"
        );
        assert!(state.take_completed().is_empty());
        state.undo().expect("one undo");
        assert!(!state.get(ids[0]).expect("task").soft_deleted);
        assert_eq!(after(&state, ids[2]), vec![1]);
        assert_eq!(after(&state, ids[3]), vec![1, 2]);
    }

    #[test]
    fn a_bulk_delete_unlinks_in_the_same_undo_entry() {
        let (mut state, ids) = board(3);
        state.set_after(ids[2], &[1, 2]).expect("set");
        state.soft_delete_batch(&[ids[0], ids[1]]).expect("delete");
        assert!(after(&state, ids[2]).is_empty());
        state.undo().expect("undo");
        assert_eq!(after(&state, ids[2]), vec![1, 2]);
    }

    #[test]
    fn a_delete_with_links_is_not_sent_to_trash_by_its_own_unlinks() {
        let (mut state, ids) = board(2);
        state.set_after(ids[1], &[1]).expect("set");
        state.soft_delete(ids[0]).expect("delete");
        assert!(
            state.trash_eligible(SystemTime::now()).is_empty(),
            "the delete is still the top undo entry"
        );
    }

    #[test]
    fn a_trash_purge_drops_leftover_links() {
        let (mut state, ids) = board(3);
        state.set_after(ids[2], &[1, 2]).expect("set");
        // A link the delete could not see: the task carries it, but trash takes the task.
        state.remove_tasks(&BTreeSet::from([ids[0]]));
        assert_eq!(after(&state, ids[2]), vec![2]);
    }

    #[test]
    fn the_last_prerequisite_done_releases_ready_tasks_only() {
        let (mut state, ids) = board(5);
        for id in &ids[2..] {
            state.set_after(*id, &[1, 2]).expect("set");
        }
        state.set_status(ids[2], HumanStatus::Ready).expect("ready");
        state
            .set_status(ids[3], HumanStatus::Review)
            .expect("review");
        state.complete(ids[0]).expect("done");
        let done = state.take_completed();
        assert!(state.released_by(&done).is_empty(), "T2 is not done yet");
        state
            .set_status(ids[1], HumanStatus::Review)
            .expect("review");
        assert!(state.take_completed().is_empty(), "review is not done");
        state.complete(ids[1]).expect("done");
        let done = state.take_completed();
        assert_eq!(state.released_by(&done), vec![(ids[2], 2)]);
    }

    #[test]
    fn a_released_start_names_its_prerequisite_and_undoes_with_the_done() {
        let (mut state, ids) = board(2);
        state.set_after(ids[1], &[1]).expect("set");
        state.set_status(ids[1], HumanStatus::Ready).expect("ready");
        state.complete(ids[0]).expect("done");
        let done = state.take_completed();
        for (id, after) in state.released_by(&done) {
            state.start_released(id, after).expect("start");
        }
        let task = state.get(ids[1]).expect("task");
        assert_eq!(task.status, HumanStatus::Started);
        let event = task.history.last().expect("event");
        assert_eq!(
            event.detail.as_ref().and_then(|detail| detail.after),
            Some(1)
        );
        assert_eq!(
            crate::activity::event_text(event).as_deref(),
            Some("started · after T1")
        );
        state.undo().expect("undo the done");
        assert_eq!(state.get(ids[0]).expect("task").status, HumanStatus::Open);
        assert_eq!(state.get(ids[1]).expect("task").status, HumanStatus::Ready);
    }
}
