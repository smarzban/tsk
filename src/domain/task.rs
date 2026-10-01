//! Task model and lifecycle commands.
//!
//! Human status remains the source of truth.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ProvenanceOrigin, TaskEvent, TaskEventKind, UndoEntry, UNDO_CAP};
use crate::scope::paths_equivalent;

/// Human-facing task progress. Source of truth for board state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanStatus {
    Open,
    Ready,
    Started,
    Blocked,
    Review,
    Done,
}

/// Where a task belongs: global or a project identified by stable path scope key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskScope {
    Global,
    Project { path: String },
}

/// One step in a task's flat, ordered steps collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// Stable identity, addressable by id prefix like a task.
    pub id: Uuid,
    /// One line of text, trimmed at the boundaries.
    pub text: String,
    pub done: bool,
}

/// Marks a human-only notice row. A notice never receives a `T` number; the board
/// paints it as `N{number}` and the CLI never lists or addresses it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notice {
    /// Stable catalog key the seeder uses to recognise a notice it already delivered.
    pub catalog_id: String,
    /// Board-only public id. Absent until the locked persistence boundary assigns it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
}

/// One durable record of an agent launch for a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dispatch {
    /// Fully rendered profile argv, retained for inspection after launch.
    pub argv: Vec<String>,
    pub worktree: String,
    pub branch: String,
    /// Branch or commit the dispatch branch was created from. Older v6 records omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    pub herdr_workspace_id: String,
    #[serde(with = "super::time_serde")]
    pub at: SystemTime,
    /// The recorded worktree has been removed or was already missing. The record stays
    /// available for inspection and a deliberate relaunch.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cleaned: bool,
}

/// One unit of intended work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: Uuid,
    /// Store-global human task number. Absent until the locked persistence boundary assigns it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
    /// Present on notice rows only; such a task has no `number`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<Notice>,
    /// Opaque semantic revision used to guard concurrent operations.
    pub revision: Uuid,
    /// Revision observed before an in-memory mutation. It is transient save intent carried to
    /// the locked store merge, never retained after a successful durable write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_base_revision: Option<Uuid>,
    pub title: String,
    pub notes: Option<String>,
    /// Optional normalized thread name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    /// Optional normalized agent profile name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// Last successful dispatch. Status changes never alter this record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch: Option<Dispatch>,
    pub status: HumanStatus,
    pub scope: TaskScope,
    pub provenance: ProvenanceOrigin,
    /// Append-only domain event history.
    pub history: Vec<TaskEvent>,
    /// Flat, ordered steps; never reordered by a verb.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    pub soft_deleted: bool,
    /// Kept, off the radar: hidden from every working lens, visible only in the
    /// archived group of the done drawer. Human status is independent of it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    #[serde(with = "super::time_serde")]
    pub created_at: SystemTime,
    #[serde(with = "super::time_serde")]
    pub updated_at: SystemTime,
}

/// Domain command failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainError {
    /// Title was empty or whitespace-only after trim.
    EmptyTitle,
    /// No task with this id exists in the domain state.
    UnknownId(Uuid),
    /// Command refused because the task is soft-deleted.
    SoftDeleted(Uuid),
    /// Undo target changed after the undoable action was recorded.
    StaleUndo(Uuid),
    /// Step text was empty or whitespace-only after trim.
    EmptyStepText,
    /// No step with this id exists on the task.
    UnknownStep(Uuid),
    /// No project record exists and no task carries this project scope path.
    UnknownProject(String),
}

impl std::fmt::Display for DomainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DomainError::EmptyTitle => write!(f, "title must be non-empty after trim"),
            DomainError::UnknownId(id) => write!(f, "unknown task id {id}"),
            DomainError::SoftDeleted(id) => write!(f, "task {id} is soft-deleted"),
            DomainError::StaleUndo(id) => {
                write!(f, "task {id} changed since the undoable action")
            }
            DomainError::EmptyStepText => write!(f, "step text must be non-empty after trim"),
            DomainError::UnknownStep(id) => write!(f, "unknown step id {id}"),
            DomainError::UnknownProject(path) => write!(f, "unknown project {path}"),
        }
    }
}

impl std::error::Error for DomainError {}

fn record_mutation(task: &mut Task, kind: TaskEventKind) {
    record_mutation_at(task, kind, SystemTime::now());
}

fn record_mutation_at(task: &mut Task, kind: TaskEventKind, at: SystemTime) {
    task.merge_base_revision = Some(task.revision);
    task.revision = Uuid::new_v4();
    task.updated_at = at;
    task.history.push(TaskEvent { kind, at });
}

/// Document version written by this binary.
pub const STORE_FORMAT_VERSION: u32 = 6;

fn default_next_notice_number() -> u64 {
    1
}

/// One per-project record, keyed by the project's scope path. Lazy: a project
/// appears in the map only while it is archived; the set of projects the board
/// shows is still derived from tasks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectRecord {
    pub archived: bool,
}

/// A soft-deleted task older than this moves to `trash.jsonl` at the next save
/// even while its undo entry is still the top of the stack.
const TRASH_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);

impl Task {
    pub fn is_notice(&self) -> bool {
        self.notice.is_some()
    }

    /// The painted public id: `N{n}` for a numbered notice, `T{n}` for a numbered
    /// task, none until the persistence boundary assigns one.
    pub fn board_identifier(&self) -> Option<String> {
        match (&self.notice, self.number) {
            (Some(notice), _) => notice.number.map(|number| format!("N{number}")),
            (None, Some(number)) => Some(format!("T{number}")),
            (None, None) => None,
        }
    }

    /// The `at` of the last `soft_deleted` history event, if any.
    pub fn soft_deleted_at(&self) -> Option<SystemTime> {
        self.last_event_at(TaskEventKind::SoftDeleted)
    }

    /// The `at` of the task's most recent status change (`StatusSet`, `Completed`
    /// or `Reopened`), or `created_at` when no such event exists.
    pub(crate) fn status_changed_at(&self) -> SystemTime {
        self.history
            .iter()
            .rev()
            .find(|event| {
                matches!(
                    event.kind,
                    TaskEventKind::StatusSet | TaskEventKind::Completed | TaskEventKind::Reopened
                )
            })
            .map(|event| event.at)
            .unwrap_or(self.created_at)
    }

    /// The `at` of the last history event of `kind`, if any.
    pub(crate) fn last_event_at(&self, kind: TaskEventKind) -> Option<SystemTime> {
        self.history
            .iter()
            .rev()
            .find(|event| event.kind == kind)
            .map(|event| event.at)
    }
}

/// In-memory task set. Persistence is Task Store.
///
/// SHORTCUT: Vec scan by id; fine until store loads many tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainState {
    /// Store document version.
    format_version: u32,
    /// The next store-global task number, allocated only while holding the store lock.
    pub next_task_number: u64,
    /// The next `N` number for notice rows. Absent from v2 documents, so it defaults.
    #[serde(default = "default_next_notice_number")]
    pub next_notice_number: u64,
    tasks: Vec<Task>,
    /// Per-project records keyed by scope path. Always serialized: an empty map
    /// writes `"projects": {}` so the v2 wire shape is pinned.
    #[serde(default)]
    projects: BTreeMap<String, ProjectRecord>,
    /// This process's not-yet-confirmed project archive/unarchive intents, recorded
    /// by the two verbs (true = archived, false = unarchived). Transient: skipped in
    /// serialization, cleared with the merge bases, and re-applied after every disk
    /// merge so a plain union can never resurrect a record this process just removed.
    #[serde(skip)]
    project_intents: BTreeMap<String, bool>,
    /// LIFO undo records for soft-delete and complete.
    undo_stack: Vec<UndoEntry>,
}

impl Default for DomainState {
    fn default() -> Self {
        Self::new()
    }
}

impl DomainState {
    pub fn new() -> Self {
        Self {
            format_version: STORE_FORMAT_VERSION,
            next_task_number: 1,
            next_notice_number: 1,
            tasks: Vec::new(),
            projects: BTreeMap::new(),
            project_intents: BTreeMap::new(),
            undo_stack: Vec::new(),
        }
    }

    pub fn format_version(&self) -> u32 {
        self.format_version
    }

    /// Inspect the top undo entry without consuming it.
    pub(crate) fn last_undo(&self) -> Option<&UndoEntry> {
        self.undo_stack.last()
    }

    /// Pop the top undo entry after its revision guard has passed.
    pub(crate) fn pop_undo(&mut self) -> Option<UndoEntry> {
        self.undo_stack.pop()
    }

    pub fn tasks(&self) -> &[Task] {
        &self.tasks
    }

    /// Per-project records keyed by scope path. Empty when no project is archived.
    pub fn projects(&self) -> &BTreeMap<String, ProjectRecord> {
        &self.projects
    }

    /// Whether the project record for `path` carries the archived flag.
    pub fn is_project_archived(&self, path: &str) -> bool {
        self.projects
            .iter()
            .any(|(stored, record)| record.archived && paths_equivalent(stored, path))
    }

    /// Scope paths of every archived project, sorted.
    pub fn archived_projects(&self) -> BTreeSet<String> {
        self.projects
            .iter()
            .filter(|(_, record)| record.archived)
            .map(|(path, _)| path.clone())
            .collect()
    }

    /// The one hidden predicate every working lens filters on: the task is archived,
    /// or its project is archived.
    pub fn is_hidden(&self, task: &Task) -> bool {
        match &task.scope {
            TaskScope::Global => task.archived,
            TaskScope::Project { path } => task.archived || self.is_project_archived(path),
        }
    }

    /// Archive a project: write its lazy record. Tasks are untouched; their own
    /// archived flags are independent of this. Records exist only while archived, so
    /// unarchiving removes the record entirely.
    ///
    /// Returns `Ok(false)` when the requested state already holds: no record change,
    /// no intent. Unknown when no task (any status, soft-deleted included) carries the
    /// scope and no record exists. Neither verb touches tasks, history, or undo.
    pub fn archive_project(&mut self, path: &str) -> Result<bool, DomainError> {
        if self.is_project_archived(path) {
            return Ok(false);
        }
        if !self.has_project_task(path) && !self.projects.contains_key(path) {
            return Err(DomainError::UnknownProject(path.to_string()));
        }
        let stored_path = self
            .projects
            .keys()
            .find(|stored| paths_equivalent(stored, path))
            .cloned()
            .unwrap_or_else(|| path.to_string());
        self.projects
            .insert(stored_path.clone(), ProjectRecord { archived: true });
        self.project_intents.insert(stored_path, true);
        Ok(true)
    }

    /// Unarchive a project: remove its record. `Ok(false)` when no record exists but
    /// tasks carry the scope (already unarchived); unknown when neither.
    pub fn unarchive_project(&mut self, path: &str) -> Result<bool, DomainError> {
        if let Some(stored_path) = self
            .projects
            .keys()
            .find(|stored| paths_equivalent(stored, path))
            .cloned()
        {
            self.projects.remove(&stored_path);
            self.project_intents.insert(stored_path, false);
            return Ok(true);
        }
        if self.has_project_task(path) {
            return Ok(false);
        }
        Err(DomainError::UnknownProject(path.to_string()))
    }

    fn has_project_task(&self, path: &str) -> bool {
        self.tasks.iter().any(|task| {
            matches!(&task.scope, TaskScope::Project { path: task_path } if paths_equivalent(task_path, path))
        })
    }

    /// Lookup by id. Soft-deleted tasks remain findable.
    pub fn get(&self, id: Uuid) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// Create a task with human status `open`, provenance, and an optional normalized thread.
    ///
    /// Rejects empty/whitespace-only titles. On success, stores exactly one task with the
    /// trimmed title and optional notes, and appends a `Created` event.
    pub fn create(
        &mut self,
        title: impl AsRef<str>,
        notes: Option<String>,
        scope: TaskScope,
        provenance: ProvenanceOrigin,
        thread: Option<String>,
    ) -> Result<Uuid, DomainError> {
        let title = title.as_ref().trim();
        if title.is_empty() {
            return Err(DomainError::EmptyTitle);
        }

        let now = SystemTime::now();
        let id = Uuid::new_v4();
        self.tasks.push(Task {
            id,
            number: None,
            notice: None,
            revision: Uuid::new_v4(),
            merge_base_revision: None,
            title: title.to_string(),
            notes,
            thread,
            assignee: None,
            dispatch: None,
            status: HumanStatus::Open,
            scope,
            provenance,
            history: vec![TaskEvent {
                kind: TaskEventKind::Created,
                at: now,
            }],
            steps: Vec::new(),
            soft_deleted: false,
            archived: false,
            created_at: now,
            updated_at: now,
        });
        Ok(id)
    }

    /// Create with an optional validated assignee in the same creation mutation.
    pub fn create_assigned(
        &mut self,
        title: impl AsRef<str>,
        notes: Option<String>,
        scope: TaskScope,
        provenance: ProvenanceOrigin,
        thread: Option<String>,
        assignee: Option<String>,
    ) -> Result<Uuid, DomainError> {
        let id = self.create(title, notes, scope, provenance, thread)?;
        self.task_mut(id)?.assignee = assignee;
        Ok(id)
    }

    /// Seeder-only create for a human notice row. Marks the task as a notice with the
    /// given status and steps (text, checked, no step events); history is the single
    /// `Created` event, as for `create`. The `N` number is assigned only under the
    /// store lock in [`Self::assign_numbers_for_persistence`], the same boundary that
    /// numbers tasks.
    pub fn create_notice(
        &mut self,
        catalog_id: impl Into<String>,
        title: impl AsRef<str>,
        notes: Option<String>,
        status: HumanStatus,
        scope: TaskScope,
        steps: Vec<(String, bool)>,
    ) -> Result<Uuid, DomainError> {
        let id = self.create(title, notes, scope, ProvenanceOrigin::Manual, None)?;
        let task = self.task_mut(id).expect("the notice was just pushed");
        task.notice = Some(Notice {
            catalog_id: catalog_id.into(),
            number: None,
        });
        task.status = status;
        task.steps = steps
            .into_iter()
            .map(|(text, done)| Step {
                id: Uuid::new_v4(),
                text,
                done,
            })
            .collect();
        Ok(id)
    }

    /// Set human status. Human status is truth; callers are user commands only.
    pub fn set_status(&mut self, id: Uuid, status: HumanStatus) -> Result<(), DomainError> {
        self.apply_status(id, status, TaskEventKind::StatusSet)
    }

    /// Complete: set human status to `done`. Pushes an undo entry.
    pub fn complete(&mut self, id: Uuid) -> Result<(), DomainError> {
        self.apply_status(id, HumanStatus::Done, TaskEventKind::Completed)?;
        let expected_revision = self.get(id).expect("completed task exists").revision;
        self.undo_stack.push(UndoEntry::Complete {
            id,
            expected_revision,
        });
        Ok(())
    }

    /// Complete after cleanup mutated the same task in this unsaved transaction.
    ///
    /// The cleanup revision must keep its original durable merge base across the completion,
    /// while the completion alone remains the undoable part.
    pub fn complete_after_cleanup(&mut self, id: Uuid) -> Result<(), DomainError> {
        let cleanup_merge_base = self.task_mut(id)?.merge_base_revision;
        self.complete(id)?;
        if cleanup_merge_base.is_some() {
            self.task_mut(id)?.merge_base_revision = cleanup_merge_base;
        }
        Ok(())
    }

    /// Complete an ordered set of tasks as one atomic, undoable action.
    /// Duplicate ids keep their first position. Empty input is a no-op.
    pub fn complete_batch(&mut self, ids: &[Uuid]) -> Result<(), DomainError> {
        let ids: Vec<_> = self
            .prevalidate_batch_ids(ids)?
            .into_iter()
            .filter(|id| {
                self.get(*id)
                    .is_some_and(|task| task.status != HumanStatus::Done)
            })
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        let at = SystemTime::now();
        let mut entries = Vec::with_capacity(ids.len());
        for id in ids {
            let task = self.task_mut(id)?;
            task.status = HumanStatus::Done;
            record_mutation_at(task, TaskEventKind::Completed, at);
            entries.push(UndoEntry::Complete {
                id,
                expected_revision: task.revision,
            });
        }
        self.undo_stack.push(UndoEntry::Batch { entries });
        Ok(())
    }

    /// Reopen a `done` task to `open` (inbox).
    pub fn reopen(&mut self, id: Uuid) -> Result<(), DomainError> {
        self.apply_status(id, HumanStatus::Open, TaskEventKind::Reopened)
    }

    /// Soft-delete: mark excluded from board views until restore. Stays in store.
    /// Pushes an undo entry so `undo` can restore.
    pub fn soft_delete(&mut self, id: Uuid) -> Result<(), DomainError> {
        let expected_revision = {
            let task = self.task_mut(id)?;
            task.soft_deleted = true;
            record_mutation(task, TaskEventKind::SoftDeleted);
            task.revision
        };
        self.undo_stack.push(UndoEntry::SoftDelete {
            id,
            expected_revision,
        });
        Ok(())
    }

    /// Soft-delete an ordered set of tasks as one atomic, undoable action.
    /// Duplicate ids keep their first position. Empty input is a no-op.
    pub fn soft_delete_batch(&mut self, ids: &[Uuid]) -> Result<(), DomainError> {
        let ids: Vec<_> = self
            .prevalidate_batch_ids(ids)?
            .into_iter()
            .filter(|id| self.get(*id).is_some_and(|task| !task.soft_deleted))
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        let at = SystemTime::now();
        let mut entries = Vec::with_capacity(ids.len());
        for id in ids {
            let task = self.task_mut(id)?;
            task.soft_deleted = true;
            record_mutation_at(task, TaskEventKind::SoftDeleted, at);
            entries.push(UndoEntry::SoftDelete {
                id,
                expected_revision: task.revision,
            });
        }
        self.undo_stack.push(UndoEntry::Batch { entries });
        Ok(())
    }

    /// Clear soft-delete so the task can reappear in views.
    pub fn restore(&mut self, id: Uuid) -> Result<(), DomainError> {
        let task = self.task_mut(id)?;
        task.soft_deleted = false;
        record_mutation(task, TaskEventKind::Restored);
        Ok(())
    }

    /// Archive: keep the task but off every working lens. Human status is untouched
    /// and no undo entry is pushed. Returns `Ok(false)` when the flag already has the
    /// requested value: no event, no revision change.
    pub fn archive_task(&mut self, id: Uuid) -> Result<bool, DomainError> {
        self.set_archived(id, true, TaskEventKind::Archived)
    }

    /// Clear the archived flag. Same contract as [`DomainState::archive_task`].
    pub fn unarchive_task(&mut self, id: Uuid) -> Result<bool, DomainError> {
        self.set_archived(id, false, TaskEventKind::Unarchived)
    }

    fn set_archived(
        &mut self,
        id: Uuid,
        archived: bool,
        kind: TaskEventKind,
    ) -> Result<bool, DomainError> {
        let task = self.task_mut(id)?;
        if task.soft_deleted {
            return Err(DomainError::SoftDeleted(id));
        }
        if task.archived == archived {
            return Ok(false);
        }
        task.archived = archived;
        record_mutation(task, kind);
        Ok(true)
    }

    /// Assign one task and make the change undoable.
    pub fn assign(&mut self, id: Uuid, assignee: Option<String>) -> Result<bool, DomainError> {
        self.assign_batch(&[id], assignee)
    }

    /// Assign an ordered set as one atomic, undoable action.
    pub fn assign_batch(
        &mut self,
        ids: &[Uuid],
        assignee: Option<String>,
    ) -> Result<bool, DomainError> {
        let ids = self.prevalidate_batch_ids(ids)?;
        let changed = ids
            .into_iter()
            .filter(|id| self.get(*id).is_some_and(|task| task.assignee != assignee))
            .collect::<Vec<_>>();
        if changed.is_empty() {
            return Ok(false);
        }
        let at = SystemTime::now();
        let mut entries = Vec::with_capacity(changed.len());
        for id in changed {
            let task = self.task_mut(id)?;
            let previous = task.assignee.clone();
            task.assignee = assignee.clone();
            record_mutation_at(task, TaskEventKind::Assigned, at);
            entries.push(UndoEntry::Assign {
                id,
                previous,
                expected_revision: task.revision,
            });
        }
        self.undo_stack.push(if entries.len() == 1 {
            entries.pop().expect("one assignment undo")
        } else {
            UndoEntry::Batch { entries }
        });
        Ok(true)
    }

    pub(crate) fn restore_assignee(
        &mut self,
        id: Uuid,
        assignee: Option<String>,
    ) -> Result<(), DomainError> {
        let task = self.task_mut(id)?;
        task.assignee = assignee;
        record_mutation(task, TaskEventKind::Assigned);
        Ok(())
    }

    /// Record one successful launch and set human status to started as one mutation.
    /// Dispatch is external and deliberately creates no undo entry.
    pub fn record_dispatch(&mut self, id: Uuid, mut dispatch: Dispatch) -> Result<(), DomainError> {
        let task = self.task_mut(id)?;
        task.status = HumanStatus::Started;
        dispatch.cleaned = false;
        let at = dispatch.at;
        task.dispatch = Some(dispatch);
        record_mutation_at(task, TaskEventKind::Dispatched, at);
        Ok(())
    }

    /// Mark the retained dispatch record cleaned without changing human status.
    pub fn record_dispatch_cleaned(&mut self, id: Uuid) -> Result<(), DomainError> {
        let task = self.task_mut(id)?;
        let dispatch = task.dispatch.as_mut().ok_or(DomainError::UnknownId(id))?;
        dispatch.cleaned = true;
        record_mutation(task, TaskEventKind::Cleaned);
        Ok(())
    }

    /// Edit title, notes, scope, and thread together. Title uses the same non-empty trim rule as create.
    pub fn edit(
        &mut self,
        id: Uuid,
        title: impl AsRef<str>,
        notes: Option<String>,
        scope: TaskScope,
        thread: Option<String>,
    ) -> Result<(), DomainError> {
        let title = title.as_ref().trim();
        if title.is_empty() {
            return Err(DomainError::EmptyTitle);
        }
        let task = self.task_mut(id)?;
        task.title = title.to_string();
        task.notes = notes;
        task.scope = scope;
        task.thread = thread;
        record_mutation(task, TaskEventKind::Edited);
        Ok(())
    }

    /// Edit ordinary fields plus assignee without creating an assignment undo entry.
    pub fn edit_with_assignee(
        &mut self,
        id: Uuid,
        title: impl AsRef<str>,
        notes: Option<String>,
        scope: TaskScope,
        thread: Option<String>,
        assignee: Option<String>,
    ) -> Result<(), DomainError> {
        let changed_assignee = self.get(id).is_some_and(|task| task.assignee != assignee);
        self.edit(id, title, notes, scope, thread)?;
        let task = self.task_mut(id)?;
        task.assignee = assignee;
        if changed_assignee {
            let at = task.updated_at;
            task.history.push(TaskEvent {
                kind: TaskEventKind::Assigned,
                at,
            });
        }
        Ok(())
    }

    /// Atomically apply task fields, staged step renames, and staged step removals.
    ///
    /// All input is validated before the task changes. The session takes one revision based on
    /// the pre-edit task, which keeps a locked store save mergeable instead of making each
    /// staged step point at the previous staged revision. Duplicate changes collapse to one;
    /// removal wins over a rename of the same step, and unchanged text is not journaled.
    #[allow(clippy::too_many_arguments)] // Mirrors the stable edit-with-renames field list.
    pub fn edit_with_step_changes(
        &mut self,
        id: Uuid,
        title: impl AsRef<str>,
        notes: Option<String>,
        scope: TaskScope,
        thread: Option<String>,
        assignee: Option<String>,
        step_renames: &[(Uuid, String)],
        step_removals: &[Uuid],
        step_adds: &[String],
    ) -> Result<(), DomainError> {
        let title = title.as_ref().trim();
        if title.is_empty() {
            return Err(DomainError::EmptyTitle);
        }
        if step_renames.iter().any(|(_, text)| text.trim().is_empty())
            || step_adds.iter().any(|text| text.trim().is_empty())
        {
            return Err(DomainError::EmptyStepText);
        }
        let task = self.task_mut(id)?;
        let removals = step_removals.iter().copied().collect::<BTreeSet<_>>();
        let renames = step_renames
            .iter()
            .map(|(step_id, text)| (*step_id, text.trim().to_string()))
            .collect::<BTreeMap<_, _>>();
        for step_id in renames.keys().chain(removals.iter()) {
            if !task.steps.iter().any(|step| step.id == *step_id) {
                return Err(DomainError::UnknownStep(*step_id));
            }
        }
        let actual_renames = renames
            .into_iter()
            .filter(|(step_id, text)| {
                !removals.contains(step_id)
                    && task
                        .steps
                        .iter()
                        .any(|step| step.id == *step_id && step.text != *text)
            })
            .collect::<Vec<_>>();
        let actual_removals = task
            .steps
            .iter()
            .filter(|step| removals.contains(&step.id))
            .map(|step| step.id)
            .collect::<Vec<_>>();

        task.title = title.to_string();
        task.notes = notes;
        let changed_assignee = task.assignee != assignee;
        task.scope = scope;
        task.thread = thread;
        task.assignee = assignee;
        task.steps.retain(|step| !removals.contains(&step.id));
        for (step_id, text) in &actual_renames {
            if let Some(step) = task.steps.iter_mut().find(|step| step.id == *step_id) {
                step.text.clone_from(text);
            }
        }
        let added: Vec<Step> = step_adds
            .iter()
            .map(|text| Step {
                id: Uuid::new_v4(),
                text: text.trim().to_string(),
                done: false,
            })
            .collect();
        let added_count = added.len();
        task.steps.extend(added);
        record_mutation(task, TaskEventKind::Edited);
        let at = task.updated_at;
        if changed_assignee {
            task.history.push(TaskEvent {
                kind: TaskEventKind::Assigned,
                at,
            });
        }
        task.history
            .extend(actual_renames.iter().map(|_| TaskEvent {
                kind: TaskEventKind::StepRenamed,
                at,
            }));
        task.history
            .extend(actual_removals.iter().map(|_| TaskEvent {
                kind: TaskEventKind::StepRemoved,
                at,
            }));
        task.history.extend((0..added_count).map(|_| TaskEvent {
            kind: TaskEventKind::StepAdded,
            at,
        }));
        Ok(())
    }

    /// Add one step at the end of the task's steps.
    ///
    /// Trims text and refuses empty-after-trim. Returns the new step's id.
    /// Never touches status, scope, or notes.
    pub fn add_step(&mut self, task_id: Uuid, text: impl AsRef<str>) -> Result<Uuid, DomainError> {
        let text = text.as_ref().trim();
        if text.is_empty() {
            return Err(DomainError::EmptyStepText);
        }
        let task = self.task_mut(task_id)?;
        let step = Step {
            id: Uuid::new_v4(),
            text: text.to_string(),
            done: false,
        };
        let step_id = step.id;
        task.steps.push(step);
        record_mutation(task, TaskEventKind::StepAdded);
        Ok(step_id)
    }

    /// Flip one step's done flag.
    ///
    /// Journals `StepChecked` when the step turns done and `StepUnchecked`
    /// when it turns open. Never touches status, scope, or notes; completing
    /// the steps never completes the task.
    pub fn toggle_step(&mut self, task_id: Uuid, step_id: Uuid) -> Result<(), DomainError> {
        let task = self.task_mut(task_id)?;
        let step = task
            .steps
            .iter_mut()
            .find(|step| step.id == step_id)
            .ok_or(DomainError::UnknownStep(step_id))?;
        step.done = !step.done;
        let kind = if step.done {
            TaskEventKind::StepChecked
        } else {
            TaskEventKind::StepUnchecked
        };
        record_mutation(task, kind);
        Ok(())
    }

    /// Rename one step. Trims text and refuses empty-after-trim.
    /// Never touches status, scope, or notes.
    pub fn rename_step(
        &mut self,
        task_id: Uuid,
        step_id: Uuid,
        text: impl AsRef<str>,
    ) -> Result<(), DomainError> {
        let text = text.as_ref().trim();
        if text.is_empty() {
            return Err(DomainError::EmptyStepText);
        }
        let task = self.task_mut(task_id)?;
        let step = task
            .steps
            .iter_mut()
            .find(|step| step.id == step_id)
            .ok_or(DomainError::UnknownStep(step_id))?;
        step.text = text.to_string();
        record_mutation(task, TaskEventKind::StepRenamed);
        Ok(())
    }

    /// Remove one step by id. Never touches status, scope, or notes.
    pub fn remove_step(&mut self, task_id: Uuid, step_id: Uuid) -> Result<(), DomainError> {
        let task = self.task_mut(task_id)?;
        let index = task
            .steps
            .iter()
            .position(|step| step.id == step_id)
            .ok_or(DomainError::UnknownStep(step_id))?;
        task.steps.remove(index);
        record_mutation(task, TaskEventKind::StepRemoved);
        Ok(())
    }

    fn apply_status(
        &mut self,
        id: Uuid,
        status: HumanStatus,
        kind: TaskEventKind,
    ) -> Result<(), DomainError> {
        let task = self.task_mut(id)?;
        task.status = status;
        record_mutation(task, kind);
        Ok(())
    }

    fn prevalidate_batch_ids(&self, ids: &[Uuid]) -> Result<Vec<Uuid>, DomainError> {
        let mut seen = BTreeSet::new();
        let mut ordered = Vec::with_capacity(ids.len());
        for id in ids.iter().copied() {
            if seen.insert(id) {
                if self.get(id).is_none() {
                    return Err(DomainError::UnknownId(id));
                }
                ordered.push(id);
            }
        }
        Ok(ordered)
    }

    fn task_mut(&mut self, id: Uuid) -> Result<&mut Task, DomainError> {
        self.tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or(DomainError::UnknownId(id))
    }

    /// Merge a fresh disk snapshot into local presentation state. Disk wins divergent tasks
    /// because this method has no mutation baseline and must not guess with wall-clock
    /// timestamps.
    pub fn merge_tasks_from_disk(&mut self, other: &DomainState) {
        for incoming in &other.tasks {
            match self.tasks.iter_mut().find(|task| task.id == incoming.id) {
                Some(existing) if existing.revision != incoming.revision => {
                    *existing = incoming.clone()
                }
                Some(_) => {}
                None => self.tasks.push(incoming.clone()),
            }
        }
        self.merge_project_records(&other.projects);
        self.drop_tasks_trashed_elsewhere(other);
        self.merge_undo_entries(other);
    }

    /// Merge one local save intent under the store lock without using wall-clock ordering.
    /// A locally changed task is accepted only when the disk still has the revision it changed
    /// from; otherwise the caller receives a typed save failure instead of overwriting a newer
    /// concurrent mutation.
    pub(crate) fn merge_for_save(&mut self, disk: &DomainState) -> Result<(), String> {
        for incoming in &disk.tasks {
            let Some(local) = self.tasks.iter_mut().find(|task| task.id == incoming.id) else {
                self.tasks.push(incoming.clone());
                continue;
            };
            if local.revision == incoming.revision || local.merge_base_revision.is_none() {
                // This local copy did not mutate the task, so fresh disk state wins.
                *local = incoming.clone();
            } else if local.merge_base_revision == Some(incoming.revision) {
                // The disk still holds exactly the version this local mutation was based on.
            } else {
                return Err(format!("task {} changed during save", local.id));
            }
        }
        self.next_task_number = self.next_task_number.max(disk.next_task_number);
        self.next_notice_number = self.next_notice_number.max(disk.next_notice_number);
        self.merge_project_records(&disk.projects);
        self.drop_tasks_trashed_elsewhere(disk);
        self.merge_undo_entries(disk);
        Ok(())
    }

    /// Replace the local project map with the disk map, then re-apply this process's
    /// own recorded intents. Last writer wins for the map, but an intent this process
    /// has not confirmed through a save yet must survive the merge: a plain union would
    /// resurrect a record the process just removed (and an archive could be lost).
    fn merge_project_records(&mut self, disk: &BTreeMap<String, ProjectRecord>) {
        self.projects = disk.clone();
        for (path, archived) in &self.project_intents {
            if *archived {
                self.projects
                    .insert(path.clone(), ProjectRecord { archived: true });
            } else {
                self.projects.remove(path);
            }
        }
    }

    /// A local task absent from disk that is soft-deleted with no merge base was trashed
    /// by another process (or by this one on an earlier save): drop it instead of
    /// resurrecting it on the next write. A local task that is not soft-deleted, or that
    /// carries a `merge_base_revision`, is a local creation or mutation and stays.
    fn drop_tasks_trashed_elsewhere(&mut self, disk: &DomainState) {
        let disk_ids: BTreeSet<Uuid> = disk.tasks.iter().map(|task| task.id).collect();
        let before = self.tasks.len();
        self.tasks.retain(|task| {
            disk_ids.contains(&task.id) || !task.soft_deleted || task.merge_base_revision.is_some()
        });
        if self.tasks.len() != before {
            self.undo_stack.retain(|entry| {
                entry
                    .targets()
                    .into_iter()
                    .all(|(id, _)| self.tasks.iter().any(|task| task.id == id))
            });
        }
    }

    /// Assign missing `T` and `N` numbers in deterministic creation order. The store
    /// calls this only under its exclusive lock, immediately before the durable
    /// replacement. Notice rows never receive a `T` number.
    pub(crate) fn assign_numbers_for_persistence(&mut self) {
        let next_after_existing = self
            .tasks
            .iter()
            .filter_map(|task| task.number)
            .max()
            .and_then(|number| number.checked_add(1))
            .unwrap_or(1);
        self.next_task_number = self.next_task_number.max(next_after_existing);
        let mut missing: Vec<usize> = self
            .tasks
            .iter()
            .enumerate()
            .filter_map(|(index, task)| {
                (task.number.is_none() && !task.is_notice()).then_some(index)
            })
            .collect();
        missing.sort_by_key(|&index| (self.tasks[index].created_at, self.tasks[index].id));
        for index in missing {
            self.tasks[index].number = Some(self.next_task_number);
            self.next_task_number = self
                .next_task_number
                .checked_add(1)
                .expect("task number exhausted");
        }

        let next_after_existing_notices = self
            .tasks
            .iter()
            .filter_map(|task| task.notice.as_ref().and_then(|notice| notice.number))
            .max()
            .and_then(|number| number.checked_add(1))
            .unwrap_or(1);
        self.next_notice_number = self.next_notice_number.max(next_after_existing_notices);
        let mut missing_notices: Vec<usize> = self
            .tasks
            .iter()
            .enumerate()
            .filter_map(|(index, task)| {
                task.notice
                    .as_ref()
                    .is_some_and(|notice| notice.number.is_none())
                    .then_some(index)
            })
            .collect();
        missing_notices.sort_by_key(|&index| (self.tasks[index].created_at, self.tasks[index].id));
        for index in missing_notices {
            let notice = self.tasks[index]
                .notice
                .as_mut()
                .expect("missing-notice index was filtered for notice rows");
            notice.number = Some(self.next_notice_number);
            self.next_notice_number = self
                .next_notice_number
                .checked_add(1)
                .expect("notice number exhausted");
        }
    }

    pub(crate) fn sync_numbers_from_persisted(&mut self, persisted: &DomainState) {
        self.next_task_number = persisted.next_task_number;
        self.next_notice_number = persisted.next_notice_number;
        for task in &mut self.tasks {
            let persisted_task = persisted.get(task.id);
            task.number = persisted_task.and_then(|persisted_task| persisted_task.number);
            task.notice = persisted_task.and_then(|persisted_task| persisted_task.notice.clone());
        }
    }

    pub(crate) fn clear_merge_bases(&mut self) {
        for task in &mut self.tasks {
            task.merge_base_revision = None;
        }
        self.project_intents.clear();
    }

    fn merge_undo_entries(&mut self, other: &DomainState) {
        for incoming in &other.undo_stack {
            let incoming_leaves = incoming.leaf_entries();
            let already_present = incoming_leaves.iter().all(|incoming_leaf| {
                self.undo_stack.iter().any(|existing| {
                    existing
                        .leaf_entries()
                        .into_iter()
                        .any(|existing_leaf| existing_leaf == *incoming_leaf)
                })
            });
            if !already_present {
                self.undo_stack.push(incoming.clone());
            }
        }
    }

    /// Soft-deleted tasks eligible to move to `trash.jsonl`, with each task's
    /// `deleted_at` (the last `soft_deleted` event). A soft-deleted task moves when:
    ///
    /// - it was deleted more than [`TRASH_AFTER`] ago (even as the top undo entry), or
    /// - no undo entry on the stack targets it, or
    /// - a later undoable action exists: some live undo entry's recorded action is
    ///   timestamped after this task's delete. A concurrent or earlier action (e.g. a
    ///   sibling writer's completion merged in from disk) does not finalize the delete.
    ///
    /// Stale entries are pruned at the persistence boundary before this runs, so a live
    /// entry's target task carries exactly the event that entry records.
    pub(crate) fn trash_eligible(&self, now: SystemTime) -> Vec<(SystemTime, Task)> {
        let entry_times: Vec<SystemTime> = self
            .undo_stack
            .iter()
            .filter_map(|entry| {
                entry
                    .leaf_entries()
                    .into_iter()
                    .filter_map(|leaf| {
                        let (id, _) = leaf.targets().into_iter().next()?;
                        let task = self.tasks.iter().find(|task| task.id == id)?;
                        match leaf {
                            UndoEntry::SoftDelete { .. } => task.soft_deleted_at(),
                            UndoEntry::Complete { .. } => {
                                task.last_event_at(TaskEventKind::Completed)
                            }
                            UndoEntry::Assign { .. } => task.last_event_at(TaskEventKind::Assigned),
                            UndoEntry::Batch { .. } => unreachable!("batches are flattened"),
                        }
                    })
                    .max()
            })
            .collect();
        let targeted: BTreeSet<Uuid> = self
            .undo_stack
            .iter()
            .flat_map(UndoEntry::targets)
            .map(|(id, _)| id)
            .collect();
        self.tasks
            .iter()
            .filter(|task| task.soft_deleted)
            .filter_map(|task| {
                let deleted_at = task.soft_deleted_at().unwrap_or(task.updated_at);
                let aged = now
                    .duration_since(deleted_at)
                    .is_ok_and(|age| age > TRASH_AFTER);
                let later_action = entry_times.iter().any(|time| *time > deleted_at);
                (aged || !targeted.contains(&task.id) || later_action)
                    .then(|| (deleted_at, task.clone()))
            })
            .collect()
    }

    /// Remove the given tasks and every undo entry targeting one of them.
    pub(crate) fn remove_tasks(&mut self, ids: &BTreeSet<Uuid>) {
        self.tasks.retain(|task| !ids.contains(&task.id));
        self.undo_stack.retain(|entry| {
            entry
                .targets()
                .into_iter()
                .all(|(id, _)| !ids.contains(&id))
        });
    }

    /// Re-insert one task restored from trash: `soft_deleted` cleared, a `restored`
    /// history event, a new revision, `updated_at` now. Number and history are kept.
    pub(crate) fn insert_restored(&mut self, task: Task) {
        let id = task.id;
        self.tasks.push(task);
        self.restore(id).expect("the task was just inserted");
    }

    /// Persistence-boundary prune, run at the same locked boundary as number
    /// assignment so every save path and `locked_transition` gets it.
    ///
    /// 1. Drop entries whose target task is missing, or whose `expected_revision`
    ///    no longer matches the task's current revision (stale, can never succeed).
    /// 2. Evict the oldest entries until the stack fits [`UNDO_CAP`].
    ///
    /// Order matters: stale entries go first so the cap never evicts a live entry
    /// to keep a dead one. `merge_undo_entries` still unions; this runs after it.
    /// In-memory `undo()` on a stale top entry is unchanged (refuse, retain)
    /// because pruning happens only at save.
    pub(crate) fn prune_undo_for_persistence(&mut self) {
        self.undo_stack.retain(|entry| {
            let targets = entry.targets();
            !targets.is_empty()
                && targets.into_iter().all(|(id, expected_revision)| {
                    self.tasks
                        .iter()
                        .find(|task| task.id == id)
                        .is_some_and(|task| task.revision == expected_revision)
                })
        });
        if self.undo_stack.len() > UNDO_CAP {
            let excess = self.undo_stack.len() - UNDO_CAP;
            self.undo_stack.drain(..excess);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn create_rejects_whitespace_only_title_and_adds_no_task() {
        let mut state = DomainState::new();
        let err = state
            .create(
                "   \t\n  ",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect_err("whitespace-only title must fail");
        assert_eq!(err, DomainError::EmptyTitle);
        assert!(state.tasks().is_empty());
    }

    #[test]
    fn create_with_title_yields_one_todo_task_without_notes() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        assert_eq!(state.tasks().len(), 1);
        let task = &state.tasks()[0];
        assert_eq!(task.id, id);
        assert_eq!(task.title, "Fix flake");
        assert_eq!(task.status, HumanStatus::Open);
        assert_eq!(task.notes, None);
        assert!(!task.soft_deleted);
        assert_eq!(task.scope, TaskScope::Global);
    }

    #[test]
    fn create_with_notes_stores_notes() {
        let mut state = DomainState::new();
        state
            .create(
                "Fix flake",
                Some("flaky under load".into()),
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("valid title creates a task");
        assert_eq!(state.tasks()[0].notes.as_deref(), Some("flaky under load"));
    }

    #[test]
    fn create_with_capture_origin_stores_capture() {
        let mut state = DomainState::new();
        let id = state
            .create(
                "From capture",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Capture,
                None,
            )
            .expect("valid title creates a task");
        assert_eq!(
            state.get(id).expect("task exists").provenance,
            ProvenanceOrigin::Capture
        );
    }

    #[test]
    fn create_with_selection_origin_stores_selection() {
        let mut state = DomainState::new();
        let id = state
            .create(
                "From selection",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Selection,
                None,
            )
            .expect("valid title creates a task");
        assert_eq!(
            state.get(id).expect("task exists").provenance,
            ProvenanceOrigin::Selection
        );
    }

    #[test]
    fn status_changed_at_tracks_status_events_not_edits() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);

        // Only a `Created` event exists: fall back to created_at.
        let created = state.get(id).expect("task").created_at;
        assert_eq!(
            state.get(id).expect("task").status_changed_at(),
            created,
            "no status event yet: status_changed_at falls back to created_at"
        );

        state
            .set_status(id, HumanStatus::Started)
            .expect("set_status");
        let after_status = state.get(id).expect("task").status_changed_at();
        assert!(after_status >= created, "a status change is recorded");

        state
            .edit(id, "Edited title", None, TaskScope::Global, None)
            .expect("edit");
        assert_eq!(
            state.get(id).expect("task").status_changed_at(),
            after_status,
            "an edit must not move the status-change time"
        );

        state.complete(id).expect("complete");
        let after_complete = state.get(id).expect("task").status_changed_at();
        assert!(
            after_complete >= after_status,
            "completing is also a status change"
        );

        // A step tick is a mutation but not a status change.
        state.add_step(id, "a step").expect("add_step");
        assert_eq!(
            state.get(id).expect("task").status_changed_at(),
            after_complete,
            "a step change must not move the status-change time"
        );
    }

    #[test]
    fn mutations_append_matching_events() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);

        let kinds = |state: &DomainState| -> Vec<TaskEventKind> {
            state
                .get(id)
                .expect("task exists")
                .history
                .iter()
                .map(|e| e.kind)
                .collect()
        };

        assert_eq!(kinds(&state), vec![TaskEventKind::Created]);

        state
            .edit(id, "Edited title", None, TaskScope::Global, None)
            .expect("edit");
        assert!(kinds(&state).contains(&TaskEventKind::Edited));

        state
            .set_status(id, HumanStatus::Started)
            .expect("set_status");
        assert!(kinds(&state).contains(&TaskEventKind::StatusSet));

        state.complete(id).expect("complete");
        assert!(kinds(&state).contains(&TaskEventKind::Completed));

        state.reopen(id).expect("reopen");
        assert!(kinds(&state).contains(&TaskEventKind::Reopened));

        state.soft_delete(id).expect("soft_delete");
        assert!(kinds(&state).contains(&TaskEventKind::SoftDeleted));

        state.restore(id).expect("restore");
        assert!(kinds(&state).contains(&TaskEventKind::Restored));

        // Full ordered history for the sequence above.
        assert_eq!(
            kinds(&state),
            vec![
                TaskEventKind::Created,
                TaskEventKind::Edited,
                TaskEventKind::StatusSet,
                TaskEventKind::Completed,
                TaskEventKind::Reopened,
                TaskEventKind::SoftDeleted,
                TaskEventKind::Restored,
            ]
        );
    }

    #[test]
    fn every_semantic_mutation_refreshes_the_opaque_revision() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        let mut previous = state.get(id).expect("task").revision;

        let mut assert_refreshed = |state: &DomainState| {
            let current = state.get(id).expect("task").revision;
            assert_ne!(current, previous);
            previous = current;
        };

        state
            .edit(id, "Edited", None, TaskScope::Global, None)
            .expect("edit");
        assert_refreshed(&state);
        state.set_status(id, HumanStatus::Started).expect("status");
        assert_refreshed(&state);
        state.complete(id).expect("complete");
        assert_refreshed(&state);
        state.reopen(id).expect("reopen");
        assert_refreshed(&state);
        state.soft_delete(id).expect("soft delete");
        assert_refreshed(&state);
        state.restore(id).expect("restore");
        assert_refreshed(&state);
    }

    #[test]
    fn restore_clears_soft_deleted() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        state.soft_delete(id).expect("soft_delete");
        assert!(state.get(id).expect("task exists").soft_deleted);
        state.restore(id).expect("restore");
        assert!(!state.get(id).expect("task exists").soft_deleted);
    }

    #[test]
    fn archive_task_sets_the_flag_keeps_status_journals_archived_and_pushes_no_undo() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        state
            .set_status(id, HumanStatus::Blocked)
            .expect("blocked status");
        let undo_before = state.last_undo().cloned();

        // Archive: the flag is set, human status is untouched, one Archived event is
        // journaled, and no undo entry appears.
        assert_eq!(state.archive_task(id), Ok(true));
        {
            let task = state.get(id).expect("task exists");
            assert!(task.archived);
            assert_eq!(task.status, HumanStatus::Blocked);
            assert_eq!(
                task.history.last().map(|event| event.kind),
                Some(TaskEventKind::Archived)
            );
        }
        assert_eq!(state.last_undo(), undo_before.as_ref());

        // Unarchive clears the flag with an Unarchived event.
        assert_eq!(state.unarchive_task(id), Ok(true));
        {
            let task = state.get(id).expect("task exists");
            assert!(!task.archived);
            assert_eq!(
                task.history.last().map(|event| event.kind),
                Some(TaskEventKind::Unarchived)
            );
        }

        // Re-archiving journals again, and a second call is a no-op: Ok(false), no event,
        // no revision change.
        assert_eq!(state.archive_task(id), Ok(true));
        let history_len = state.get(id).expect("task exists").history.len();
        assert_eq!(state.archive_task(id), Ok(false));
        assert_eq!(
            state.get(id).expect("task exists").history.len(),
            history_len,
            "an already-archived task gains no event"
        );
        assert_eq!(
            state.last_undo(),
            undo_before.as_ref(),
            "archive never touches the undo stack"
        );

        // Soft-deleted tasks are refused by both verbs.
        let gone = state
            .create(
                "gone",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create");
        state.soft_delete(gone).expect("soft delete");
        assert_eq!(
            state.archive_task(gone),
            Err(DomainError::SoftDeleted(gone))
        );
        assert_eq!(
            state.unarchive_task(gone),
            Err(DomainError::SoftDeleted(gone))
        );
    }

    #[test]
    fn archive_project_writes_one_record_and_unarchive_removes_it() {
        let mut state = DomainState::new();
        let id = state
            .create(
                "In a",
                None,
                TaskScope::Project {
                    path: "/repos/a".into(),
                },
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create");
        assert!(state.projects().is_empty());

        assert_eq!(state.archive_project("/repos/a"), Ok(true));
        let records = state.projects();
        assert_eq!(
            records.len(),
            1,
            "exactly one record for one archived project"
        );
        assert!(records.get("/repos/a").expect("record").archived);
        assert!(state.is_project_archived("/repos/a"));
        assert!(
            state.is_hidden(state.get(id).expect("task exists")),
            "a task of an archived project is hidden"
        );

        // Already archived: a no-op.
        assert_eq!(state.archive_project("/repos/a"), Ok(false));

        assert_eq!(state.unarchive_project("/repos/a"), Ok(true));
        assert!(state.projects().is_empty());
        assert!(!state.is_project_archived("/repos/a"));
        assert!(!state.is_hidden(state.get(id).expect("task exists")));
        // No record but tasks exist: a no-op, not an error.
        assert_eq!(state.unarchive_project("/repos/a"), Ok(false));

        // No task in the scope and no record: unknown.
        assert_eq!(
            state.archive_project("/nowhere"),
            Err(DomainError::UnknownProject("/nowhere".into()))
        );
        assert_eq!(
            state.unarchive_project("/nowhere"),
            Err(DomainError::UnknownProject("/nowhere".into()))
        );
        assert!(state.projects().is_empty(), "refusals write no record");
    }

    #[test]
    fn task_and_project_flags_are_independent() {
        let mut state = DomainState::new();
        let scope = TaskScope::Project {
            path: "/repos/p".into(),
        };
        let t = state
            .create("T", None, scope.clone(), ProvenanceOrigin::Manual, None)
            .expect("create");
        let other = state
            .create("Other", None, scope, ProvenanceOrigin::Manual, None)
            .expect("create");

        state.archive_task(t).expect("archive task T");
        state
            .archive_project("/repos/p")
            .expect("archive project P");
        state
            .unarchive_project("/repos/p")
            .expect("unarchive project P");

        let task_t = state.get(t).expect("task exists");
        let task_other = state.get(other).expect("task exists");
        assert!(
            task_t.archived,
            "T is still archived after the project round-trip"
        );
        assert!(!task_other.archived, "P's other task is untouched");
        assert!(state.is_hidden(task_t), "T is hidden by its own flag");
        assert!(!state.is_hidden(task_other));
        assert!(state.projects().is_empty());
    }

    #[test]
    fn set_status_accepts_each_human_status() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        for status in [
            HumanStatus::Open,
            HumanStatus::Ready,
            HumanStatus::Started,
            HumanStatus::Blocked,
            HumanStatus::Review,
            HumanStatus::Done,
        ] {
            state
                .set_status(id, status)
                .expect("set_status must accept every human status");
            assert_eq!(state.get(id).expect("task exists").status, status);
        }
    }

    #[test]
    fn complete_sets_status_done() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        state.complete(id).expect("complete known id");
        assert_eq!(
            state.get(id).expect("task exists").status,
            HumanStatus::Done
        );
    }

    #[test]
    fn reopen_on_done_sets_status_open() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        state.complete(id).expect("complete known id");
        state.reopen(id).expect("reopen known id");
        assert_eq!(
            state.get(id).expect("task exists").status,
            HumanStatus::Open
        );
    }

    #[test]
    fn soft_delete_flags_task_but_keeps_it_in_store() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        state.soft_delete(id).expect("soft_delete known id");
        assert_eq!(state.tasks().len(), 1);
        let task = state.get(id).expect("soft-deleted task still gettable");
        assert!(task.soft_deleted);
    }

    #[test]
    fn edit_updates_title_notes_and_scope() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        let project = TaskScope::Project {
            path: "/home/me/proj".into(),
        };
        state
            .edit(
                id,
                "  New title  ",
                Some("updated notes".into()),
                project.clone(),
                None,
            )
            .expect("edit known id");
        let task = state.get(id).expect("task exists");
        assert_eq!(task.title, "New title");
        assert_eq!(task.notes.as_deref(), Some("updated notes"));
        assert_eq!(task.scope, project);
    }

    #[test]
    fn task_session_edit_renames_steps_on_one_revision() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        let first = state.add_step(id, "first").expect("first step");
        let second = state.add_step(id, "second").expect("second step");
        let before = state.get(id).expect("task").clone();

        state
            .edit_with_step_changes(
                id,
                "Edited title",
                Some("edited notes".into()),
                TaskScope::Global,
                None,
                None,
                &[
                    (first, "first revised".into()),
                    (second, "second revised".into()),
                ],
                &[],
                &[],
            )
            .expect("atomic session edit");

        let task = state.get(id).expect("task");
        assert_eq!(task.title, "Edited title");
        assert_eq!(task.steps[0].text, "first revised");
        assert_eq!(task.steps[1].text, "second revised");
        assert_eq!(task.merge_base_revision, Some(before.revision));
        assert_ne!(task.revision, before.revision);
        assert_eq!(
            task.history
                .iter()
                .rev()
                .take(3)
                .map(|event| &event.kind)
                .collect::<Vec<_>>(),
            vec![
                &TaskEventKind::StepRenamed,
                &TaskEventKind::StepRenamed,
                &TaskEventKind::Edited,
            ]
        );
    }

    #[test]
    fn task_session_edit_ignores_unchanged_and_removed_step_renames() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        let unchanged = state.add_step(id, "unchanged").expect("unchanged step");
        let removed = state.add_step(id, "removed").expect("removed step");
        let before = state.get(id).expect("task").history.len();

        state
            .edit_with_step_changes(
                id,
                "Sample task",
                None,
                TaskScope::Global,
                None,
                None,
                &[
                    (unchanged, " unchanged ".into()),
                    (removed, "renamed but removed".into()),
                ],
                &[removed, removed],
                &[],
            )
            .expect("normalize overlapping changes");

        let task = state.get(id).expect("task");
        assert_eq!(task.steps.len(), 1);
        assert_eq!(task.steps[0].text, "unchanged");
        assert_eq!(
            task.history[before..]
                .iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            vec![TaskEventKind::Edited, TaskEventKind::StepRemoved]
        );
    }

    #[test]
    fn edit_carrying_thread_journals_one_event_and_bumps_revision_once() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        let before = state.get(id).expect("task").clone();

        state
            .edit(
                id,
                "Edited title",
                Some("edited notes".into()),
                TaskScope::Project {
                    path: "/repos/threads".into(),
                },
                Some("release-2026".into()),
            )
            .expect("edit carrying thread");

        let task = state.get(id).expect("task");
        assert_eq!(task.title, "Edited title");
        assert_eq!(task.notes.as_deref(), Some("edited notes"));
        assert_eq!(task.thread.as_deref(), Some("release-2026"));
        assert_eq!(
            task.scope,
            TaskScope::Project {
                path: "/repos/threads".into(),
            }
        );
        assert_eq!(task.history.len(), before.history.len() + 1);
        assert_eq!(
            task.history.last().map(|event| event.kind),
            Some(TaskEventKind::Edited)
        );
        assert_ne!(task.revision, before.revision);
    }

    #[test]
    fn commands_reject_unknown_id() {
        let mut state = DomainState::new();
        let missing = Uuid::new_v4();
        assert_eq!(
            state.set_status(missing, HumanStatus::Started),
            Err(DomainError::UnknownId(missing))
        );
        assert_eq!(
            state.complete(missing),
            Err(DomainError::UnknownId(missing))
        );
        assert_eq!(state.reopen(missing), Err(DomainError::UnknownId(missing)));
        assert_eq!(
            state.soft_delete(missing),
            Err(DomainError::UnknownId(missing))
        );
        assert_eq!(state.restore(missing), Err(DomainError::UnknownId(missing)));
        assert_eq!(
            state.edit(missing, "x", None, TaskScope::Global, None),
            Err(DomainError::UnknownId(missing))
        );
    }

    #[test]
    fn steps_mutations_journal_events_and_bump_revision() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        let mut previous_revision = state.get(id).expect("task").revision;
        let mut previous_len = state.get(id).expect("task").history.len();

        let mut assert_journaled = |state: &DomainState, kind: TaskEventKind| {
            let task = state.get(id).expect("task exists");
            let revision = task.revision;
            assert_ne!(revision, previous_revision, "revision must change");
            previous_revision = revision;
            assert_eq!(
                task.history.len(),
                previous_len + 1,
                "exactly one history event must be appended"
            );
            previous_len = task.history.len();
            assert_eq!(task.history.last().expect("event").kind, kind);
        };

        let step = state.add_step(id, "  First step  ").expect("add step");
        assert_journaled(&state, TaskEventKind::StepAdded);
        assert_eq!(
            state.get(id).expect("task").steps,
            vec![Step {
                id: step,
                text: "First step".into(),
                done: false,
            }],
            "add must trim and append at the end"
        );

        state.toggle_step(id, step).expect("toggle on");
        assert_journaled(&state, TaskEventKind::StepChecked);
        assert!(state.get(id).expect("task").steps[0].done);

        state.toggle_step(id, step).expect("toggle off");
        assert_journaled(&state, TaskEventKind::StepUnchecked);
        assert!(!state.get(id).expect("task").steps[0].done);

        state
            .rename_step(id, step, "  Renamed step  ")
            .expect("rename steps step");
        assert_journaled(&state, TaskEventKind::StepRenamed);
        assert_eq!(
            state.get(id).expect("task").steps[0].text,
            "Renamed step",
            "rename must trim"
        );

        state.remove_step(id, step).expect("remove step");
        assert_journaled(&state, TaskEventKind::StepRemoved);
        assert!(state.get(id).expect("task").steps.is_empty());
    }

    #[test]
    fn toggle_step_keeps_human_status_including_completing_last_step() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        state
            .set_status(id, HumanStatus::Started)
            .expect("set_status");
        let step = state.add_step(id, "Only step").expect("add step");
        state
            .toggle_step(id, step)
            .expect("toggle the only step done");
        let task = state.get(id).expect("task exists");
        assert!(task.steps[0].done, "the only step is now done");
        assert_eq!(
            task.status,
            HumanStatus::Started,
            "completing the steps must never change human status"
        );
    }

    #[test]
    fn step_commands_reject_unknown_ids_and_empty_text() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        let step = state.add_step(id, "Step").expect("add step");
        let missing_task = Uuid::new_v4();
        let missing_item = Uuid::new_v4();

        assert_eq!(
            state.add_step(missing_task, "x"),
            Err(DomainError::UnknownId(missing_task))
        );
        assert_eq!(
            state.toggle_step(missing_task, step),
            Err(DomainError::UnknownId(missing_task))
        );
        assert_eq!(
            state.rename_step(missing_task, step, "x"),
            Err(DomainError::UnknownId(missing_task))
        );
        assert_eq!(
            state.remove_step(missing_task, step),
            Err(DomainError::UnknownId(missing_task))
        );
        assert_eq!(
            state.toggle_step(id, missing_item),
            Err(DomainError::UnknownStep(missing_item))
        );
        assert_eq!(
            state.rename_step(id, missing_item, "x"),
            Err(DomainError::UnknownStep(missing_item))
        );
        assert_eq!(
            state.remove_step(id, missing_item),
            Err(DomainError::UnknownStep(missing_item))
        );
        assert_eq!(state.add_step(id, "   "), Err(DomainError::EmptyStepText));
        assert_eq!(
            state.rename_step(id, step, "  "),
            Err(DomainError::EmptyStepText)
        );

        let task = state.get(id).expect("task exists");
        assert_eq!(
            task.steps,
            vec![Step {
                id: step,
                text: "Step".into(),
                done: false,
            }],
            "refused commands must not mutate the steps"
        );
        assert_eq!(
            task.history.len(),
            2,
            "no journal writes for refused commands"
        );
    }

    #[test]
    fn locked_create_assigns_number_one_then_two() {
        let dir = std::env::temp_dir().join(format!("tsk-number-domain-{}", Uuid::new_v4()));
        let store = crate::store::TaskStore::new(&dir);
        let first = store
            .locked_transition(|state| {
                state
                    .create(
                        "first",
                        None,
                        TaskScope::Global,
                        ProvenanceOrigin::Manual,
                        None,
                    )
                    .map_err(|error| error.to_string())
            })
            .expect("persist first");
        let second = store
            .locked_transition(|state| {
                state
                    .create(
                        "second",
                        None,
                        TaskScope::Global,
                        ProvenanceOrigin::Manual,
                        None,
                    )
                    .map_err(|error| error.to_string())
            })
            .expect("persist second");
        let state = store.load().expect("load");
        assert_eq!(state.get(first).and_then(|task| task.number), Some(1));
        assert_eq!(state.get(second).and_then(|task| task.number), Some(2));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn create_notice_defers_n_until_persistence_and_never_gets_a_t_number() {
        let mut state = DomainState::new();
        let first = state
            .create_notice(
                "welcome",
                "  Welcome to tsk  ",
                Some("read me".into()),
                HumanStatus::Ready,
                TaskScope::Global,
                vec![("open the board".into(), false), ("press ?".into(), true)],
            )
            .expect("notice");
        let second = state
            .create_notice(
                "whats-new",
                "What's new",
                None,
                HumanStatus::Started,
                TaskScope::Global,
                Vec::new(),
            )
            .expect("notice");
        assert_eq!(
            state.create_notice(
                "empty",
                "  ",
                None,
                HumanStatus::Ready,
                TaskScope::Global,
                Vec::new()
            ),
            Err(DomainError::EmptyTitle)
        );
        assert_eq!(state.next_notice_number, 1);
        assert_eq!(state.next_task_number, 1);

        let task = state.get(first).expect("first notice");
        assert_eq!(task.title, "Welcome to tsk");
        assert_eq!(task.notes.as_deref(), Some("read me"));
        assert_eq!(task.number, None);
        assert_eq!(
            task.notice,
            Some(Notice {
                catalog_id: "welcome".into(),
                number: None,
            })
        );
        assert_eq!(task.board_identifier(), None);
        assert_eq!(
            task.steps
                .iter()
                .map(|step| (step.text.as_str(), step.done))
                .collect::<Vec<_>>(),
            vec![("open the board", false), ("press ?", true)]
        );
        assert_eq!(
            task.history
                .iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            vec![TaskEventKind::Created]
        );
        let task = state.get(second).expect("second notice");
        assert_eq!(task.status, HumanStatus::Started);
        assert_eq!(task.notice.as_ref().map(|notice| notice.number), Some(None));

        state.assign_numbers_for_persistence();
        assert_eq!(
            state
                .get(first)
                .expect("notice")
                .notice
                .as_ref()
                .and_then(|notice| notice.number),
            Some(1)
        );
        assert_eq!(
            state
                .get(second)
                .expect("notice")
                .notice
                .as_ref()
                .and_then(|notice| notice.number),
            Some(2)
        );
        assert_eq!(state.next_notice_number, 3);

        let ordinary = create_sample(&mut state);
        state.assign_numbers_for_persistence();
        assert_eq!(state.get(first).expect("notice").number, None);
        assert_eq!(state.get(second).expect("notice").number, None);
        assert_eq!(state.get(ordinary).expect("task").number, Some(1));
        assert_eq!(state.next_task_number, 2);
    }

    #[test]
    fn concurrent_notice_creates_get_distinct_n_numbers_under_merge_and_assign() {
        let mut left = DomainState::new();
        let mut right = DomainState::new();
        let left_id = left
            .create_notice(
                "left",
                "Left notice",
                None,
                HumanStatus::Ready,
                TaskScope::Global,
                Vec::new(),
            )
            .expect("left");
        let right_id = right
            .create_notice(
                "right",
                "Right notice",
                None,
                HumanStatus::Ready,
                TaskScope::Global,
                Vec::new(),
            )
            .expect("right");

        left.merge_for_save(&right).expect("merge");
        left.assign_numbers_for_persistence();

        let left_n = left
            .get(left_id)
            .expect("left")
            .notice
            .as_ref()
            .and_then(|notice| notice.number);
        let right_n = left
            .get(right_id)
            .expect("right")
            .notice
            .as_ref()
            .and_then(|notice| notice.number);
        assert_eq!(left_n, Some(1));
        assert_eq!(right_n, Some(2));
        assert_ne!(left_n, right_n);
        assert_eq!(left.next_notice_number, 3);
        assert_eq!(
            left.get(left_id)
                .expect("left")
                .board_identifier()
                .as_deref(),
            Some("N1")
        );
        assert_eq!(
            left.get(right_id)
                .expect("right")
                .board_identifier()
                .as_deref(),
            Some("N2")
        );
    }

    #[test]
    fn board_identifier_paints_n_for_notices_and_t_for_numbered_tasks() {
        let mut state = DomainState::new();
        let notice = state
            .create_notice(
                "welcome",
                "Welcome",
                None,
                HumanStatus::Ready,
                TaskScope::Global,
                Vec::new(),
            )
            .expect("notice");
        let draft = create_sample(&mut state);
        assert_eq!(state.get(notice).expect("notice").board_identifier(), None);
        assert_eq!(state.get(draft).expect("draft").board_identifier(), None);
        state.assign_numbers_for_persistence();
        assert_eq!(
            state.get(notice).expect("notice").board_identifier(),
            Some("N1".to_string())
        );
        assert_eq!(
            state.get(draft).expect("task").board_identifier(),
            Some("T1".to_string())
        );
        assert!(state.get(notice).expect("notice").is_notice());
        assert!(!state.get(draft).expect("task").is_notice());
    }

    #[test]
    fn locked_notice_and_task_keep_separate_counters_across_a_save() {
        let dir = std::env::temp_dir().join(format!("tsk-notice-domain-{}", Uuid::new_v4()));
        let store = crate::store::TaskStore::new(&dir);
        let notice = store
            .locked_transition(|state| {
                state
                    .create_notice(
                        "welcome",
                        "Welcome",
                        None,
                        HumanStatus::Ready,
                        TaskScope::Global,
                        Vec::new(),
                    )
                    .map_err(|error| error.to_string())
            })
            .expect("persist notice");
        let task = store
            .locked_transition(|state| {
                state
                    .create(
                        "ordinary",
                        None,
                        TaskScope::Global,
                        ProvenanceOrigin::Manual,
                        None,
                    )
                    .map_err(|error| error.to_string())
            })
            .expect("persist task");
        let state = store.load().expect("load");
        assert_eq!(
            state.get(notice).expect("notice").board_identifier(),
            Some("N1".to_string())
        );
        assert_eq!(
            state.get(task).expect("task").board_identifier(),
            Some("T1".to_string())
        );
        assert_eq!(state.next_notice_number, 2);
        assert_eq!(state.next_task_number, 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn edit_status_scope_thread_complete_soft_delete_leave_number_unchanged() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        state.assign_numbers_for_persistence();
        let number = state.get(id).and_then(|task| task.number);
        state
            .edit(
                id,
                "edited",
                None,
                TaskScope::Project {
                    path: "/project".into(),
                },
                Some("thread".into()),
            )
            .expect("edit");
        state.set_status(id, HumanStatus::Started).expect("status");
        state.complete(id).expect("complete");
        state.soft_delete(id).expect("delete");
        assert_eq!(state.get(id).and_then(|task| task.number), number);
    }

    #[test]
    fn undo_of_complete_and_of_soft_delete_keeps_the_same_number() {
        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        state.assign_numbers_for_persistence();
        let number = state.get(id).and_then(|task| task.number);
        state.complete(id).expect("complete");
        state.undo().expect("undo complete");
        state.soft_delete(id).expect("delete");
        state.undo().expect("undo delete");
        assert_eq!(state.get(id).and_then(|task| task.number), number);
    }

    #[test]
    fn removed_status_and_step_wire_names_are_rejected() {
        assert!(serde_json::from_str::<HumanStatus>("\"todo\"").is_err());
        assert!(serde_json::from_str::<HumanStatus>("\"doing\"").is_err());
        assert!(serde_json::from_str::<TaskEventKind>("\"checklist_item_added\"").is_err());

        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        let mut document = serde_json::to_value(&state).expect("serialize current task");
        document["tasks"][0]["checklist"] = serde_json::json!([]);
        let error = serde_json::from_value::<DomainState>(document)
            .expect_err("old checklist field must be rejected");
        assert!(error.to_string().contains("checklist"));
        assert_eq!(state.get(id).expect("task").steps, Vec::<Step>::new());
    }

    #[test]
    fn current_schema_requires_counter_task_revision_and_undo_revision() {
        let mut document = serde_json::to_value(DomainState::new()).expect("serialize state");
        document
            .as_object_mut()
            .expect("state object")
            .remove("next_task_number");
        assert!(serde_json::from_value::<DomainState>(document).is_err());

        let mut state = DomainState::new();
        let id = create_sample(&mut state);
        let mut document = serde_json::to_value(&state).expect("serialize task");
        document["tasks"][0]
            .as_object_mut()
            .expect("task object")
            .remove("revision");
        assert!(serde_json::from_value::<DomainState>(document).is_err());

        state.complete(id).expect("complete");
        let mut document = serde_json::to_value(state).expect("serialize undo");
        let variant = document["undo_stack"][0]
            .as_object_mut()
            .expect("undo variant")
            .values_mut()
            .next()
            .expect("variant payload")
            .as_object_mut()
            .expect("undo payload");
        variant.remove("expected_revision");
        assert!(serde_json::from_value::<DomainState>(document).is_err());
    }

    #[test]
    fn removed_dark_engine_fields_are_rejected() {
        let mut state = DomainState::new();
        create_sample(&mut state);
        let document = serde_json::to_value(state).expect("serialize current task");

        for field in ["capsule", "agent_meta", "last_observed"] {
            let mut with_removed_field = document.clone();
            with_removed_field["tasks"][0][field] = serde_json::Value::Null;
            assert!(
                serde_json::from_value::<DomainState>(with_removed_field).is_err(),
                "removed task field {field} must be rejected"
            );
        }

        let mut with_attempts = document;
        with_attempts["active_attempts"] = serde_json::json!([]);
        assert!(serde_json::from_value::<DomainState>(with_attempts).is_err());
    }
}
