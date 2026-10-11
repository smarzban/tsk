//! Headless step creation, toggling, rename, move, and remove by step short id.

use std::path::PathBuf;

use serde::Serialize;

use crate::cli::add::has_c0_control;
use crate::cli::parser::TaskAddress;
use crate::domain::{DomainError, DomainState, Step};
use crate::store::{default_state_dir, TaskStore};
use uuid::Uuid;

/// The `steps` action parsed from argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepsAction {
    Add {
        text: String,
        at: Option<StepPlacement>,
    },
    Toggle {
        short_id: String,
    },
    Rename {
        short_id: String,
        text: String,
    },
    Move {
        short_id: String,
        to: StepPlacement,
    },
    Remove {
        short_id: String,
    },
}

/// Where `add` inserts or `move` places a step. Anchors are step short ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepPlacement {
    Before(String),
    After(String),
    First,
    Last,
}

/// A steps failure after parsing and before presenting output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepsError {
    UnknownTask,
    SoftDeletedTask(Uuid),
    EmptyStepText,
    InvalidStepText,
    UnknownStep,
    AmbiguousStep,
    SameStep,
    Store(String),
}

impl StepsError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownTask => "unknown-task",
            Self::SoftDeletedTask(_) => "soft-deleted-task",
            Self::EmptyStepText => "empty-step-text",
            Self::InvalidStepText => "invalid-step-text",
            Self::UnknownStep => "unknown-step",
            Self::AmbiguousStep => "ambiguous-step",
            Self::SameStep => "same-step",
            Self::Store(_) => "store-error",
        }
    }
}

/// A successful steps mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepsResult {
    Added {
        short_id: String,
        text: String,
    },
    Toggled {
        short_id: String,
        text: String,
        done: bool,
    },
    Renamed {
        short_id: String,
        text: String,
    },
    Moved {
        short_id: String,
        text: String,
        position: usize,
        count: usize,
    },
    Removed {
        short_id: String,
        text: String,
    },
}

/// Apply one steps action, refusing before any mutation.
///
/// Refusals are resolved at the boundary under the store lock and never reach
/// the durable write, so a refused steps command persists nothing.
pub fn run(
    task_address: TaskAddress,
    action: StepsAction,
    state_dir: Option<PathBuf>,
) -> Result<StepsResult, StepsError> {
    let store = TaskStore::new(state_dir.unwrap_or_else(default_state_dir));
    store
        .locked_transition_if_changed(|domain| {
            let task_id = domain
                .tasks()
                .iter()
                .find(|task| task_address.matches(task))
                .map(|task| task.id);
            let outcome = task_id.map_or(Err(StepsError::UnknownTask), |task_id| {
                apply(domain, task_id, &action)
            });
            let changed = outcome.as_ref().is_ok_and(|(_, changed)| *changed);
            let outcome = outcome.map(|(result, _)| result);
            Ok((outcome, changed))
        })
        .map_err(StepsError::Store)?
}

fn apply(
    domain: &mut DomainState,
    task_id: Uuid,
    action: &StepsAction,
) -> Result<(StepsResult, bool), StepsError> {
    // A deleted task's steps are not scriptable (park/link discipline, not edit's).
    match domain.get(task_id) {
        None => return Err(StepsError::UnknownTask),
        Some(task) if task.soft_deleted => return Err(StepsError::SoftDeletedTask(task_id)),
        _ => {}
    }
    match action {
        StepsAction::Add { text, at } => {
            // Same boundary rule as task titles: any C0 control refuses before trimming.
            if has_c0_control(text) {
                return Err(StepsError::InvalidStepText);
            }
            let text = text.trim();
            if text.is_empty() {
                return Err(StepsError::EmptyStepText);
            }
            let steps = &domain.get(task_id).expect("task exists").steps;
            let index = match at {
                Some(placement) => placement_index(steps, placement)?,
                None => steps.len(),
            };
            let step_id = domain
                .insert_step(task_id, text, index)
                .map_err(map_domain_error)?;
            let steps = &domain.get(task_id).expect("task exists").steps;
            Ok((
                StepsResult::Added {
                    short_id: step_short_id(steps, step_id),
                    text: text.to_owned(),
                },
                true,
            ))
        }
        StepsAction::Toggle { short_id } => {
            let steps = domain.get(task_id).expect("task exists").steps.clone();
            let step_id = resolve_existing_step(&steps, short_id)?;
            domain
                .toggle_step(task_id, step_id)
                .map_err(map_domain_error)?;
            let done = domain
                .get(task_id)
                .expect("task exists")
                .steps
                .iter()
                .any(|step| step.id == step_id && step.done);
            Ok((
                StepsResult::Toggled {
                    short_id: step_short_id(&steps, step_id),
                    text: steps
                        .iter()
                        .find(|step| step.id == step_id)
                        .expect("toggled step exists")
                        .text
                        .clone(),
                    done,
                },
                true,
            ))
        }
        StepsAction::Rename { short_id, text } => {
            if has_c0_control(text) {
                return Err(StepsError::InvalidStepText);
            }
            let text = text.trim();
            if text.is_empty() {
                return Err(StepsError::EmptyStepText);
            }
            let steps = domain.get(task_id).expect("task exists").steps.clone();
            let step_id = resolve_existing_step(&steps, short_id)?;
            let printed = step_short_id(&steps, step_id);
            let current = steps
                .iter()
                .find(|step| step.id == step_id)
                .expect("renamed step exists")
                .text
                .as_str();
            let changed = current != text;
            if changed {
                domain
                    .rename_step(task_id, step_id, text)
                    .map_err(map_domain_error)?;
            }
            Ok((
                StepsResult::Renamed {
                    short_id: printed,
                    text: text.to_owned(),
                },
                changed,
            ))
        }
        StepsAction::Move { short_id, to } => {
            let steps = domain.get(task_id).expect("task exists").steps.clone();
            let step_id = resolve_existing_step(&steps, short_id)?;
            let printed = step_short_id(&steps, step_id);
            // Anchors resolve against the full list (the printed short ids), then place the
            // step among the others, so `--after` its own predecessor is a no-op.
            if let StepPlacement::Before(anchor) | StepPlacement::After(anchor) = to {
                if resolve_existing_step(&steps, anchor)? == step_id {
                    return Err(StepsError::SameStep);
                }
            }
            let others: Vec<Step> = steps
                .iter()
                .filter(|step| step.id != step_id)
                .cloned()
                .collect();
            let index = match to {
                StepPlacement::Before(anchor) | StepPlacement::After(anchor) => {
                    let anchor = resolve_existing_step(&steps, anchor)?;
                    let position = others
                        .iter()
                        .position(|step| step.id == anchor)
                        .expect("anchor is another step");
                    position + usize::from(matches!(to, StepPlacement::After(_)))
                }
                StepPlacement::First => 0,
                StepPlacement::Last => others.len(),
            };
            let changed = domain
                .move_step(task_id, step_id, index)
                .map_err(map_domain_error)?;
            let step = steps
                .iter()
                .find(|step| step.id == step_id)
                .expect("moved step exists");
            Ok((
                StepsResult::Moved {
                    short_id: printed,
                    text: step.text.clone(),
                    position: index + 1,
                    count: steps.len(),
                },
                changed,
            ))
        }
        StepsAction::Remove { short_id } => {
            let steps = domain.get(task_id).expect("task exists").steps.clone();
            let step_id = resolve_existing_step(&steps, short_id)?;
            let text = steps
                .iter()
                .find(|step| step.id == step_id)
                .expect("removed step exists")
                .text
                .clone();
            let printed = step_short_id(&steps, step_id);
            domain
                .remove_step(task_id, step_id)
                .map_err(map_domain_error)?;
            Ok((
                StepsResult::Removed {
                    short_id: printed,
                    text,
                },
                true,
            ))
        }
    }
}

/// The insert index an `add` placement names in the current list.
fn placement_index(steps: &[Step], placement: &StepPlacement) -> Result<usize, StepsError> {
    let anchor_index = |short_id: &str| {
        let anchor = resolve_existing_step(steps, short_id)?;
        Ok(steps
            .iter()
            .position(|step| step.id == anchor)
            .expect("resolved step exists"))
    };
    match placement {
        StepPlacement::Before(anchor) => anchor_index(anchor),
        StepPlacement::After(anchor) => anchor_index(anchor).map(|index| index + 1),
        StepPlacement::First => Ok(0),
        StepPlacement::Last => Ok(steps.len()),
    }
}

fn resolve_existing_step(steps: &[Step], short_id: &str) -> Result<Uuid, StepsError> {
    match resolve_step_short_id(steps, short_id) {
        ShortIdResolution::One(id) => Ok(id),
        ShortIdResolution::Ambiguous => Err(StepsError::AmbiguousStep),
        ShortIdResolution::None => Err(StepsError::UnknownStep),
    }
}

/// Domain commands re-check what the boundary already validated; mirror the twins.
fn map_domain_error(error: DomainError) -> StepsError {
    match error {
        DomainError::UnknownId(_) => StepsError::UnknownTask,
        DomainError::EmptyStepText => StepsError::EmptyStepText,
        DomainError::UnknownStep(_) => StepsError::UnknownStep,
        other => StepsError::Store(other.to_string()),
    }
}

/// The result of matching one short id against a task's steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortIdResolution {
    One(Uuid),
    Ambiguous,
    None,
}

/// Resolve a step short id: an unambiguous prefix of one step id in the list.
///
/// A printed short id is never empty, so the empty operand addresses no step
/// rather than whichever step a bare prefix match would hit.
///
/// SHORTCUT(T-5): linear scan per lookup; fine for typed step lists, index by
/// prefix if lists ever grow unbounded.
pub(crate) fn resolve_step_short_id(steps: &[Step], short_id: &str) -> ShortIdResolution {
    if short_id.is_empty() {
        return ShortIdResolution::None;
    }
    let mut found = None;
    for step in steps {
        if step.id.to_string().starts_with(short_id) {
            if found.is_some() {
                return ShortIdResolution::Ambiguous;
            }
            found = Some(step.id);
        }
    }
    found.map_or(ShortIdResolution::None, ShortIdResolution::One)
}

/// The shortest prefix of one step id that no sibling step id shares.
pub(crate) fn step_short_id(steps: &[Step], id: Uuid) -> String {
    let full = id.to_string();
    for length in 1..full.len() {
        let prefix = &full[..length];
        if steps
            .iter()
            .all(|step| step.id == id || !step.id.to_string().starts_with(prefix))
        {
            return prefix.to_owned();
        }
    }
    full
}

/// One step view for single-task listing, in stored order.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct StepLine {
    pub(crate) id: Uuid,
    pub(crate) done: bool,
    pub(crate) short_id: String,
    pub(crate) text: String,
}

/// The per-step views a single-task listing prints, in stored order.
pub(crate) fn step_lines(steps: &[Step]) -> Vec<StepLine> {
    steps
        .iter()
        .map(|step| StepLine {
            id: step.id,
            done: step.done,
            short_id: step_short_id(steps, step.id),
            text: step.text.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(id: &str, text: &str, done: bool) -> Step {
        Step {
            id: Uuid::parse_str(id).expect("test step id"),
            text: text.into(),
            done,
        }
    }

    #[test]
    fn short_ids_stay_the_shortest_unambiguous_prefix() {
        let steps = [
            step("abcdef01-0000-4000-8000-000000000001", "one", false),
            step("abcdef02-0000-4000-8000-000000000002", "two", true),
        ];
        assert_eq!(step_short_id(&steps, steps[0].id), "abcdef01");
        assert_eq!(step_short_id(&steps, steps[1].id), "abcdef02");

        let alone = [steps[0].clone()];
        assert_eq!(step_short_id(&alone, steps[0].id), "a");
    }

    #[test]
    fn prefix_resolution_is_one_ambiguous_or_none() {
        let steps = [
            step("abcdef01-0000-4000-8000-000000000001", "one", false),
            step("abcdef01-0000-4000-8000-000000000002", "two", false),
            step("99999999-0000-4000-8000-000000000003", "three", false),
        ];
        assert_eq!(
            resolve_step_short_id(&steps, "abcdef01-0000-4000-8000-000000000001"),
            ShortIdResolution::One(steps[0].id)
        );
        assert_eq!(
            resolve_step_short_id(&steps, "abcdef01"),
            ShortIdResolution::Ambiguous
        );
        assert_eq!(
            resolve_step_short_id(&steps, "99999999-0000-4000-8000-000000000003"),
            ShortIdResolution::One(steps[2].id)
        );
        assert_eq!(
            resolve_step_short_id(&steps, "12341234"),
            ShortIdResolution::None
        );
        let sole = [steps[2].clone()];
        assert_eq!(
            resolve_step_short_id(&sole, ""),
            ShortIdResolution::None,
            "the empty operand must address no step, not the sole one"
        );
    }
}
