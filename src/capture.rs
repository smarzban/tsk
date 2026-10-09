//! Capture pipeline: snapshot + user fields → domain create.
//!
//! Application path used by Capture UI. Creates only through Task Domain;
//! optional Task Store save when a store is provided.

use uuid::Uuid;

use crate::context::InvocationSnapshot;
use crate::domain::{
    normalize_thread, thread_refusal_message, DomainError, DomainState, TaskScope,
};
use crate::store::{StoreError, TaskStore};

/// Task id returned by a successful capture (domain `Uuid`).
pub type TaskId = Uuid;

/// Failures from capture create and optional persist.
#[derive(Debug)]
pub enum CaptureError {
    Domain(DomainError),
    Store(StoreError),
    UnknownBase(String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureError::Domain(e) => write!(f, "{e}"),
            CaptureError::Store(e) => write!(f, "{e}"),
            CaptureError::UnknownBase(reason) => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for CaptureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CaptureError::Domain(e) => Some(e),
            CaptureError::Store(e) => Some(e),
            CaptureError::UnknownBase(_) => None,
        }
    }
}

impl From<DomainError> for CaptureError {
    fn from(value: DomainError) -> Self {
        CaptureError::Domain(value)
    }
}

impl From<StoreError> for CaptureError {
    fn from(value: StoreError) -> Self {
        CaptureError::Store(value)
    }
}

/// Create one task from a capture form submission.
///
/// - Scope: `scope_override` when set, else `snapshot.default_scope`.
/// - Provenance origin comes from the snapshot.
/// - `thread`, when supplied, is already normalized and enters the same create mutation.
/// - Empty/whitespace title is rejected by the domain; no task is created.
/// - When `store` is `Some`, persists domain state after a successful create.
pub fn capture_save(
    state: &mut DomainState,
    store: Option<&TaskStore>,
    snapshot: &InvocationSnapshot,
    title: impl AsRef<str>,
    notes: Option<String>,
    scope_override: Option<TaskScope>,
    thread: Option<String>,
) -> Result<TaskId, CaptureError> {
    let scope = scope_override.unwrap_or_else(|| snapshot.default_scope.clone());
    let id = state.create(title, notes, scope, snapshot.provenance, thread)?;
    if let Some(store) = store {
        // Merge with any concurrent writer before persist (board + capture).
        store.reload_merge_save(state)?;
    }
    Ok(id)
}

/// Board capture variant carrying validated assignment and dispatch base fields.
#[allow(clippy::too_many_arguments)]
pub fn capture_save_configured(
    state: &mut DomainState,
    store: Option<&TaskStore>,
    snapshot: &InvocationSnapshot,
    title: impl AsRef<str>,
    notes: Option<String>,
    scope_override: Option<TaskScope>,
    thread: Option<String>,
    assignee: Option<String>,
    base: Option<String>,
) -> Result<TaskId, CaptureError> {
    let scope = scope_override.unwrap_or_else(|| snapshot.default_scope.clone());
    // An expanded draft may change project after its !b token was lifted. Validate
    // again at save against the final destination, before any domain mutation.
    if let Some(branch) = base.as_deref() {
        crate::git_base::validate_task_base(&scope, branch, false)
            .map_err(CaptureError::UnknownBase)?;
    }

    let id = state.create_configured(
        title,
        notes,
        scope,
        snapshot.provenance,
        thread,
        assignee,
        base,
    )?;
    if let Some(store) = store {
        store.reload_merge_save(state)?;
    }
    Ok(id)
}

/// Directives lifted from a quick-add title before capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickAddTokens {
    pub title: String,
    pub scope: Option<TaskScope>,
    pub thread: Option<String>,
    pub assignee: Option<String>,
    pub base: Option<String>,
    /// `!w T202`, repeatable: the tasks the new task runs after.
    pub after: Vec<u64>,
}

/// Lift and validate whitespace-delimited `!p`, `!t`, `!a`, `!b`, and `!w` directives.
///
/// Base validation uses the effective task destination, never the process checkout.
pub fn lift_quick_add_tokens(
    value: &str,
    domain: &DomainState,
    snapshot: Option<&InvocationSnapshot>,
    default_scope: &TaskScope,
    agent_names: &[String],
) -> Result<QuickAddTokens, String> {
    let words: Vec<&str> = value.split_whitespace().collect();
    let mut title = Vec::new();
    let mut scope = None;
    let mut thread = None;
    let mut assignee = None;
    let mut base = None;
    let mut after = Vec::new();
    let mut index = 0;

    while let Some(word) = words.get(index) {
        match *word {
            "!p" => {
                let argument = quick_add_token_argument(&words, index);
                scope = Some(match argument {
                    Some(path) => {
                        let resolved = crate::scope::resolve_project_path(path, domain, snapshot)
                            .map_err(|error| error.message(path))?;
                        if domain.is_project_archived(&resolved) {
                            return Err(format!(
                                "project {} is archived",
                                crate::ui::render::short_project(&resolved)
                            ));
                        }
                        TaskScope::Project { path: resolved }
                    }
                    None => TaskScope::Global,
                });
                index += usize::from(argument.is_some()) + 1;
            }
            "!t" => {
                let argument = quick_add_token_argument(&words, index);
                thread = match argument {
                    Some(name) => Some(normalize_thread(name).map_err(thread_refusal_message)?),
                    None => None,
                };
                index += usize::from(argument.is_some()) + 1;
            }
            "!a" => {
                let argument = quick_add_token_argument(&words, index);
                assignee = match argument {
                    Some(name) => {
                        let normalized = normalize_thread(name).map_err(|error| {
                            thread_refusal_message(error).replacen("thread", "agent name", 1)
                        })?;
                        if agent_names.iter().any(|name| name == &normalized) {
                            Some(normalized)
                        } else {
                            return Err(format!("unknown agent {normalized}"));
                        }
                    }
                    None => None,
                };
                index += usize::from(argument.is_some()) + 1;
            }
            "!b" => {
                let argument = quick_add_token_argument(&words, index);
                base = argument.map(str::to_owned);
                index += usize::from(argument.is_some()) + 1;
            }
            // `!w T202`: run after T202 ("wait for"). Unknown or done tasks refuse.
            "!w" => {
                let number = quick_add_token_argument(&words, index)
                    .and_then(|argument| crate::cli::parser::parse_after_number(argument).ok())
                    .ok_or_else(|| "!w needs a task number like T12".to_string())?;
                domain
                    .check_new_after(&[number])
                    .map_err(|error| error.to_string())?;
                if !after.contains(&number) {
                    after.push(number);
                }
                index += 2;
            }
            _ => {
                title.push(*word);
                index += 1;
            }
        }
    }

    if let Some(branch) = base.as_deref() {
        crate::git_base::validate_task_base(
            scope.as_ref().unwrap_or(default_scope),
            branch,
            false,
        )?;
    }

    Ok(QuickAddTokens {
        title: title.join(" "),
        scope,
        thread,
        assignee,
        base,
        after,
    })
}

fn quick_add_token_argument<'a>(words: &'a [&str], index: usize) -> Option<&'a str> {
    words
        .get(index + 1)
        .copied()
        .filter(|word| !matches!(*word, "!p" | "!t" | "!a" | "!b" | "!w") && !word.starts_with('#'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{build_snapshot, RawHostContext};
    use crate::domain::{HumanStatus, ProvenanceOrigin, TaskEventKind};
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!("tsk-t9-{label}-{nanos}-{seq}"))
    }

    struct TempDirGuard(PathBuf);

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn project_snapshot(path: &str) -> InvocationSnapshot {
        InvocationSnapshot {
            default_scope: TaskScope::Project {
                path: path.to_string(),
            },
            this_repo: Some(PathBuf::from(path)),
            title_prefill: None,
            provenance: ProvenanceOrigin::Capture,
        }
    }

    fn global_snapshot() -> InvocationSnapshot {
        InvocationSnapshot {
            default_scope: TaskScope::Global,
            this_repo: None,
            title_prefill: None,
            provenance: ProvenanceOrigin::Capture,
        }
    }

    #[test]
    fn configured_capture_revalidates_base_against_final_scope_before_mutation() {
        let mut state = DomainState::new();
        let snapshot = project_snapshot("/unused/initial-repo");
        let before = state.clone();
        let result = capture_save_configured(
            &mut state,
            None,
            &snapshot,
            "Captured",
            None,
            Some(TaskScope::Global),
            None,
            None,
            Some("main".into()),
        );
        assert!(matches!(result, Err(CaptureError::UnknownBase(_))));
        assert_eq!(
            serde_json::to_value(state).unwrap(),
            serde_json::to_value(before).unwrap()
        );
    }

    #[test]
    fn default_create_uses_snapshot_project_scope() {
        let snap = project_snapshot("/repos/app");
        let mut state = DomainState::new();
        let id = capture_save(&mut state, None, &snap, "Ship capture", None, None, None)
            .expect("create");

        let task = state.get(id).expect("task");
        assert_eq!(
            task.scope,
            TaskScope::Project {
                path: "/repos/app".into(),
            }
        );
        assert_eq!(task.title, "Ship capture");
        assert_eq!(task.status, HumanStatus::Open);
        assert_eq!(task.provenance, ProvenanceOrigin::Capture);
        assert_eq!(state.tasks().len(), 1);
    }

    #[test]
    fn capture_creation_carries_supplied_normalized_thread() {
        let snap = project_snapshot("/repos/app");
        let mut state = DomainState::new();

        let id = capture_save(
            &mut state,
            None,
            &snap,
            "Threaded capture",
            None,
            None,
            Some("release-2026".into()),
        )
        .expect("create threaded capture");

        let task = state.get(id).expect("task");
        assert_eq!(task.thread.as_deref(), Some("release-2026"));
        assert_eq!(
            task.history
                .iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            vec![TaskEventKind::Created]
        );
    }

    #[test]
    fn default_create_uses_snapshot_global_when_no_repo() {
        let snap = global_snapshot();
        let mut state = DomainState::new();
        let id =
            capture_save(&mut state, None, &snap, "Loose note", None, None, None).expect("create");
        assert_eq!(state.get(id).expect("task").scope, TaskScope::Global);
    }

    #[test]
    fn override_to_global_saves_global() {
        let snap = project_snapshot("/repos/app");
        let mut state = DomainState::new();
        let id = capture_save(
            &mut state,
            None,
            &snap,
            "Global instead",
            Some("notes".into()),
            Some(TaskScope::Global),
            None,
        )
        .expect("create");

        let task = state.get(id).expect("task");
        assert_eq!(task.scope, TaskScope::Global);
        assert_eq!(task.notes.as_deref(), Some("notes"));
    }

    #[test]
    fn override_to_other_project_path_saves_that_path() {
        let snap = project_snapshot("/repos/app");
        let mut state = DomainState::new();
        let other = TaskScope::Project {
            path: "/repos/other".into(),
        };
        let id = capture_save(
            &mut state,
            None,
            &snap,
            "Other project",
            None,
            Some(other.clone()),
            None,
        )
        .expect("create");

        assert_eq!(state.get(id).expect("task").scope, other);
    }

    #[test]
    fn empty_title_rejected_at_domain_boundary() {
        let snap = project_snapshot("/repos/app");
        let mut state = DomainState::new();
        let err = capture_save(&mut state, None, &snap, "   \t  ", None, None, None)
            .expect_err("empty title must fail");
        match err {
            CaptureError::Domain(DomainError::EmptyTitle) => {}
            other => panic!("expected EmptyTitle, got {other:?}"),
        }
        assert!(state.tasks().is_empty());
    }

    #[test]
    fn selection_provenance_and_title_from_snapshot_fields() {
        let raw = RawHostContext {
            cwd: Some("/tmp/nongit-selection".into()),
            selected_text: Some("from selection".into()),
            ..RawHostContext::default()
        };
        let snap = build_snapshot(&raw, PathBuf::from("/tmp/nongit-selection"));
        assert_eq!(snap.provenance, ProvenanceOrigin::Selection);

        let mut state = DomainState::new();
        // Title may still be user-edited; prefill is UI concern. Provenance stays Selection.
        let id = capture_save(&mut state, None, &snap, "from selection", None, None, None)
            .expect("create");
        let task = state.get(id).expect("task");
        assert_eq!(task.provenance, ProvenanceOrigin::Selection);
    }

    #[test]
    fn capture_save_persists_when_store_provided() {
        let dir = temp_dir("persist");
        fs::create_dir_all(&dir).expect("mkdir");
        let _guard = TempDirGuard(dir.clone());
        let store = TaskStore::new(&dir);

        let snap = project_snapshot("/repos/app");
        let mut state = DomainState::new();
        let id = capture_save(
            &mut state,
            Some(&store),
            &snap,
            "Persisted",
            None,
            None,
            None,
        )
        .expect("create+save");

        let reloaded = store.load().expect("load");
        let task = reloaded.get(id).expect("task on disk");
        assert_eq!(task.title, "Persisted");
        assert_eq!(
            task.scope,
            TaskScope::Project {
                path: "/repos/app".into(),
            }
        );
    }
}
