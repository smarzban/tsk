//! Headless task dispatch.

use std::path::PathBuf;

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
    let (state_dir, mut host) = system_host(state_dir);
    let result = run_with_host_base(
        target,
        again,
        Some(state_dir),
        dispatch::running_inside_herdr(),
        base_override.as_deref(),
        &mut host,
    )?;
    if let Some(naming) = &result.naming {
        // Best effort, like naming itself: the dispatch already succeeded.
        let _ = dispatch::spawn_agent_naming_process(naming);
    }
    Ok(result)
}

/// The board store the command works on, and a real host whose launchers live beside it.
pub(crate) fn system_host(state_dir: Option<PathBuf>) -> (PathBuf, SystemDispatchHost) {
    let state_dir = state_dir.unwrap_or_else(default_state_dir);
    let host = SystemDispatchHost::in_state_dir(&state_dir);
    (state_dir, host)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::DispatchHost;

    #[test]
    fn dispatch_and_clean_hosts_keep_launchers_in_the_given_state_dir() {
        let dir = std::env::temp_dir().join("tsk-cli-host-state");
        let (state_dir, host) = system_host(Some(dir.clone()));
        assert_eq!(state_dir, dir);
        let launcher = host.launcher_path("w1").expect("launcher path");
        assert_eq!(launcher, dir.join("launchers").join("dispatch-w1.ps1"));

        let (default, host) = system_host(None);
        assert_eq!(default, default_state_dir());
        assert!(host
            .launcher_path("w1")
            .expect("launcher path")
            .starts_with(std::path::absolute(default).expect("absolute")));
    }
}
