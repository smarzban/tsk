//! The inline project title editor: `ctrl+e` on a Projects view row or a project picker
//! row, or the palette's **set project title…** on a project tab.
//!
//! The editor replaces the row's name with a one-line draft prefilled with the title, or
//! the folder name when there is none. `Enter` saves, `Esc` cancels, and an empty draft
//! clears the title. A refused title (taken, or not a valid title) paints on the status row
//! and keeps the editor open. The editor outlives the save call: it closes only when the
//! confirmed sync shows the title it saved, so a failed save that is cancelled leaves it
//! open with its draft.

use std::path::{Path, PathBuf};

use crate::domain::{DomainError, DomainState, TaskScope};
use crate::scope::paths_equivalent;
use crate::ui::edit::{seeded_draft, EditBuffer};

use super::model::{BoardLocation, BoardModel, PickerTab, ProjectScopeOption};
use super::IntentOutcome;

/// The open project title editor.
#[derive(Debug, Clone)]
pub(crate) struct ProjectTitleEdit {
    /// The project's stored path.
    pub(crate) path: String,
    pub(crate) buffer: EditBuffer,
    /// The title the last `Enter` saved, waiting for the confirmed save to close the editor.
    pub(super) pending: Option<Option<String>>,
    /// The palette opened the picker for this edit: close it with the editor.
    pub(super) close_picker: bool,
}

/// The status-row refusal for a desk row: only a project carries a title.
pub(super) const DESK_HAS_NO_TITLE: &str = "the desk has no title: pick a project";

impl BoardModel {
    /// The open project title editor, if any.
    pub(crate) fn project_title_edit(&self) -> Option<&ProjectTitleEdit> {
        self.project_title_edit.as_ref()
    }

    /// The project a title edit would act on here: the picker's highlighted row, the
    /// Projects view's cursor row, or the project tab's project. `Err` carries the refusal.
    fn project_title_target(&self) -> Result<String, &'static str> {
        if let Some(picker) = self.project_picker.as_ref() {
            let path = match picker.tab {
                PickerTab::Main => match picker.options.get(picker.selected) {
                    Some(ProjectScopeOption::Project(path)) => Some(path.clone()),
                    _ => return Err(DESK_HAS_NO_TITLE),
                },
                PickerTab::Archived => picker.archived.get(picker.archived_selected).cloned(),
            };
            return path
                .map(|path| path.to_string_lossy().into_owned())
                .ok_or("no project selected");
        }
        if self.projects_overview() {
            return self
                .selected_project_row()
                .map(|row| row.path)
                .ok_or("no project selected");
        }
        match &self.board_location {
            BoardLocation::Project(path) => Ok(path.to_string_lossy().into_owned()),
            _ => Err("open a project to title it"),
        }
    }

    /// Open the editor on the project the cursor names. On a project tab (the palette's
    /// route) the picker opens first, so the editor sits on that project's row.
    pub(super) fn begin_project_title_edit(&mut self) {
        let opened_picker = self.project_picker.is_none()
            && !self.projects_overview()
            && matches!(self.board_location, BoardLocation::Project(_));
        let path = match self.project_title_target() {
            Ok(path) => path,
            Err(refusal) => {
                self.set_message(refusal);
                return;
            }
        };
        let prefill = self.project_name(Path::new(&path));
        if !self.knows_project(&path) {
            // The current directory's row before its first task: the store keeps titles
            // only for projects it knows, so the title would have nowhere to live.
            self.set_message(format!(
                "{prefill} has no tasks yet: add one, then title it"
            ));
            return;
        }
        if opened_picker {
            self.open_picker_on(&path);
        }
        self.clear_message();
        self.project_title_edit = Some(ProjectTitleEdit {
            path,
            buffer: seeded_draft(&prefill),
            pending: None,
            close_picker: opened_picker,
        });
    }

    /// Whether the store knows the project at `path`: a task carries it, or it has a record.
    fn knows_project(&self, path: &str) -> bool {
        self.tasks.iter().any(|task| {
            matches!(&task.scope, TaskScope::Project { path: scope } if paths_equivalent(scope, path))
        }) || self.project_names().title(path).is_some()
            || crate::scope::archived_path_contains(&self.archived_projects, path)
    }

    /// Open the project picker with `path` highlighted.
    fn open_picker_on(&mut self, path: &str) {
        let options = self.project_options();
        let selected = options
            .iter()
            .position(|option| match option {
                ProjectScopeOption::Project(option) => {
                    paths_equivalent(&option.to_string_lossy(), path)
                }
                ProjectScopeOption::Home => false,
            })
            .unwrap_or(0);
        self.close_popup();
        self.close_command_surface();
        self.project_picker = Some(super::model::ProjectPickerState {
            options,
            selected,
            tab: PickerTab::Main,
            archived: self.archived_project_options(),
            archived_selected: 0,
        });
        self.popup = crate::ui::mouse::BoardPopup::ProjectPicker;
    }

    /// Close the editor without saving.
    pub(super) fn cancel_project_title_edit(&mut self) {
        if let Some(edit) = self.project_title_edit.take() {
            if edit.close_picker {
                self.close_popup();
            }
        }
        self.clear_message();
    }

    /// Close the editor once the confirmed state carries the title it saved, keeping the
    /// cursor on that project wherever the new name sorts it.
    pub(super) fn finish_project_title_save(&mut self) {
        let Some(edit) = self.project_title_edit.as_ref() else {
            return;
        };
        let Some(pending) = edit.pending.as_ref() else {
            return;
        };
        if self.project_names().title(&edit.path) != pending.as_deref() {
            return;
        }
        let edit = self.project_title_edit.take().expect("checked above");
        self.follow_project(&edit.path);
        if edit.close_picker {
            self.close_popup();
        }
    }

    /// Put the Projects view cursor and the picker highlight back on `path` after a rename
    /// re-sorted the lists.
    fn follow_project(&mut self, path: &str) {
        if let Some(index) = self
            .project_rows()
            .iter()
            .position(|row| paths_equivalent(&row.path, path))
        {
            self.projects_selected = index;
        }
        self.refresh_project_picker();
        if let Some(picker) = self.project_picker.as_mut() {
            let target = PathBuf::from(path);
            if let Some(index) = picker.options.iter().position(
                |option| matches!(option, ProjectScopeOption::Project(known) if *known == target),
            ) {
                picker.selected = index;
            }
            if let Some(index) = picker.archived.iter().position(|known| *known == target) {
                picker.archived_selected = index;
            }
        }
    }
}

/// `Enter` in the editor: save the draft as the title. An untitled project's unchanged
/// folder name and an unchanged title close the editor with nothing to save.
pub(super) fn confirm_project_title(
    domain: &mut DomainState,
    model: &mut BoardModel,
) -> Result<IntentOutcome, DomainError> {
    let Some(edit) = model.project_title_edit.as_ref() else {
        return Ok(IntentOutcome::None);
    };
    let path = edit.path.clone();
    let draft = crate::domain::normalize_project_title(edit.buffer.value());
    let untitled_folder = domain.project_title(&path).is_none()
        && draft.as_deref() == Some(crate::scope::project_basename(&path));
    if untitled_folder {
        model.project_title_edit = None;
        model.follow_project(&path);
        return Ok(IntentOutcome::None);
    }
    match domain.set_project_title(&path, draft.as_deref().unwrap_or("")) {
        Ok(true) => {
            let saved = domain.project_title(&path).map(str::to_string);
            if let Some(edit) = model.project_title_edit.as_mut() {
                edit.pending = Some(saved);
            }
            model.undo_hint = domain.last_undo().cloned();
            Ok(IntentOutcome::Persist)
        }
        Ok(false) => {
            let edit = model.project_title_edit.take().expect("checked above");
            if edit.close_picker {
                model.close_popup();
            }
            Ok(IntentOutcome::None)
        }
        Err(
            error @ (DomainError::ProjectTitleTaken { .. } | DomainError::ProjectTitleInvalid(_)),
        ) => {
            model.set_message(error.to_string());
            Ok(IntentOutcome::None)
        }
        Err(DomainError::UnknownProject(_)) => {
            model.set_message("that project has no tasks left: add one, then title it");
            Ok(IntentOutcome::None)
        }
        Err(error) => Err(error),
    }
}
