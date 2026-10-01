//! Capture UI: form draft, save/cancel, keyboard + mouse.
//!
//! Pure model + intent apply so unit tests need no TTY. Creates only through [`crate::capture::capture_save`].

use std::path::{Path, PathBuf};

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use uuid::Uuid;

use crate::capture::{capture_save, CaptureError};
use crate::context::InvocationSnapshot;
use crate::domain::{DomainState, TaskScope};
use crate::save_recovery::SaveRecovery;
use crate::scope::{archived_path_contains, paths_equivalent};
use crate::store::TaskStore;

use super::edit::{
    edit_block_region, escaped_draft_rows, flatten_line_breaks, place_edit_cursor_at, seeded_draft,
    EditBuffer,
};
use super::input::{
    CaptureIntent, CAPTURE_HELP_LINE, CAPTURE_NOTES_HELP_LINE, CAPTURE_SAVE_RECOVERY_HELP_LINE,
};
use super::mouse::capture_layout_for_model;
use super::render;
use super::{present_line, present_lines, terminal_text};

/// Form field with keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptureField {
    #[default]
    Title,
    Notes,
    Thread,
    Scope,
    Assignee,
}

/// Visible capture surface title.
pub const CAPTURE_TITLE: &str = "Capture";

/// Explicit Capture scope control.
///
/// Presentation only: the durable value stays [`TaskScope`]. `Other` covers any
/// project path that is not the resolved current repository, so it is the only
/// choice that discloses a full path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureScopeChoice {
    /// The resolved current repository, named without showing its path.
    ThisProject,
    Global,
    /// An explicitly chosen or typed project path.
    Other,
}

/// Scope controls in presentation order: full label, narrow label, choice.
///
/// The narrow label keeps every control present on a cramped row instead of
/// dropping one, which would make it mouse-unreachable.
pub const CAPTURE_SCOPE_CONTROLS: &[(&str, &str, CaptureScopeChoice)] = &[
    ("This project", "Proj", CaptureScopeChoice::ThisProject),
    ("Desk", "Desk", CaptureScopeChoice::Global),
    ("Other\u{2026}", "Other\u{2026}", CaptureScopeChoice::Other),
];

/// Message shown when This project is chosen without a resolved repository.
pub const CAPTURE_THIS_PROJECT_UNAVAILABLE: &str = "This project unavailable here";

/// What either surface says when the domain refuses a title that trims to nothing.
///
/// Capture and the board reject on the same rule, so they state it in the same words; both
/// read this constant rather than keeping a phrasing each that could drift apart.
pub const TITLE_REQUIRED_MESSAGE: &str = "Title required";

/// Columns a field row spends on its focus marker and label (`"▎ Title: "`) before the
/// value begins; the rest of the row is the field's value region.
///
/// The renderer owns this geometry because it is the one that paints the label
/// ([`field_line_presented`]); [`super::mouse`] imports it so click regions land on the
/// columns actually painted, instead of keeping a second copy that could drift.
pub const CAPTURE_FIELD_LABEL_WIDTH: u16 = 10;

/// Result of applying a [`CaptureIntent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureOutcome {
    /// Stay on the form (draft edit, validation, etc.).
    None,
    /// Task created (caller may already have persisted via `store` arg).
    Saved(Uuid),
    /// User discarded the draft; leave capture.
    Cancelled,
}

/// Pure capture form state for one invocation.
#[derive(Debug)]
pub struct CaptureModel {
    /// One cursor-carrying draft per editable text field, so each keeps its own cursor
    /// across focus changes.
    title: EditBuffer,
    notes: EditBuffer,
    /// Optional task thread, normalized with the shared domain rule at save time.
    thread: EditBuffer,
    /// Field-local thread validation feedback, painted on the Thread row.
    thread_refusal: Option<String>,
    /// Scope that will be saved (starts as snapshot default; user may override).
    scope: TaskScope,
    this_repo: Option<PathBuf>,
    /// True when the invocation repository is an archived project. AC-39: the surface
    /// then never offers it as a scope, exactly as if there were no repository.
    this_repo_archived: bool,
    focused: CaptureField,
    /// When `Some`, scope row is editing an arbitrary project path.
    scope_path_edit: Option<String>,
    /// Chrome message (e.g. empty-title validation).
    message: Option<String>,
    /// Failed baseline and working create state, retained until Retry or Cancel.
    save_recovery: SaveRecovery<DomainState>,
    /// The id allocated in the staged working state, returned only after persistence succeeds.
    pending_saved_id: Option<Uuid>,
    /// The Notes wrap width the last painted frame used. The renderer records it
    /// because vertical arrow movement wraps at the painted width, which only the
    /// render path knows; zero until then keeps the arrows inert.
    pub notes_width: std::cell::Cell<usize>,
}

impl CaptureModel {
    /// Resolve the form's initial scope from its immutable invocation snapshot.
    ///
    /// Board quick-add adjusts that snapshot once for its current lens, then calls this same
    /// derivation rather than keeping a second default-scope rule.
    pub fn default_scope(snapshot: &InvocationSnapshot) -> TaskScope {
        snapshot.default_scope.clone()
    }

    /// Build a draft from an invocation snapshot.
    pub fn from_snapshot(snapshot: &InvocationSnapshot) -> Self {
        Self {
            title: seeded_draft(snapshot.title_prefill.as_deref().unwrap_or_default()),
            notes: seeded_draft(""),
            thread: seeded_draft(""),
            thread_refusal: None,
            scope: Self::default_scope(snapshot),
            this_repo: snapshot.this_repo.clone(),
            this_repo_archived: false,
            focused: CaptureField::Title,
            scope_path_edit: None,
            message: None,
            save_recovery: SaveRecovery::new(),
            pending_saved_id: None,
            notes_width: std::cell::Cell::new(0),
        }
    }

    pub fn title(&self) -> &str {
        self.title.value()
    }

    pub fn notes(&self) -> &str {
        self.notes.value()
    }

    pub fn thread(&self) -> &str {
        self.thread.value()
    }

    pub fn scope(&self) -> &TaskScope {
        &self.scope
    }

    pub fn this_repo(&self) -> Option<&Path> {
        self.this_repo.as_deref()
    }

    pub fn focused(&self) -> CaptureField {
        self.focused
    }

    /// True while the scope row is accepting an arbitrary project path.
    pub fn is_path_editing(&self) -> bool {
        self.scope_path_edit.is_some()
    }

    /// Current path-edit buffer, if path editing is active.
    pub fn scope_path_edit(&self) -> Option<&str> {
        self.scope_path_edit.as_deref()
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// The scope control that currently reads as selected.
    pub fn scope_choice(&self) -> CaptureScopeChoice {
        if self.scope_path_edit.is_some() {
            return CaptureScopeChoice::Other;
        }
        match &self.scope {
            TaskScope::Global => CaptureScopeChoice::Global,
            TaskScope::Project { path } => match self.this_repo.as_deref() {
                Some(repo) if repo == Path::new(path) => CaptureScopeChoice::ThisProject,
                _ => CaptureScopeChoice::Other,
            },
        }
    }

    /// Whether a current repository exists to name as This project. An archived
    /// invocation repository reads as unavailable (AC-39).
    pub fn this_project_available(&self) -> bool {
        self.this_repo.is_some() && !self.this_repo_archived
    }

    /// Tell the draft which projects are archived. When the invocation repository is one
    /// of them, This project stops being on offer and a scope already pointing at it
    /// falls back to the desk.
    pub fn mark_archived_projects(&mut self, archived: &std::collections::BTreeSet<String>) {
        let Some(repo) = self.this_repo.as_deref() else {
            return;
        };
        let path = repo.to_string_lossy().into_owned();
        self.this_repo_archived = archived_path_contains(archived, &path);
        if self.this_repo_archived
            && matches!(&self.scope, TaskScope::Project { path: scope_path } if archived
                .iter()
                .any(|stored| paths_equivalent(stored, scope_path)))
        {
            self.scope = TaskScope::Global;
        }
    }

    /// Whether the full project path may be shown: only while Other is selected or edited.
    pub fn shows_project_path(&self) -> bool {
        self.scope_choice() == CaptureScopeChoice::Other
    }

    /// Full path presented on the disclosed path row, if any.
    pub fn disclosed_path(&self) -> Option<&str> {
        if let Some(buffer) = self.scope_path_edit.as_deref() {
            return Some(buffer);
        }
        match (&self.scope, self.scope_choice()) {
            (TaskScope::Project { path }, CaptureScopeChoice::Other) => Some(path.as_str()),
            _ => None,
        }
    }

    /// Legend for the current focus: Enter saves everywhere except in Notes, where it opens
    /// a line, so that row names the save chord instead.
    pub fn help_line(&self) -> &'static str {
        if self.save_recovery.is_pending() {
            CAPTURE_SAVE_RECOVERY_HELP_LINE
        } else if self.focused == CaptureField::Notes {
            CAPTURE_NOTES_HELP_LINE
        } else {
            CAPTURE_HELP_LINE
        }
    }

    /// Whether a submitted draft is waiting for an explicit Retry or Cancel.
    pub fn is_save_recovery(&self) -> bool {
        self.save_recovery.is_pending()
    }

    fn begin_save_recovery(&mut self, id: Uuid) {
        self.pending_saved_id = Some(id);
        self.message = Some(format!(
            "save failed: {}",
            self.save_recovery.error().unwrap_or("save failed")
        ));
    }

    fn refresh_save_recovery_message(&mut self) {
        self.message = Some(format!(
            "save failed: {}",
            self.save_recovery.error().unwrap_or("save failed")
        ));
    }

    fn end_save_recovery(&mut self) -> Uuid {
        self.pending_saved_id
            .take()
            .expect("pending Capture recovery has a task id")
    }

    /// The focused field's draft, or `None` on the scope row.
    ///
    /// This is what the single `focused_buffer_mut` became: the scope path stays a plain
    /// `String` with append-and-backspace typing, so one `&mut String` can
    /// no longer stand for all three fields. Callers that also touch the scope path match
    /// on [`CaptureModel::focused`] and reach `scope_path_edit` directly.
    fn focused_draft_mut(&mut self) -> Option<&mut EditBuffer> {
        match self.focused {
            CaptureField::Title => Some(&mut self.title),
            CaptureField::Notes => Some(&mut self.notes),
            CaptureField::Thread => Some(&mut self.thread),
            CaptureField::Scope | CaptureField::Assignee => None,
        }
    }

    fn focus_next(&mut self) {
        self.abandon_path_edit();
        self.focused = match self.focused {
            CaptureField::Title => CaptureField::Notes,
            CaptureField::Notes => CaptureField::Thread,
            CaptureField::Thread => CaptureField::Scope,
            CaptureField::Scope => CaptureField::Title,
            CaptureField::Assignee => CaptureField::Title,
        };
    }

    fn focus_prev(&mut self) {
        self.abandon_path_edit();
        self.focused = match self.focused {
            CaptureField::Title => CaptureField::Scope,
            CaptureField::Notes => CaptureField::Title,
            CaptureField::Thread => CaptureField::Notes,
            CaptureField::Scope => CaptureField::Thread,
            CaptureField::Assignee => CaptureField::Scope,
        };
    }

    fn cycle_scope(&mut self) {
        self.scope_path_edit = None;
        let repo = self
            .this_project_available()
            .then_some(self.this_repo.as_deref())
            .flatten();
        self.scope = match (&self.scope, repo) {
            (TaskScope::Global, Some(repo)) => TaskScope::Project {
                path: repo.to_string_lossy().into_owned(),
            },
            (TaskScope::Project { .. }, _) | (TaskScope::Global, None) => TaskScope::Global,
        };
        self.message = None;
    }

    /// Apply an explicit scope control choice; never synthesizes a missing project.
    fn select_scope(&mut self, choice: CaptureScopeChoice) {
        self.focused = CaptureField::Scope;
        match choice {
            CaptureScopeChoice::ThisProject => match self
                .this_project_available()
                .then_some(self.this_repo.as_deref())
                .flatten()
            {
                Some(repo) => {
                    self.scope_path_edit = None;
                    self.scope = TaskScope::Project {
                        path: repo.to_string_lossy().into_owned(),
                    };
                    self.message = None;
                }
                None => {
                    self.message = Some(CAPTURE_THIS_PROJECT_UNAVAILABLE.into());
                }
            },
            CaptureScopeChoice::Global => {
                self.scope_path_edit = None;
                self.scope = TaskScope::Global;
                self.message = None;
            }
            CaptureScopeChoice::Other => self.begin_scope_path_edit(),
        }
    }

    fn begin_scope_path_edit(&mut self) {
        let initial = match &self.scope {
            TaskScope::Project { path } => path.clone(),
            TaskScope::Global => String::new(),
        };
        self.scope_path_edit = Some(initial);
        self.focused = CaptureField::Scope;
        self.message = None;
    }

    fn confirm_scope_path_edit(&mut self) {
        if let Some(path) = self.scope_path_edit.take() {
            let path = path.trim();
            if !path.is_empty() {
                self.scope = TaskScope::Project {
                    path: path.to_string(),
                };
            }
            // Empty path: leave prior scope unchanged.
        }
        self.message = None;
    }

    fn abandon_path_edit(&mut self) {
        self.scope_path_edit = None;
    }
}

/// Apply a capture intent. On save, calls [`capture_save`] (domain create + optional store).
///
/// Empty/whitespace title does not create a task and leaves domain/store unchanged.
pub fn apply_capture_intent(
    domain: &mut DomainState,
    store: Option<&TaskStore>,
    snapshot: &InvocationSnapshot,
    model: &mut CaptureModel,
    intent: CaptureIntent,
) -> Result<CaptureOutcome, CaptureError> {
    if model.save_recovery.is_pending() {
        return match intent {
            CaptureIntent::RetrySave => {
                let saved = model.save_recovery.retry(|working| match store {
                    Some(store) => store
                        .reload_merge_save(working)
                        .map_err(|error| error.to_string()),
                    None => Ok(()),
                });
                match saved {
                    Some(working) => {
                        *domain = working;
                        let id = model.end_save_recovery();
                        model.message = None;
                        Ok(CaptureOutcome::Saved(id))
                    }
                    None => {
                        model.refresh_save_recovery_message();
                        Ok(CaptureOutcome::None)
                    }
                }
            }
            CaptureIntent::CancelSave => {
                *domain = model
                    .save_recovery
                    .cancel()
                    .expect("pending Capture recovery has a baseline");
                let _ = model.end_save_recovery();
                model.message = None;
                Ok(CaptureOutcome::Cancelled)
            }
            _ => {
                model.refresh_save_recovery_message();
                Ok(CaptureOutcome::None)
            }
        };
    }

    match intent {
        CaptureIntent::Cancel => {
            // Esc while path-editing aborts the path buffer only (stay on form).
            if model.is_path_editing() {
                model.abandon_path_edit();
                model.message = None;
                return Ok(CaptureOutcome::None);
            }
            model.message = None;
            model.thread_refusal = None;
            Ok(CaptureOutcome::Cancelled)
        }
        CaptureIntent::FocusNext => {
            model.focus_next();
            Ok(CaptureOutcome::None)
        }
        CaptureIntent::FocusPrev => {
            model.focus_prev();
            Ok(CaptureOutcome::None)
        }
        CaptureIntent::FocusField(field) => {
            if field != CaptureField::Scope {
                model.abandon_path_edit();
            }
            model.focused = field;
            model.message = None;
            Ok(CaptureOutcome::None)
        }
        CaptureIntent::CycleScope => {
            model.cycle_scope();
            Ok(CaptureOutcome::None)
        }
        CaptureIntent::SelectScope(choice) => {
            model.select_scope(choice);
            Ok(CaptureOutcome::None)
        }
        CaptureIntent::BeginScopePathEdit => {
            model.begin_scope_path_edit();
            Ok(CaptureOutcome::None)
        }
        CaptureIntent::Insert(c) => {
            match model.focused {
                // The scope path keeps its append-and-backspace buffer.
                CaptureField::Scope => {
                    if let Some(path) = model.scope_path_edit.as_mut() {
                        path.push(c);
                        model.message = None;
                    }
                }
                CaptureField::Assignee => {}
                CaptureField::Title | CaptureField::Notes | CaptureField::Thread => {
                    edit_draft(model, |draft| draft.insert_char(c));
                }
            }
            Ok(CaptureOutcome::None)
        }
        CaptureIntent::Backspace => {
            match model.focused {
                CaptureField::Scope => {
                    if let Some(path) = model.scope_path_edit.as_mut() {
                        path.pop();
                        model.message = None;
                    }
                }
                CaptureField::Assignee => {}
                CaptureField::Title | CaptureField::Notes | CaptureField::Thread => {
                    edit_draft(model, EditBuffer::backspace);
                }
            }
            Ok(CaptureOutcome::None)
        }
        CaptureIntent::InsertText(text) => {
            // Notes keeps a pasted break verbatim; Title is one line, so each break
            // becomes a space rather than gluing the words on either side of it together.
            match model.focused {
                // The scope path keeps its append-and-backspace buffer, and it
                // is one line, so a pasted break folds to a space exactly as Title's does.
                CaptureField::Scope => {
                    if let Some(path) = model.scope_path_edit.as_mut() {
                        path.push_str(&flatten_line_breaks(&text));
                        model.message = None;
                    }
                }
                CaptureField::Assignee => {}
                CaptureField::Title | CaptureField::Thread => {
                    edit_draft(model, |draft| {
                        draft.insert_text(&flatten_line_breaks(&text))
                    });
                }
                CaptureField::Notes => {
                    edit_draft(model, |draft| draft.insert_text(&text));
                }
            }
            Ok(CaptureOutcome::None)
        }
        CaptureIntent::InsertLineBreak => {
            edit_draft(model, |draft| draft.insert_char('\n'));
            Ok(CaptureOutcome::None)
        }
        CaptureIntent::DeleteForward => {
            edit_draft(model, EditBuffer::delete_forward);
            Ok(CaptureOutcome::None)
        }
        CaptureIntent::MoveLeft => {
            move_draft(model, EditBuffer::move_left);
            Ok(CaptureOutcome::None)
        }
        CaptureIntent::MoveRight => {
            move_draft(model, EditBuffer::move_right);
            Ok(CaptureOutcome::None)
        }
        CaptureIntent::MoveLineStart => {
            move_draft(model, EditBuffer::move_line_start);
            Ok(CaptureOutcome::None)
        }
        CaptureIntent::MoveLineEnd => {
            move_draft(model, EditBuffer::move_line_end);
            Ok(CaptureOutcome::None)
        }
        CaptureIntent::MoveWordLeft => {
            move_draft(model, EditBuffer::move_word_left);
            Ok(CaptureOutcome::None)
        }
        CaptureIntent::MoveWordRight => {
            move_draft(model, EditBuffer::move_word_right);
            Ok(CaptureOutcome::None)
        }
        CaptureIntent::MoveUp | CaptureIntent::MoveDown => {
            let delta: isize = if matches!(intent, CaptureIntent::MoveDown) {
                1
            } else {
                -1
            };
            let target = (model.focused == CaptureField::Notes)
                .then(|| {
                    crate::ui::edit::wrapped_vertical_move(
                        &model.notes,
                        model.notes_width.get(),
                        delta,
                    )
                })
                .flatten();
            if let Some(target) = target {
                edit_draft(model, |draft| draft.set_cursor(target));
            }
            Ok(CaptureOutcome::None)
        }
        CaptureIntent::Save => {
            // Enter while path-editing confirms Project{path}.
            if model.is_path_editing() {
                model.confirm_scope_path_edit();
                return Ok(CaptureOutcome::None);
            }
            if model.title().trim().is_empty() {
                model.message = Some(TITLE_REQUIRED_MESSAGE.into());
                return Ok(CaptureOutcome::None);
            }
            let notes = if model.notes().trim().is_empty() {
                None
            } else {
                Some(model.notes().to_string())
            };
            let thread_value = model.thread().trim();
            let thread = if thread_value.is_empty() {
                None
            } else {
                match crate::domain::normalize_thread(thread_value) {
                    Ok(thread) => Some(thread),
                    Err(error) => {
                        model.thread_refusal = Some(crate::domain::thread_refusal_message(error));
                        model.message = None;
                        return Ok(CaptureOutcome::None);
                    }
                }
            };
            // The controller owns independent baseline and working snapshots, so an error
            // cannot leak a staged task into the active domain or close this form.
            let baseline =
                serde_json::from_value(serde_json::to_value(&*domain).expect("serialize domain"))
                    .expect("deserialize domain");
            let mut working = std::mem::take(domain);
            let id = match capture_save(
                &mut working,
                None,
                snapshot,
                model.title(),
                notes,
                Some(model.scope.clone()),
                thread,
            ) {
                Ok(id) => id,
                Err(error) => {
                    *domain = working;
                    return Err(error);
                }
            };
            let save_result = match store {
                Some(store) => store
                    .reload_merge_save(&mut working)
                    .map_err(|error| error.to_string()),
                None => Ok(()),
            };
            if let Err(error) = save_result {
                model.save_recovery.fail(baseline, working, error);
                model.begin_save_recovery(id);
                return Ok(CaptureOutcome::None);
            }
            *domain = working;
            model.message = None;
            Ok(CaptureOutcome::Saved(id))
        }
        CaptureIntent::RetrySave | CaptureIntent::CancelSave => Ok(CaptureOutcome::None),
    }
}

/// Apply one value-changing draft operation to the focused field's own buffer.
///
/// Changing the value dismisses the form message, because the draft the message was about
/// is no longer the draft on screen.
///
/// Inert on the scope row: its path buffer is not one of the pinned editable text fields
/// and keeps its plain-`String` handling, so every cursor intent is a no-op there.
fn edit_draft(model: &mut CaptureModel, operation: impl FnOnce(&mut EditBuffer)) {
    let editing_thread = model.focused == CaptureField::Thread;
    if let Some(draft) = model.focused_draft_mut() {
        operation(draft);
        model.message = None;
        if editing_thread {
            model.thread_refusal = None;
        }
    }
}

/// Apply one pure cursor movement to the focused field's own buffer.
///
/// Leaves the form message alone (as the board's editor does): moving around after a
/// rejected save changes nothing about the draft, so [`TITLE_REQUIRED_MESSAGE`] must stay
/// readable.
fn move_draft(model: &mut CaptureModel, operation: impl FnOnce(&mut EditBuffer)) {
    if let Some(draft) = model.focused_draft_mut() {
        operation(draft);
    }
}

/// Human-readable scope for the form line.
pub fn format_scope(scope: &TaskScope) -> String {
    match scope {
        TaskScope::Global => "desk".into(),
        TaskScope::Project { path } => format!("project:{path}"),
    }
}

fn field_style(focused: bool) -> Style {
    if focused {
        render::style_reverse_bold()
    } else {
        render::style_plain()
    }
}

/// Non-color marker for a scope control: selected, available, or unavailable.
fn scope_marker(choice: CaptureScopeChoice, model: &CaptureModel) -> char {
    if choice == CaptureScopeChoice::ThisProject && !model.this_project_available() {
        return '×';
    }
    if model.scope_choice() == choice {
        '•'
    } else {
        ' '
    }
}

fn scope_control_style(choice: CaptureScopeChoice, model: &CaptureModel) -> Style {
    if choice == CaptureScopeChoice::ThisProject && !model.this_project_available() {
        return render::style_dim();
    }
    if model.scope_choice() == choice {
        render::style_reverse_bold()
    } else {
        render::style_plain()
    }
}

fn field_line(label: &str, value: &str, focused: bool) -> Line<'static> {
    field_line_presented(label, terminal_text(value), focused)
}

/// A field row whose value is already escaped and already width-bounded.
///
/// The focused editor path reaches here directly: its window came from
/// [`escaped_line_window`], and escaping or truncating it again would misplace the cursor.
/// The window is painted as one unsplit span, for the reasons that helper documents.
fn field_line_presented(label: &str, value: String, focused: bool) -> Line<'static> {
    field_row_presented(Some(label), value, focused)
}

/// One rendered row of a field: the focus marker, the label columns, then the value.
///
/// `label` is `None` on the second and later rows of a multi-row field, which keep
/// those columns blank so every rendered line of the field starts at the same column the
/// hit test skips to. The focus marker repeats down the field, since it marks the field
/// rather than one of its rows.
fn field_row_presented(label: Option<&str>, value: String, focused: bool) -> Line<'static> {
    let marker = if focused { "▎" } else { " " };
    let marker_style = if focused {
        render::style_bold()
    } else {
        render::style_dim()
    };
    let label_style = if focused {
        render::style_bold()
    } else {
        render::style_dim()
    };
    let label_columns = match label {
        Some(label) => format!(" {label:<7} "),
        None => " ".repeat(CAPTURE_FIELD_LABEL_WIDTH as usize - 1),
    };
    Line::from(vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(label_columns, label_style),
        Span::styled(value, field_style(focused)),
    ])
}

/// One text field's rendered rows, plus the row and column to put the terminal cursor on
/// when the field has focus.
///
/// Focused fields render through the cursor viewport so the visible window follows the
/// cursor; unfocused fields keep the plain truncating presenter they always had.
///
/// both paths split on line breaks before escaping, so a multiline draft is
/// presented as separate rendered lines and never shows a `\u{000a}` escape. The field's
/// height comes from the shared Capture layout, which the hit test reads too, so a Notes
/// field several rows tall is the same region to the renderer and to the mouse. Title's
/// row is one row tall there, which is the presenters' one-row case unchanged.
fn text_field_block(
    label: &str,
    draft: &EditBuffer,
    row: ratatui::layout::Rect,
    focused: bool,
) -> (
    Vec<Line<'static>>,
    Option<(ratatui::layout::Rect, u16, u16)>,
) {
    let region = edit_block_region(row, CAPTURE_FIELD_LABEL_WIDTH);
    let (width, height) = (region.width as usize, region.height as usize);
    let (values, cursor) = if focused {
        let (windows, cursor_row, column) = escaped_draft_rows(draft, width, height);
        (windows, Some((region, cursor_row, column)))
    } else {
        (present_lines(draft.value(), width, height), None)
    };
    let lines = values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            field_row_presented(if index == 0 { Some(label) } else { None }, value, focused)
        })
        .collect();
    (lines, cursor)
}

/// The Notes field: the draft wrapped to word boundaries across the field's rows,
/// with a vertical window. Focus follows the caret so typing at the end is always
/// visible; reading windows from the top and names hidden rows on a dim tail.
fn notes_field_block(
    draft: &EditBuffer,
    row: ratatui::layout::Rect,
    focused: bool,
) -> (
    Vec<Line<'static>>,
    Option<(ratatui::layout::Rect, u16, u16)>,
) {
    let region = edit_block_region(row, CAPTURE_FIELD_LABEL_WIDTH);
    let width = region.width as usize;
    let height = region.height as usize;
    if width == 0 || height == 0 {
        return (Vec::new(), None);
    }
    let rows = crate::ui::edit::wrap_text(draft.value(), width);
    let (start, cursor) = if focused {
        let (cursor_row, cursor_col) =
            crate::ui::edit::locate_wrapped_cursor(&rows, draft.cursor());
        (
            cursor_row.saturating_sub(height - 1),
            Some((cursor_row, cursor_col)),
        )
    } else {
        (0, None)
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    let hidden = rows.len().saturating_sub(start + height);
    for (index, wrapped) in rows.iter().skip(start).take(height).enumerate() {
        let absolute = start + index;
        let last_shown = index + 1 == height;
        let text = if last_shown && hidden > 0 && !focused {
            // Reading window: name what does not fit instead of implying an end.
            let suffix = format!(" \u{2026} {hidden} more");
            let budget = width.saturating_sub(render::display_width(&suffix));
            format!("{}{suffix}", present_line(&wrapped.text, budget))
        } else {
            wrapped.text.clone()
        };
        lines.push(field_row_presented(
            if absolute == 0 { Some("Notes:") } else { None },
            text,
            focused,
        ));
    }
    if lines.is_empty() {
        lines.push(field_row_presented(Some("Notes:"), String::new(), focused));
    }
    let cursor = cursor.map(|(cursor_row, cursor_col)| {
        (
            region,
            u16::try_from(cursor_row.saturating_sub(start)).unwrap_or(u16::MAX),
            u16::try_from(cursor_col).unwrap_or(u16::MAX),
        )
    });
    (lines, cursor)
}

/// Draw the capture form into any ratatui frame (live TTY or TestBackend).
///
/// Field and button positions match [`super::mouse::capture_layout`] for hit-testing.
pub fn draw_capture(frame: &mut Frame, model: &CaptureModel) {
    let area = frame.area();
    let layout = capture_layout_for_model(area, model);
    let block = Block::bordered()
        .border_style(render::style_dim())
        .title(Span::styled(
            format!(" {CAPTURE_TITLE} "),
            render::style_bold(),
        ));
    frame.render_widget(block, area);

    frame.render_widget(
        Paragraph::new(present_line(
            "  New task  ·  title required, notes optional",
            layout.subtitle_area.width as usize,
        ))
        .style(render::style_dim()),
        layout.subtitle_area,
    );

    let title_focused = model.focused == CaptureField::Title;
    let notes_focused = model.focused == CaptureField::Notes;
    let thread_focused = model.focused == CaptureField::Thread;
    let (title_lines, title_cursor) =
        text_field_block("Title:", &model.title, layout.title_area, title_focused);
    // A multiline Notes draft occupies the whole Notes field, wrapped at word
    // boundaries; the focused window follows the caret, an unfocused one reads
    // from the top with a dim tail naming what does not fit.
    model.notes_width.set(
        layout
            .notes_area
            .width
            .saturating_sub(CAPTURE_FIELD_LABEL_WIDTH) as usize,
    );
    let (notes_lines, notes_cursor) =
        notes_field_block(&model.notes, layout.notes_area, notes_focused);
    let (thread_lines, thread_cursor) =
        text_field_block("Thread:", &model.thread, layout.thread_area, thread_focused);

    // Keep "Title:" / "Notes:" / "Scope:" tokens for render tests and screen readers.
    frame.render_widget(
        Paragraph::new(title_lines).style(field_style(title_focused)),
        layout.title_area,
    );
    frame.render_widget(
        Paragraph::new(notes_lines).style(field_style(notes_focused)),
        layout.notes_area,
    );
    frame.render_widget(
        Paragraph::new(thread_lines).style(field_style(thread_focused)),
        layout.thread_area,
    );
    // The terminal's own cursor carries the edit position; only the focused field has one,
    // and only while the field actually accepts input: every editing intent is inert until
    // a failed save is retried or cancelled, so no cursor is offered there.
    if !model.is_save_recovery() {
        if let Some((region, row, column)) = title_cursor.or(notes_cursor).or(thread_cursor) {
            place_edit_cursor_at(frame, region, row, column);
        }
    }

    if let Some(refusal) = model.thread_refusal.as_deref() {
        frame.render_widget(
            Paragraph::new(present_line(
                refusal,
                layout
                    .thread_area
                    .width
                    .saturating_sub(CAPTURE_FIELD_LABEL_WIDTH) as usize,
            ))
            .style(render::style_reverse_bold()),
            ratatui::layout::Rect::new(
                layout
                    .thread_area
                    .x
                    .saturating_add(CAPTURE_FIELD_LABEL_WIDTH),
                layout.thread_area.y,
                layout
                    .thread_area
                    .width
                    .saturating_sub(CAPTURE_FIELD_LABEL_WIDTH),
                1,
            ),
        );
    }

    // Scope row: label, then one explicit control per choice.
    let scope_focused = model.focused == CaptureField::Scope;
    frame.render_widget(
        Paragraph::new(field_line("Scope:", "", scope_focused)),
        layout.scope_area,
    );
    for chip in &layout.scope_chips {
        let text = present_line(
            &format!("({}) {}", scope_marker(chip.value, model), chip.label),
            chip.rect.width as usize,
        );
        frame.render_widget(
            Paragraph::new(text).style(scope_control_style(chip.value, model)),
            chip.rect,
        );
    }

    // Full project path: disclosed only while Other is selected or edited.
    if !layout.scope_path_area.is_empty() {
        let path = model.disclosed_path().unwrap_or_default();
        let editing = model.is_path_editing();
        let label = if editing { "Path>" } else { "Path:" };
        let value_width = layout
            .scope_path_area
            .width
            .saturating_sub(label.chars().count() as u16 + 3) as usize;
        let value = present_line(path, value_width);
        let line = Line::from(vec![
            Span::styled("  ", render::style_dim()),
            Span::styled(format!("{label} "), render::style_dim()),
            Span::styled(value, field_style(editing)),
        ]);
        frame.render_widget(Paragraph::new(line), layout.scope_path_area);
    }

    match model.message.as_deref() {
        Some(msg) if !msg.is_empty() => {
            frame.render_widget(
                Paragraph::new(format!(
                    "  {}  ",
                    present_line(msg, layout.message_area.width.saturating_sub(4) as usize)
                ))
                .style(render::style_reverse_bold()),
                layout.message_area,
            );
        }
        _ => {
            frame.render_widget(
                Paragraph::new(present_line(
                    "  1 this project  ·  2 desk  ·  3 other path",
                    layout.message_area.width as usize,
                ))
                .style(render::style_dim()),
                layout.message_area,
            );
        }
    }

    // Chip rects are clamped to the room the row had, so present the label rather than
    // letting ratatui clip a chip that did not fit.
    frame.render_widget(
        Paragraph::new(present_line(
            layout.save_chip.label,
            layout.save_chip.rect.width as usize,
        ))
        .style(render::style_reverse_bold()),
        layout.save_chip.rect,
    );
    frame.render_widget(
        Paragraph::new(present_line(
            layout.cancel_chip.label,
            layout.cancel_chip.rect.width as usize,
        ))
        .style(render::style_plain()),
        layout.cancel_chip.rect,
    );
    frame.render_widget(
        Paragraph::new(present_line(
            model.help_line(),
            layout.help_area.width as usize,
        ))
        .style(render::style_dim()),
        layout.help_area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{build_snapshot, RawHostContext};
    use crate::domain::{HumanStatus, ProvenanceOrigin};
    use crate::ui::input::{
        intent_primary_capture_action, map_capture_key, primary_capture_action_sample_key,
        PrimaryCaptureAction, CAPTURE_HELP_LINE, PRIMARY_CAPTURE_ACTIONS,
    };
    use crate::ui::mouse::{capture_layout_for_model, left_click, map_capture_mouse};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::{CrosstermBackend, TestBackend};
    use ratatui::layout::Rect;
    use ratatui::{Terminal, TerminalOptions, Viewport};
    use std::cell::RefCell;
    use std::env;
    use std::fs;
    use std::io::{self, Write};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!("tsk-t13-{label}-{nanos}-{seq}"))
    }

    struct TempDirGuard(PathBuf);

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn snapshot_with_selection(text: &str) -> InvocationSnapshot {
        let raw = RawHostContext {
            cwd: Some("/tmp/nongit-capture-ui".into()),
            selected_text: Some(text.into()),
            ..RawHostContext::default()
        };
        build_snapshot(&raw, PathBuf::from("/tmp/nongit-capture-ui"))
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

    /// Snapshot with no resolvable repository: This project must stay unavailable.
    fn no_project_snapshot() -> InvocationSnapshot {
        InvocationSnapshot {
            default_scope: TaskScope::Global,
            this_repo: None,
            title_prefill: None,
            provenance: ProvenanceOrigin::Capture,
        }
    }

    fn render_rows(model: &CaptureModel, width: u16, height: u16) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| draw_capture(frame, model)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer.cell((x, y)).expect("cell").symbol())
                    .collect::<String>()
            })
            .collect()
    }

    fn render_plain(model: &CaptureModel, width: u16, height: u16) -> String {
        render_rows(model, width, height).join("\n")
    }

    #[derive(Clone, Default)]
    struct AnsiCaptureWriter(Rc<RefCell<Vec<u8>>>);

    impl Write for AnsiCaptureWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Render through the production Crossterm backend rather than TestBackend so this test
    /// exercises the actual SGR stream that reaches a terminal.
    fn render_capture_sgr(model: &CaptureModel, width: u16, height: u16) -> String {
        let writer = AnsiCaptureWriter::default();
        let bytes = Rc::clone(&writer.0);
        let backend = CrosstermBackend::new(writer);
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, width, height)),
            },
        )
        .expect("terminal");
        terminal.draw(|frame| draw_capture(frame, model)).unwrap();
        drop(terminal);
        let output = bytes.borrow().clone();
        String::from_utf8(output).expect("Crossterm writes ANSI text")
    }

    fn apply(
        domain: &mut DomainState,
        snapshot: &InvocationSnapshot,
        model: &mut CaptureModel,
        intent: CaptureIntent,
    ) -> CaptureOutcome {
        apply_capture_intent(domain, None, snapshot, model, intent).expect("intent")
    }

    /// V1 mono contract: render each standalone-capture state through Crossterm, then scan the
    /// emitted SGR stream. TestBackend buffer checks alone cannot catch an accidental color code.
    #[test]
    fn standalone_capture_frames_emit_no_color_sgr_in_normal_validation_and_save_recovery_states() {
        let snapshot = project_snapshot("/repos/app");
        let normal = CaptureModel::from_snapshot(&snapshot);

        let mut validation_domain = DomainState::new();
        let mut validation = CaptureModel::from_snapshot(&snapshot);
        let validation_outcome = apply_capture_intent(
            &mut validation_domain,
            None,
            &snapshot,
            &mut validation,
            CaptureIntent::Save,
        )
        .expect("validation stays on Capture");
        assert_eq!(validation_outcome, CaptureOutcome::None);
        assert_eq!(validation.message(), Some(TITLE_REQUIRED_MESSAGE));

        let directory = temp_dir("capture-sgr-recovery");
        fs::create_dir_all(&directory).expect("mkdir");
        let _guard = TempDirGuard(directory.clone());
        let blocked_state_dir = directory.join("not-a-directory");
        fs::write(&blocked_state_dir, "block state-directory creation").expect("blocking file");
        let store = TaskStore::new(&blocked_state_dir);
        let mut recovery_domain = DomainState::new();
        let mut recovery = CaptureModel::from_snapshot(&snapshot);
        let recovery_outcome = apply_capture_intent(
            &mut recovery_domain,
            Some(&store),
            &snapshot,
            &mut recovery,
            CaptureIntent::InsertText("Retry this save".into()),
        )
        .expect("draft edit");
        assert_eq!(recovery_outcome, CaptureOutcome::None);
        let recovery_outcome = apply_capture_intent(
            &mut recovery_domain,
            Some(&store),
            &snapshot,
            &mut recovery,
            CaptureIntent::Save,
        )
        .expect("save failure stays on Capture");
        assert_eq!(recovery_outcome, CaptureOutcome::None);
        assert!(recovery.is_save_recovery());
        assert!(
            recovery
                .message()
                .is_some_and(|message| message.contains("failed")),
            "recovery must say failed"
        );

        let backend = TestBackend::new(80, 16);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| draw_capture(frame, &recovery))
            .unwrap();
        let layout = capture_layout_for_model(Rect::new(0, 0, 80, 16), &recovery);
        let message: String = (layout.message_area.x..layout.message_area.right())
            .map(|x| {
                terminal
                    .backend()
                    .buffer()
                    .cell((x, layout.message_area.y))
                    .expect("message cell")
                    .symbol()
            })
            .collect();
        let failed_at = message.find("failed").expect("painted failed message") as u16;
        for x in failed_at..failed_at + "failed".len() as u16 {
            assert!(
                terminal
                    .backend()
                    .buffer()
                    .cell((layout.message_area.x + x, layout.message_area.y))
                    .expect("failed cell")
                    .modifier
                    .contains(ratatui::style::Modifier::REVERSED),
                "failed save must be reversed: {message:?}"
            );
        }

        for model in [&normal, &validation, &recovery] {
            let sgr = render_capture_sgr(model, 80, 16);
            super::super::render::assert_no_color_sgr(&sgr);
        }
    }

    #[test]
    fn focused_scope_row_keeps_unselected_scope_chips_non_reversed() {
        let snapshot = project_snapshot("/repos/app");
        let mut model = CaptureModel::from_snapshot(&snapshot);
        model.focused = CaptureField::Scope;
        let layout = capture_layout_for_model(Rect::new(0, 0, 80, 16), &model);
        let backend = TestBackend::new(80, 16);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| draw_capture(frame, &model)).unwrap();
        let buffer = terminal.backend().buffer();

        for choice in [CaptureScopeChoice::Global, CaptureScopeChoice::Other] {
            let chip = layout
                .scope_chips
                .iter()
                .find(|chip| chip.value == choice)
                .expect("unselected scope chip");
            for y in chip.rect.y..chip.rect.bottom() {
                for x in chip.rect.x..chip.rect.right() {
                    assert!(
                        !buffer[(x, y)]
                            .modifier
                            .contains(ratatui::style::Modifier::REVERSED),
                        "unselected {choice:?} chip inherited reverse at ({x}, {y})"
                    );
                }
            }
        }

        let selected = layout
            .scope_chips
            .iter()
            .find(|chip| chip.value == CaptureScopeChoice::ThisProject)
            .expect("selected scope chip");
        assert!(
            (selected.rect.x..selected.rect.right()).any(|x| {
                buffer[(x, selected.rect.y)]
                    .modifier
                    .contains(ratatui::style::Modifier::REVERSED)
            }),
            "selected scope chip lost its reverse pointer"
        );
    }

    #[test]
    fn capture_card_shows_title_notes_and_scope_controls_without_project_path() {
        let snap = project_snapshot("/repos/deep/app-name");
        let model = CaptureModel::from_snapshot(&snap);
        assert_eq!(model.scope_choice(), CaptureScopeChoice::ThisProject);
        assert!(model.this_project_available());
        assert!(!model.shows_project_path());

        let plain = render_plain(&model, 80, 14);
        for token in [
            "Title:",
            "Notes:",
            "Scope:",
            "This project",
            "Desk",
            "Other",
        ] {
            assert!(plain.contains(token), "missing {token:?}: {plain}");
        }
        // Selected scope reads without color, and the repository path stays hidden.
        assert!(
            plain.contains("(•) This project"),
            "missing selected marker: {plain}"
        );
        assert!(
            !plain.contains("/repos/deep/app-name"),
            "project path leaked before Other: {plain}"
        );
        assert!(
            !plain.contains("Path"),
            "path row shown before Other: {plain}"
        );
    }

    #[test]
    fn keyboard_selects_each_scope_control_and_other_reveals_path() {
        let snap = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);
        model.focused = CaptureField::Scope;

        let cases = [
            ('1', CaptureScopeChoice::ThisProject),
            ('2', CaptureScopeChoice::Global),
            ('3', CaptureScopeChoice::Other),
        ];
        for (key, choice) in cases {
            assert_eq!(
                map_capture_key(CaptureField::Scope, press(KeyCode::Char(key))),
                Some(CaptureIntent::SelectScope(choice)),
                "key {key:?}"
            );
        }

        // Global: explicit, no path.
        apply(
            &mut domain,
            &snap,
            &mut model,
            CaptureIntent::SelectScope(CaptureScopeChoice::Global),
        );
        assert_eq!(model.scope(), &TaskScope::Global);
        assert_eq!(model.scope_choice(), CaptureScopeChoice::Global);
        assert!(!model.shows_project_path());
        let plain = render_plain(&model, 80, 14);
        assert!(plain.contains("(•) Desk"), "desk not marked: {plain}");
        assert!(!plain.contains("/repos/app"), "path leaked: {plain}");

        // This project: durable scope value is the resolved repository path.
        apply(
            &mut domain,
            &snap,
            &mut model,
            CaptureIntent::SelectScope(CaptureScopeChoice::ThisProject),
        );
        assert_eq!(
            model.scope(),
            &TaskScope::Project {
                path: "/repos/app".into(),
            }
        );
        assert_eq!(model.scope_choice(), CaptureScopeChoice::ThisProject);
        assert!(!model.shows_project_path());

        // Other: reveals and edits the full path.
        apply(
            &mut domain,
            &snap,
            &mut model,
            CaptureIntent::SelectScope(CaptureScopeChoice::Other),
        );
        assert!(model.is_path_editing());
        assert_eq!(model.scope_choice(), CaptureScopeChoice::Other);
        assert!(model.shows_project_path());
        let plain = render_plain(&model, 80, 14);
        assert!(plain.contains("(•) Other"), "other not marked: {plain}");
        assert!(
            plain.contains("/repos/app"),
            "path hidden while editing Other: {plain}"
        );

        for c in "-x".chars() {
            apply(&mut domain, &snap, &mut model, CaptureIntent::Insert(c));
        }
        apply(&mut domain, &snap, &mut model, CaptureIntent::Save);
        assert_eq!(
            model.scope(),
            &TaskScope::Project {
                path: "/repos/app-x".into(),
            }
        );
        // Confirmed Other stays disclosed: the path is no longer the current project.
        assert_eq!(model.scope_choice(), CaptureScopeChoice::Other);
        assert!(model.shows_project_path());
        let plain = render_plain(&model, 80, 14);
        assert!(
            plain.contains("/repos/app-x"),
            "confirmed Other path hidden: {plain}"
        );
    }

    #[test]
    fn mouse_chooses_each_available_scope_control() {
        let snap = project_snapshot("/repos/app");
        let model = CaptureModel::from_snapshot(&snap);
        let layout = capture_layout_for_model(Rect::new(0, 0, 80, 14), &model);
        assert!(layout.this_project_available);

        for choice in [
            CaptureScopeChoice::ThisProject,
            CaptureScopeChoice::Global,
            CaptureScopeChoice::Other,
        ] {
            let chip = layout
                .scope_chips
                .iter()
                .find(|chip| chip.value == choice)
                .unwrap_or_else(|| panic!("no chip for {choice:?}"));
            let column = chip.rect.x + chip.rect.width / 2;
            assert_eq!(
                map_capture_mouse(&layout, left_click(column, chip.rect.y)),
                Some(CaptureIntent::SelectScope(choice)),
                "click on {choice:?}"
            );
        }
    }

    #[test]
    fn this_project_unavailable_without_repository_does_not_synthesize_scope() {
        let snap = no_project_snapshot();
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);
        assert!(!model.this_project_available());

        apply(
            &mut domain,
            &snap,
            &mut model,
            CaptureIntent::SelectScope(CaptureScopeChoice::ThisProject),
        );
        assert_eq!(model.scope(), &TaskScope::Global);
        assert_eq!(model.scope_choice(), CaptureScopeChoice::Global);
        let message = model.message().expect("unavailable message");
        assert!(
            message.to_lowercase().contains("unavailable"),
            "unclear message: {message}"
        );

        // Non-color cue for the unavailable control, and no clickable route to it.
        let plain = render_plain(&model, 80, 14);
        assert!(
            plain.contains("(×) This project"),
            "missing unavailable marker: {plain}"
        );
        let layout = capture_layout_for_model(Rect::new(0, 0, 80, 14), &model);
        assert!(!layout.this_project_available);
        let chip = layout
            .scope_chips
            .iter()
            .find(|chip| chip.value == CaptureScopeChoice::ThisProject)
            .expect("this project geometry");
        assert_eq!(
            map_capture_mouse(&layout, left_click(chip.rect.x + 1, chip.rect.y)),
            None,
            "unavailable This project must not be clickable"
        );
    }

    #[test]
    fn other_scope_path_row_is_display_width_bounded() {
        let long = "/repos/very/long/workspace/path/that/never/fits/in/a/narrow/pane";
        let snap = project_snapshot(long);
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);
        apply(
            &mut domain,
            &snap,
            &mut model,
            CaptureIntent::SelectScope(CaptureScopeChoice::Other),
        );

        let width = 34u16;
        let rows = render_rows(&model, width, 12);
        let path_row = rows
            .iter()
            .find(|row| row.contains("Path"))
            .unwrap_or_else(|| panic!("no path row: {rows:?}"));
        assert!(
            path_row.contains('…'),
            "missing omission marker: {path_row}"
        );
        assert!(
            !path_row.contains("narrow/pane"),
            "unbounded path row: {path_row}"
        );
        for row in &rows {
            assert_eq!(
                row.chars().count(),
                width as usize,
                "row overflows allocated width: {row}"
            );
        }
    }

    #[test]
    fn prefill_title_from_snapshot_selection() {
        let snap = snapshot_with_selection("selected title");
        assert_eq!(snap.title_prefill.as_deref(), Some("selected title"));
        let model = CaptureModel::from_snapshot(&snap);
        assert_eq!(model.title(), "selected title");
        assert!(model.notes().is_empty());
        assert_eq!(model.focused(), CaptureField::Title);
    }

    #[test]
    fn save_with_empty_title_does_not_create_leaves_store_unchanged() {
        let dir = temp_dir("empty-title");
        fs::create_dir_all(&dir).expect("mkdir");
        let _guard = TempDirGuard(dir.clone());
        let store = TaskStore::new(&dir);
        store.save(&DomainState::new()).expect("seed empty");

        let snap = project_snapshot("/repos/app");
        let mut domain = store.load().expect("load");
        let mut model = CaptureModel::from_snapshot(&snap);
        // Whitespace-only title.
        model.title = seeded_draft("   \t  ");

        let outcome = apply_capture_intent(
            &mut domain,
            Some(&store),
            &snap,
            &mut model,
            CaptureIntent::Save,
        )
        .expect("apply");
        assert_eq!(outcome, CaptureOutcome::None);
        assert!(domain.tasks().is_empty());
        assert_eq!(model.message.as_deref(), Some(TITLE_REQUIRED_MESSAGE));

        let reloaded = store.load().expect("reload");
        assert!(reloaded.tasks().is_empty());
    }

    /// on the sibling surface: Capture is held to the same contract as the board.
    /// A save refused for an empty title keeps the whole draft, the focus, and the cursor, and
    /// says a title is required in the same words the board uses.
    #[test]
    fn empty_title_save_keeps_the_draft_focus_and_cursor_and_says_a_title_is_required() {
        let snap = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);
        model.title = seeded_draft("   \t ");
        model.notes = seeded_draft("notes worth keeping");
        // Leave the caret somewhere other than the end, and the focus somewhere other than
        // the field that will be refused, so "unchanged" is a real claim about both.
        model.title.move_line_start();
        model.focused = CaptureField::Notes;
        let cursor = model.title.cursor();

        let outcome =
            apply_capture_intent(&mut domain, None, &snap, &mut model, CaptureIntent::Save)
                .expect("a refused save is not an error");

        assert_eq!(outcome, CaptureOutcome::None);
        assert_eq!(model.title(), "   \t ");
        assert_eq!(model.title.cursor(), cursor, "the cursor must not jump");
        assert_eq!(model.notes(), "notes worth keeping");
        assert_eq!(
            model.focused(),
            CaptureField::Notes,
            "a refused save must not move the focus"
        );
        assert_eq!(model.message(), Some(TITLE_REQUIRED_MESSAGE));
        assert!(domain.tasks().is_empty(), "nothing may be created");
    }

    #[test]
    fn save_with_title_creates_task() {
        let snap = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);
        model.title = seeded_draft("Ship form");
        model.notes = seeded_draft("optional notes");

        let outcome =
            apply_capture_intent(&mut domain, None, &snap, &mut model, CaptureIntent::Save)
                .expect("save");
        let CaptureOutcome::Saved(id) = outcome else {
            panic!("expected Saved, got {outcome:?}");
        };
        let task = domain.get(id).expect("task");
        assert_eq!(task.title, "Ship form");
        assert_eq!(task.notes.as_deref(), Some("optional notes"));
        assert_eq!(task.status, HumanStatus::Open);
        assert_eq!(
            task.scope,
            TaskScope::Project {
                path: "/repos/app".into(),
            }
        );
        assert_eq!(domain.tasks().len(), 1);
    }

    #[test]
    fn save_without_notes_still_creates() {
        let snap = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);
        model.title = seeded_draft("No notes");

        let outcome =
            apply_capture_intent(&mut domain, None, &snap, &mut model, CaptureIntent::Save)
                .expect("save");
        let CaptureOutcome::Saved(id) = outcome else {
            panic!("expected Saved");
        };
        assert!(domain.get(id).expect("task").notes.is_none());
    }

    #[test]
    fn cancel_discards_draft() {
        let snap = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);
        model.title = seeded_draft("Would discard");
        model.notes = seeded_draft("draft notes");

        let outcome =
            apply_capture_intent(&mut domain, None, &snap, &mut model, CaptureIntent::Cancel)
                .expect("cancel");
        assert_eq!(outcome, CaptureOutcome::Cancelled);
        assert!(domain.tasks().is_empty());
    }

    #[test]
    fn cycle_scope_overrides_before_save() {
        let snap = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);
        assert!(matches!(model.scope(), TaskScope::Project { .. }));

        apply_capture_intent(
            &mut domain,
            None,
            &snap,
            &mut model,
            CaptureIntent::CycleScope,
        )
        .expect("cycle");
        assert_eq!(model.scope(), &TaskScope::Global);

        model.title = seeded_draft("Global task");
        let outcome =
            apply_capture_intent(&mut domain, None, &snap, &mut model, CaptureIntent::Save)
                .expect("save");
        let CaptureOutcome::Saved(id) = outcome else {
            panic!("expected Saved");
        };
        assert_eq!(domain.get(id).expect("task").scope, TaskScope::Global);
    }

    #[test]
    fn path_edit_sets_arbitrary_project_scope_before_save() {
        let snap = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);
        model.focused = CaptureField::Scope;

        apply_capture_intent(
            &mut domain,
            None,
            &snap,
            &mut model,
            CaptureIntent::BeginScopePathEdit,
        )
        .expect("begin path");
        assert!(model.is_path_editing());

        // Clear prefilled this_repo path and type a different project.
        while model.scope_path_edit().is_some_and(|p| !p.is_empty()) {
            apply_capture_intent(
                &mut domain,
                None,
                &snap,
                &mut model,
                CaptureIntent::Backspace,
            )
            .expect("backspace");
        }
        for c in "/repos/other-project".chars() {
            apply_capture_intent(
                &mut domain,
                None,
                &snap,
                &mut model,
                CaptureIntent::Insert(c),
            )
            .expect("insert");
        }
        // Enter confirms path (does not save yet).
        let outcome =
            apply_capture_intent(&mut domain, None, &snap, &mut model, CaptureIntent::Save)
                .expect("confirm path");
        assert_eq!(outcome, CaptureOutcome::None);
        assert!(!model.is_path_editing());
        assert_eq!(
            model.scope(),
            &TaskScope::Project {
                path: "/repos/other-project".into(),
            }
        );

        model.focused = CaptureField::Title;
        model.title = seeded_draft("Other project task");
        let outcome =
            apply_capture_intent(&mut domain, None, &snap, &mut model, CaptureIntent::Save)
                .expect("save");
        let CaptureOutcome::Saved(id) = outcome else {
            panic!("expected Saved, got {outcome:?}");
        };
        assert_eq!(
            domain.get(id).expect("task").scope,
            TaskScope::Project {
                path: "/repos/other-project".into(),
            }
        );
    }

    #[test]
    fn each_primary_capture_action_has_at_least_one_key_binding() {
        use crate::ui::input::primary_capture_action_sample_focus;
        for &action in PRIMARY_CAPTURE_ACTIONS {
            let key = primary_capture_action_sample_key(action);
            let focus = primary_capture_action_sample_focus(action);
            let intent = map_capture_key(focus, key)
                .unwrap_or_else(|| panic!("no intent for sample key of {action:?}"));
            assert_eq!(
                intent_primary_capture_action(&intent),
                Some(action),
                "sample key for {action:?} mapped to {intent:?}"
            );
        }
    }

    #[test]
    fn capture_binding_table_covers_save_cancel_edit_fields() {
        // capture subset: save, cancel, edit fields, change scope.
        let title = CaptureField::Title;
        let scope = CaptureField::Scope;
        let cases: &[(
            CaptureField,
            KeyCode,
            CaptureIntent,
            Option<PrimaryCaptureAction>,
        )] = &[
            (
                title,
                KeyCode::Enter,
                CaptureIntent::Save,
                Some(PrimaryCaptureAction::SaveCapture),
            ),
            (
                title,
                KeyCode::Esc,
                CaptureIntent::Cancel,
                Some(PrimaryCaptureAction::CancelCapture),
            ),
            (
                title,
                KeyCode::Char('x'),
                CaptureIntent::Insert('x'),
                Some(PrimaryCaptureAction::EditField),
            ),
            (
                title,
                KeyCode::Char('s'),
                CaptureIntent::Insert('s'),
                Some(PrimaryCaptureAction::EditField),
            ),
            (
                title,
                KeyCode::Backspace,
                CaptureIntent::Backspace,
                Some(PrimaryCaptureAction::EditField),
            ),
            (
                title,
                KeyCode::Tab,
                CaptureIntent::FocusNext,
                Some(PrimaryCaptureAction::EditField),
            ),
            (
                scope,
                KeyCode::Char('s'),
                CaptureIntent::CycleScope,
                Some(PrimaryCaptureAction::ChangeScope),
            ),
            (
                scope,
                KeyCode::Char(' '),
                CaptureIntent::CycleScope,
                Some(PrimaryCaptureAction::ChangeScope),
            ),
            (
                scope,
                KeyCode::Char('p'),
                CaptureIntent::BeginScopePathEdit,
                Some(PrimaryCaptureAction::ChangeScope),
            ),
            (
                scope,
                KeyCode::Char('e'),
                CaptureIntent::BeginScopePathEdit,
                Some(PrimaryCaptureAction::ChangeScope),
            ),
        ];
        for (focus, code, expected, primary) in cases {
            let intent = map_capture_key(*focus, press(*code)).expect("mapped");
            assert_eq!(&intent, expected);
            assert_eq!(intent_primary_capture_action(&intent), *primary);
        }
        assert_eq!(
            map_capture_key(
                title,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
            ),
            Some(CaptureIntent::Cancel)
        );
    }

    /// One number for the label columns: the renderer paints them and the hit-test skips
    /// them, so if the two ever disagree the cursor and the click regions land on
    /// different columns of the same row.
    #[test]
    fn the_rendered_label_width_is_the_hit_tested_label_width() {
        let snap = project_snapshot("/repos/app");
        let mut model = CaptureModel::from_snapshot(&snap);
        model.title = seeded_draft("VALUE");

        let (width, height) = (80u16, 16u16);
        let layout = capture_layout_for_model(Rect::new(0, 0, width, height), &model);
        let rows = render_rows(&model, width, height);

        // Render side: the column where the painted value actually begins.
        let title_row = &rows[layout.title_area.y as usize];
        let value_column = title_row
            .chars()
            .position(|c| c == 'V')
            .unwrap_or_else(|| panic!("no painted value on the title row: {title_row:?}"))
            as u16;
        let rendered_label_width = value_column - layout.title_area.x;

        // Hit-test side: where mouse geometry expects a row's value/controls to begin.
        let first_chip = layout.scope_chips.first().expect("a scope chip");
        let hit_tested_label_width = first_chip.rect.x - layout.scope_area.x;

        assert_eq!(
            rendered_label_width, hit_tested_label_width,
            "render label width {rendered_label_width} vs hit-test {hit_tested_label_width}"
        );
        assert_eq!(rendered_label_width, CAPTURE_FIELD_LABEL_WIDTH);
    }

    /// Enter opens a line in Notes, so the Notes legend has to name the save chord;
    /// Title and Scope keep the legend that says Enter saves.
    #[test]
    fn the_notes_focused_legend_names_the_save_chord_and_the_newline_key() {
        let snap = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);

        apply(&mut domain, &snap, &mut model, CaptureIntent::FocusNext);
        assert_eq!(model.focused(), CaptureField::Notes);
        let plain = render_plain(&model, 100, 16);
        assert!(plain.contains("ctrl+enter / alt+enter save"), "{plain}");
        assert!(plain.contains("enter newline"), "{plain}");

        // Title is one line: Enter still saves there.
        apply(&mut domain, &snap, &mut model, CaptureIntent::FocusPrev);
        assert_eq!(model.focused(), CaptureField::Title);
        let plain = render_plain(&model, 100, 16);
        assert!(plain.contains("enter save"), "{plain}");
        assert!(!plain.contains("enter newline"), "{plain}");

        // The scope row is unchanged by this task, and so is its legend.
        model.focused = CaptureField::Scope;
        assert_eq!(model.help_line(), CAPTURE_HELP_LINE);
    }

    /// A rejected save leaves a message the user still has to read: moving the cursor
    /// changes nothing about the draft, so it must not wipe it. Editing the value does.
    #[test]
    fn cursor_movement_keeps_the_form_message_and_editing_clears_it() {
        let snap = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);
        model.title = seeded_draft("  ");

        apply(&mut domain, &snap, &mut model, CaptureIntent::Save);
        assert_eq!(model.message(), Some(TITLE_REQUIRED_MESSAGE));

        for movement in [
            CaptureIntent::MoveLeft,
            CaptureIntent::MoveRight,
            CaptureIntent::MoveLineStart,
            CaptureIntent::MoveLineEnd,
            CaptureIntent::MoveWordLeft,
            CaptureIntent::MoveWordRight,
        ] {
            apply(&mut domain, &snap, &mut model, movement.clone());
            assert_eq!(
                model.message(),
                Some(TITLE_REQUIRED_MESSAGE),
                "{movement:?} wiped the form message without changing the draft"
            );
        }

        apply(&mut domain, &snap, &mut model, CaptureIntent::Insert('a'));
        assert_eq!(model.message(), None, "typing left a stale message");
    }

    /// Every editing intent is inert until a failed save is retried or cancelled, so the
    /// form must not blink a cursor in a field that accepts nothing.
    #[test]
    fn no_terminal_cursor_is_placed_while_a_save_failure_is_pending() {
        let dir = temp_dir("recovery-cursor");
        fs::create_dir_all(&dir).expect("mkdir");
        let _guard = TempDirGuard(dir.clone());
        // A plain file where the state directory should be: every save attempt fails.
        let blocked = dir.join("not-a-directory");
        fs::write(&blocked, "not a state directory").expect("blocking file");
        let store = TaskStore::new(&blocked);

        let snap = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);
        model.title = seeded_draft("Pending");

        let outcome = apply_capture_intent(
            &mut domain,
            Some(&store),
            &snap,
            &mut model,
            CaptureIntent::Save,
        )
        .expect("a save failure stays on the form");
        assert_eq!(outcome, CaptureOutcome::None);
        assert!(model.is_save_recovery());
        assert_eq!(model.focused(), CaptureField::Title);

        let backend = TestBackend::new(80, 16);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| draw_capture(frame, &model)).unwrap();
        assert!(
            !terminal.backend().cursor_visible(),
            "cursor offered in a field that accepts nothing during save recovery"
        );
    }

    #[test]
    fn help_lines_use_the_shared_lowercase_verb_grammar() {
        assert_eq!(
            CAPTURE_HELP_LINE,
            "tab fields · 1–3 scope · 3 other path · enter save · esc cancel"
        );
        assert_eq!(
            CAPTURE_NOTES_HELP_LINE,
            "tab fields · ctrl+enter / alt+enter save · enter newline · esc cancel"
        );
        assert_eq!(CAPTURE_SAVE_RECOVERY_HELP_LINE, "r retry · c cancel");
    }

    #[test]
    fn field_edit_intents_mutate_focused_buffer() {
        let snap = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);

        apply_capture_intent(
            &mut domain,
            None,
            &snap,
            &mut model,
            CaptureIntent::Insert('H'),
        )
        .unwrap();
        apply_capture_intent(
            &mut domain,
            None,
            &snap,
            &mut model,
            CaptureIntent::Insert('i'),
        )
        .unwrap();
        assert_eq!(model.title(), "Hi");

        apply_capture_intent(
            &mut domain,
            None,
            &snap,
            &mut model,
            CaptureIntent::FocusNext,
        )
        .unwrap();
        assert_eq!(model.focused(), CaptureField::Notes);
        apply_capture_intent(
            &mut domain,
            None,
            &snap,
            &mut model,
            CaptureIntent::Insert('n'),
        )
        .unwrap();
        assert_eq!(model.notes(), "n");

        apply_capture_intent(
            &mut domain,
            None,
            &snap,
            &mut model,
            CaptureIntent::Backspace,
        )
        .unwrap();
        assert!(model.notes().is_empty());
    }

    /// typing lands at the cursor, not at the end of the value, in both text fields.
    #[test]
    fn typing_inserts_at_the_cursor_in_each_text_field() {
        let snap = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);

        for c in "abc".chars() {
            apply(&mut domain, &snap, &mut model, CaptureIntent::Insert(c));
        }
        apply(&mut domain, &snap, &mut model, CaptureIntent::MoveLeft);
        apply(&mut domain, &snap, &mut model, CaptureIntent::Insert('X'));
        assert_eq!(model.title(), "abXc");

        apply(&mut domain, &snap, &mut model, CaptureIntent::FocusNext);
        assert_eq!(model.focused(), CaptureField::Notes);
        for c in "one two".chars() {
            apply(&mut domain, &snap, &mut model, CaptureIntent::Insert(c));
        }
        apply(&mut domain, &snap, &mut model, CaptureIntent::MoveWordLeft);
        apply(&mut domain, &snap, &mut model, CaptureIntent::Insert('!'));
        assert_eq!(model.notes(), "one !two");
    }

    /// each field owns its cursor, so leaving a field and coming back resumes where
    /// the cursor was rather than jumping to the end of the value.
    #[test]
    fn switching_focus_between_fields_preserves_each_fields_own_cursor() {
        let snap = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);

        for c in "title".chars() {
            apply(&mut domain, &snap, &mut model, CaptureIntent::Insert(c));
        }
        // Park the Title cursor between "ti" and "tle".
        apply(&mut domain, &snap, &mut model, CaptureIntent::MoveLineStart);
        apply(&mut domain, &snap, &mut model, CaptureIntent::MoveRight);
        apply(&mut domain, &snap, &mut model, CaptureIntent::MoveRight);

        apply(&mut domain, &snap, &mut model, CaptureIntent::FocusNext);
        for c in "notes".chars() {
            apply(&mut domain, &snap, &mut model, CaptureIntent::Insert(c));
        }
        // Park the Notes cursor at its start; this must not disturb Title's own cursor.
        apply(&mut domain, &snap, &mut model, CaptureIntent::MoveLineStart);

        // The complete ring returns to Title after Thread and Scope.
        apply(&mut domain, &snap, &mut model, CaptureIntent::FocusNext);
        apply(&mut domain, &snap, &mut model, CaptureIntent::FocusNext);
        apply(&mut domain, &snap, &mut model, CaptureIntent::FocusNext);
        assert_eq!(model.focused(), CaptureField::Title);
        apply(&mut domain, &snap, &mut model, CaptureIntent::Insert('#'));
        assert_eq!(model.title(), "ti#tle", "Title lost its own cursor");

        apply(&mut domain, &snap, &mut model, CaptureIntent::FocusNext);
        assert_eq!(model.focused(), CaptureField::Notes);
        apply(&mut domain, &snap, &mut model, CaptureIntent::Insert('#'));
        assert_eq!(model.notes(), "#notes", "Notes lost its own cursor");
    }

    /// Notes keeps a pasted line break; Title is one line, so each break
    /// becomes a space rather than gluing the words on either side of it together.
    #[test]
    fn a_pasted_line_break_is_kept_in_notes_and_flattened_in_the_title() {
        let snap = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);

        apply(
            &mut domain,
            &snap,
            &mut model,
            CaptureIntent::InsertText("first\r\nsecond\nthird".into()),
        );
        assert_eq!(model.title(), "first second third");

        apply(&mut domain, &snap, &mut model, CaptureIntent::FocusNext);
        apply(
            &mut domain,
            &snap,
            &mut model,
            CaptureIntent::InsertText("first\nsecond".into()),
        );
        assert_eq!(model.notes(), "first\nsecond");
    }

    /// Enter opens a line in Notes rather than saving.
    #[test]
    fn a_line_break_intent_splits_the_notes_draft_at_the_cursor() {
        let snap = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);
        apply(&mut domain, &snap, &mut model, CaptureIntent::FocusNext);

        for c in "ab".chars() {
            apply(&mut domain, &snap, &mut model, CaptureIntent::Insert(c));
        }
        apply(
            &mut domain,
            &snap,
            &mut model,
            CaptureIntent::InsertLineBreak,
        );
        apply(&mut domain, &snap, &mut model, CaptureIntent::Insert('c'));
        assert_eq!(model.notes(), "ab\nc");
    }

    /// forward delete removes the character at the cursor and leaves it in place.
    #[test]
    fn forward_delete_removes_the_character_at_the_cursor() {
        let snap = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);
        for c in "abc".chars() {
            apply(&mut domain, &snap, &mut model, CaptureIntent::Insert(c));
        }
        apply(&mut domain, &snap, &mut model, CaptureIntent::MoveLineStart);
        apply(&mut domain, &snap, &mut model, CaptureIntent::DeleteForward);
        assert_eq!(model.title(), "bc");
        apply(&mut domain, &snap, &mut model, CaptureIntent::Insert('Z'));
        assert_eq!(model.title(), "Zbc");
    }

    /// a Title draft wider than its row paints a window that follows the cursor,
    /// with the terminal's own cursor inside the row's value region.
    #[test]
    fn a_title_wider_than_its_row_paints_a_window_holding_the_cursor() {
        let snap = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);
        for c in format!("HEAD{}TAIL", "-".repeat(200)).chars() {
            apply(&mut domain, &snap, &mut model, CaptureIntent::Insert(c));
        }

        let (width, height) = (60u16, 14u16);
        let layout = capture_layout_for_model(Rect::new(0, 0, width, height), &model);
        let title_area = layout.title_area;
        assert!(title_area.width > 0, "no title row to paint into");

        let render = |model: &CaptureModel| {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).expect("terminal");
            terminal.draw(|frame| draw_capture(frame, model)).unwrap();
            let buffer = terminal.backend().buffer().clone();
            let row: String = (title_area.x..title_area.x + title_area.width)
                .map(|x| buffer.cell((x, title_area.y)).expect("cell").symbol())
                .collect();
            assert!(
                terminal.backend().cursor_visible(),
                "a focused field must show the terminal cursor"
            );
            (row, terminal.backend().cursor_position())
        };
        // The value region is the row minus its label columns.
        let region = title_area.x + CAPTURE_FIELD_LABEL_WIDTH..title_area.x + title_area.width;

        // The cursor sits at the draft's end, so the window shows the tail.
        let (tail_row, tail_cursor) = render(&model);
        assert!(tail_row.contains("TAIL"), "{tail_row:?}");
        assert!(!tail_row.contains("HEAD"), "{tail_row:?}");
        assert_eq!(tail_cursor.y, title_area.y);
        assert!(
            region.contains(&tail_cursor.x),
            "cursor {tail_cursor:?} outside the value region for {tail_row:?}"
        );

        // Moving to the line start scrolls the window back to the head.
        apply(&mut domain, &snap, &mut model, CaptureIntent::MoveLineStart);
        let (head_row, head_cursor) = render(&model);
        assert!(head_row.contains("HEAD"), "{head_row:?}");
        assert!(!head_row.contains("TAIL"), "{head_row:?}");
        assert_eq!(
            head_cursor.x,
            title_area.x + CAPTURE_FIELD_LABEL_WIDTH,
            "cursor {head_cursor:?} not at the first value column of {head_row:?}"
        );
    }

    /// Every painted row of the Notes field, plus the terminal cursor.
    fn rendered_notes(
        model: &CaptureModel,
        area: ratatui::layout::Rect,
        notes_area: ratatui::layout::Rect,
    ) -> (Vec<String>, ratatui::layout::Position) {
        let backend = TestBackend::new(area.width, area.height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| draw_capture(frame, model)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let rows = (notes_area.y..notes_area.y + notes_area.height)
            .map(|y| {
                (notes_area.x..notes_area.x + notes_area.width)
                    .map(|x| buffer.cell((x, y)).expect("cell").symbol())
                    .collect::<String>()
            })
            .collect();
        (rows, terminal.backend().cursor_position())
    }

    /// the Capture Notes field is several rows tall, so a three-line note is
    /// presented as three rendered lines, focused and unfocused, with no `\u{000a}` escape
    /// anywhere and no line joined onto another. Focused, the terminal cursor lands on the
    /// row and column of the line the logical cursor is on.
    #[test]
    fn the_capture_notes_field_shows_a_multiline_draft_as_separate_rows() {
        let snap = project_snapshot("/repos/app");
        let mut model = CaptureModel::from_snapshot(&snap);
        model.notes = seeded_draft("first\nsecond\nthird");

        let area = Rect::new(0, 0, 60, 14);
        let layout = capture_layout_for_model(area, &model);
        let notes_area = layout.notes_area;
        assert!(
            notes_area.height >= 3,
            "the Notes field has no room to present three lines: {notes_area:?}"
        );

        // Title has focus at first, so the Notes field takes the unfocused path.
        assert_eq!(model.focused(), CaptureField::Title);
        let (unfocused, _) = rendered_notes(&model, area, notes_area);
        for (row, expected) in unfocused.iter().zip(["first", "second", "third"]) {
            assert!(row.contains(expected), "missing {expected:?} in {row:?}");
            assert!(
                !row.contains("\\u{000a}"),
                "an escaped break reached the Notes field: {row:?}"
            );
            assert!(
                !row.contains('…'),
                "a fitting note claimed overflow: {row:?}"
            );
        }
        assert!(
            !unfocused[0].contains("second"),
            "two stored lines share one rendered row: {:?}",
            unfocused[0]
        );

        // Focused, the same three lines are visible and the cursor sits at the end of the
        // last one, which is where the seeded draft parks it.
        model.focused = CaptureField::Notes;
        let (focused, cursor) = rendered_notes(&model, area, notes_area);
        for (row, expected) in focused.iter().zip(["first", "second", "third"]) {
            assert!(row.contains(expected), "missing {expected:?} in {row:?}");
            assert!(
                !row.contains("\\u{000a}"),
                "an escaped break reached the focused Notes field: {row:?}"
            );
        }
        assert_eq!(
            cursor.y,
            notes_area.y + 2,
            "the cursor missed the row of its own line in {focused:?}"
        );
        assert_eq!(
            cursor.x,
            notes_area.x + CAPTURE_FIELD_LABEL_WIDTH + 5,
            "the cursor missed the end of its own line in {focused:?}"
        );
    }

    /// a note with more lines than the field has rows marks the omission visibly on
    /// its last row when unfocused, and scrolls to keep the cursor visible when focused.
    #[test]
    fn a_capture_note_taller_than_its_field_marks_the_overflow_and_keeps_the_cursor_visible() {
        let snap = project_snapshot("/repos/app");
        let mut model = CaptureModel::from_snapshot(&snap);
        model.notes = seeded_draft("one\ntwo\nthree\nfour\nfive");

        let area = Rect::new(0, 0, 60, 14);
        let layout = capture_layout_for_model(area, &model);
        let notes_area = layout.notes_area;
        let rows = notes_area.height as usize;
        assert!(
            (2..5).contains(&rows),
            "field height {rows} cannot show this case"
        );

        let (unfocused, _) = rendered_notes(&model, area, notes_area);
        assert!(
            unfocused[rows - 1].contains("2 more"),
            "the omitted lines were dropped silently: {unfocused:?}"
        );
        assert!(
            !unfocused.iter().any(|row| row.contains("five")),
            "an omitted line was reflowed into view: {unfocused:?}"
        );

        // Focused with the cursor parked at the end, the field scrolls to the last line.
        model.focused = CaptureField::Notes;
        let (focused, cursor) = rendered_notes(&model, area, notes_area);
        assert!(
            focused[rows - 1].contains("five"),
            "the cursor's line is not visible: {focused:?}"
        );
        assert_eq!(cursor.y, notes_area.y + notes_area.height - 1);
        assert_eq!(cursor.x, notes_area.x + CAPTURE_FIELD_LABEL_WIDTH + 4);
    }

    /// Up and Down walk the WRAPPED rows of the Notes draft: over logical lines
    /// (blank lines included) once the renderer has recorded the field width.
    #[test]
    fn notes_arrows_walk_wrapped_rows_once_the_paint_width_is_known() {
        let snap = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);
        model.focused = CaptureField::Notes;
        model.notes = EditBuffer::new("aa\n\nbb", 0);
        // No frame painted yet: the arrows stay inert rather than guessing a width.
        apply(&mut domain, &snap, &mut model, CaptureIntent::MoveDown);
        assert_eq!(model.notes.cursor(), 0);

        // Paint one frame at 60x14 so the model records the field's width, then
        // move: Down crosses onto the blank middle row, Down again onto "bb".
        let mut terminal = Terminal::new(TestBackend::new(60, 14)).expect("terminal");
        terminal
            .draw(|frame| draw_capture(frame, &model))
            .expect("draw");
        assert!(model.notes_width.get() > 0, "the frame recorded the width");

        apply(&mut domain, &snap, &mut model, CaptureIntent::MoveDown);
        assert_eq!(
            model.notes.cursor(),
            3,
            "Down lands on the blank middle row"
        );
        apply(&mut domain, &snap, &mut model, CaptureIntent::MoveDown);
        assert_eq!(model.notes.cursor(), 4, "Down lands on the last line");
        apply(&mut domain, &snap, &mut model, CaptureIntent::MoveUp);
        assert_eq!(model.notes.cursor(), 3, "Up returns to the blank row");
        // Title keeps its single-line map: the intents are Notes-only there.
        model.focused = CaptureField::Title;
        apply(&mut domain, &snap, &mut model, CaptureIntent::MoveDown);
        assert_eq!(model.title.cursor(), model.title.char_count());
    }

    /// The taller Notes field only claims rows the form does not otherwise need: at every
    /// terminal size the form already supported, Scope, the buttons and the help row stay
    /// visible and nothing overlaps the Notes region.
    #[test]
    fn the_notes_field_keeps_every_capture_row_visible_at_the_smallest_supported_size() {
        let snap = project_snapshot("/repos/app");
        let mut model = CaptureModel::from_snapshot(&snap);
        model.notes = seeded_draft("first\nsecond\nthird");

        for (width, height) in [(50u16, 14u16), (60, 10), (80, 16), (120, 10)] {
            let layout = capture_layout_for_model(Rect::new(0, 0, width, height), &model);
            let notes_area = layout.notes_area;
            assert!(
                notes_area.height >= 1,
                "the Notes field vanished at {width}x{height}"
            );
            assert_eq!(
                layout.thread_area.y,
                notes_area.y + notes_area.height,
                "Notes and Thread disagree about the row after the field at {width}x{height}"
            );
            assert_eq!(
                layout.scope_area.y,
                layout.thread_area.y + layout.thread_area.height,
                "Thread and Scope disagree about their shared layout at {width}x{height}"
            );
            assert!(
                layout.help_area.y > layout.save_chip.rect.y && layout.help_area.y < height,
                "the help row left the form at {width}x{height}"
            );

            let rows = render_rows(&model, width, height);
            let plain = rows.join("\n");
            for token in ["Title:", "Notes:", "Thread:", "Scope:", "Save", "Cancel"] {
                assert!(
                    plain.contains(token),
                    "missing {token:?} at {width}x{height}: {plain}"
                );
            }
            assert!(
                !rows[layout.help_area.y as usize].trim().is_empty(),
                "the help row painted nothing at {width}x{height}"
            );
        }
    }

    #[test]
    fn capture_render_visibly_encodes_untrusted_control_sequences() {
        let snap = project_snapshot("/repos/app");
        let mut model = CaptureModel::from_snapshot(&snap);
        model.title = seeded_draft("title\u{1b}]52;clipboard\u{7}");
        model.notes = seeded_draft("notes\u{1b}[2J");
        model.scope = TaskScope::Project {
            path: "scope\u{1b}]8;;https://example.test\u{7}".into(),
        };
        model.message = Some("recovery\u{1b}[H".into());

        let backend = TestBackend::new(120, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw_capture(frame, &model)).unwrap();
        let plain: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();

        for expected in [
            "title\\u{001b}]52;clipboard\\u{0007}",
            "notes\\u{001b}[2J",
            "scope\\u{001b}]8;;https://example.test\\u{0007}",
            "recovery\\u{001b}[H",
        ] {
            assert!(
                plain.contains(expected),
                "missing encoded text {expected:?}: {plain}"
            );
        }
        assert!(
            !plain.contains('\u{1b}'),
            "raw escape reached terminal: {plain}"
        );
    }

    #[test]
    fn render_buffer_contains_title_fields_and_help() {
        let snap = snapshot_with_selection("prefill me");
        let model = CaptureModel::from_snapshot(&snap);
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw_capture(frame, &model)).unwrap();
        let plain: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(plain.contains(CAPTURE_TITLE), "missing title: {plain}");
        assert!(plain.contains("prefill me"), "missing prefill: {plain}");
        assert!(plain.contains("Title:"), "missing title field: {plain}");
        assert!(plain.contains("Notes:"), "missing notes field: {plain}");
        assert!(plain.contains("Scope:"), "missing scope field: {plain}");
        assert!(
            plain.contains("Enter save")
                || plain.contains("save")
                || plain.contains("Save")
                || plain.contains("Esc"),
            "missing help: {plain}"
        );
    }

    #[test]
    fn capture_form_accepts_optional_thread_with_shared_validation() {
        let snapshot = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snapshot);
        apply(
            &mut domain,
            &snapshot,
            &mut model,
            CaptureIntent::InsertText("Threaded capture".into()),
        );
        model.focused = CaptureField::Thread;
        apply(
            &mut domain,
            &snapshot,
            &mut model,
            CaptureIntent::InsertText("Release-2026".into()),
        );
        let CaptureOutcome::Saved(id) =
            apply(&mut domain, &snapshot, &mut model, CaptureIntent::Save)
        else {
            panic!("threaded capture did not save");
        };
        assert_eq!(
            domain.get(id).expect("task").thread.as_deref(),
            Some("release-2026")
        );
    }

    #[test]
    fn capture_form_refuses_invalid_thread_without_persisting() {
        let snapshot = project_snapshot("/repos/app");
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snapshot);
        apply(
            &mut domain,
            &snapshot,
            &mut model,
            CaptureIntent::InsertText("Invalid threaded capture".into()),
        );
        model.focused = CaptureField::Thread;
        apply(
            &mut domain,
            &snapshot,
            &mut model,
            CaptureIntent::InsertText("bad_name".into()),
        );
        assert_eq!(
            apply(&mut domain, &snapshot, &mut model, CaptureIntent::Save),
            CaptureOutcome::None
        );
        assert!(domain.tasks().is_empty());
        let refusal = model.thread_refusal.as_deref().expect("thread refusal");
        assert!(refusal.contains("hyphens"), "{refusal}");
        assert!(render_plain(&model, 80, 16).contains("hyphens"));
    }

    #[cfg(unix)]
    #[test]
    fn capture_scope_never_offers_an_archived_this_project() {
        // The archived record names the project through a symlink while the invocation
        // resolved the real directory; both must count as the same project. Built here so
        // the test holds on Linux too (it used to lean on macOS's `/tmp` → `/private/tmp`).
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let unique = format!(
            "tsk-capture-archived-{}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );
        let root = std::env::temp_dir().join(unique);
        let real = root.join("real");
        let link = root.join("link");
        std::fs::create_dir_all(&real).expect("real dir");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        let real = std::fs::canonicalize(&real).expect("canonical real");
        let real_str = real.to_string_lossy().into_owned();
        let link_str = link.to_string_lossy().into_owned();

        let snapshot = InvocationSnapshot {
            default_scope: TaskScope::Project {
                path: real_str.clone(),
            },
            this_repo: Some(real.clone()),
            title_prefill: None,
            provenance: ProvenanceOrigin::Capture,
        };
        let mut model = CaptureModel::from_snapshot(&snapshot);
        assert!(model.this_project_available());

        let mut archived = std::collections::BTreeSet::new();
        archived.insert(link_str);
        model.mark_archived_projects(&archived);

        assert!(
            !model.this_project_available(),
            "an archived invocation repo is not on offer"
        );
        assert_eq!(
            model.scope(),
            &TaskScope::Global,
            "a scope pointing at it falls back to the desk"
        );
        model.select_scope(CaptureScopeChoice::ThisProject);
        assert_eq!(
            model.scope(),
            &TaskScope::Global,
            "choosing This project cannot select the archived repo"
        );
        assert_eq!(
            model.message(),
            Some(CAPTURE_THIS_PROJECT_UNAVAILABLE),
            "and it says so"
        );
        model.cycle_scope();
        assert_eq!(
            model.scope(),
            &TaskScope::Global,
            "cycling never lands on the archived repo either"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
