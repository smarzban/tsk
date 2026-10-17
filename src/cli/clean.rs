//! Headless dispatch cleanup.

use std::path::PathBuf;

use crate::cli::parser::TaskAddress;
use crate::dispatch::{self, CleanupError, CleanupResult, DispatchHost};
use crate::store::{default_state_dir, TaskStore};

pub fn run(target: TaskAddress, state_dir: Option<PathBuf>) -> Result<CleanupResult, CleanupError> {
    let (state_dir, mut host) = crate::cli::dispatch::system_host(state_dir);
    run_with_host(
        target,
        Some(state_dir),
        dispatch::running_inside_herdr(),
        &mut host,
    )
}

/// Clean one dispatch, saving its marker only after host cleanup succeeds.
///
pub fn run_with_host(
    target: TaskAddress,
    state_dir: Option<PathBuf>,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Result<CleanupResult, CleanupError> {
    let state_dir = state_dir.unwrap_or_else(default_state_dir);
    let store = TaskStore::new(&state_dir);
    let mut state = store
        .load()
        .map_err(|error| CleanupError::Store(error.to_string()))?;
    let id = state
        .tasks()
        .iter()
        .find(|task| target.matches(task))
        .map(|task| task.id)
        .ok_or(CleanupError::UnknownTask)?;
    let result = dispatch::clean_with_host(&mut state, id, in_herdr, host)?;
    store
        .reload_merge_save(&mut state)
        .map_err(|error| CleanupError::Store(error.to_string()))?;
    Ok(result)
}

/// Why `--merge` merged nothing. The task, its dispatch and GitHub are unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeRefused {
    Cleanup(CleanupError),
    /// `[merge]` in `config.toml` is invalid.
    Config(String),
    Merge(dispatch::MergeError),
}

impl MergeRefused {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Cleanup(error) => error.code(),
            Self::Config(_) => "merge-config",
            Self::Merge(error) => error.code(),
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Cleanup(error) => error.to_string(),
            Self::Config(reason) => reason.clone(),
            Self::Merge(error) => error.message(),
        }
    }

    pub fn is_store(&self) -> bool {
        matches!(self, Self::Cleanup(CleanupError::Store(_)))
    }
}

/// What `--merge` merged, for the result line and `--json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    pub number: u64,
    pub record: crate::domain::Dispatch,
    pub outcome: dispatch::MergeOutcome,
}

/// Merge the task's open pull request, before anything else: the done and the cleanup follow
/// only a merge that succeeded. Refuses a dispatch cleanup would refuse (none, cleaned, dirty).
pub fn merge_with_host(
    target: TaskAddress,
    state_dir: Option<PathBuf>,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Result<Merged, MergeRefused> {
    let state_dir = state_dir.unwrap_or_else(default_state_dir);
    let state = TaskStore::new(&state_dir)
        .load()
        .map_err(|error| MergeRefused::Cleanup(CleanupError::Store(error.to_string())))?;
    let id = state
        .tasks()
        .iter()
        .find(|task| target.matches(task))
        .map(|task| task.id)
        .ok_or(MergeRefused::Cleanup(CleanupError::UnknownTask))?;
    let preview = dispatch::inspect_cleanup_cached_with_host(&state, id, in_herdr, host)
        .map_err(MergeRefused::Cleanup)?;
    if preview.inspection.dirty {
        return Err(MergeRefused::Cleanup(CleanupError::DirtyWorktree));
    }
    let method = crate::agents::MergeMethod::load(&state_dir)
        .map_err(|error| MergeRefused::Config(error.to_string()))?;
    let row = dispatch::MergeRow {
        task_id: id,
        number: preview.number,
        project: preview.project,
        record: preview.record,
    };
    let outcome = dispatch::pr::merge_row(host, &row, method).map_err(MergeRefused::Merge)?;
    Ok(Merged {
        number: row.number,
        record: row.record,
        outcome,
    })
}
