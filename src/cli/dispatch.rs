//! Headless task dispatch.

use std::path::PathBuf;
use std::sync::Mutex;
use std::thread::JoinHandle;

use crate::agents::AgentProfiles;
use crate::cli::parser::TaskAddress;
use crate::dispatch::{self, DispatchError, DispatchHost, DispatchResult, SystemDispatchHost};
use crate::store::{default_state_dir, TaskStore};

pub fn run(
    target: TaskAddress,
    again: bool,
    base_override: Option<String>,
    state_dir: Option<PathBuf>,
) -> Result<DispatchResult, DispatchError> {
    dispatch::ensure_platform_supported()?;
    let state_dir = state_dir.unwrap_or_else(default_state_dir);
    let mut host = SystemDispatchHost::in_state_dir(&state_dir);
    let result = run_with_host_base(
        target,
        again,
        Some(state_dir),
        dispatch::running_inside_herdr(),
        base_override.as_deref(),
        &mut host,
    )?;
    if let Some(naming) = result.naming.clone() {
        let handle = dispatch::spawn_agent_naming(naming);
        if let Ok(mut pending) = PENDING_NAMING.lock() {
            pending.push(handle);
        }
    }
    Ok(result)
}

static PENDING_NAMING: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

/// Let a dispatch's background agent naming finish before the process exits; bounded by its
/// detection timeout. Call only after the command's output is written.
pub fn wait_for_agent_naming() {
    let handles = PENDING_NAMING
        .lock()
        .map(|mut pending| std::mem::take(&mut *pending))
        .unwrap_or_default();
    for handle in handles {
        let _ = handle.join();
    }
}

pub fn run_with_host(
    target: TaskAddress,
    again: bool,
    state_dir: Option<PathBuf>,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Result<DispatchResult, DispatchError> {
    run_with_host_base(target, again, state_dir, in_herdr, None, host)
}

pub fn run_with_host_base(
    target: TaskAddress,
    again: bool,
    state_dir: Option<PathBuf>,
    in_herdr: bool,
    base_override: Option<&str>,
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
    let result = dispatch::run_with_host_base(
        &mut state,
        id,
        &profiles,
        again,
        in_herdr,
        base_override,
        host,
    )?;
    store
        .reload_merge_save(&mut state)
        .map_err(|error| DispatchError::Store(error.to_string()))?;
    Ok(result)
}
