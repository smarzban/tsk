//! Undo stack for soft-delete, complete, status changes, steps, archive, assignment, base,
//! block, who a block or review waits on, `after`, a start that dispatched, and a name removed
//! from the assignee picker.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AssigneeKind, BlockOn, DomainError, DomainState, HumanStatus, Step};

/// Maximum undo entries retained in a saved document.
pub const UNDO_CAP: usize = 50;

/// What a done entry written before store v6 restores: `open`.
fn pre_v6_previous() -> HumanStatus {
    HumanStatus::Open
}

/// One reversible user action on the undo stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UndoEntry {
    SoftDelete {
        id: Uuid,
        expected_revision: Uuid,
    },
    /// A done; reversing restores `previous`. An entry written before store v6 carries no
    /// `previous` and reads as `open`, the status such an undo always went back to.
    Complete {
        id: Uuid,
        #[serde(default = "pre_v6_previous")]
        previous: HumanStatus,
        expected_revision: Uuid,
    },
    /// An assignment; reversing restores `previous` with its kind. An entry written before
    /// store v6 carries no kind and reads as an agent, the only kind there was.
    Assign {
        id: Uuid,
        previous: Option<String>,
        #[serde(default, skip_serializing_if = "AssigneeKind::is_agent")]
        previous_kind: AssigneeKind,
        expected_revision: Uuid,
    },
    SetBase {
        id: Uuid,
        previous: Option<String>,
        expected_revision: Uuid,
    },
    /// A block opened by the board; reversing drops it and restores `previous`.
    Block {
        id: Uuid,
        previous: HumanStatus,
        expected_revision: Uuid,
    },
    /// A board start: one that dispatched, or a start-only row of the bulk start card (inside
    /// a batch). Reversing restores `previous` and leaves any agent and dispatch record in place.
    Start {
        id: Uuid,
        previous: HumanStatus,
        expected_revision: Uuid,
    },
    /// A plain status change on the board (a start that launched nothing, ready, open, an
    /// unblock); reversing restores `previous`.
    Status {
        id: Uuid,
        previous: HumanStatus,
        expected_revision: Uuid,
    },
    /// A change to a task's steps (tick, untick, add, remove, or new text); reversing puts the
    /// `previous` steps back.
    Steps {
        id: Uuid,
        previous: Vec<Step>,
        expected_revision: Uuid,
    },
    /// A board archive or unarchive (`ctrl+f`); reversing puts the `previous` flag back.
    Archive {
        id: Uuid,
        previous: bool,
        expected_revision: Uuid,
    },
    /// A change to a task's `after` list (set, chain, or a delete that unlinked it); reversing
    /// puts `previous` back.
    SetAfter {
        id: Uuid,
        previous: Vec<u64>,
        expected_revision: Uuid,
    },
    /// A change to who the open block or review round waits on; reversing puts `previous`
    /// back on the same record.
    SetOn {
        id: Uuid,
        previous: BlockOn,
        expected_revision: Uuid,
    },
    /// A person name removed from the assignee picker; reversing puts it back. It targets no
    /// task, so no revision guard applies.
    HideAssignee {
        name: String,
    },
    /// An assignment put a removed name back on the picker; reversing hides it again. It rides
    /// in the assignment's entry, or alone when the assignment changed nothing else.
    ShowAssignee {
        name: String,
    },
    Batch {
        entries: Vec<UndoEntry>,
    },
}

impl UndoEntry {
    /// Return the single-task entries in action order, flattening nested batches.
    pub(crate) fn leaf_entries(&self) -> Vec<&UndoEntry> {
        fn collect<'a>(entry: &'a UndoEntry, leaves: &mut Vec<&'a UndoEntry>) {
            match entry {
                UndoEntry::Batch { entries } => {
                    for child in entries {
                        collect(child, leaves);
                    }
                }
                UndoEntry::SoftDelete { .. }
                | UndoEntry::Complete { .. }
                | UndoEntry::Assign { .. }
                | UndoEntry::SetBase { .. }
                | UndoEntry::Block { .. }
                | UndoEntry::Start { .. }
                | UndoEntry::Status { .. }
                | UndoEntry::Steps { .. }
                | UndoEntry::Archive { .. }
                | UndoEntry::SetAfter { .. }
                | UndoEntry::SetOn { .. }
                | UndoEntry::HideAssignee { .. }
                | UndoEntry::ShowAssignee { .. } => leaves.push(entry),
            }
        }

        let mut leaves = Vec::new();
        collect(self, &mut leaves);
        leaves
    }

    pub(crate) fn targets(&self) -> Vec<(Uuid, Uuid)> {
        self.leaf_entries()
            .into_iter()
            .filter_map(|entry| match *entry {
                UndoEntry::SoftDelete {
                    id,
                    expected_revision,
                }
                | UndoEntry::Complete {
                    id,
                    expected_revision,
                    ..
                }
                | UndoEntry::Assign {
                    id,
                    expected_revision,
                    ..
                }
                | UndoEntry::SetBase {
                    id,
                    expected_revision,
                    ..
                }
                | UndoEntry::Block {
                    id,
                    expected_revision,
                    ..
                }
                | UndoEntry::Start {
                    id,
                    expected_revision,
                    ..
                }
                | UndoEntry::Status {
                    id,
                    expected_revision,
                    ..
                }
                | UndoEntry::Steps {
                    id,
                    expected_revision,
                    ..
                }
                | UndoEntry::Archive {
                    id,
                    expected_revision,
                    ..
                }
                | UndoEntry::SetAfter {
                    id,
                    expected_revision,
                    ..
                }
                | UndoEntry::SetOn {
                    id,
                    expected_revision,
                    ..
                } => Some((id, expected_revision)),
                UndoEntry::HideAssignee { .. } | UndoEntry::ShowAssignee { .. } => None,
                UndoEntry::Batch { .. } => unreachable!("batches are flattened"),
            })
            .collect()
    }

    /// Point every leaf that expects `id` at `from` to expect `to` instead.
    pub(crate) fn retarget(&mut self, id: Uuid, from: Uuid, to: Uuid) {
        match self {
            UndoEntry::Batch { entries } => {
                for entry in entries {
                    entry.retarget(id, from, to);
                }
            }
            UndoEntry::SoftDelete {
                id: target,
                expected_revision,
            }
            | UndoEntry::Complete {
                id: target,
                expected_revision,
                ..
            }
            | UndoEntry::Assign {
                id: target,
                expected_revision,
                ..
            }
            | UndoEntry::SetBase {
                id: target,
                expected_revision,
                ..
            }
            | UndoEntry::Block {
                id: target,
                expected_revision,
                ..
            }
            | UndoEntry::Start {
                id: target,
                expected_revision,
                ..
            }
            | UndoEntry::Status {
                id: target,
                expected_revision,
                ..
            }
            | UndoEntry::Steps {
                id: target,
                expected_revision,
                ..
            }
            | UndoEntry::Archive {
                id: target,
                expected_revision,
                ..
            }
            | UndoEntry::SetAfter {
                id: target,
                expected_revision,
                ..
            }
            | UndoEntry::SetOn {
                id: target,
                expected_revision,
                ..
            } => {
                if *target == id && *expected_revision == from {
                    *expected_revision = to;
                }
            }
            UndoEntry::HideAssignee { .. } | UndoEntry::ShowAssignee { .. } => {}
        }
    }

    fn reverse(self, state: &mut DomainState) -> Result<(), DomainError> {
        match self {
            UndoEntry::SoftDelete { id, .. } => state.restore(id),
            UndoEntry::Complete { id, previous, .. } => state.restore_uncompleted(id, previous),
            UndoEntry::Assign {
                id,
                previous,
                previous_kind,
                ..
            } => state.restore_assignee(id, previous, previous_kind),
            UndoEntry::SetBase { id, previous, .. } => state.restore_base(id, previous),
            UndoEntry::Block { id, previous, .. } => state.restore_unblocked(id, previous),
            UndoEntry::Start { id, previous, .. } | UndoEntry::Status { id, previous, .. } => {
                state.restore_status(id, previous)
            }
            UndoEntry::Steps { id, previous, .. } => state.restore_steps(id, previous),
            UndoEntry::Archive { id, previous, .. } => state.restore_archived(id, previous),
            UndoEntry::SetAfter { id, previous, .. } => state.restore_after(id, previous),
            UndoEntry::SetOn { id, previous, .. } => state.restore_on(id, previous),
            UndoEntry::HideAssignee { name } => {
                state.unhide_assignee(&name);
                Ok(())
            }
            UndoEntry::ShowAssignee { name } => {
                state.rehide_assignee(&name);
                Ok(())
            }
            UndoEntry::Batch { entries } => {
                for entry in entries.into_iter().rev() {
                    entry.reverse(state)?;
                }
                Ok(())
            }
        }
    }
}

impl DomainState {
    /// True when `entry` is the top of the undo stack and every task it targets is still at
    /// the revision it recorded, so `undo` would reverse exactly that entry.
    pub(crate) fn undo_entry_is_live_top(&self, entry: &UndoEntry) -> bool {
        self.last_undo() == Some(entry)
            && entry
                .targets()
                .into_iter()
                .all(|(id, revision)| self.get(id).is_some_and(|task| task.revision == revision))
    }

    /// Apply the inverse of the last undo entry when its target revision still matches.
    ///
    /// Empty stack is a documented no-op. Stale entries are retained so a refused Undo never
    /// changes durable state or exposes an older entry accidentally.
    pub fn undo(&mut self) -> Result<(), DomainError> {
        let Some(entry) = self.last_undo().cloned() else {
            return Ok(());
        };
        for (id, expected_revision) in entry.targets() {
            let current_revision = self.get(id).ok_or(DomainError::UnknownId(id))?.revision;
            if current_revision != expected_revision {
                return Err(DomainError::StaleUndo(id));
            }
        }

        self.pop_undo()
            .expect("undo entry remains present after revision checks")
            .reverse(self)
    }

    /// Whether the top undo entry would apply now: every task it targets is still at the
    /// revision it recorded.
    pub fn top_undo_is_live(&self) -> bool {
        self.last_undo().is_some_and(|entry| {
            entry.targets().into_iter().all(|(id, expected_revision)| {
                self.get(id).map(|task| task.revision) == Some(expected_revision)
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{HumanStatus, ProvenanceOrigin, TaskScope};

    static TEMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn temp_state_dir(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let seq = TEMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tsk-undo-cap-{label}-{nanos}-{seq}"));
        std::fs::create_dir_all(&dir).expect("create temp state dir");
        dir
    }

    struct TempDirGuard(std::path::PathBuf);
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn create_sample(state: &mut DomainState) -> Uuid {
        state
            .create(
                "Fix flake",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("valid title creates a task")
    }

    fn persisted_undo_len(state: &DomainState) -> usize {
        serde_json::to_value(state).expect("state serializes")["undo_stack"]
            .as_array()
            .expect("undo_stack is an array")
            .len()
    }

    #[test]
    fn undo_cap_is_fifty() {
        assert_eq!(UNDO_CAP, 50);
    }

    #[test]
    fn save_keeps_exactly_fifty_undo_entries_and_evicts_the_oldest() {
        let dir = temp_state_dir("cap");
        let _guard = TempDirGuard(dir.clone());
        let store = crate::store::TaskStore::new(&dir);
        let mut state = DomainState::new();
        let mut ids = Vec::new();
        for index in 0..=UNDO_CAP {
            let id = state
                .create(
                    format!("task {index}"),
                    None,
                    TaskScope::Global,
                    ProvenanceOrigin::Manual,
                    None,
                )
                .expect("create");
            state.complete(id).expect("complete");
            ids.push(id);
        }
        assert_eq!(persisted_undo_len(&state), UNDO_CAP + 1);

        store.save(&state).expect("save");
        let mut loaded = store.load().expect("reload");
        assert_eq!(
            persisted_undo_len(&loaded),
            UNDO_CAP,
            "the saved document carries exactly UNDO_CAP entries"
        );

        for _ in 0..UNDO_CAP {
            loaded.undo().expect("undo a kept entry");
        }
        assert_eq!(
            loaded.get(ids[0]).expect("task 0").status,
            HumanStatus::Done,
            "the evicted oldest entry can no longer undo its task"
        );
        assert_eq!(
            loaded.get(ids[1]).expect("task 1").status,
            HumanStatus::Open,
            "the oldest kept entry still undoes its task"
        );
    }

    #[test]
    fn save_drops_stale_undo_entries_and_keeps_live_ones_beneath_them() {
        let dir = temp_state_dir("stale");
        let _guard = TempDirGuard(dir.clone());
        let store = crate::store::TaskStore::new(&dir);
        let mut state = DomainState::new();
        let stale_target = create_sample(&mut state);
        let live_target = create_sample(&mut state);
        state.complete(stale_target).expect("complete");
        state
            .edit(
                stale_target,
                "edited after the undoable action",
                None,
                TaskScope::Global,
                None,
            )
            .expect("edit moves the revision on");
        state.complete(live_target).expect("complete");
        assert_eq!(persisted_undo_len(&state), 2);

        store.save(&state).expect("save");
        let mut loaded = store.load().expect("reload");
        assert_eq!(
            persisted_undo_len(&loaded),
            1,
            "the stale entry is absent from the saved document"
        );
        loaded.undo().expect("undo the kept entry");
        assert_eq!(
            loaded.get(live_target).expect("live task").status,
            HumanStatus::Open,
            "the live entry beneath the stale one is kept"
        );
        assert_eq!(
            loaded.get(stale_target).expect("stale task").status,
            HumanStatus::Done,
            "the stale entry's task is untouched"
        );
    }

    #[test]
    fn stale_top_entry_still_refuses_in_memory_because_pruning_runs_only_at_save() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        state.complete(id).expect("complete");
        state
            .edit(id, "changed", None, TaskScope::Global, None)
            .expect("edit");

        let error = state.undo().expect_err("a stale top entry refuses");
        assert_eq!(error, DomainError::StaleUndo(id));
        assert_eq!(
            persisted_undo_len(&state),
            1,
            "a refused undo retains the entry in memory"
        );
    }

    #[test]
    fn prune_runs_before_the_cap_so_a_dead_entry_never_evicts_a_live_one() {
        let dir = temp_state_dir("order");
        let _guard = TempDirGuard(dir.clone());
        let store = crate::store::TaskStore::new(&dir);
        let mut state = DomainState::new();
        let mut ids = Vec::new();
        for index in 0..=UNDO_CAP {
            let id = state
                .create(
                    format!("task {index}"),
                    None,
                    TaskScope::Global,
                    ProvenanceOrigin::Manual,
                    None,
                )
                .expect("create");
            state.complete(id).expect("complete");
            ids.push(id);
        }
        // Task 25's entry goes stale: 51 entries, one stale in the middle.
        state
            .edit(ids[25], "moved on", None, TaskScope::Global, None)
            .expect("edit");

        store.save(&state).expect("save");
        let mut loaded = store.load().expect("reload");
        assert_eq!(
            persisted_undo_len(&loaded),
            UNDO_CAP,
            "prune-first drops only the stale entry; the cap then does nothing"
        );
        for _ in 0..UNDO_CAP {
            loaded.undo().expect("every kept entry undoes");
        }
        assert_eq!(
            loaded.get(ids[0]).expect("task 0").status,
            HumanStatus::Open,
            "a cap-first prune would have evicted this live oldest entry"
        );
    }

    #[test]
    fn cap_runs_after_merge_undo_entries_union() {
        let dir = temp_state_dir("merge-then-cap");
        let _guard = TempDirGuard(dir.clone());
        let store = crate::store::TaskStore::new(&dir);
        // Disk side: UNDO_CAP/2 completions saved by one writer.
        let mut disk_side = DomainState::new();
        for index in 0..UNDO_CAP / 2 {
            let id = disk_side
                .create(
                    format!("disk {index}"),
                    None,
                    TaskScope::Global,
                    ProvenanceOrigin::Manual,
                    None,
                )
                .expect("create");
            disk_side.complete(id).expect("complete");
        }
        store.save(&disk_side).expect("seed disk");

        // Local side: UNDO_CAP completions of its own, merged with the disk entries on save.
        let mut local = store.load().expect("load");
        for index in 0..UNDO_CAP {
            let id = local
                .create(
                    format!("local {index}"),
                    None,
                    TaskScope::Global,
                    ProvenanceOrigin::Manual,
                    None,
                )
                .expect("create");
            local.complete(id).expect("complete");
        }
        store.reload_merge_save(&mut local).expect("merge-save");

        let loaded = store.load().expect("reload");
        assert_eq!(
            persisted_undo_len(&loaded),
            UNDO_CAP,
            "the union is capped after merging, not before"
        );
    }

    #[test]
    fn bulk_complete_pushes_one_ordered_deduped_batch_and_one_undo_reopens_all() {
        let mut state = DomainState::new();
        let first = create_sample(&mut state);
        let second = create_sample(&mut state);

        state
            .complete_batch(&[second, first, second])
            .expect("bulk complete");

        assert_eq!(state.get(first).expect("first").status, HumanStatus::Done);
        assert_eq!(state.get(second).expect("second").status, HumanStatus::Done);
        let undo = serde_json::to_value(state.last_undo().expect("one batch undo")).expect("json");
        let entries = undo["batch"]["entries"].as_array().expect("batch entries");
        assert_eq!(entries.len(), 2, "duplicate ids collapse");
        assert_eq!(entries[0]["complete"]["id"], second.to_string());
        assert_eq!(entries[1]["complete"]["id"], first.to_string());

        state.undo().expect("undo whole batch");
        assert_eq!(state.get(first).expect("first").status, HumanStatus::Open);
        assert_eq!(state.get(second).expect("second").status, HumanStatus::Open);
        assert!(state.last_undo().is_none(), "one undo consumes the batch");
    }

    #[test]
    fn bulk_complete_undo_only_reopens_tasks_changed_by_the_batch() {
        let mut state = DomainState::new();
        let already_done = create_sample(&mut state);
        let newly_done = create_sample(&mut state);
        state.complete(already_done).expect("complete first task");

        state
            .complete_batch(&[already_done, newly_done])
            .expect("complete mixed batch");
        let undo = serde_json::to_value(state.last_undo().expect("batch undo")).expect("json");
        assert_eq!(
            undo["batch"]["entries"]
                .as_array()
                .expect("batch entries")
                .len(),
            1,
            "the batch records only its changed task"
        );

        state.undo().expect("undo mixed batch");
        assert_eq!(
            state.get(already_done).expect("already done").status,
            HumanStatus::Done
        );
        assert_eq!(
            state.get(newly_done).expect("newly done").status,
            HumanStatus::Open
        );
    }

    #[test]
    fn bulk_verbs_prevalidate_every_id_before_mutating() {
        for complete in [false, true] {
            let mut state = DomainState::new();
            let first = create_sample(&mut state);
            let missing = Uuid::new_v4();
            let before = serde_json::to_value(&state).expect("snapshot");

            let result = if complete {
                state.complete_batch(&[first, missing])
            } else {
                state.soft_delete_batch(&[first, missing])
            };
            assert_eq!(result, Err(DomainError::UnknownId(missing)));
            assert_eq!(serde_json::to_value(&state).expect("snapshot"), before);
        }
    }

    #[test]
    fn stale_batch_child_refuses_without_consuming_or_reversing_any_child() {
        let mut state = DomainState::new();
        let first = create_sample(&mut state);
        let second = create_sample(&mut state);
        state
            .complete_batch(&[first, second])
            .expect("bulk complete");
        state
            .edit(second, "changed", None, TaskScope::Global, None)
            .expect("make the second child stale");
        let before = serde_json::to_value(&state).expect("snapshot");

        assert_eq!(state.undo(), Err(DomainError::StaleUndo(second)));
        assert_eq!(
            serde_json::to_value(&state).expect("snapshot"),
            before,
            "refused batch undo changes neither tasks nor stack"
        );
    }

    #[test]
    fn persistence_prunes_a_whole_batch_when_one_child_is_stale() {
        let dir = temp_state_dir("stale-batch");
        let _guard = TempDirGuard(dir.clone());
        let store = crate::store::TaskStore::new(&dir);
        let mut state = DomainState::new();
        let first = create_sample(&mut state);
        let second = create_sample(&mut state);
        state
            .complete_batch(&[first, second])
            .expect("bulk complete");
        state
            .edit(second, "changed", None, TaskScope::Global, None)
            .expect("make one child stale");

        store.save(&state).expect("save prunes stale batch");
        let loaded = store.load().expect("reload");
        assert!(loaded.last_undo().is_none(), "the batch stays atomic");
    }

    #[test]
    fn undo_merge_dedupes_single_entries_already_held_by_a_batch() {
        let mut state = DomainState::new();
        let first = create_sample(&mut state);
        let second = create_sample(&mut state);
        state
            .complete_batch(&[first, second])
            .expect("bulk complete");

        let mut other_document = serde_json::to_value(&state).expect("state json");
        let children = other_document["undo_stack"][0]["batch"]["entries"]
            .as_array()
            .expect("batch children")
            .clone();
        other_document["undo_stack"] = serde_json::Value::Array(children);
        let other: DomainState = serde_json::from_value(other_document).expect("single stack");

        state.merge_tasks_from_disk(&other);
        let merged = serde_json::to_value(&state).expect("merged json");
        assert_eq!(
            merged["undo_stack"].as_array().expect("undo stack").len(),
            1,
            "leaf-equivalent entries are not duplicated across batch boundaries"
        );
    }

    #[test]
    fn bulk_soft_delete_remains_live_and_undoable_across_an_immediate_save() {
        let dir = temp_state_dir("batch-trash-targets");
        let _guard = TempDirGuard(dir.clone());
        let store = crate::store::TaskStore::new(&dir);
        let mut state = DomainState::new();
        let first = create_sample(&mut state);
        let second = create_sample(&mut state);
        state
            .soft_delete_batch(&[first, second])
            .expect("bulk soft delete");

        store.save(&state).expect("save");
        assert!(
            !dir.join("trash.jsonl").exists(),
            "children of the same batch are not later actions"
        );
        let mut loaded = store.load().expect("reload");
        assert!(loaded.get(first).expect("first").soft_deleted);
        assert!(loaded.get(second).expect("second").soft_deleted);
        loaded.undo().expect("undo bulk delete");
        assert!(!loaded.get(first).expect("first").soft_deleted);
        assert!(!loaded.get(second).expect("second").soft_deleted);
    }

    #[test]
    fn old_single_entry_variants_still_deserialize() {
        let id = Uuid::new_v4();
        let expected_revision = Uuid::new_v4();
        for (variant, expected) in [
            (
                "soft_delete",
                UndoEntry::SoftDelete {
                    id,
                    expected_revision,
                },
            ),
            (
                "complete",
                UndoEntry::Complete {
                    id,
                    previous: HumanStatus::Open,
                    expected_revision,
                },
            ),
        ] {
            let value = serde_json::json!({
                (variant): { "id": id, "expected_revision": expected_revision }
            });
            assert_eq!(
                serde_json::from_value::<UndoEntry>(value).expect("old variant"),
                expected
            );
        }
    }

    #[test]
    fn soft_delete_then_undo_clears_soft_deleted() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        state.soft_delete(id).expect("soft_delete");
        assert!(state.get(id).expect("task exists").soft_deleted);

        state.undo().expect("undo after soft_delete");

        let task = state.get(id).expect("task still in store");
        assert!(!task.soft_deleted);
        assert_eq!(task.status, HumanStatus::Open);
        assert_eq!(task.title, "Fix flake");
    }

    #[test]
    fn complete_then_undo_returns_status_open() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        state.complete(id).expect("complete");
        assert_eq!(
            state.get(id).expect("task exists").status,
            HumanStatus::Done
        );

        state.undo().expect("undo after complete");

        assert_eq!(
            state.get(id).expect("task exists").status,
            HumanStatus::Open
        );
    }

    /// Assigning a person over an agent of the same name is a change, and undo puts the
    /// agent back with its kind, so its start dispatches again.
    #[test]
    fn undoing_an_assignment_restores_the_previous_kind() {
        use crate::domain::{Assignee, AssigneeKind};
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        state
            .assign(id, Some(Assignee::agent("dana")))
            .expect("agent");
        assert!(state
            .assign(id, Some(Assignee::person("dana")))
            .expect("person"));
        assert_eq!(state.get(id).unwrap().assignee_kind, AssigneeKind::Person);
        assert_eq!(state.get(id).unwrap().agent_assignee(), None);

        state.undo().expect("undo");
        let task = state.get(id).unwrap();
        assert_eq!(task.assigned(), Some(Assignee::agent("dana")));
        assert_eq!(task.agent_assignee(), Some("dana"));

        state.assign(id, Some(Assignee::you())).expect("you");
        state.assign(id, None).expect("unassign");
        assert_eq!(
            state.get(id).unwrap().assignee_kind,
            AssigneeKind::Agent,
            "an unassigned task carries the default kind"
        );
        state.undo().expect("undo unassign");
        assert_eq!(state.get(id).unwrap().assigned(), Some(Assignee::you()));
    }

    #[test]
    fn bulk_assignment_is_one_batch_and_one_undo_restores_every_previous_value() {
        let mut state = DomainState::new();
        let first = create_sample(&mut state);
        let second = state
            .create(
                "second",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create second");
        state
            .assign(first, Some("old-agent".into()))
            .expect("seed assignment");
        while state.pop_undo().is_some() {}

        state
            .assign_batch(&[second, first, first], Some("new-agent".into()))
            .expect("bulk assign");
        assert_eq!(persisted_undo_len(&state), 1);
        assert!(
            matches!(state.last_undo(), Some(UndoEntry::Batch { entries }) if entries.len() == 2)
        );
        assert_eq!(
            state.get(first).expect("first").assignee.as_deref(),
            Some("new-agent")
        );
        assert_eq!(
            state.get(second).expect("second").assignee.as_deref(),
            Some("new-agent")
        );

        state.undo().expect("undo batch");
        assert_eq!(
            state.get(first).expect("first").assignee.as_deref(),
            Some("old-agent")
        );
        assert_eq!(state.get(second).expect("second").assignee, None);
    }

    #[test]
    fn repeated_assignment_is_a_noop_without_an_undo_entry() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        state.assign(id, Some("agent".into())).expect("assign");
        while state.pop_undo().is_some() {}

        state.assign(id, Some("agent".into())).expect("repeat");
        assert_eq!(persisted_undo_len(&state), 0);
    }

    #[test]
    fn bulk_base_change_is_one_batch_and_one_undo_restores_every_previous_value() {
        let mut state = DomainState::new();
        let first = create_sample(&mut state);
        let second = state
            .create(
                "second",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create second");
        state
            .set_base(first, Some("release".into()))
            .expect("seed base");
        while state.pop_undo().is_some() {}

        state
            .set_base_batch(&[second, first, first], Some("dispatch".into()))
            .expect("bulk base");
        assert_eq!(persisted_undo_len(&state), 1);
        assert!(
            matches!(state.last_undo(), Some(UndoEntry::Batch { entries }) if entries.len() == 2)
        );
        assert_eq!(
            state.get(first).expect("first").base.as_deref(),
            Some("dispatch")
        );
        assert_eq!(
            state.get(second).expect("second").base.as_deref(),
            Some("dispatch")
        );

        state.undo().expect("undo batch");
        assert_eq!(
            state.get(first).expect("first").base.as_deref(),
            Some("release")
        );
        assert_eq!(state.get(second).expect("second").base, None);
    }

    /// The statuses a done can leave, each set up the way the board reaches it.
    fn task_in(state: &mut DomainState, status: HumanStatus) -> Uuid {
        let id = create_sample(state);
        match status {
            HumanStatus::Open => {}
            HumanStatus::Blocked => state
                .block(
                    id,
                    crate::domain::BlockDraft {
                        why: Some("waiting on design".into()),
                        ..Default::default()
                    },
                    "you",
                )
                .expect("block"),
            other => state.set_status(id, other).expect("set status"),
        }
        id
    }

    const BEFORE_DONE: [HumanStatus; 5] = [
        HumanStatus::Open,
        HumanStatus::Ready,
        HumanStatus::Started,
        HumanStatus::Blocked,
        HumanStatus::Review,
    ];

    #[test]
    fn undo_of_done_restores_the_status_it_left() {
        for previous in BEFORE_DONE {
            let mut state = DomainState::new();
            let id = task_in(&mut state, previous);
            state.complete(id).expect("done");
            assert_eq!(
                state.last_undo(),
                Some(&UndoEntry::Complete {
                    id,
                    previous,
                    expected_revision: state.get(id).expect("task").revision,
                })
            );
            state.undo().expect("undo the done");
            let task = state.get(id).expect("task");
            assert_eq!(task.status, previous, "undo of a done from {previous:?}");
            assert_eq!(
                task.history.last().map(|event| event.kind),
                Some(crate::domain::TaskEventKind::Reopened)
            );
        }
    }

    #[test]
    fn undo_of_a_marked_done_restores_each_status_it_left() {
        let mut state = DomainState::new();
        let ids: Vec<Uuid> = BEFORE_DONE
            .iter()
            .map(|status| task_in(&mut state, *status))
            .collect();
        state.complete_batch(&ids).expect("bulk done");
        assert!(ids
            .iter()
            .all(|id| state.get(*id).expect("task").status == HumanStatus::Done));
        state.undo().expect("undo the bulk done");
        for (id, previous) in ids.iter().zip(BEFORE_DONE) {
            assert_eq!(state.get(*id).expect("task").status, previous);
        }
    }

    #[test]
    fn undo_of_done_from_blocked_reopens_the_same_block() {
        let mut state = DomainState::new();
        let id = task_in(&mut state, HumanStatus::Blocked);
        state.complete(id).expect("done");
        assert!(state.get(id).expect("task").block.is_none());
        state.undo().expect("undo the done");
        let task = state.get(id).expect("task");
        let block = task.block.as_ref().expect("the block is open again");
        assert_eq!(block.why.as_deref(), Some("waiting on design"));
        assert_eq!(block.closed_at, None);
    }

    #[test]
    fn undo_of_done_survives_a_save_and_reload() {
        let dir = temp_state_dir("previous");
        let _guard = TempDirGuard(dir.clone());
        let store = crate::store::TaskStore::new(&dir);
        let mut state = DomainState::new();
        let id = task_in(&mut state, HumanStatus::Started);
        state.complete(id).expect("done");
        store.save(&state).expect("save");
        let mut loaded = store.load().expect("reload");
        loaded.undo().expect("undo the done");
        assert_eq!(loaded.get(id).expect("task").status, HumanStatus::Started);
    }

    #[test]
    fn undo_of_a_reopen_puts_the_done_back_single_and_marked() {
        let mut state = DomainState::new();
        let first = task_in(&mut state, HumanStatus::Started);
        let second = task_in(&mut state, HumanStatus::Blocked);
        state.complete(first).expect("done");
        state.complete(second).expect("done");
        state.take_completed();

        assert!(state
            .change_status(&[first], HumanStatus::Open, false)
            .expect("reopen"));
        assert!(matches!(state.last_undo(), Some(UndoEntry::Status { .. })));
        state.undo().expect("undo the reopen");
        assert_eq!(state.get(first).expect("first").status, HumanStatus::Done);

        assert!(state
            .change_status(&[first, second], HumanStatus::Open, true)
            .expect("marked reopen"));
        assert!(
            matches!(state.last_undo(), Some(UndoEntry::Batch { entries }) if entries.len() == 2)
        );
        state.undo().expect("undo the marked reopen");
        for id in [first, second] {
            assert_eq!(state.get(id).expect("task").status, HumanStatus::Done);
        }
        assert!(
            state.get(second).expect("second").block.is_none(),
            "done keeps no open block"
        );
        assert!(
            state.take_completed().is_empty(),
            "undoing a reopen releases nothing: the release belonged to the original done"
        );
    }

    #[test]
    fn change_status_skips_tasks_already_there_and_pushes_nothing_when_none_move() {
        let mut state = DomainState::new();
        let id = task_in(&mut state, HumanStatus::Ready);
        while state.pop_undo().is_some() {}
        assert!(!state
            .change_status(&[id], HumanStatus::Ready, false)
            .expect("no-op"));
        assert_eq!(persisted_undo_len(&state), 0);
    }

    #[test]
    fn steps_undo_restores_ticks_text_adds_and_removals_and_skips_no_ops() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        let alpha = state.add_step(id, "alpha").expect("alpha");
        let bravo = state.add_step(id, "bravo").expect("bravo");
        while state.pop_undo().is_some() {}
        let original = state.get(id).expect("task").steps.clone();
        type StepChange = Box<dyn Fn(&mut DomainState) -> Result<(), DomainError>>;
        let changes: [(&str, StepChange); 4] = [
            ("tick", Box::new(move |s| s.toggle_step(id, alpha))),
            (
                "rename",
                Box::new(move |s| s.rename_step(id, bravo, "bravo!")),
            ),
            (
                "add",
                Box::new(move |s| s.add_step(id, "charlie").map(|_| ())),
            ),
            ("remove", Box::new(move |s| s.remove_step(id, alpha))),
        ];
        for (label, change) in changes {
            state.with_steps_undo(id, |s| change(s)).expect(label);
            assert_ne!(state.get(id).expect("task").steps, original, "{label}");
            state.undo().expect(label);
            assert_eq!(
                state.get(id).expect("task").steps,
                original,
                "undo of {label}"
            );
        }
        state.with_steps_undo(id, |_| Ok(())).expect("no-op");
        assert!(
            !matches!(state.last_undo(), Some(UndoEntry::Steps { .. })),
            "a change that leaves the steps alone pushes nothing"
        );
    }

    #[test]
    fn undo_with_empty_stack_is_noop() {
        let mut state = DomainState::new();
        // Documented no-op: empty stack must not panic and must succeed.
        state.undo().expect("empty undo is Ok");
        assert!(state.tasks().is_empty());
    }
}
