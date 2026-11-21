//! Headless artifact add and remove by task address.

use std::path::PathBuf;

use crate::cli::parser::TaskAddress;
use crate::domain::{actor_from_env, validate_label, validate_target, ArtifactError, DomainState};
use crate::store::{default_state_dir, TaskStore};

/// The `artifacts` action parsed from argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactsAction {
    Add {
        target: String,
        label: Option<String>,
    },
    Remove {
        /// 1-based, in `tsk list T --json` order.
        position: usize,
    },
}

/// An artifact failure after parsing and before presenting output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactsError {
    UnknownTask,
    SoftDeletedTask,
    Target(ArtifactError),
    Label(ArtifactError),
    UnknownArtifact,
    Store(String),
}

impl ArtifactsError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownTask => "unknown-task",
            Self::SoftDeletedTask => "soft-deleted-task",
            Self::Target(error) | Self::Label(error) => error.code(),
            Self::UnknownArtifact => "unknown-artifact",
            Self::Store(_) => "store-error",
        }
    }
}

/// One artifact after a successful mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactsResult {
    pub number: u64,
    pub target: String,
    pub label: Option<String>,
    pub count: usize,
    /// False when an add repeated an existing target without a new label.
    pub changed: bool,
}

/// Apply one artifacts action under the store lock. A refusal persists nothing.
pub fn run(
    task_address: TaskAddress,
    action: ArtifactsAction,
    state_dir: Option<PathBuf>,
) -> Result<ArtifactsResult, ArtifactsError> {
    let store = TaskStore::new(state_dir.unwrap_or_else(default_state_dir));
    store
        .locked_transition_if_changed(|domain: &mut DomainState| {
            let id = domain
                .tasks()
                .iter()
                .find(|task| task_address.matches(task))
                .map(|task| task.id);
            let outcome = id.map_or(Err(ArtifactsError::UnknownTask), |id| {
                apply(domain, id, &action)
            });
            let changed = outcome.as_ref().is_ok_and(|result| result.changed);
            Ok((outcome, changed))
        })
        .map_err(ArtifactsError::Store)?
}

fn apply(
    domain: &mut DomainState,
    id: uuid::Uuid,
    action: &ArtifactsAction,
) -> Result<ArtifactsResult, ArtifactsError> {
    let task = domain.get(id).ok_or(ArtifactsError::UnknownTask)?;
    if task.soft_deleted {
        return Err(ArtifactsError::SoftDeletedTask);
    }
    let number = task.number.ok_or(ArtifactsError::UnknownTask)?;
    let by = actor_from_env();
    let (target, label, changed) = match action {
        ArtifactsAction::Add { target, label } => {
            let target = validate_target(target).map_err(ArtifactsError::Target)?;
            let label = validate_label(label.as_deref()).map_err(ArtifactsError::Label)?;
            let changed = domain
                .add_artifact(id, target.clone(), label.clone(), 0, &by)
                .map_err(|error| ArtifactsError::Store(error.to_string()))?;
            (target, label, changed)
        }
        ArtifactsAction::Remove { position } => {
            let index = position
                .checked_sub(1)
                .ok_or(ArtifactsError::UnknownArtifact)?;
            if index >= task.artifacts.len() {
                return Err(ArtifactsError::UnknownArtifact);
            }
            let target = domain
                .remove_artifact(id, index, &by)
                .map_err(|error| ArtifactsError::Store(error.to_string()))?
                .ok_or(ArtifactsError::UnknownArtifact)?;
            (target, None, true)
        }
    };
    let artifact = domain
        .get(id)
        .and_then(|task| {
            task.artifacts
                .iter()
                .find(|artifact| artifact.target == target)
        })
        .cloned();
    Ok(ArtifactsResult {
        number,
        target,
        label: label.or_else(|| {
            artifact
                .as_ref()
                .and_then(|artifact| artifact.label.clone())
        }),
        count: domain.get(id).expect("task exists").artifacts.len(),
        changed,
    })
}

/// Validate and parse one target and optional label for a status `--artifact` flag. The round
/// is stamped by the caller from the block or review round the status opens.
pub fn parse_attached(
    raw: &str,
    label: Option<&str>,
) -> Result<(String, Option<String>), ArtifactError> {
    Ok((
        validate_target(raw)?,
        validate_label(label).map_err(|error| match error {
            ArtifactError::EmptyLabel => ArtifactError::Invalid,
            other => other,
        })?,
    ))
}
