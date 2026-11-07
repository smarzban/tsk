//! Headless task archive / unarchive.

use std::path::PathBuf;

use crate::cli::parser::TaskAddress;
use crate::domain::{DomainError, DomainState};
use crate::store::{default_state_dir, TaskStore};

/// A successful archive/unarchive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveResult {
    pub number: u64,
    pub title: String,
    pub archived: bool,
}

/// An archive failure after parsing and before presenting output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveCliError {
    /// Well-formed address with no matching task.
    UnknownTask(String),
    /// The addressed task is soft-deleted.
    SoftDeleted(String),
    /// No project with that name has tasks (project verbs).
    UnknownProject(String),
    Store(String),
}

impl ArchiveCliError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownTask(_) => "unknown-task",
            Self::SoftDeleted(_) => "soft-deleted-task",
            Self::UnknownProject(_) => "unknown-project",
            Self::Store(_) => "store-error",
        }
    }
}

fn display_for(target: &TaskAddress) -> String {
    match target {
        TaskAddress::Number(number) => format!("T{number}"),
        TaskAddress::Id(id) => id.to_string(),
    }
}

/// A successful project archive/unarchive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectResult {
    /// The resolved project's short (basename) name.
    pub name: String,
    pub archived: bool,
}

/// Archive or unarchive one project by `!p` name rules (basename case-insensitive or
/// a `/path` verbatim). Idempotent: the domain verb's bool is the `changed` flag.
pub fn run_project(
    name: String,
    archive: bool,
    state_dir: Option<PathBuf>,
) -> Result<ProjectResult, ArchiveCliError> {
    let store = TaskStore::new(state_dir.unwrap_or_else(default_state_dir));
    store
        .locked_transition_if_changed(|domain: &mut DomainState| {
            // Project verbs keep the old permissive path contract so outside-Git invocation
            // directories, removed directories, and legacy relative scopes do not alter how an
            // existing project is addressed.
            let resolved = match crate::scope::resolve_permissive_project_path(
                &name,
                domain,
                Some(&crate::context::snapshot_from_env()),
            ) {
                Ok(resolved) => resolved,
                Err(error) => {
                    return Ok((
                        Err(ArchiveCliError::UnknownProject(error.message(&name))),
                        false,
                    ))
                }
            };
            let result = if archive {
                domain.archive_project(&resolved)
            } else {
                domain.unarchive_project(&resolved)
            };
            match result {
                Ok(changed) => {
                    let name = domain.project_name(&resolved);
                    Ok((
                        Ok(ProjectResult {
                            name,
                            archived: archive,
                        }),
                        changed,
                    ))
                }
                Err(_) => Ok((
                    Err(ArchiveCliError::UnknownProject(format!(
                        "no project named {name} has tasks"
                    ))),
                    false,
                )),
            }
        })
        .map_err(ArchiveCliError::Store)?
}

/// A successful project title change, or a repeat of one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTitleResult {
    /// The resolved project's stored path.
    pub path: String,
    /// The title now set, `None` when cleared.
    pub title: Option<String>,
}

/// A `tsk project title` refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectTitleError {
    /// No project of that name, or a name two projects share.
    UnknownProject(String),
    /// Another project already carries the title.
    TitleTaken(String),
    /// The title cannot be a project title.
    InvalidTitle(String),
    Store(String),
}

impl ProjectTitleError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownProject(_) => "unknown-project",
            Self::TitleTaken(_) => "title-taken",
            Self::InvalidTitle(_) => "invalid-title",
            Self::Store(_) => "store-error",
        }
    }
}

/// Set or clear one project's title. The project resolves like `tsk project archive`, its
/// title included. Idempotent: setting the title it already has writes nothing.
pub fn run_project_title(
    name: String,
    title: Option<String>,
    state_dir: Option<PathBuf>,
) -> Result<ProjectTitleResult, ProjectTitleError> {
    let store = TaskStore::new(state_dir.unwrap_or_else(default_state_dir));
    store
        .locked_transition_if_changed(|domain: &mut DomainState| {
            let resolved = match crate::scope::resolve_permissive_project_path(
                &name,
                domain,
                Some(&crate::context::snapshot_from_env()),
            ) {
                Ok(resolved) => resolved,
                Err(error) => {
                    return Ok((
                        Err(ProjectTitleError::UnknownProject(error.message(&name))),
                        false,
                    ))
                }
            };
            let outcome = match domain.set_project_title(&resolved, title.as_deref().unwrap_or(""))
            {
                Ok(changed) => (
                    Ok(ProjectTitleResult {
                        title: domain.project_title(&resolved).map(str::to_string),
                        path: resolved,
                    }),
                    changed,
                ),
                Err(DomainError::UnknownProject(_)) => (
                    Err(ProjectTitleError::UnknownProject(format!(
                        "no project named {name} has tasks"
                    ))),
                    false,
                ),
                Err(error @ DomainError::ProjectTitleTaken { .. }) => {
                    (Err(ProjectTitleError::TitleTaken(error.to_string())), false)
                }
                Err(error) => (
                    Err(ProjectTitleError::InvalidTitle(error.to_string())),
                    false,
                ),
            };
            Ok(outcome)
        })
        .map_err(ProjectTitleError::Store)?
}

/// Archive or unarchive one task by address.
///
/// The decision is resolved under the store lock; the domain verb's own bool is the
/// `changed` flag, so repeating an archive on an already-archived task reports the same
/// output and writes nothing.
pub fn run_task(
    target: TaskAddress,
    archive: bool,
    state_dir: Option<PathBuf>,
) -> Result<ArchiveResult, ArchiveCliError> {
    let store = TaskStore::new(state_dir.unwrap_or_else(default_state_dir));
    let display = display_for(&target);
    store
        .locked_transition_if_changed(|state: &mut DomainState| {
            let found = state
                .tasks()
                .iter()
                .find(|task| target.matches(task))
                .map(|task| (task.id, task.number, task.title.clone(), task.soft_deleted));
            let (outcome, changed) = match found {
                None => (
                    Err(ArchiveCliError::UnknownTask(format!(
                        "{display} is not on the board"
                    ))),
                    false,
                ),
                Some((id, number, title, soft_deleted)) => {
                    if soft_deleted {
                        (
                            Err(ArchiveCliError::SoftDeleted(format!(
                                "{display} is deleted"
                            ))),
                            false,
                        )
                    } else {
                        let Some(number) = number else {
                            return Ok((
                                Err(ArchiveCliError::UnknownTask(format!(
                                    "{display} is not on the board"
                                ))),
                                false,
                            ));
                        };
                        let result = if archive {
                            state.archive_task(id)
                        } else {
                            state.unarchive_task(id)
                        };
                        match result {
                            Ok(changed) => (
                                Ok(ArchiveResult {
                                    number,
                                    title,
                                    archived: archive,
                                }),
                                changed,
                            ),
                            Err(DomainError::SoftDeleted(_)) => (
                                Err(ArchiveCliError::SoftDeleted(format!(
                                    "{display} is deleted"
                                ))),
                                false,
                            ),
                            Err(_) => (
                                Err(ArchiveCliError::UnknownTask(format!(
                                    "{display} is not on the board"
                                ))),
                                false,
                            ),
                        }
                    }
                }
            };
            Ok((outcome, changed))
        })
        .map_err(ArchiveCliError::Store)?
}
