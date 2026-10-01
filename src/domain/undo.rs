//! Undo stack for soft-delete and complete.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{DomainError, DomainState};

/// Maximum undo entries retained in a saved document.
pub const UNDO_CAP: usize = 50;

/// One reversible user action on the undo stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UndoEntry {
    SoftDelete {
        id: Uuid,
        expected_revision: Uuid,
    },
    Complete {
        id: Uuid,
        expected_revision: Uuid,
    },
    Assign {
        id: Uuid,
        previous: Option<String>,
        expected_revision: Uuid,
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
                | UndoEntry::Assign { .. } => leaves.push(entry),
            }
        }

        let mut leaves = Vec::new();
        collect(self, &mut leaves);
        leaves
    }

    pub(crate) fn targets(&self) -> Vec<(Uuid, Uuid)> {
        self.leaf_entries()
            .into_iter()
            .map(|entry| match *entry {
                UndoEntry::SoftDelete {
                    id,
                    expected_revision,
                }
                | UndoEntry::Complete {
                    id,
                    expected_revision,
                }
                | UndoEntry::Assign {
                    id,
                    expected_revision,
                    ..
                } => (id, expected_revision),
                UndoEntry::Batch { .. } => unreachable!("batches are flattened"),
            })
            .collect()
    }

    fn reverse(self, state: &mut DomainState) -> Result<(), DomainError> {
        match self {
            UndoEntry::SoftDelete { id, .. } => state.restore(id),
            UndoEntry::Complete { id, .. } => state.reopen(id),
            UndoEntry::Assign { id, previous, .. } => state.restore_assignee(id, previous),
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
    fn undo_with_empty_stack_is_noop() {
        let mut state = DomainState::new();
        // Documented no-op: empty stack must not panic and must succeed.
        state.undo().expect("empty undo is Ok");
        assert!(state.tasks().is_empty());
    }
}
