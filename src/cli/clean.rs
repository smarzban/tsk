//! Headless dispatch cleanup.

use std::path::PathBuf;

use crate::cli::parser::TaskAddress;
use crate::dispatch::{self, CleanupError, CleanupResult, DispatchHost, SystemDispatchHost};
use crate::store::{default_state_dir, TaskStore};

pub fn run(target: TaskAddress, state_dir: Option<PathBuf>) -> Result<CleanupResult, CleanupError> {
    let mut host = SystemDispatchHost;
    run_with_host(
        target,
        state_dir,
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
