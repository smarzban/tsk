//! Headless task dispatch.

use std::path::PathBuf;

use crate::agents::AgentProfiles;
use crate::cli::parser::TaskAddress;
use crate::dispatch::{self, DispatchError, DispatchHost, DispatchResult, SystemDispatchHost};
use crate::store::{default_state_dir, TaskStore};

pub fn run(
    target: TaskAddress,
    again: bool,
    state_dir: Option<PathBuf>,
) -> Result<DispatchResult, DispatchError> {
    let mut host = SystemDispatchHost;
    run_with_host(
        target,
        again,
        state_dir,
        dispatch::running_inside_herdr(),
        &mut host,
    )
}

pub fn run_with_host(
    target: TaskAddress,
    again: bool,
    state_dir: Option<PathBuf>,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Result<DispatchResult, DispatchError> {
    let state_dir = state_dir.unwrap_or_else(default_state_dir);
    let store = TaskStore::new(&state_dir);
    let mut state = store
        .load()
        .map_err(|error| DispatchError::Store(error.to_string()))?;
    let id = state
        .tasks()
        .iter()
        .find(|task| target.matches(task))
        .map(|task| task.id)
        .ok_or(DispatchError::UnknownTask)?;
    let profiles = AgentProfiles::load(&state_dir)
        .map_err(|error| DispatchError::AgentConfig(error.to_string()))?;
    let result = dispatch::run_with_host(&mut state, id, &profiles, again, in_herdr, host)?;
    store
        .reload_merge_save(&mut state)
        .map_err(|error| DispatchError::Store(error.to_string()))?;
    Ok(result)
}
