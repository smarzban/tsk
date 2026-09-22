//! Board intent reducer.

use std::path::PathBuf;
use std::time::Instant;

use uuid::Uuid;

use crate::context::InvocationSnapshot;
use crate::domain::{
    normalize_thread, thread_refusal_message, DomainError, DomainState, HumanStatus, TaskScope,
    ThreadError,
};
use crate::scope::paths_equivalent;
use crate::ui::capture::{CaptureField, TITLE_REQUIRED_MESSAGE};
use crate::ui::edit::{flatten_line_breaks, EditBuffer};
use crate::ui::input::{BoardIntent, MarkDirection};
use crate::ui::mouse::BoardPopup;
use crate::ui::queue::{NavTab, ARCHIVED_HEADER_ROW_ID, INBOX_HEADER_ROW_ID};
use crate::ui::tier::{FocusedSurface, WideStage};

use super::commands::{resolve_board_command, CommandSurface};
use super::model::{
    BoardForm, BoardInputMode, BoardLocation, BoardModel, IntentOutcome, ListPickerValue,
    PickerTab, ProjectPickerState, ProjectScopeOption, ProjectsView, StepEditor, StepEditorSave,
    TaskEditSave, DIRTY_TASK_SWITCH_REFUSAL,
};

/// What the row says when an action that aims at the selection is asked for on a board that
/// has none. One wording, so the same refusal always reads the same way.
const NO_SELECTION: &str = "select a task first";

fn take_verb_targets(model: &mut BoardModel) -> (Vec<Uuid>, bool) {
    let bulk = model.task_list_owns_input() && model.mark_mode_active() && model.marked_count() > 0;
    let targets = model.verb_target_ids();
    model.clear_marks();
    (targets, bulk)
}

fn set_status_batch(
    domain: &mut DomainState,
    targets: &[Uuid],
    status: HumanStatus,
) -> Result<bool, DomainError> {
    let baseline = domain.clone();
    let mut changed = false;
    for &id in targets {
        if domain.get(id).is_some_and(|task| task.status == status) {
            continue;
        }
        if let Err(error) = domain.set_status(id, status) {
            *domain = baseline;
            return Err(error);
        }
        changed = true;
    }
    Ok(changed)
}

fn reopen_batch(domain: &mut DomainState, targets: &[Uuid]) -> Result<bool, DomainError> {
    let baseline = domain.clone();
    let mut changed = false;
    for &id in targets {
        let Some(status) = domain.get(id).map(|task| task.status) else {
            *domain = baseline;
            return Err(DomainError::UnknownId(id));
        };
        let result = match status {
            HumanStatus::Open => continue,
            HumanStatus::Done => domain.reopen(id),
            _ => domain.set_status(id, HumanStatus::Open),
        };
        if let Err(error) = result {
            *domain = baseline;
            return Err(error);
        }
        changed = true;
    }
    Ok(changed)
}

fn file_batch(domain: &mut DomainState, targets: &[Uuid]) -> Result<bool, DomainError> {
    let baseline = domain.clone();
    let mut changed = false;
    for &id in targets {
        let Some(archived) = domain.get(id).map(|task| task.archived) else {
            *domain = baseline;
            return Err(DomainError::UnknownId(id));
        };
        let result = if archived {
            domain.unarchive_task(id)
        } else {
            domain.archive_task(id)
        };
        match result {
            Ok(task_changed) => changed |= task_changed,
            Err(error) => {
                *domain = baseline;
                return Err(error);
            }
        }
    }
    Ok(changed)
}

/// What the row says when an Undo is refused because its target moved on.
///
/// The words are the domain's, taken from [`DomainError::StaleUndo`] rather than restated
/// here, so this cannot drift from the refusal it presents. Only the *order* is this
/// layer's: the domain leads with the identifier, which is right for a log line or an error
/// chain, but the board has one chrome row that clips, and a 36-character uuid in front
/// spends the whole budget on the part the reader cannot act on. Reading the same words
/// reason-first means a clipped row loses the uuid instead of the meaning.
///
/// If the domain ever stops leading with the identifier, there is nothing to move and its
/// line is shown exactly as stated.
fn stale_undo_message(id: Uuid) -> String {
    let stated = DomainError::StaleUndo(id).to_string();
    let identifier = format!("task {id}");
    match stated.strip_prefix(&identifier).map(str::trim_start) {
        Some(reason) if !reason.is_empty() => format!("{reason} · {identifier}"),
        _ => stated,
    }
}

/// A **mutating board action**: an intent this reducer may report as persisting domain
/// state, and so the one predicate every rule about mutating actions reads.
///
/// The board loop uses it to decide whether a save baseline has to be loaded before the
/// intent is applied, and [`apply_intent`] uses it to time the delete recovery notice
///. Both read this list rather than keeping one of their own, so the pinned
/// definition and the classification cannot drift apart.
pub fn board_intent_may_persist(model: &BoardModel, intent: &BoardIntent) -> bool {
    let dropdown_assignment = matches!(
        intent,
        BoardIntent::ConfirmFormDropdown | BoardIntent::SelectFormDropdownOption(_)
    ) && model.input_mode == BoardInputMode::FormDropdown
        && model
            .form
            .as_ref()
            .is_some_and(|form| form.focus == CaptureField::Assignee)
        && model.pending_assignee_targets.is_some();
    dropdown_assignment
        || matches!(
            intent,
            BoardIntent::ConfirmEdit
                | BoardIntent::ConfirmEditNext
                | BoardIntent::ConfirmFormAssignee
                | BoardIntent::SetStatus(_)
                | BoardIntent::Complete
                | BoardIntent::Reopen
                | BoardIntent::SoftDelete
                | BoardIntent::Undo
                | BoardIntent::File
                | BoardIntent::LaunchUnarchive
                | BoardIntent::PrimaryVerb
                | BoardIntent::Dispatch
                | BoardIntent::DispatchAgain
                | BoardIntent::ConfirmCleanup
                | BoardIntent::KeepCleanup
                | BoardIntent::ToggleBlock
                | BoardIntent::ToggleReview
                | BoardIntent::ToggleStep
                | BoardIntent::QuickAddSave
                | BoardIntent::QuickAddSaveNext
        )
}

/// The chrome row's lifetime rule, in one place.
///
/// The row carries two different things with two different lifetimes: the **message**, which
/// is feedback about the action the user just took, and the **delete recovery notice**, which
/// outlives its action on purpose so the way back stays on screen. Every event:
///
/// - **an edit opens** — the message is cleared (a refusal left by an earlier action would
///   read as a refusal of an edit nobody has attempted yet); the notice is kept, because
///   opening an edit is not a mutating action and says the notice survives one.
///   Applied by the two `BeginEdit…` arms, which know whether a field actually opened.
/// - **an edit is cancelled** — the message is cleared, for the same reason in reverse: the
///   refusal explained a field that is now gone; the notice is kept. Applied by the
///   `CancelEdit` arm.
/// - **a mutating board action** — *both* are cleared, and only here. The notice because
///   says so; the message because it was feedback about an earlier action, and a
///   message left standing would be read as this action's answer. Whatever the action then
///   reports (a confirmation, a refusal, the stale-undo message) owns the row on its own,
///   because this clearing happens before the intent is applied. The exception is an action
///   that **refused**: clears the notice for a change, and a refusal is not one, so
///   [`apply_intent`] puts the notice back whenever the intent it ran reported nothing to
///   persist: a refused Undo or Done keeps the way back it never spent. That is one rule at one
///   place rather than a per-arm exception, and it reads off the same outcome the caller
///   persists on.
/// - **any other action** — both are left exactly as they are.
///
/// Two events outside this reducer touch the notice. A failed save
/// ([`BoardModel::begin_save_recovery`]) *suspends* it, because a deletion that did not
/// reach disk has nothing to undo, and a successful Retry
/// ([`BoardModel::end_save_recovery`]) puts it back. An open command surface *covers* the
/// notice for as long as it is open
/// ([`BoardModel::visible_delete_notice`]): the notice is not taken down, but for that
/// duration it is genuinely not painted, which is the cost of the row never advertising a
/// click the modal surface would swallow.
///
/// When both channels are set the row carries **both**, notice first, message second, legend
/// last (see [`fit_chrome_row`]). Nothing on this row wins by taking another thing off it.
fn apply_chrome_row_lifetime(model: &mut BoardModel, intent: &BoardIntent) {
    if board_intent_may_persist(model, intent) {
        model.clear_delete_notice();
        model.clear_message();
    }
}

/// Apply a board intent to domain + model.
///
/// Mutating intents call Task Domain only. Caller persists with Task Store when outcome is
/// [`IntentOutcome::Persist`]. The capture snapshot is retained by the form.
/// Intents the read-only archived focus refuses (AC-42): everything that would mutate a
/// task or open a capture/edit surface. `Undo` is excluded: it is the unarchive route.
fn read_only_focus_refuses(model: &BoardModel, intent: &BoardIntent) -> bool {
    if matches!(intent, BoardIntent::Undo) {
        return false;
    }
    if board_intent_may_persist(model, intent) {
        return true;
    }
    matches!(
        intent,
        BoardIntent::OpenCapture
            | BoardIntent::ExpandQuickAdd
            | BoardIntent::BeginEditTitle
            | BoardIntent::BeginEditNotes
            | BoardIntent::BeginEditScope
            | BoardIntent::BeginEditAssignee
            | BoardIntent::BeginAddStep
            | BoardIntent::ToggleThreadEditing
            | BoardIntent::FormCycleScope
            // AC-44: Tab, shift+Tab, a field click, and the step cursor are edit
            // entries on the task page, so the read-only page refuses them too.
            | BoardIntent::FormFocusNext
            | BoardIntent::FormFocusPrev
            | BoardIntent::FocusFormField(_)
            | BoardIntent::FocusFormCursor(_, _, _)
            | BoardIntent::SelectStep(_)
    )
}

pub fn apply_intent(
    domain: &mut DomainState,
    model: &mut BoardModel,
    intent: BoardIntent,
    snapshot: Option<&InvocationSnapshot>,
) -> Result<IntentOutcome, DomainError> {
    // A successful quick add remains emphasized only until the next input intent.
    model.clear_saved_task();
    // Mark-then-confirm (AC-11): any intent other than the delete verb's own
    // confirmation routes clears an armed steps delete mark. Command confirmations
    // are excluded here because they recurse below as the intent they resolved to, which
    // then faces this same rule as itself.
    if model.pending_delete.is_some()
        && !matches!(
            intent,
            BoardIntent::SoftDelete | BoardIntent::ConfirmCommand | BoardIntent::SelectCommand(_)
        )
    {
        model.pending_delete = None;
        model.pending_delete_bulk = false;
    }
    if model.empty_add_step_editor()
        && !matches!(
            intent,
            BoardIntent::EditInsert(_)
                | BoardIntent::EditInsertText(_)
                | BoardIntent::EditBackspace
                | BoardIntent::EditDeleteForward
                | BoardIntent::EditMoveLeft
                | BoardIntent::EditMoveRight
                | BoardIntent::EditMoveLineStart
                | BoardIntent::EditMoveLineEnd
                | BoardIntent::EditMoveWordLeft
                | BoardIntent::EditMoveWordRight
                | BoardIntent::ConfirmEdit
                | BoardIntent::ConfirmEditNext
                | BoardIntent::CancelEdit
                | BoardIntent::BeginAddStep
        )
    {
        let _ = model.park_rename_step_draft();
    }
    if !model.task_editing()
        && model
            .form
            .as_ref()
            .is_some_and(|form| form.steps.delete_mark.is_some())
        && !matches!(
            intent,
            BoardIntent::SoftDelete | BoardIntent::ConfirmCommand | BoardIntent::SelectCommand(_)
        )
    {
        if let Some(form) = model.form.as_mut() {
            form.steps.delete_mark = None;
        }
        // The press-again hint lives exactly as long as the mark it explains
        // (AC-23): the intervening intent that disarms the mark takes the footer
        // message down with it, before whatever the intent itself has to report.
        model.clear_message();
    }
    // AC-42: the read-only archived focus refuses every mutating verb before the reducer
    // sees it. `Undo` is the one way out (it unarchives, AC-43), and navigation, peek,
    // the drawer, the palette, help and the picker all stay live.
    // An open popup owns its own intents (the picker's ctrl+f/ctrl+u, the palette, the
    // launch card, help), so the read-only gate stands down while one is up: otherwise it
    // would refuse a picker verb in the name of the project behind the card.
    if model.focus_is_archived()
        && model.popup() == BoardPopup::None
        && model.project_picker.is_none()
        && model.surface == CommandSurface::None
        && read_only_focus_refuses(model, &intent)
    {
        if let Some(refusal) = model.archived_focus_refusal() {
            model.set_message(refusal);
        }
        return Ok(IntentOutcome::None);
    }
    let notice_before = (
        model.delete_notice().map(str::to_string),
        model.delete_notice_count,
    );
    let mutating = board_intent_may_persist(model, &intent);
    let result = apply_board_intent(domain, model, intent, snapshot);
    // A command confirmation carries no lifetime of its own: it recurses with the command it
    // resolved to, and that intent is classified on the way through, so it is the recursion
    // that restores.
    if mutating && !matches!(result, Ok(IntentOutcome::Persist)) {
        model.delete_notice = notice_before.0;
        model.delete_notice_count = notice_before.1;
    }
    result
}

fn apply_board_intent(
    domain: &mut DomainState,
    model: &mut BoardModel,
    intent: BoardIntent,
    snapshot: Option<&InvocationSnapshot>,
) -> Result<IntentOutcome, DomainError> {
    // While the task page owns input, its verbs act on the page's own task: keep the
    // selection pinned to the bound id, whatever deck visibility did to it meanwhile
    // (a completed task leaves the list, but the page and its verbs stay on it).
    //
    // Closing the page ENDS that contract, so the closing intents are excluded. Pinning
    // through the close left `selection_id` on a task that had just left the deck (complete
    // the page's task with `d`, then Esc): the board then painted no highlighted row while
    // the pin still named the hidden task, and the next `space` or `d` mutated something the
    // user could not see. Excluding them lets `reanchor_selection` move the pin to a visible
    // row, which is what it already does for every other way a task leaves the deck.
    if model.focused_surface() == FocusedSurface::Task
        && model.input_mode == BoardInputMode::TaskPage
        && matches!(
            intent,
            BoardIntent::SelectNext | BoardIntent::SelectPrev | BoardIntent::SelectIndex(_)
        )
    {
        return Ok(IntentOutcome::None);
    }
    let closes_the_page = matches!(
        intent,
        BoardIntent::CloseLayer | BoardIntent::OpenTaskPage | BoardIntent::StageLeft
    );
    if model.focused_surface() == FocusedSurface::Task
        && !closes_the_page
        && matches!(
            model.input_mode,
            BoardInputMode::TaskPage
                | BoardInputMode::EditTitle
                | BoardInputMode::EditNotes
                | BoardInputMode::EditScope
                | BoardInputMode::EditAssignee
                | BoardInputMode::FormDropdown
        )
    {
        if let Some(bound) = model
            .form
            .as_ref()
            .filter(|form| form.is_task())
            .and_then(BoardForm::task_id)
        {
            model.selection_id = Some(bound);
        }
    }
    // What this intent does to the chrome row, decided once, before it is applied.
    apply_chrome_row_lifetime(model, &intent);

    // A command surface is a dispatch surface: any real intent closes it first, so the
    // reducer below runs exactly as it does for the direct keyboard or chip route.
    // CloseLayer owns the progressive dismiss order, including the command
    // surface as its first layer, so it must not be pre-cleared here.
    if !matches!(
        intent,
        BoardIntent::OpenCommandPalette
            | BoardIntent::CommandNext
            | BoardIntent::CommandPrev
            | BoardIntent::CommandQueryInsert(_)
            | BoardIntent::CommandQueryInsertText(_)
            | BoardIntent::CommandQueryBackspace
            | BoardIntent::ConfirmCommand
            | BoardIntent::SelectCommand(_)
            | BoardIntent::CloseLayer
    ) {
        model.close_command_surface();
    }

    match intent {
        BoardIntent::OpenCommandPalette => {
            if model.project_picker.is_some() {
                return Ok(IntentOutcome::None);
            }
            model.open_command_surface(CommandSurface::Palette);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CommandNext => {
            model.move_command_selection(true);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CommandPrev => {
            model.move_command_selection(false);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CommandQueryInsert(character) => {
            if model.surface == CommandSurface::Palette {
                model.command_query.push(character);
                model.command_selected = 0;
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CommandQueryInsertText(text) => {
            if model.surface == CommandSurface::Palette {
                // The query is a single search line, so each pasted break becomes one space
                // rather than gluing the words on either side of it together (as Title does).
                model.command_query.push_str(&flatten_line_breaks(&text));
                model.command_selected = 0;
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CommandQueryBackspace => {
            if model.surface == CommandSurface::Palette {
                model.command_query.pop();
                model.command_selected = 0;
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CloseCommandSurface => return Ok(IntentOutcome::None),
        // The app loop performs the OSC 52 write so pure reducer tests stay terminal-free.
        BoardIntent::CopyTaskNumber(_) => return Ok(IntentOutcome::None),
        BoardIntent::ConfirmCommand | BoardIntent::SelectCommand(_) => {
            // Resolve to the existing intent, then run that exact route. `SelectCommand`
            // reaches here only defensively: every real caller -- the key, paste and
            // mouse routes, and `apply_board_intent_with_save_recovery` -- already runs it
            // through `resolve_board_command` before an intent gets this far, exactly as
            // they already do for `ConfirmCommand`; this arm just keeps a direct
            // `apply_intent` call (as tests make) from skipping that resolution.
            let Some(resolved) = resolve_board_command(model, intent) else {
                return Ok(IntentOutcome::None);
            };
            return apply_intent(domain, model, resolved, snapshot);
        }
        BoardIntent::Quit => return Ok(IntentOutcome::Quit),
        BoardIntent::OpenCapture => {
            model.close_popup();
            model.form = None;
            // A non-All board scope wins for this quick-add draft. All projects retains the
            // invocation default.
            let scope = model
                .quick_add_scope()
                .or_else(|| snapshot.map(crate::ui::capture::CaptureModel::default_scope))
                .unwrap_or(TaskScope::Global);
            // The default never resolves to an archived project, however the archive
            // arrived (launch card kept, picker verb, or a sibling process): fall back to
            // the desk.
            let scope = match scope {
                TaskScope::Project { ref path } if domain.is_project_archived(path) => {
                    TaskScope::Global
                }
                other => other,
            };
            model.quick_add = Some(super::model::QuickAddState::new(snapshot.cloned(), scope));
            model.quick_add_save = None;
            model.input_mode = BoardInputMode::QuickAdd;
            model.clear_message();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddInsert(character) => {
            model.invalidate_quick_add_stash();
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.insert_char(character);
                refresh_quick_add_scope(model, domain);
                model.clear_message();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddInsertText(text) => {
            model.invalidate_quick_add_stash();
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.insert_text(&flatten_line_breaks(&text));
                refresh_quick_add_scope(model, domain);
                model.clear_message();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddBackspace => {
            model.invalidate_quick_add_stash();
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.backspace();
                refresh_quick_add_scope(model, domain);
                model.clear_message();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddDeleteForward => {
            model.invalidate_quick_add_stash();
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.delete_forward();
                refresh_quick_add_scope(model, domain);
                model.clear_message();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddMoveLeft => {
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.move_left();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddMoveRight => {
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.move_right();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddMoveLineStart => {
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.move_line_start();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddMoveLineEnd => {
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.move_line_end();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddMoveWordLeft => {
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.move_word_left();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddMoveWordRight => {
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.move_word_right();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ExpandQuickAdd => {
            let Some(quick_add) = model.quick_add.as_ref() else {
                return Ok(IntentOutcome::None);
            };
            if model.form.as_ref().is_some_and(|form| !form.is_task()) {
                // The task page is view-first for persisted tasks. A capture draft has nothing
                // to view, so quick-add deliberately opens its page in Notes edit mode.
                model.focus_form_field(CaptureField::Notes);
                return Ok(IntentOutcome::None);
            }
            let lifted = match lift_quick_add_tokens(
                quick_add.title.value(),
                domain,
                quick_add.snapshot.as_ref().as_ref(),
                &model.agent_names,
            ) {
                Ok(lifted) => lifted,
                Err(message) => {
                    model.set_message(message);
                    return Ok(IntentOutcome::None);
                }
            };
            let scope = lifted.scope.unwrap_or_else(|| quick_add.scope.clone());
            let snapshot = quick_add.snapshot.as_ref().clone();
            let mut form = BoardForm::capture(
                snapshot,
                model.this_repo.as_deref(),
                &model.tasks,
                &model.archived_projects,
            );
            form.title = crate::ui::edit::seeded_draft(&lifted.title);
            form.scope = scope;
            form.thread =
                crate::ui::edit::seeded_draft(lifted.thread.as_deref().unwrap_or_default());
            form.assignee = lifted.assignee;
            form.set_agent_names(&model.agent_names);
            form.focus = CaptureField::Notes;
            form.select_current_scope();
            model.form = Some(form);
            model.input_mode = BoardInputMode::EditNotes;
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CancelQuickAdd => {
            model.discard_quick_add();
            // A save+next acknowledgement has no free status row while the line is open.
            // Keep it for the restored board row when the user finally closes quick-add.
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddSelectIndex(index) => {
            model.discard_quick_add();
            return apply_board_intent(domain, model, BoardIntent::SelectIndex(index), snapshot);
        }
        BoardIntent::QuickAddSave | BoardIntent::QuickAddSaveNext => {
            return quick_add_save(
                domain,
                model,
                matches!(intent, BoardIntent::QuickAddSaveNext),
            );
        }
        BoardIntent::FormFocusNext => {
            if model.input_mode == BoardInputMode::EditStep {
                if model.park_rename_step_draft() {
                    model.input_mode = BoardInputMode::TaskPage;
                    if !move_step_within_edit_group(model, true) {
                        select_add_step(model);
                    }
                }
            } else if matches!(
                model.input_mode,
                BoardInputMode::TaskPage | BoardInputMode::CapturePage
            ) {
                if model.capture_draft_open() {
                    if model
                        .form
                        .as_ref()
                        .is_some_and(|form| form.steps.add_selected)
                    {
                        model.focus_form_field(CaptureField::Assignee);
                    } else if !move_capture_step_with_tab(model, true) {
                        select_add_step(model);
                    }
                } else if model.task_editing() {
                    if model
                        .form
                        .as_ref()
                        .is_some_and(|form| form.steps.add_selected)
                    {
                        model.focus_form_field(CaptureField::Assignee);
                    } else if !move_step_within_edit_group(model, true) {
                        select_add_step(model);
                    }
                } else if !move_step_with_tab(model, true) && !select_first_step_from_page(model) {
                    model.enter_page_field_focus();
                }
            } else if model.input_mode == BoardInputMode::EditNotes
                && model.form.as_ref().is_some_and(|form| form.is_task())
            {
                select_step_from_tab(model, true);
            } else if model.input_mode == BoardInputMode::EditNotes && model.capture_draft_open() {
                select_first_capture_step_or_add(model);
            } else if model.input_mode == BoardInputMode::EditScope
                && model.form.as_ref().is_some_and(|form| form.is_task())
            {
                model.focus_form_field(CaptureField::Title);
            } else if model.input_mode == BoardInputMode::EditAssignee
                && model.form.as_ref().is_some_and(|form| form.is_task())
            {
                model.focus_form_field(CaptureField::Thread);
            } else if matches!(
                model.input_mode,
                BoardInputMode::SelectThread | BoardInputMode::EditThread
            ) && model.form.as_ref().is_some_and(|form| form.is_task())
            {
                model.focus_form_field(CaptureField::Scope);
            } else if model.form.is_some() && model.input_mode != BoardInputMode::FormDropdown {
                model.move_form_focus(true);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FormFocusPrev => {
            if model.input_mode == BoardInputMode::EditStep {
                if model.park_rename_step_draft() {
                    model.input_mode = BoardInputMode::TaskPage;
                    if !move_step_within_edit_group(model, false) {
                        model.focus_form_field(CaptureField::Notes);
                    }
                }
                return Ok(IntentOutcome::None);
            } else if matches!(
                model.input_mode,
                BoardInputMode::TaskPage | BoardInputMode::CapturePage
            ) {
                if model.capture_draft_open() {
                    if model
                        .form
                        .as_ref()
                        .is_some_and(|form| form.steps.add_selected)
                    {
                        if !select_last_capture_step(model) {
                            model.focus_form_field(CaptureField::Notes);
                        }
                    } else if !move_capture_step_with_tab(model, false) {
                        model.focus_form_field(CaptureField::Notes);
                    }
                } else if model.task_editing() {
                    if model
                        .form
                        .as_ref()
                        .is_some_and(|form| form.steps.add_selected)
                    {
                        select_last_step_for_edit(model);
                    } else if !move_step_within_edit_group(model, false) {
                        model.focus_form_field(CaptureField::Notes);
                    }
                } else if !move_step_with_tab(model, false) {
                    model.enter_page_field_focus();
                }
                return Ok(IntentOutcome::None);
            }
            if model.input_mode == BoardInputMode::EditTitle
                && model.form.as_ref().is_some_and(|form| form.is_task())
            {
                model.focus_form_field(CaptureField::Scope);
            } else if matches!(
                model.input_mode,
                BoardInputMode::SelectThread | BoardInputMode::EditThread
            ) && model.form.is_some()
            {
                model.focus_form_field(CaptureField::Assignee);
            } else if model.input_mode == BoardInputMode::EditScope
                && model.form.as_ref().is_some_and(|form| form.is_task())
            {
                model.focus_form_field(CaptureField::Thread);
            } else if model.input_mode == BoardInputMode::EditAssignee && model.form.is_some() {
                select_add_step(model);
            } else if model.form.is_some() && model.input_mode != BoardInputMode::FormDropdown {
                model.move_form_focus(false);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FocusFormCursor(field, row, column) => {
            let cursor = model.form.as_ref().and_then(|form| {
                let (draft, width) = match field {
                    CaptureField::Title => (&form.title, form.title_wrap_width.get()),
                    CaptureField::Notes => (&form.notes, form.notes_width.get()),
                    _ => return None,
                };
                crate::ui::edit::wrap_text(draft.value(), width.max(1))
                    .get(row)
                    .map(|row| row.cursor_at(column))
            });
            model.focus_form_field(field);
            let focused = matches!(
                (field, model.input_mode),
                (CaptureField::Title, BoardInputMode::EditTitle)
                    | (CaptureField::Notes, BoardInputMode::EditNotes)
            );
            if let Some(cursor) = cursor.filter(|_| focused) {
                edit_draft(model, |draft| draft.set_cursor(cursor));
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FocusFormField(field) => {
            model.focus_form_field(field);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ToggleThreadEditing => {
            model.toggle_thread_editing();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FormAssigneeNext | BoardIntent::FormAssigneePrev => {
            model.cycle_form_assignee(matches!(intent, BoardIntent::FormAssigneeNext));
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ConfirmFormAssignee => {
            if let Some(targets) = model.pending_assignee_targets.take() {
                let assignee = model.form.as_ref().and_then(|form| form.assignee.clone());
                let changed = domain.assign_batch(&targets, assignee)?;
                model.clear_marks();
                model.form = None;
                model.input_mode = BoardInputMode::Normal;
                return Ok(if changed {
                    IntentOutcome::Persist
                } else {
                    IntentOutcome::None
                });
            }
            model.move_form_focus(true);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FormCycleScope => {
            if model
                .form
                .as_ref()
                .is_some_and(|form| form.focus == CaptureField::Scope)
                && model.input_mode != BoardInputMode::FormDropdown
            {
                model.cycle_form_scope();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::OpenFormDropdown(field) => {
            model.open_form_dropdown(field);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FormDropdownNext => {
            model.move_form_dropdown(true);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FormDropdownPrev => {
            model.move_form_dropdown(false);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ConfirmFormDropdown => {
            let field = model.form.as_ref().map(|form| form.focus);
            if model.input_mode == BoardInputMode::FormDropdown
                && model.close_form_dropdown(true)
                && field == Some(CaptureField::Assignee)
            {
                if let Some(targets) = model.pending_assignee_targets.as_ref() {
                    let assignee = model.form.as_ref().and_then(|form| form.assignee.clone());
                    let changed = domain.assign_batch(targets, assignee)?;
                    model.clear_marks();
                    if changed {
                        return Ok(IntentOutcome::Persist);
                    }
                    model.finish_pending_assignee_assignment();
                    return Ok(IntentOutcome::None);
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CancelFormDropdown => {
            if model.input_mode == BoardInputMode::FormDropdown {
                model.close_form_dropdown(false);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SelectFormDropdownOption(index) => {
            let field = model.form.as_ref().map(|form| form.focus);
            if model.select_form_dropdown_option(index) && field == Some(CaptureField::Assignee) {
                if let Some(targets) = model.pending_assignee_targets.as_ref() {
                    let assignee = model.form.as_ref().and_then(|form| form.assignee.clone());
                    let changed = domain.assign_batch(targets, assignee)?;
                    model.clear_marks();
                    if changed {
                        return Ok(IntentOutcome::Persist);
                    }
                    model.finish_pending_assignee_assignment();
                    return Ok(IntentOutcome::None);
                }
            }
            return Ok(IntentOutcome::None);
        }

        BoardIntent::SelectNext => {
            // The projects index is keyboard-navigable in place: its rows are project
            // rows, never tasks, so the pin stays untouched while the cursor moves.
            if model.nav_tab() == NavTab::Projects
                && matches!(model.projects_view, ProjectsView::Overview)
            {
                model.move_projects_cursor(true);
                return Ok(IntentOutcome::None);
            }
            model.select_next();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SelectPrev => {
            if model.nav_tab() == NavTab::Projects
                && matches!(model.projects_view, ProjectsView::Overview)
            {
                model.move_projects_cursor(false);
                return Ok(IntentOutcome::None);
            }
            model.select_prev();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ToggleMarkMode => {
            if !model.mark_mode_active() && !model.task_list_owns_input() {
                return Ok(IntentOutcome::None);
            }
            model.toggle_mark_mode();
            model.clear_message();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::MarkToggle => {
            if !model.task_list_owns_input() || !model.mark_mode_active() {
                return Ok(IntentOutcome::None);
            }
            model.toggle_selected_mark();
            model.clear_message();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::MarkToggleAt(idx) => {
            if !model.task_list_owns_input() || !model.mark_mode_active() {
                return Ok(IntentOutcome::None);
            }
            if model.select_index(idx) {
                model.toggle_selected_mark();
                model.last_row_click = None;
                model.clear_message();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::MarkExtend(direction) => {
            if !model.task_list_owns_input() || !model.mark_mode_active() {
                return Ok(IntentOutcome::None);
            }
            model.mark_selected();
            match direction {
                MarkDirection::Up => model.select_prev(),
                MarkDirection::Down => model.select_next(),
            };
            model.clear_message();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::MarkClear => {
            model.clear_marks();
            model.clear_message();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SelectIndex(idx) => {
            if !model.select_index(idx) {
                return Ok(IntentOutcome::None);
            }
            // A row click also expands that row's peek; a second click on the same row
            // inside the double-click window opens the task page instead.
            if let Some(id) = model.selected_id() {
                let now = Instant::now();
                let is_double = model.last_row_click.is_some_and(|(at, last)| {
                    last == id && now.duration_since(at) <= ROW_DOUBLE_CLICK_WINDOW
                });
                if is_double {
                    model.last_row_click = None;
                    open_full_task_page(domain, model, id);
                } else {
                    model.last_row_click = Some((now, id));
                    if model.detail_open == Some(id) {
                        model.detail_open = None;
                    } else {
                        model.detail_open = Some(id);
                    }
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FocusBoardAndSelectIndex(idx) => {
            if !model.select_index(idx) {
                return Ok(IntentOutcome::None);
            }
            // A row click opens or retargets the task beside the board, with board focus.
            // A second click on the same row inside the double-click window opens the
            // full task page.
            model.detail_open = None;
            if matches!(model.wide_stage, WideStage::FullBoard | WideStage::Rail) {
                model.wide_stage = WideStage::Split;
            }
            if let Some(id) = model.selected_id() {
                let now = Instant::now();
                let is_double = model.last_row_click.is_some_and(|(at, last)| {
                    last == id && now.duration_since(at) <= ROW_DOUBLE_CLICK_WINDOW
                });
                if is_double {
                    model.last_row_click = None;
                    open_full_task_page(domain, model, id);
                } else {
                    model.last_row_click = Some((now, id));
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ListScrollTo(offset) => {
            if model.focused_surface() != FocusedSurface::Board {
                return Ok(IntentOutcome::None);
            }
            model.set_list_scroll(offset);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::BeginAddStep => {
            model.close_popup();
            // Ctrl+A and the trailing target both open the independent add row from a task
            // page or an expanded capture. Park a rename first, so Ctrl+A never drops an
            // existing staged rename while replacing its cursor with the add row.
            if model.form.is_some() && model.park_rename_step_draft() {
                open_step_editor(model, "", None);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::BeginEditTitle
        | BoardIntent::BeginEditNotes
        | BoardIntent::BeginEditScope
        | BoardIntent::BeginEditAssignee => {
            model.close_popup();
            let assignee_targets =
                (intent == BoardIntent::BeginEditAssignee).then(|| model.verb_target_ids());
            if intent != BoardIntent::BeginEditAssignee {
                model.clear_marks();
            }
            // Ctrl+E on an already-open inline row keeps that row focused. Field traversal is
            // explicit through Tab or clicks, so this never discards or redirects its draft.
            if intent == BoardIntent::BeginEditTitle && model.input_mode == BoardInputMode::EditStep
            {
                return Ok(IntentOutcome::None);
            }
            let focus = match intent {
                BoardIntent::BeginEditTitle => CaptureField::Title,
                BoardIntent::BeginEditNotes => CaptureField::Notes,
                BoardIntent::BeginEditScope => CaptureField::Scope,
                BoardIntent::BeginEditAssignee => CaptureField::Assignee,
                _ => unreachable!("matched task-form entry intent"),
            };
            // Ctrl+E on a selected step begins the whole task edit session and opens that
            // row's in-place editor, including when the selection was made in task view.
            if intent == BoardIntent::BeginEditTitle {
                if let Some((task_id, step_id)) = selected_step(domain, model) {
                    let text = domain.get(task_id).and_then(|task| {
                        task.steps
                            .iter()
                            .find(|step| step.id == step_id)
                            .map(|step| step.text.clone())
                    });
                    if let Some(text) = text {
                        if let Some(form) = model.form.as_mut().filter(|form| form.is_task()) {
                            form.editing = true;
                        }
                        open_step_editor(model, &text, Some(step_id));
                        return Ok(IntentOutcome::None);
                    }
                }
            }
            // The page already open: move focus into the asked field, keep every draft, and
            // transfer input ownership before the editor can accept a key.
            if model.form.as_ref().is_some_and(BoardForm::is_task) {
                enter_task_stage(model);
                model.pending_assignee_targets = assignee_targets;
                model.focus_form_field(focus);
                return Ok(IntentOutcome::None);
            }
            if let Some(id) = model.selected_id() {
                if let Some(task) = domain.get(id) {
                    // One immutable id and three independent drafts are captured at open.
                    // `sync_from_domain` deliberately never writes this form, so background
                    // refresh can reanchor selection without redirecting its later save.
                    let mut form = BoardForm::task(
                        task,
                        model.this_repo.as_deref(),
                        &model.tasks,
                        focus,
                        &model.archived_projects,
                        &model.agent_names,
                    );
                    // A direct board edit is a real edit session too, so its confirmed task
                    // page keeps step interaction available after the field saves.
                    form.editing = true;
                    model.pending_assignee_targets = assignee_targets;
                    model.input_mode = form.parent_mode();
                    model.form = Some(form);
                    enter_task_stage(model);
                    model.clear_message();
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditInsert(c) => {
            edit_draft(model, |draft| draft.insert_char(c));
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditInsertText(text) => {
            // Title and Thread stay one line in either form; Notes preserves pasted line
            // breaks. The step editor is one line by construction, so it flattens like Title.
            let single_line = model.input_mode == BoardInputMode::EditStep
                || model.form.as_ref().is_some_and(|form| {
                    matches!(form.focus, CaptureField::Title | CaptureField::Thread)
                });
            edit_draft(model, |draft| {
                if single_line {
                    draft.insert_text(&flatten_line_breaks(&text));
                } else {
                    draft.insert_text(&text);
                }
            });
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditInsertLineBreak => {
            if model
                .form
                .as_ref()
                .is_some_and(|form| form.focus == CaptureField::Notes)
            {
                edit_draft(model, |draft| draft.insert_char('\n'));
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditBackspace => {
            edit_draft(model, EditBuffer::backspace);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditDeleteForward => {
            edit_draft(model, EditBuffer::delete_forward);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditMoveLeft => {
            edit_draft(model, EditBuffer::move_left);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditMoveRight => {
            edit_draft(model, EditBuffer::move_right);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditMoveUp | BoardIntent::EditMoveDown => {
            // Vertical movement wraps at the painted notes width the renderer
            // recorded; until a frame has been drawn (width 0) it stays inert.
            let delta: isize = if matches!(intent, BoardIntent::EditMoveDown) {
                1
            } else {
                -1
            };
            let target = model
                .form
                .as_ref()
                .filter(|form| {
                    form.focus == CaptureField::Notes
                        && model.input_mode == BoardInputMode::EditNotes
                })
                .and_then(|form| {
                    crate::ui::edit::wrapped_vertical_move(
                        &form.notes,
                        form.notes_width.get(),
                        delta,
                    )
                });
            if let Some(target) = target {
                edit_draft(model, |draft| draft.set_cursor(target));
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditMoveLineStart => {
            edit_draft(model, EditBuffer::move_line_start);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditMoveLineEnd => {
            edit_draft(model, EditBuffer::move_line_end);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditMoveWordLeft => {
            edit_draft(model, EditBuffer::move_word_left);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditMoveWordRight => {
            edit_draft(model, EditBuffer::move_word_right);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CancelEdit => {
            // AC-45: Esc leaves the read-only archived focus, which hides that project's
            // tasks again. Nothing else on the board is open in that state.
            if model.focus_is_archived() && model.input_mode == BoardInputMode::Normal {
                model.leave_archived_focus();
                return Ok(IntentOutcome::None);
            }
            // The inline step editor cancels to page view: draft discarded, no mutation,
            // and the page's step cursor state stays intact.
            if model.input_mode == BoardInputMode::EditStep && model.form.is_some() {
                let capture = model.form.as_ref().is_some_and(|form| !form.is_task());
                close_step_editor(model);
                if capture {
                    if let Some(form) = model.form.as_ref() {
                        model.input_mode = form.parent_mode();
                    }
                }
                return Ok(IntentOutcome::None);
            }
            // Field edit on the task page: Esc cancels the field being edited (its draft
            // resets to the saved value) and steps back to view mode. Drafts on other
            // fields survive; the second Esc closes the page.
            if matches!(
                model.input_mode,
                BoardInputMode::EditTitle
                    | BoardInputMode::EditNotes
                    | BoardInputMode::EditThread
                    | BoardInputMode::EditScope
                    | BoardInputMode::EditAssignee
            ) && model.form.as_ref().is_some_and(BoardForm::is_task)
            {
                let field = model
                    .form
                    .as_ref()
                    .map(|form| form.focus)
                    .unwrap_or(CaptureField::Title);
                let saved = model
                    .form
                    .as_ref()
                    .and_then(|form| form.task_id())
                    .and_then(|id| model.tasks.iter().find(|task| task.id == id))
                    .cloned();
                if let (Some(form), Some(task)) = (model.form.as_mut(), saved) {
                    form.reset_field_to_saved(field, &task);
                }
                model.pending_assignee_targets = None;
                model.input_mode = BoardInputMode::TaskPage;
                model.clear_message();
                return Ok(IntentOutcome::None);
            }
            // Esc from an expanded capture returns to its retained quick-add line. Keep the
            // complete form as a stash so Tab can restore its notes and scope.
            if model.quick_add.is_some() && model.form.as_ref().is_some_and(|form| !form.is_task())
            {
                model.input_mode = BoardInputMode::QuickAdd;
                model.clear_message();
                return Ok(IntentOutcome::None);
            }
            // All other complete forms discard as before. Dropdown Esc has its own intent.
            if model.form.take().is_some() {
                model.pending_assignee_targets = None;
                model.input_mode = if model.quick_add.is_some() {
                    BoardInputMode::QuickAdd
                } else {
                    BoardInputMode::Normal
                };
                model.clear_message();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ConfirmEditNext => {
            model.pending_assignee_targets = None;
            // Shift+Enter on an existing step, or on a typed new step, commits the complete
            // task edit session. Keep the active row allocated until persistence confirms.
            if model.input_mode == BoardInputMode::EditStep {
                if model.stage_active_rename_draft() {
                    let outcome = confirm_edit(domain, model, None)?;
                    model.sync_from_domain(domain);
                    return Ok(outcome);
                }
                return confirm_add_step(domain, model, snapshot, true);
            }
            return apply_intent(domain, model, BoardIntent::ConfirmEdit, snapshot);
        }
        BoardIntent::ConfirmEdit => {
            model.pending_assignee_targets = None;
            if model.input_mode == BoardInputMode::EditStep {
                // Plain Enter parks an existing-step rename. New-step adds save and open the
                // next empty row, including an empty draft which stays on the line as a refusal.
                if model.has_active_step_rename() {
                    if model.park_rename_step_draft() {
                        model.input_mode = BoardInputMode::TaskPage;
                    }
                    return Ok(IntentOutcome::None);
                }
                return confirm_add_step(domain, model, snapshot, false);
            }
            if model.form.as_ref().is_some_and(|form| !form.is_task()) {
                // Capture keeps its immutable invocation snapshot in the shared form. Without
                // it there is nothing to save against, so the draft remains visible and intact.
                let form = model.form.as_ref().expect("capture form checked above");
                let Some(snap) = form.snapshot().cloned() else {
                    model.set_message("capture context unavailable; press Esc and try again");
                    return Ok(IntentOutcome::None);
                };
                let title = form.title.value().to_string();
                let notes =
                    (!form.notes.value().trim().is_empty()).then(|| form.notes.value().to_string());
                let scope_override = Some(form.scope.clone());
                let thread = match normalize_optional_thread(form.thread.value()) {
                    Ok(thread) => thread,
                    Err(error) => {
                        if let Some(form) = model.form.as_mut() {
                            form.thread_refusal = Some(thread_refusal_message(error));
                        }
                        return Ok(IntentOutcome::None);
                    }
                };
                let assignee = form.assignee.clone();
                let pending_adds = form.steps.pending_adds.clone();
                return match crate::capture::capture_save_assigned(
                    domain,
                    None,
                    &snap,
                    title,
                    notes,
                    scope_override,
                    thread,
                    assignee,
                ) {
                    Ok(id) => {
                        for text in &pending_adds {
                            domain.add_step(id, text)?;
                        }
                        let expanded_quick_add = model.quick_add.is_some();
                        if expanded_quick_add {
                            // The app save boundary still owns this create. Retain the expanded
                            // form until it succeeds so recovery Cancel can return to a complete
                            // quick-add stash instead of an edit mode with no form.
                            model.begin_quick_add_save(id, false);
                        } else {
                            model.input_mode = BoardInputMode::Normal;
                            model.clear_message();
                        }
                        Ok(IntentOutcome::Persist)
                    }
                    Err(crate::capture::CaptureError::Domain(DomainError::EmptyTitle)) => {
                        model.set_message(TITLE_REQUIRED_MESSAGE);
                        Ok(IntentOutcome::None)
                    }
                    Err(error) => {
                        model.set_message(error.to_string());
                        Ok(IntentOutcome::None)
                    }
                };
            }
            let outcome = confirm_edit(domain, model, None)?;
            model.sync_from_domain(domain);
            return Ok(outcome);
        }

        BoardIntent::OpenProjectSelector => {
            // the project-scope chip opens this dropdown from any board surface.
            // A save-failure modal owns its decision rather than being dismissed by the chip.
            if matches!(model.popup, BoardPopup::SaveRecovery) {
                return Ok(IntentOutcome::None);
            }
            if model.projects_overview() {
                if model
                    .right_seat()
                    .is_some_and(|right| right.has_unsaved_work())
                {
                    model.set_message(DIRTY_TASK_SWITCH_REFUSAL);
                    return Ok(IntentOutcome::None);
                }
                model.wide_stage = WideStage::FullBoard;
                model.stage_origin = None;
                model.drop_project_preview();
            }
            // AC-45: `P` leaves the read-only archived lens before the picker paints, so
            // its tasks are hidden again and Esc from the picker lands home, not back in
            // a lens the user thought they had left.
            if model.focus_is_archived() {
                model.leave_archived_focus();
            }
            let options = model.project_options();
            // Highlight the option that matches the current destination.
            let selected = match &model.board_location {
                BoardLocation::Desk | BoardLocation::Projects => 0,
                BoardLocation::Project(path) | BoardLocation::ArchivedProject(path) => options
                    .iter()
                    .position(|option| option == &ProjectScopeOption::Project(path.clone()))
                    .unwrap_or(0),
            };
            model.close_popup();
            model.close_command_surface();
            model.close_help();
            model.project_picker = Some(ProjectPickerState {
                options,
                selected,
                tab: PickerTab::Main,
                archived: model.archived_project_options(),
                archived_selected: 0,
            });
            model.popup = BoardPopup::ProjectPicker;
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ProjectPickerNext => {
            model.move_project_picker(true);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ProjectPickerPrev => {
            model.move_project_picker(false);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ProjectPickerSwitchTab | BoardIntent::SelectPickerTab(_) => {
            if let Some(picker) = model.project_picker.as_mut() {
                picker.tab = match intent {
                    BoardIntent::SelectPickerTab(tab) => tab,
                    _ => match picker.tab {
                        PickerTab::Main => PickerTab::Archived,
                        PickerTab::Archived => PickerTab::Main,
                    },
                };
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ConfirmProjectChoice => {
            // Session-only navigation: nothing durable is touched, so no outcome persists.
            // AC-41: on the archived tab, Enter opens that project in read-only focus.
            if model.picker_tab() == Some(PickerTab::Archived) {
                let chosen = model
                    .project_picker
                    .as_ref()
                    .and_then(|picker| picker.archived.get(picker.archived_selected).cloned());
                if let Some(path) = chosen {
                    model.clear_marks();
                    model.project_picker = None;
                    model.open_archived_focus(path);
                    model.clear_message();
                }
                return Ok(IntentOutcome::None);
            }
            let Some(picker) = model.project_picker.take() else {
                return Ok(IntentOutcome::None);
            };
            let chosen = picker.options.get(picker.selected).cloned();
            model.clear_marks();
            model.set_board_scope(chosen.unwrap_or(ProjectScopeOption::Home));
            model.clear_message();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CancelProjectPicker => {
            if model.project_picker.is_some() {
                model.close_popup();
                model.clear_message();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SelectProjectOption(index) => {
            // Mouse-only jump: a dropdown row click chooses its option directly, the way
            // `SelectIndex` chooses a task row directly, instead of stepping
            // `ProjectPickerNext`/`Prev` to it first. Session-only
            // navigation: nothing durable is touched, so no outcome persists.
            // The archived tab has no choose action: its rows are inert.
            if model.picker_tab() == Some(PickerTab::Archived) {
                return Ok(IntentOutcome::None);
            }
            let Some(picker) = model.project_picker.take() else {
                return Ok(IntentOutcome::None);
            };
            let Some(chosen) = picker.options.get(index).cloned() else {
                // Out of range against the picker this click actually opened: put it back
                // rather than silently discard an open selection.
                model.project_picker = Some(picker);
                return Ok(IntentOutcome::None);
            };
            model.clear_marks();
            model.set_board_scope(chosen);
            model.clear_message();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SelectNavTab(tab) => {
            if matches!(model.board_location, BoardLocation::Projects)
                && tab != NavTab::Projects
                && model
                    .right_seat()
                    .is_some_and(|right| right.has_unsaved_work())
            {
                model.set_message(DIRTY_TASK_SWITCH_REFUSAL);
                return Ok(IntentOutcome::None);
            }
            let switched = model.select_nav_tab(tab);
            // Slot 2 never changes meaning: with no project selected it opens the
            // picker, and picking it again while its project is open answers the
            // painted `▾` with the same picker. From the read-only archived focus,
            // `2` stays put (the focus already occupies slot 2).
            if !switched && tab == NavTab::ProjectBoard && !model.focus_is_archived() {
                return apply_board_intent(
                    domain,
                    model,
                    BoardIntent::OpenProjectSelector,
                    snapshot,
                );
            }
            if switched {
                model.clear_marks();
            }
            model.clear_message();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::OpenThreadFilterPicker => {
            if model.project_picker.is_some() || model.popup == BoardPopup::SaveRecovery {
                return Ok(IntentOutcome::None);
            }
            model.close_command_surface();
            model.close_popup();
            model.close_help();
            model.open_thread_filter_picker();
            if model.list_picker.is_some() {
                model.input_mode = BoardInputMode::ListPicker;
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::OpenProjectsViewPicker => {
            if model.project_picker.is_some() || model.popup == BoardPopup::SaveRecovery {
                return Ok(IntentOutcome::None);
            }
            model.close_command_surface();
            model.close_popup();
            model.close_help();
            model.open_projects_view_picker();
            if model.list_picker.is_some() {
                model.input_mode = BoardInputMode::ListPicker;
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ListPickerNext => {
            model.move_list_picker(true);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ListPickerPrev => {
            model.move_list_picker(false);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ListPickerQueryInsert(character) => {
            model.list_picker_query_insert(character);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ListPickerQueryInsertText(text) => {
            model.list_picker_query_insert_text(&flatten_line_breaks(&text));
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ListPickerQueryBackspace => {
            model.list_picker_query_backspace();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SelectListOption(index) => {
            // Mouse-only jump onto a visible picker row, same discipline as
            // `SelectCommand`/`SelectProjectOption`: name the row directly.
            if let Some(picker) = model.list_picker.as_mut() {
                picker.selected = index;
            }
            return apply_board_intent(domain, model, BoardIntent::ConfirmListPicker, snapshot);
        }
        BoardIntent::ConfirmListPicker => {
            let drops_project_preview = model.projects_preview_active()
                && model.list_picker_kind() == Some(crate::ui::board::ListPickerKind::ProjectsView)
                && model
                    .visible_list_picker_options()
                    .get(model.list_picker_selected())
                    .is_some_and(|(_, option)| {
                        matches!(
                            &option.value,
                            ListPickerValue::ProjectsOverview | ListPickerValue::ProjectsThread(_)
                        )
                    });
            if drops_project_preview
                && model
                    .right_seat()
                    .is_some_and(|right| right.has_unsaved_work())
            {
                model.set_message(DIRTY_TASK_SWITCH_REFUSAL);
                return Ok(IntentOutcome::None);
            }
            if let Some(value) = model.confirm_list_picker() {
                model.clear_marks();
                let previous = model.selection_id;
                let previous_visible = model.visible_ids();
                model.reanchor_selection(previous, &previous_visible);
                if matches!(
                    value,
                    ListPickerValue::ProjectsOverview | ListPickerValue::ProjectsThread(_)
                ) {
                    model.wide_stage = WideStage::FullBoard;
                    model.stage_origin = None;
                    model.drop_project_preview();
                }
                model.input_mode = BoardInputMode::Normal;
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CancelListPicker => {
            model.cancel_list_picker();
            model.input_mode = BoardInputMode::Normal;
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FocusSearch => {
            if model.input_mode != BoardInputMode::Search {
                model.search_return_mode = model.input_mode;
                model.input_mode = BoardInputMode::Search;
            }
            model.search_pinned = false;
            model.clear_message();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::PinSearch => {
            if model.input_mode == BoardInputMode::Search {
                model.input_mode = model.search_return_mode;
                if model.search_query.trim().is_empty() {
                    model.search_query.clear();
                    model.search_pinned = false;
                } else {
                    model.search_pinned = true;
                }
                model.clear_message();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SearchQueryInsert(character) => {
            if model.projects_preview_active()
                && model
                    .right_seat()
                    .is_some_and(|right| right.has_unsaved_work())
            {
                model.set_message(DIRTY_TASK_SWITCH_REFUSAL);
                return Ok(IntentOutcome::None);
            }
            let previous_visible = model.visible_ids();
            let previous = model.selection_id;
            model.search_query.push(character);
            model.search_pinned = false;
            model.clear_message();
            if model.projects_overview() {
                model.projects_selected = 0;
                if model.projects_preview_active() {
                    model.bind_project_preview();
                }
            } else {
                model.reanchor_selection(previous, &previous_visible);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SearchQueryInsertText(text) => {
            if model.projects_preview_active()
                && model
                    .right_seat()
                    .is_some_and(|right| right.has_unsaved_work())
            {
                model.set_message(DIRTY_TASK_SWITCH_REFUSAL);
                return Ok(IntentOutcome::None);
            }
            let previous_visible = model.visible_ids();
            let previous = model.selection_id;
            model.search_query.push_str(&text);
            model.search_pinned = false;
            model.clear_message();
            if model.projects_overview() {
                model.projects_selected = 0;
                if model.projects_preview_active() {
                    model.bind_project_preview();
                }
            } else {
                model.reanchor_selection(previous, &previous_visible);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SearchQueryBackspace => {
            if model.projects_preview_active()
                && model
                    .right_seat()
                    .is_some_and(|right| right.has_unsaved_work())
            {
                model.set_message(DIRTY_TASK_SWITCH_REFUSAL);
                return Ok(IntentOutcome::None);
            }
            let previous_visible = model.visible_ids();
            let previous = model.selection_id;
            model.search_query.pop();
            model.search_pinned = false;
            model.clear_message();
            if model.projects_overview() {
                model.projects_selected = 0;
                if model.projects_preview_active() {
                    model.bind_project_preview();
                }
            } else {
                model.reanchor_selection(previous, &previous_visible);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SelectProjectRow(index) => {
            // Mouse route onto an index row: a click selects it (the status row then names
            // its path), a second click inside the ordinary index view opens the project in
            // slot 2. In the live Rail preview, a row click is deliberately only a focus
            // transfer back to the index, so it cannot unexpectedly leave the projects tab.
            let Some(row) = model.project_rows().into_iter().nth(index) else {
                return Ok(IntentOutcome::None);
            };
            let path = PathBuf::from(row.path);
            if model.projects_preview_active() && model.wide_stage == WideStage::Rail {
                let previous = model.projects_selected;
                let changing_project = model.selected_project_row().is_some_and(|current| {
                    !paths_equivalent(&current.path, &path.to_string_lossy())
                });
                if changing_project
                    && model
                        .right_seat()
                        .is_some_and(|right| right.has_unsaved_work())
                {
                    model.set_message(DIRTY_TASK_SWITCH_REFUSAL);
                    return Ok(IntentOutcome::None);
                }
                model.projects_selected = index;
                if !model.bind_project_preview() {
                    model.projects_selected = previous;
                    return Ok(IntentOutcome::None);
                }
                model.clear_project_preview_ephemeral_state();
                model.wide_stage = WideStage::Split;
                model.last_project_row_click = None;
                model.clear_message();
                return Ok(IntentOutcome::None);
            }
            let previous = model.projects_selected;
            let preview_active = model.projects_preview_active();
            if preview_active
                && model
                    .right_seat()
                    .is_some_and(|right| right.has_unsaved_work())
                && model.selected_project_row().is_some_and(|current| {
                    !paths_equivalent(&current.path, &path.to_string_lossy())
                })
            {
                model.set_message(DIRTY_TASK_SWITCH_REFUSAL);
                return Ok(IntentOutcome::None);
            }
            model.projects_selected = index;
            if preview_active && !model.bind_project_preview() {
                model.projects_selected = previous;
                return Ok(IntentOutcome::None);
            }
            model.clear_message();
            let now = Instant::now();
            let is_double = model
                .last_project_row_click
                .as_ref()
                .is_some_and(|(at, last)| {
                    *last == path && now.duration_since(*at) <= ROW_DOUBLE_CLICK_WINDOW
                });
            if is_double {
                model.last_project_row_click = None;
                if model
                    .right_seat()
                    .is_some_and(|right| right.has_unsaved_work())
                {
                    model.set_message(DIRTY_TASK_SWITCH_REFUSAL);
                    return Ok(IntentOutcome::None);
                }
                model.set_board_scope(ProjectScopeOption::Project(path));
            } else {
                model.last_project_row_click = Some((now, path));
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ToggleAllGroups => {
            model.clear_marks();
            let previous_visible = model.visible_ids();
            let previous = model.selection_id;
            if model.toggle_all_groups() {
                model.reanchor_selection(previous, &previous_visible);
            }
            model.clear_message();
            return Ok(IntentOutcome::None);
        }
        // The app-level save-recovery boundary handles these while unresolved.
        BoardIntent::RetrySave | BoardIntent::CancelSave => return Ok(IntentOutcome::None),
        BoardIntent::ToggleStep => {
            model.close_popup();
            let Some((task_id, step_id)) = selected_step(domain, model) else {
                return Ok(IntentOutcome::None);
            };
            domain.toggle_step(task_id, step_id)?;
        }
        BoardIntent::Dispatch | BoardIntent::DispatchAgain => {
            // Host work and its one durable save are owned by the application boundary.
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ConfirmCleanup | BoardIntent::KeepCleanup => {
            // Cleanup host work and completion persistence are owned by the app boundary.
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CancelCleanup => {
            model.close_popup();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::PrimaryVerb => {
            model.close_popup();
            // Status verbs always act on tasks, even with a step selected: Enter owns steps.
            let (targets, _) = take_verb_targets(model);
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            let baseline = domain.clone();
            let mut changed = false;
            for id in targets {
                let Some(status) = domain.get(id).map(|task| task.status) else {
                    *domain = baseline;
                    model.set_message("that task is no longer here");
                    return Ok(IntentOutcome::None);
                };
                if matches!(status, HumanStatus::Open | HumanStatus::Ready) {
                    if let Err(error) = domain.set_status(id, HumanStatus::Started) {
                        *domain = baseline;
                        return Err(error);
                    }
                    changed = true;
                }
            }
            if !changed {
                return Ok(IntentOutcome::None);
            }
        }
        BoardIntent::ToggleBlock => {
            model.close_popup();
            let (targets, bulk) = take_verb_targets(model);
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            if !bulk
                && targets.iter().any(|id| {
                    domain
                        .get(*id)
                        .is_some_and(|task| task.status == HumanStatus::Done)
                })
            {
                model.set_message("completed tasks cannot be blocked");
                return Ok(IntentOutcome::None);
            }
            let all_blocked = targets.iter().all(|id| {
                domain
                    .get(*id)
                    .is_some_and(|task| task.status == HumanStatus::Blocked)
            });
            let status = if all_blocked {
                HumanStatus::Ready
            } else {
                HumanStatus::Blocked
            };
            set_status_batch(domain, &targets, status)?;
        }
        BoardIntent::ToggleReview => {
            model.close_popup();
            let (targets, bulk) = take_verb_targets(model);
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            if !bulk
                && targets.iter().any(|id| {
                    domain
                        .get(*id)
                        .is_some_and(|task| task.status == HumanStatus::Done)
                })
            {
                model.set_message("completed tasks cannot go to review");
                return Ok(IntentOutcome::None);
            }
            let all_review = targets.iter().all(|id| {
                domain
                    .get(*id)
                    .is_some_and(|task| task.status == HumanStatus::Review)
            });
            let status = if all_review {
                HumanStatus::Ready
            } else {
                HumanStatus::Review
            };
            set_status_batch(domain, &targets, status)?;
        }
        BoardIntent::StageRight => {
            stage_right(domain, model);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::StageLeft => {
            stage_left(model);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::OpenTaskPage => {
            // The projects index: Enter opens the selected project in slot 2. Index
            // rows are navigation; the task-page surface never opens from them.
            if model.nav_tab() == NavTab::Projects
                && matches!(model.projects_view, ProjectsView::Overview)
            {
                if let Some(row) = model.selected_project_row() {
                    if model
                        .right_seat()
                        .is_some_and(|right| right.has_unsaved_work())
                    {
                        model.set_message(DIRTY_TASK_SWITCH_REFUSAL);
                        return Ok(IntentOutcome::None);
                    }
                    model.set_board_scope(ProjectScopeOption::Project(PathBuf::from(row.path)));
                    model.clear_message();
                }
                return Ok(IntentOutcome::None);
            }
            // Enter on a group header toggles that group instead of opening a page:
            // the header is chrome, never a task.
            if model.archived_header_selected() {
                model.clear_marks();
                let previous_visible = model.visible_ids();
                model.toggle_archived_collapsed();
                model.reanchor_selection(Some(ARCHIVED_HEADER_ROW_ID), &previous_visible);
                return Ok(IntentOutcome::None);
            }
            if model.inbox_header_selected() {
                model.clear_marks();
                let previous_visible = model.visible_ids();
                model.toggle_inbox_collapsed();
                model.reanchor_selection(Some(INBOX_HEADER_ROW_ID), &previous_visible);
                return Ok(IntentOutcome::None);
            }
            // A capture's staged rows have no domain task identity. Their page owns Enter, so
            // it must never fall through to the board selection hidden behind the draft.
            if model.capture_draft_open() && model.input_mode == BoardInputMode::CapturePage {
                if model
                    .form
                    .as_ref()
                    .is_some_and(|form| form.steps.add_selected)
                {
                    open_step_editor(model, "", None);
                } else if let Some(index) = model.form.as_ref().and_then(|form| form.steps.cursor) {
                    open_capture_pending_step_editor(model, index);
                }
                return Ok(IntentOutcome::None);
            }
            // Enter never opens inline step editing. A selected step remains selected in either
            // page state; Ctrl+E is the deliberate route into its editor.
            if selected_step(domain, model).is_some() {
                return Ok(IntentOutcome::None);
            }
            if model
                .form
                .as_ref()
                .is_some_and(|form| form.steps.add_selected)
            {
                open_step_editor(model, "", None);
                return Ok(IntentOutcome::None);
            }
            if model.form.as_ref().is_some_and(BoardForm::is_task) {
                // Enter beside the rail opens the full page; on the full page (or the
                // single-pane page) it toggles the page shut, as it always has.
                if model.focused_surface() == FocusedSurface::Board
                    || model.wide_stage == WideStage::Rail
                {
                    let Some(id) = model.selected_id() else {
                        return Ok(IntentOutcome::None);
                    };
                    open_full_task_page(domain, model, id);
                } else {
                    leave_task_page(model);
                }
                return Ok(IntentOutcome::None);
            }
            let Some(id) = model.selected_id() else {
                return Ok(IntentOutcome::None);
            };
            open_full_task_page(domain, model, id);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SelectStep(index) => {
            // Existing steps belong to the enclosing task draft. Parking a rename before
            // selecting another row permits several step edits before one final Shift+Enter.
            // New-step add remains a focused save-and-next line and cannot be abandoned by a
            // row click.
            let editing = model.task_editing();
            if editing && !model.park_rename_step_draft() {
                return Ok(IntentOutcome::None);
            }
            let selected = model
                .form
                .as_ref()
                .filter(|form| form.is_task())
                .and_then(|form| {
                    form.task_id()
                        .map(|task_id| (task_id, &form.steps.removals))
                })
                .and_then(|(task_id, removals)| {
                    domain.get(task_id).and_then(|task| {
                        task.steps
                            .iter()
                            .enumerate()
                            .filter(|(_, step)| !removals.contains(&step.id))
                            .nth(index)
                            .map(|(source_index, step)| (source_index, step.id, step.text.clone()))
                    })
                });
            if let Some((source_index, step_id, text)) = selected {
                if let Some(form) = model.form.as_mut() {
                    form.steps.cursor = Some(source_index);
                    form.steps.add_selected = false;
                    steps_scroll_to_cursor(form, source_index);
                }
                if editing {
                    open_step_editor(model, &text, Some(step_id));
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::PageScrollTo(offset) => {
            if model.focused_surface() != FocusedSurface::Task && !model.expanded_capture_open() {
                return Ok(IntentOutcome::None);
            }
            if let Some(form) = model.form.as_mut() {
                if matches!(
                    model.input_mode,
                    BoardInputMode::TaskPage
                        | BoardInputMode::EditStep
                        | BoardInputMode::EditTitle
                        | BoardInputMode::EditNotes
                        | BoardInputMode::EditScope
                        | BoardInputMode::EditThread
                        | BoardInputMode::SelectThread
                ) {
                    form.manual_page_scroll = true;
                    let horizon = form.notes_max_scroll.get();
                    form.notes_scroll = offset.min(horizon);
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::PageWheelScrollUp | BoardIntent::PageWheelScrollDown => {
            if model.focused_surface() != FocusedSurface::Task && !model.expanded_capture_open() {
                return Ok(IntentOutcome::None);
            }
            if let Some(form) = model.form.as_mut() {
                if matches!(
                    model.input_mode,
                    BoardInputMode::TaskPage
                        | BoardInputMode::EditStep
                        | BoardInputMode::EditTitle
                        | BoardInputMode::EditNotes
                        | BoardInputMode::EditScope
                        | BoardInputMode::EditThread
                        | BoardInputMode::SelectThread
                ) {
                    form.manual_page_scroll = true;
                    let horizon = form.notes_max_scroll.get();
                    form.notes_scroll = match intent {
                        BoardIntent::PageWheelScrollUp => form.notes_scroll.saturating_sub(1),
                        BoardIntent::PageWheelScrollDown => {
                            form.notes_scroll.saturating_add(1).min(horizon)
                        }
                        _ => unreachable!("wheel intents matched above"),
                    };
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::PageScrollUp => {
            if model.focused_surface() != FocusedSurface::Task {
                return Ok(IntentOutcome::None);
            }
            // A fresh add is independent of the surrounding task edit session, so it may be
            // open directly from task view. It has no rename draft to park: keep its cursor
            // alive and scroll the same shared body that task view uses.
            if model.input_mode == BoardInputMode::EditStep && !model.park_rename_step_draft() {
                if let Some(form) = model.form.as_mut() {
                    form.notes_scroll = form.notes_scroll.saturating_sub(1);
                }
                return Ok(IntentOutcome::None);
            }
            if model.task_editing() {
                model.input_mode = BoardInputMode::TaskPage;
                if model
                    .form
                    .as_ref()
                    .is_some_and(|form| form.steps.add_selected)
                {
                    select_last_step_for_edit(model);
                } else if !move_step_within_edit_group(model, false) {
                    model.focus_form_field(CaptureField::Notes);
                }
                return Ok(IntentOutcome::None);
            }
            if let Some(form) = model.form.as_mut().filter(|form| form.is_task()) {
                if model.input_mode == BoardInputMode::TaskPage {
                    // Keyboard arrows own an active step cursor before they move the
                    // shared stream. Wheel intents above remain the explicit reading
                    // route, so an active cursor does not become inert just because
                    // notes overflow (AC-18, AC-26).
                    match form.steps.cursor {
                        Some(0) => form.steps.cursor = None,
                        Some(index) => {
                            let cursor = index - 1;
                            form.steps.cursor = Some(cursor);
                            steps_scroll_to_cursor(form, cursor);
                        }
                        None => form.notes_scroll = form.notes_scroll.saturating_sub(1),
                    }
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::PageScrollDown => {
            if model.focused_surface() != FocusedSurface::Task {
                return Ok(IntentOutcome::None);
            }
            // See Up above. This must precede `task_editing()`, because Ctrl+A and the trailing
            // add target also open a fresh editor directly from task view.
            if model.input_mode == BoardInputMode::EditStep && !model.park_rename_step_draft() {
                if let Some(form) = model.form.as_mut() {
                    let horizon = form.notes_max_scroll.get();
                    form.notes_scroll = form.notes_scroll.saturating_add(1).min(horizon);
                }
                return Ok(IntentOutcome::None);
            }
            if model.task_editing() {
                model.input_mode = BoardInputMode::TaskPage;
                if !model
                    .form
                    .as_ref()
                    .is_some_and(|form| form.steps.add_selected)
                {
                    let _ = move_step_within_edit_group(model, true);
                }
                return Ok(IntentOutcome::None);
            }
            if let Some(form) = model.form.as_mut().filter(|form| form.is_task()) {
                if model.input_mode == BoardInputMode::TaskPage {
                    let steps = form
                        .task_id()
                        .and_then(|id| domain.get(id))
                        .map(|task| task.steps.len())
                        .unwrap_or(0);
                    match form.steps.cursor {
                        Some(index) if steps > 0 => {
                            let cursor = (index + 1).min(steps - 1);
                            form.steps.cursor = Some(cursor);
                            steps_scroll_to_cursor(form, cursor);
                        }
                        None if steps > 0 && !form.steps.add_selected => {
                            form.steps.cursor = Some(0);
                            steps_scroll_to_cursor(form, 0);
                        }
                        _ => {
                            let horizon = form.notes_max_scroll.get();
                            form.notes_scroll = form.notes_scroll.saturating_add(1).min(horizon);
                        }
                    }
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::PeekDetail => {
            let Some(id) = model.selected_id() else {
                return Ok(IntentOutcome::None);
            };
            model.detail_open = Some(id);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CollapseDetail => {
            model.detail_open = None;
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ToggleDoneDrawer => {
            model.clear_marks();
            let previous_visible = model.visible_ids();
            let previous = model.selection_id;
            model.drawer_open = !model.drawer_open;
            model.reanchor_selection(previous, &previous_visible);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ToggleArchivedGroup => {
            // Enter or a click on the header: flip that one group. The drawer is already
            // open, because the header only paints inside it.
            model.clear_marks();
            let previous_visible = model.visible_ids();
            model.toggle_archived_collapsed();
            model.select_archived_header();
            model.reanchor_selection(Some(ARCHIVED_HEADER_ROW_ID), &previous_visible);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ToggleInboxGroup => {
            model.clear_marks();
            let previous_visible = model.visible_ids();
            model.toggle_inbox_collapsed();
            model.select_inbox_header();
            model.reanchor_selection(Some(INBOX_HEADER_ROW_ID), &previous_visible);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::OpenHelp => {
            let active_mode = model.input_mode();
            let return_mode = model.input_mode;
            model.close_command_surface();
            if !matches!(
                active_mode,
                BoardInputMode::ProjectPicker | BoardInputMode::LaunchCard
            ) {
                model.close_popup();
            }
            model.help_return_mode = return_mode;
            model.help_query.clear();
            model.help_scroll = 0;
            model.help_max_scroll.set(usize::MAX);
            model.input_mode = BoardInputMode::Help;
            return Ok(IntentOutcome::None);
        }
        BoardIntent::HelpQueryInsert(character) => {
            if model.input_mode == BoardInputMode::Help {
                model.help_query.push(character);
                model.help_scroll = 0;
                model.help_max_scroll.set(usize::MAX);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::HelpQueryInsertText(text) => {
            if model.input_mode == BoardInputMode::Help {
                model.help_query.push_str(&flatten_line_breaks(&text));
                model.help_scroll = 0;
                model.help_max_scroll.set(usize::MAX);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::HelpQueryBackspace => {
            if model.input_mode == BoardInputMode::Help {
                model.help_query.pop();
                model.help_scroll = 0;
                model.help_max_scroll.set(usize::MAX);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CloseHelp => {
            model.close_help();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::HelpScrollUp | BoardIntent::HelpScrollDown => {
            if model.input_mode != BoardInputMode::Help {
                return Ok(IntentOutcome::None);
            }
            // The painter records the wrapped-row horizon for the last frame. Source catalog
            // rows are not a valid second bound because one description may occupy several
            // screen rows at compact widths.
            let horizon = model.help_max_scroll.get();
            model.help_scroll = match intent {
                BoardIntent::HelpScrollUp => model.help_scroll.saturating_sub(1),
                _ => model.help_scroll.saturating_add(1).min(horizon),
            };
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CloseLayer => {
            if model.clear_marks() {
                model.clear_message();
                return Ok(IntentOutcome::None);
            }
            // Progressive close: transient surface (palette/help/dropdown) →
            // open detail → quit. SaveRecovery is not dismissible here; the save-recovery
            // gate owns Retry/Cancel.
            // A clean parked edit is not a visible layer. Check root before resetting it;
            // the application boundary has already refused any dirty root quit.
            if model.root_escape_requests_quit() {
                return Ok(IntentOutcome::Quit);
            }
            if model.surface != CommandSurface::None {
                model.close_command_surface();
                return Ok(IntentOutcome::None);
            }
            if model.input_mode == BoardInputMode::Help {
                if model.help_query.is_empty() {
                    model.close_help();
                } else {
                    model.help_query.clear();
                    model.help_scroll = 0;
                    model.help_max_scroll.set(usize::MAX);
                }
                return Ok(IntentOutcome::None);
            }
            if model.input_mode == BoardInputMode::Search {
                let previous_visible = model.visible_ids();
                let previous = model.selection_id;
                model.search_query.clear();
                model.search_pinned = false;
                model.projects_selected = 0;
                model.input_mode = model.search_return_mode;
                model.clear_message();
                if model.projects_overview() {
                    if model.projects_preview_active() {
                        model.bind_project_preview();
                    }
                } else {
                    model.reanchor_selection(previous, &previous_visible);
                }
                return Ok(IntentOutcome::None);
            }
            if model.search_pinned {
                let previous_visible = model.visible_ids();
                let previous = model.selection_id;
                model.search_query.clear();
                model.search_pinned = false;
                model.projects_selected = 0;
                model.clear_message();
                if model.projects_overview() {
                    if model.projects_preview_active() {
                        model.bind_project_preview();
                    }
                } else {
                    model.reanchor_selection(previous, &previous_visible);
                }
                return Ok(IntentOutcome::None);
            }
            if model.input_mode == BoardInputMode::FormDropdown {
                model.close_form_dropdown(false);
                return Ok(IntentOutcome::None);
            }
            // Split's task session is parked, not the active editor. Collapsing the
            // right column must keep that draft, just like the left arrow does.
            if model.task_editing()
                && !(model.wide_stage == WideStage::Split
                    && model.focused_surface() == FocusedSurface::Board)
            {
                let saved = model
                    .form
                    .as_ref()
                    .and_then(BoardForm::task_id)
                    .and_then(|id| model.tasks.iter().find(|task| task.id == id))
                    .cloned();
                if let (Some(form), Some(task)) = (model.form.as_mut(), saved) {
                    *form = BoardForm::task(
                        &task,
                        model.this_repo.as_deref(),
                        &model.tasks,
                        CaptureField::Title,
                        &model.archived_projects,
                        &model.agent_names,
                    );
                    model.input_mode = BoardInputMode::TaskPage;
                    model.clear_message();
                    return Ok(IntentOutcome::None);
                }
            }
            if model.form.as_ref().is_some_and(BoardForm::is_task)
                && model.focused_surface() == FocusedSurface::Task
            {
                leave_task_page(model);
                return Ok(IntentOutcome::None);
            }
            if model.focused_surface() == FocusedSurface::Board
                && model.form.as_ref().is_some_and(BoardForm::is_task)
            {
                // A parked page is not a visible layer on the board: Esc falls through to
                // the board's own close order instead of silently dropping the session.
            } else if model.form.take().is_some() {
                model.input_mode = if model.quick_add.is_some() {
                    BoardInputMode::QuickAdd
                } else {
                    BoardInputMode::Normal
                };
                model.clear_message();
                return Ok(IntentOutcome::None);
            }
            if model.quick_add.take().is_some() {
                model.input_mode = BoardInputMode::Normal;
                model.clear_message();
                return Ok(IntentOutcome::None);
            }
            let before = model.popup;
            model.close_popup();
            if model.popup != before {
                return Ok(IntentOutcome::None);
            }
            if model.detail_open.is_some() {
                model.detail_open = None;
                return Ok(IntentOutcome::None);
            }
            // After visible layers, a split is the next layer to close. Use the shared
            // transition so task drafts stay parked and dirty project previews refuse.
            if model.frame_wide() && model.wide_stage == WideStage::Split {
                stage_left(model);
                return Ok(IntentOutcome::None);
            }
            // AC-45: with no layer above it, Esc leaves the read-only archived focus for
            // the desk, which hides that project's tasks again. It never quits from there.
            if model.focus_is_archived() {
                model.leave_archived_focus();
                return Ok(IntentOutcome::None);
            }
            return Ok(IntentOutcome::None);
        }
        // The five intents below aim at the selection, and an empty board has none. Each
        // says so rather than returning to a row that has just been cleared for an action
        // that then did nothing: a silent no-op is the failure the row exists to prevent.
        BoardIntent::SetStatus(status) => {
            let (targets, _) = take_verb_targets(model);
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            if !set_status_batch(domain, &targets, status)? {
                model.close_popup();
                return Ok(IntentOutcome::None);
            }
            model.close_popup();
        }
        BoardIntent::Complete => {
            model.close_popup();
            // Task-level, even with a step selected (Enter owns the step).
            let (targets, bulk) = take_verb_targets(model);
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            if targets.iter().all(|id| {
                domain
                    .get(*id)
                    .is_some_and(|task| task.status == HumanStatus::Done)
            }) {
                return Ok(IntentOutcome::None);
            }
            if bulk {
                domain.complete_batch(&targets)?;
            } else {
                domain.complete(targets[0])?;
            }
        }
        BoardIntent::Reopen => {
            model.close_popup();
            let (targets, _) = take_verb_targets(model);
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            if !reopen_batch(domain, &targets)? {
                return Ok(IntentOutcome::None);
            }
        }
        BoardIntent::SoftDelete => {
            model.close_popup();
            // On the page with the step cursor active, the delete verb is the
            // steps's mark-then-confirm: the first press visibly marks the
            // highlighted step, a second press removes it, and the task-level soft
            // delete below never runs.
            match page_step_delete(domain, model)? {
                PageStepDelete::Marked | PageStepDelete::Staged => {
                    model.clear_marks();
                    return Ok(IntentOutcome::None);
                }
                PageStepDelete::Removed => {
                    model.clear_marks();
                }
                PageStepDelete::NotApplicable => {
                    let pending = model.pending_delete.clone();
                    let bulk = if pending.is_some() {
                        model.pending_delete_bulk
                    } else {
                        model.task_list_owns_input()
                            && model.mark_mode_active()
                            && model.marked_count() > 0
                    };
                    let targets = pending
                        .as_ref()
                        .map(|targets| targets.iter().copied().collect())
                        .unwrap_or_else(|| model.verb_target_ids());
                    if targets.is_empty() {
                        model.set_message(NO_SELECTION);
                        return Ok(IntentOutcome::None);
                    }
                    if pending.is_none() {
                        model.pending_delete = Some(targets.iter().copied().collect());
                        model.pending_delete_bulk = bulk;
                        model.clear_marks();
                        if bulk {
                            let noun = if targets.len() == 1 { "task" } else { "tasks" };
                            model.set_message(format!(
                                "press ctrl+x again to delete {} {noun}",
                                targets.len()
                            ));
                        } else {
                            model.set_message("press ctrl+x again to delete");
                        }
                        return Ok(IntentOutcome::None);
                    }
                    model.pending_delete = None;
                    model.pending_delete_bulk = false;
                    model.clear_marks();
                    // Read the title before the delete, and only arm the notice once the delete
                    // itself succeeded: a refused delete has nothing to recover from.
                    let title = targets
                        .first()
                        .and_then(|id| domain.get(*id))
                        .map(|task| task.title.clone());
                    if bulk {
                        domain.soft_delete_batch(&targets)?;
                        model.arm_bulk_delete_notice(targets.len());
                    } else {
                        let id = targets[0];
                        domain.soft_delete(id)?;
                        if let Some(title) = title {
                            model.arm_delete_notice(&title);
                        }
                    }
                    // Deleting from the page deletes the page's own task: the surface closes and
                    // the undo route back to it lives on the board row, same as the notice says.
                    // A task-owned stage (G or F) hands the slider back to the board the same
                    // way `Esc` would, so the arrows keep answering on the row with the undo.
                    if !bulk
                        && model
                            .form
                            .as_ref()
                            .filter(|form| form.is_task())
                            .and_then(BoardForm::task_id)
                            == Some(targets[0])
                    {
                        model.form = None;
                        model.input_mode = BoardInputMode::Normal;
                        if matches!(model.wide_stage, WideStage::Rail | WideStage::FullTask) {
                            // Normal mode is board-owned, so the stage must be too: back to
                            // the full board when F was entered from it, otherwise to A.
                            model.wide_stage = match model.stage_origin.take() {
                                Some(WideStage::FullBoard) => WideStage::FullBoard,
                                _ => WideStage::Split,
                            };
                        }
                    }
                }
            }
        }
        BoardIntent::LaunchUnarchive => {
            let Some(path) = model.launch_card.clone() else {
                return Ok(IntentOutcome::None);
            };
            let scope_path = path.to_string_lossy().into_owned();
            domain.unarchive_project(&scope_path)?;
            // A failed save lands in Save Recovery via the existing boundary; the session
            // default stays the project until the durable write lands.
            model.launch_card = None;
            model.popup = BoardPopup::None;
            model.clear_message();
        }
        BoardIntent::LaunchKeepArchived => {
            let Some(path) = model.launch_card.take() else {
                return Ok(IntentOutcome::None);
            };
            model.popup = BoardPopup::None;
            model.session_default_scope = Some(TaskScope::Global);
            let lossy = path.to_string_lossy().into_owned();
            let name = crate::ui::render::short_project(&lossy);
            model.set_message(format!(
                "project {name} is archived · quick-add goes to your desk this session"
            ));
            // Session-only change: nothing durable to write.
            return Ok(IntentOutcome::None);
        }
        BoardIntent::File => {
            // Picker open: archive the selected main-tab project, or unarchive the
            // selected archived-tab entry. The picker stays open either way.
            if model.project_picker.is_some() {
                model.clear_marks();
            }
            if let Some(picker) = model.project_picker.as_ref() {
                let tab = picker.tab;
                let chosen = match tab {
                    PickerTab::Main => picker.options.get(picker.selected).cloned(),
                    PickerTab::Archived => picker
                        .archived
                        .get(picker.archived_selected)
                        .map(|path| ProjectScopeOption::Project(path.clone())),
                };
                let Some(ProjectScopeOption::Project(path)) = chosen else {
                    // Main + Home (or an empty tab) is inert.
                    return Ok(IntentOutcome::None);
                };
                let scope_path = path.to_string_lossy().into_owned();
                let previous_visible = model.visible_ids();
                let previous = model.selection_id;
                let result = match tab {
                    PickerTab::Main => domain.archive_project(&scope_path),
                    PickerTab::Archived => domain.unarchive_project(&scope_path).map(|_| true),
                };
                if let Err(error) = result {
                    // Unknown project and friends leave the picker untouched and paint
                    // the refusal on the status slot, per the status-slot rule.
                    if error == DomainError::UnknownProject(scope_path.clone()) {
                        let short = crate::ui::render::short_project(&scope_path);
                        model.set_message(format!("nothing to archive in {short}"));
                    } else {
                        model.set_message(error.to_string());
                    }
                    return Ok(IntentOutcome::None);
                }
                // `sync_from_domain` resets a focus that now points at an archived project.
                model.sync_from_domain(domain);
                model.refresh_project_picker();
                model.reanchor_selection(previous, &previous_visible);
                return Ok(IntentOutcome::Persist);
            }
            model.close_popup();
            let (targets, _) = take_verb_targets(model);
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            if !file_batch(domain, &targets)? {
                return Ok(IntentOutcome::None);
            }
            // Success has no message: disappearing rows are the feedback.
        }
        BoardIntent::Undo => {
            model.clear_marks();
            // Picker open: the archived tab's ctrl+u is the unarchive route; the main
            // tab stays inert (undo exactly as before the feature).
            if let Some(picker) = model.project_picker.as_ref() {
                if picker.tab == PickerTab::Main {
                    return Ok(IntentOutcome::None);
                }
                let Some(path) = picker.archived.get(picker.archived_selected).cloned() else {
                    return Ok(IntentOutcome::None);
                };
                let scope_path = path.to_string_lossy().into_owned();
                if domain.unarchive_project(&scope_path).is_err() {
                    return Ok(IntentOutcome::None);
                }
                model.sync_from_domain(domain);
                model.refresh_project_picker();
                return Ok(IntentOutcome::Persist);
            }
            // AC-43: in the read-only archived focus ctrl+u unarchives that project in
            // place and the focus becomes a normal project focus.
            if let Some(path) = model.archived_focus().map(std::path::Path::to_path_buf) {
                let scope_path = path.to_string_lossy().into_owned();
                if domain.unarchive_project(&scope_path).is_err() {
                    return Ok(IntentOutcome::None);
                }
                model.sync_from_domain(domain);
                model.enter_project_focus(path);
                model.clear_message();
                return Ok(IntentOutcome::Persist);
            }
            model.close_popup();
            // ctrl+u on an archived selection is the unarchive route, never an undo:
            // the stack is not popped (AC-3).
            if let Some(id) = model.selected_id() {
                if domain.get(id).is_some_and(|task| task.archived) {
                    domain.unarchive_task(id)?;
                    model.sync_from_domain(domain);
                    return Ok(IntentOutcome::Persist);
                }
            }
            if let Err(error) = domain.undo() {
                if let DomainError::StaleUndo(id) = error {
                    model.sync_from_domain(domain);
                    // Nothing moved, so this reports nothing to persist -- which is what
                    // puts the way back on the row beside the refusal that explains why
                    // this attempt did not take. The refusal is stated the
                    // way this row can be read when it clips, not the way the domain logs
                    // it; the error itself is unchanged.
                    model.set_message(stale_undo_message(id));
                    return Ok(IntentOutcome::None);
                }
                return Err(error);
            }
        }
    }

    model.sync_from_domain(domain);
    Ok(IntentOutcome::Persist)
}

/// Apply one draft operation, but only while a field edit is actually open.
///
/// Every editing intent is inert in every other mode, exactly as the insert and backspace
/// intents already were before the cursor arrived.
fn edit_draft(model: &mut BoardModel, operation: impl FnOnce(&mut EditBuffer)) {
    if model.input_mode == BoardInputMode::FormDropdown {
        return;
    }
    // The steps step editor owns the keyboard in its mode: its draft is the page
    // form's steps editor buffer, not the task form's title/notes fields. Any
    // edit-draft intent takes the line's refusal down (AC-13) — including a cursor
    // move that changes no text — the same lifetime quick-add's message follows.
    if model.input_mode == BoardInputMode::EditStep {
        if let Some(editor) = model
            .form
            .as_mut()
            .and_then(|form| form.steps.editor.as_mut())
        {
            editor.refusal = None;
            operation(&mut editor.buffer);
        }
        return;
    }
    let Some(form) = model.form.as_mut() else {
        return;
    };
    form.manual_page_scroll = false;
    match form.focus {
        CaptureField::Title => operation(&mut form.title),
        CaptureField::Notes => operation(&mut form.notes),
        CaptureField::Thread => {
            form.thread_refusal = None;
            operation(&mut form.thread);
        }
        CaptureField::Scope | CaptureField::Assignee => {}
    }
}

/// Move the slider one stage to the right. Stages A, G and F need a selected task; the
/// pane binds (or rebinds) its page to the selection on the way into G.
fn stage_right(domain: &DomainState, model: &mut BoardModel) {
    // A nested project board is painted in the outer task column, so its ordinary FullBoard
    // stage is exposed to input as a narrow Rail. Keep that translation local: a right arrow
    // opens the nested task page, never an unpainted intermediate Split stage.
    if model.preview_seat {
        if model.wide_stage != WideStage::FullTask {
            if let Some(id) = model.selected_id().filter(|id| domain.get(*id).is_some()) {
                open_full_task_page(domain, model, id);
            }
        }
        return;
    }
    if model.projects_overview() {
        match model.wide_stage {
            WideStage::FullBoard | WideStage::Split => {
                if model.bind_project_preview() {
                    model.wide_stage = match model.wide_stage {
                        WideStage::FullBoard => WideStage::Split,
                        WideStage::Split => WideStage::Rail,
                        WideStage::Rail | WideStage::FullTask => unreachable!(),
                    };
                }
            }
            WideStage::Rail | WideStage::FullTask => {}
        }
        return;
    }
    match model.wide_stage {
        WideStage::FullBoard => {
            if model.selected_id().is_some() {
                model.wide_stage = WideStage::Split;
            }
        }
        WideStage::Split => {
            if model.selected_id().is_some() {
                bind_selected_task_page(domain, model);
                model.wide_stage = WideStage::Rail;
            }
        }
        WideStage::Rail => {
            // A disk merge can remove the task G is showing; the parked form still pins it
            // as the selection, so ask the domain, not the pin. F needs a live task.
            if model
                .selected_id()
                .is_some_and(|id| domain.get(id).is_some())
            {
                model.stage_origin = Some(WideStage::Rail);
                model.wide_stage = WideStage::FullTask;
            }
        }
        WideStage::FullTask => {}
    }
}

/// Move the slider one stage to the left. The page session is parked, never dropped: G → A
/// keeps the pane bound to the same task (a dirty draft included), and F always returns to G.
fn stage_left(model: &mut BoardModel) {
    // The nested board has no painted intermediate stages. Its page-owned left arrow returns to
    // the preview board; the outer app handles the board-owned left arrow and returns to the
    // projects index.
    if model.preview_seat {
        if model.wide_stage == WideStage::FullTask {
            leave_task_page(model);
        } else {
            model.wide_stage = WideStage::FullBoard;
            model.stage_origin = None;
        }
        return;
    }
    if model.projects_overview() {
        match model.wide_stage {
            WideStage::FullBoard => {}
            WideStage::Split => {
                if model
                    .right_seat()
                    .is_some_and(|right| right.has_unsaved_work())
                {
                    model.set_message(DIRTY_TASK_SWITCH_REFUSAL);
                } else {
                    model.wide_stage = WideStage::FullBoard;
                    model.drop_project_preview();
                }
            }
            WideStage::Rail => {
                model.clear_project_preview_ephemeral_state();
                model.wide_stage = WideStage::Split;
            }
            WideStage::FullTask => {
                model.stage_origin = None;
                model.wide_stage = WideStage::FullBoard;
                model.drop_project_preview();
            }
        }
        return;
    }
    match model.wide_stage {
        WideStage::FullBoard => {}
        WideStage::Split => model.wide_stage = WideStage::FullBoard,
        WideStage::Rail => model.wide_stage = WideStage::Split,
        WideStage::FullTask => {
            model.stage_origin = None;
            model.wide_stage = WideStage::Rail;
        }
    }
}

/// Enter the task-owned stage nearest to the current board-owned one, when a task-side
/// action (a field edit from the board) needs the task to own input.
fn enter_task_stage(model: &mut BoardModel) {
    match model.wide_stage {
        WideStage::FullBoard => {
            model.stage_origin = Some(WideStage::FullBoard);
            model.wide_stage = WideStage::FullTask;
        }
        WideStage::Split => model.wide_stage = WideStage::Rail,
        WideStage::Rail | WideStage::FullTask => {}
    }
}

/// Close the task page from a task-owned stage. F returns to the stage `Enter` left; G
/// returns to A. A page that returns to the bare board is closed, one that returns to a
/// pane-bearing stage stays parked so its scroll and step cursor survive.
fn leave_task_page(model: &mut BoardModel) {
    let target = match model.wide_stage {
        WideStage::FullTask => model.stage_origin.take().unwrap_or(WideStage::FullBoard),
        WideStage::Rail => WideStage::Split,
        stage @ (WideStage::Split | WideStage::FullBoard) => stage,
    };
    model.stage_origin = None;
    model.wide_stage = target;
    if target == WideStage::FullBoard {
        model.form = None;
        model.input_mode = if model.quick_add.is_some() {
            BoardInputMode::QuickAdd
        } else {
            BoardInputMode::Normal
        };
    }
    model.clear_message();
}

/// Make sure the task page is bound to the selected task before a task-owned stage paints it.
fn bind_selected_task_page(domain: &DomainState, model: &mut BoardModel) {
    // `BoardModel::input_mode()` reports Normal while a task form is parked, so every
    // refocus route must restore the raw TaskPage mode before task input can dispatch.
    let Some(id) = model.selected_id() else {
        return;
    };
    if model.edit_target() != Some(id) || model.input_mode != BoardInputMode::TaskPage {
        open_task_page_on(domain, model, id);
    }
}

/// Open the full task page (stage F) on `id`, remembering the stage it left. Shared by the
/// keyboard route (`Enter`) and the mouse route (a row double-click).
fn open_full_task_page(domain: &DomainState, model: &mut BoardModel, id: Uuid) {
    if domain.get(id).is_none() {
        return;
    }
    if model.wide_stage != WideStage::FullTask {
        model.stage_origin = Some(model.wide_stage);
    }
    if model.edit_target() != Some(id) || model.input_mode != BoardInputMode::TaskPage {
        open_task_page_on(domain, model, id);
    }
    model.wide_stage = WideStage::FullTask;
}

fn open_task_page_on(domain: &DomainState, model: &mut BoardModel, id: Uuid) {
    let Some(task) = domain.get(id) else {
        return;
    };
    model.close_popup();
    model.close_command_surface();
    model.close_help();
    // The page replaces the peek: both would otherwise describe the same task twice.
    model.detail_open = None;
    let form = BoardForm::task(
        task,
        model.this_repo.as_deref(),
        &model.tasks,
        CaptureField::Title,
        &model.archived_projects,
        &model.agent_names,
    );
    model.form = Some(form);
    model.input_mode = BoardInputMode::TaskPage;
    model.clear_message();
}

/// The row double-click window: a second click on the same row within it opens the page.
pub(super) const ROW_DOUBLE_CLICK_WINDOW: std::time::Duration =
    std::time::Duration::from_millis(400);

/// Keep the quick-add line's visible destination truthful while `!p` tokens are typed.
///
/// The row names where Enter will save (`Add to desk` / `Add to <project>`), so every
/// buffer change re-lifts the tokens: an override applies the moment it is typed and
/// reverts the moment it is deleted. An invalid `!p` leaves the last good destination
/// painted; the save itself refuses with the same words.
fn refresh_quick_add_scope(model: &mut BoardModel, domain: &DomainState) {
    let Some(quick_add) = model.quick_add.as_ref() else {
        return;
    };
    let default = quick_add.default.clone();
    let lifted = lift_quick_add_tokens(
        quick_add.title.value(),
        domain,
        quick_add.snapshot.as_ref().as_ref(),
        &model.agent_names,
    );
    let Ok(lifted) = lifted else {
        return;
    };
    if let Some(quick_add) = model.quick_add.as_mut() {
        quick_add.scope = lifted.scope.unwrap_or(default);
    }
}

/// Save the line through the same capture pipeline the expanded form uses.
fn quick_add_save(
    domain: &mut DomainState,
    model: &mut BoardModel,
    keep_open: bool,
) -> Result<IntentOutcome, DomainError> {
    let Some(quick_add) = model.quick_add.as_ref() else {
        return Ok(IntentOutcome::None);
    };
    let Some(snapshot) = quick_add.snapshot.as_ref().clone() else {
        model.set_message("capture context unavailable; press Esc and try again");
        return Ok(IntentOutcome::None);
    };
    let lifted = match lift_quick_add_tokens(
        quick_add.title.value(),
        domain,
        quick_add.snapshot.as_ref().as_ref(),
        &model.agent_names,
    ) {
        Ok(lifted) => lifted,
        Err(message) => {
            model.set_message(message);
            return Ok(IntentOutcome::None);
        }
    };
    let scope = lifted.scope.unwrap_or_else(|| quick_add.scope.clone());
    match crate::capture::capture_save_assigned(
        domain,
        None,
        &snapshot,
        lifted.title,
        None,
        Some(scope.clone()),
        lifted.thread,
        lifted.assignee,
    ) {
        Ok(id) => {
            // Do not discard the draft until the app save boundary confirms persistence. A
            // failed save keeps this exact state behind SaveRecovery for retry or cancel.
            model.form = None;
            model.begin_quick_add_save(id, keep_open);
            Ok(IntentOutcome::Persist)
        }
        Err(crate::capture::CaptureError::Domain(DomainError::EmptyTitle)) => {
            model.set_message(TITLE_REQUIRED_MESSAGE);
            Ok(IntentOutcome::None)
        }
        Err(error) => {
            model.set_message(error.to_string());
            Ok(IntentOutcome::None)
        }
    }
}

/// Directives lifted from a quick-add title before capture.
///
/// This parser is deliberately private to quick-add. Title, notes, and checklist editors retain
/// their literal text, while the status-row capture can apply scope and thread together.
struct QuickAddTokens {
    title: String,
    scope: Option<TaskScope>,
    thread: Option<String>,
    assignee: Option<String>,
}

/// Lift whitespace-delimited `!p` and `!t` directives in either order.
///
/// A directive consumes only its immediate non-directive argument. Parsing completes before any
/// value is returned, so a malformed thread cannot partially apply a preceding scope override.
fn lift_quick_add_tokens(
    value: &str,
    domain: &DomainState,
    snapshot: Option<&InvocationSnapshot>,
    agent_names: &[String],
) -> Result<QuickAddTokens, String> {
    let words: Vec<&str> = value.split_whitespace().collect();
    let mut title = Vec::new();
    let mut scope = None;
    let mut thread = None;
    let mut assignee = None;
    let mut index = 0;

    while let Some(word) = words.get(index) {
        match *word {
            "!p" => {
                let argument = quick_add_token_argument(&words, index);
                scope = Some(match argument {
                    Some(path) => {
                        let resolved = crate::scope::resolve_project_path(path, domain, snapshot)
                            .map_err(|error| error.message(path))?;
                        if domain.is_project_archived(&resolved) {
                            return Err(format!(
                                "project {} is archived",
                                crate::ui::render::short_project(&resolved)
                            ));
                        }
                        TaskScope::Project { path: resolved }
                    }
                    None => TaskScope::Global,
                });
                index += usize::from(argument.is_some()) + 1;
            }
            "!t" => {
                let argument = quick_add_token_argument(&words, index);
                thread = match argument {
                    Some(name) => Some(normalize_thread(name).map_err(thread_refusal_message)?),
                    None => None,
                };
                index += usize::from(argument.is_some()) + 1;
            }
            "!a" => {
                let argument = quick_add_token_argument(&words, index);
                assignee = match argument {
                    Some(name) => {
                        let normalized = normalize_thread(name).map_err(|error| {
                            thread_refusal_message(error).replacen("thread", "agent name", 1)
                        })?;
                        if agent_names.iter().any(|name| name == &normalized) {
                            Some(normalized)
                        } else {
                            return Err(format!("unknown agent {normalized}"));
                        }
                    }
                    None => None,
                };
                index += usize::from(argument.is_some()) + 1;
            }
            _ => {
                title.push(*word);
                index += 1;
            }
        }
    }

    Ok(QuickAddTokens {
        title: title.join(" "),
        scope,
        thread,
        assignee,
    })
}

/// A directive consumes one argument only when the next word is neither another directive nor
/// a literal `#` title word.
fn quick_add_token_argument<'a>(words: &'a [&str], index: usize) -> Option<&'a str> {
    words
        .get(index + 1)
        .copied()
        .filter(|word| *word != "!p" && *word != "!t" && *word != "!a" && !word.starts_with('#'))
}

fn normalize_optional_thread(value: &str) -> Result<Option<String>, ThreadError> {
    let value = value.trim();
    if value.is_empty() {
        Ok(None)
    } else {
        normalize_thread(value).map(Some)
    }
}

fn confirm_edit(
    domain: &mut DomainState,
    model: &mut BoardModel,
    extra_step: Option<&str>,
) -> Result<IntentOutcome, DomainError> {
    // The task bound at open, not `model.selected_id()`: refresh may move the visible pin, but
    // it never changes the form's immutable id or any of its three drafts.
    let Some(form) = model.form.as_ref().filter(|form| form.is_task()) else {
        return Ok(IntentOutcome::None);
    };
    let id = form.task_id().expect("task form has immutable id");
    let title = form.title.value().trim().to_string();
    let notes = (!form.notes.value().trim().is_empty()).then(|| form.notes.value().to_string());
    let scope = form.scope.clone();
    let assignee = form.assignee.clone();
    let thread = match normalize_optional_thread(form.thread.value()) {
        Ok(thread) => thread,
        Err(error) => {
            if let Some(form) = model.form.as_mut() {
                form.thread_refusal = Some(thread_refusal_message(error));
            }
            return Ok(IntentOutcome::None);
        }
    };
    let step_renames = form
        .steps
        .drafts
        .iter()
        .filter(|(step_id, _)| !form.steps.removals.contains(step_id))
        .map(|(step_id, draft)| (*step_id, draft.value().trim().to_string()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let step_removals = form.steps.removals.clone();
    if step_renames.values().any(String::is_empty) {
        model.set_message(STEP_TEXT_REQUIRED);
        return Ok(IntentOutcome::None);
    }
    let task = domain.get(id).ok_or(DomainError::UnknownId(id))?.clone();
    let selected_step = form
        .steps
        .cursor
        .and_then(|source_index| task.steps.get(source_index))
        .map(|step| step.id);
    // `DomainState::edit` does not reject a soft-deleted task itself. Refuse before touching
    // the form so's bound-task and draft-recovery guarantees remain intact.
    if task.soft_deleted {
        return Err(DomainError::SoftDeleted(id));
    }

    // The full page session changes under one revision, then the app persists exactly once.
    // A chained `edit` plus `rename_step` sequence would advance the merge base after each
    // draft and make the store correctly refuse the later revision as a conflicting writer.
    let step_rename_list = step_renames
        .iter()
        .map(|(step_id, text)| (*step_id, text.clone()))
        .collect::<Vec<_>>();
    let extra_steps = extra_step
        .map(|text| vec![text.trim().to_string()])
        .unwrap_or_default();
    domain.edit_with_step_changes(
        id,
        &title,
        notes.clone(),
        scope.clone(),
        thread.clone(),
        assignee.clone(),
        &step_rename_list,
        &step_removals.iter().copied().collect::<Vec<_>>(),
        &extra_steps,
    )?;

    // Retain the complete form and mode until the persistence boundary confirms this exact
    // task-session save. A failed save can then Retry or Cancel without orphaning drafts.
    model.task_edit_save = Some(TaskEditSave {
        id,
        title,
        notes,
        scope,
        thread,
        assignee,
        step_renames,
        step_removals,
        selected_step,
    });
    Ok(IntentOutcome::Persist)
}

// ---------------------------------------------------------------------------
// Steps step cursor, verbs, and one-line editor (T-3)
// ---------------------------------------------------------------------------

/// Move from the form's end fields into its existing-step group or trailing add target.
fn select_step_from_tab(model: &mut BoardModel, forward: bool) {
    let target = model.form.as_ref().and_then(|form| {
        if !form.is_task()
            || !form.editing
            || (forward && form.focus != CaptureField::Notes)
            || (!forward && form.focus != CaptureField::Thread)
        {
            None
        } else {
            let task_id = form.task_id()?;
            let visible: Vec<usize> = model
                .tasks
                .iter()
                .find(|task| task.id == task_id)?
                .steps
                .iter()
                .enumerate()
                .filter(|(_, step)| !form.steps.removals.contains(&step.id))
                .map(|(source_index, _)| source_index)
                .collect();
            Some(if forward {
                visible.first().copied()
            } else {
                visible.last().copied()
            })
        }
    });
    let Some(target) = target else {
        return;
    };
    let Some(index) = target else {
        select_add_step(model);
        return;
    };
    let text = model.form.as_ref().and_then(|form| {
        form.task_id().and_then(|task_id| {
            model
                .tasks
                .iter()
                .find(|task| task.id == task_id)
                .and_then(|task| {
                    task.steps
                        .get(index)
                        .map(|step| (step.id, step.text.clone()))
                })
        })
    });
    if let Some((step_id, text)) = text {
        if let Some(form) = model.form.as_mut() {
            form.steps.cursor = Some(index);
            form.steps.add_selected = false;
            steps_scroll_to_cursor(form, index);
        }
        open_step_editor(model, &text, Some(step_id));
    }
}

/// Start a task-page Tab cycle on its first stored step, or its trailing add target when
/// the checklist is empty, without entering the task edit session.
fn select_first_step_from_page(model: &mut BoardModel) -> bool {
    let count = model.form.as_ref().and_then(|form| {
        let task_id = form.task_id()?;
        model
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .map(|task| task.steps.len())
    });
    let Some(count) = count else {
        return false;
    };
    if count == 0 {
        select_add_step(model);
    } else if let Some(form) = model.form.as_mut().filter(|form| form.is_task()) {
        form.steps.cursor = Some(0);
        form.steps.add_selected = false;
        steps_scroll_to_cursor(form, 0);
    }
    true
}

/// Cycle view-mode Tab through every stored step and then its trailing add target.
fn move_step_with_tab(model: &mut BoardModel, forward: bool) -> bool {
    let state = model.form.as_ref().and_then(|form| {
        if !form.is_task() {
            return None;
        }
        let task_id = form.task_id()?;
        let count = model
            .tasks
            .iter()
            .find(|task| task.id == task_id)?
            .steps
            .len();
        Some((form.steps.cursor, form.steps.add_selected, count))
    });
    let Some((cursor, add_selected, count)) = state else {
        return false;
    };
    if add_selected {
        if count == 0 {
            return true;
        }
        let index = if forward { 0 } else { count - 1 };
        let form = model.form.as_mut().expect("task form remains open");
        form.steps.cursor = Some(index);
        form.steps.add_selected = false;
        steps_scroll_to_cursor(form, index);
        return true;
    }
    let Some(index) = cursor else {
        return false;
    };
    if (forward && index + 1 == count) || (!forward && index == 0) {
        select_add_step(model);
    } else {
        let next = if forward { index + 1 } else { index - 1 };
        let form = model.form.as_mut().expect("task form remains open");
        form.steps.cursor = Some(next);
        form.steps.add_selected = false;
        steps_scroll_to_cursor(form, next);
    }
    true
}

/// Move within stored task-edit steps without wrapping, opening the reached row inline.
fn move_step_within_edit_group(model: &mut BoardModel, forward: bool) -> bool {
    let target = model.form.as_ref().and_then(|form| {
        if !form.is_task() || !form.editing {
            return None;
        }
        let index = form.steps.cursor?;
        let task_id = form.task_id()?;
        let task = model.tasks.iter().find(|task| task.id == task_id)?;
        let visible: Vec<usize> = task
            .steps
            .iter()
            .enumerate()
            .filter(|(_, step)| !form.steps.removals.contains(&step.id))
            .map(|(source_index, _)| source_index)
            .collect();
        let position = visible
            .iter()
            .position(|source_index| *source_index == index)?;
        let target = if forward {
            *visible.get(position + 1)?
        } else {
            *visible.get(position.checked_sub(1)?)?
        };
        task.steps
            .get(target)
            .map(|step| (target, step.id, step.text.clone()))
    });
    let Some((target, step_id, text)) = target else {
        return false;
    };
    if let Some(form) = model.form.as_mut() {
        form.steps.cursor = Some(target);
        form.steps.add_selected = false;
        steps_scroll_to_cursor(form, target);
    }
    open_step_editor(model, &text, Some(step_id));
    true
}

fn select_add_step(model: &mut BoardModel) {
    if let Some(form) = model.form.as_mut() {
        form.steps.cursor = None;
        form.steps.add_selected = true;
        model.input_mode = if form.is_task() {
            BoardInputMode::TaskPage
        } else {
            BoardInputMode::CapturePage
        };
    }
}

/// Expanded capture stages new steps locally, so its ring selects those rows without trying to
/// resolve task-backed step identities that do not exist until the capture is saved.
fn select_first_capture_step_or_add(model: &mut BoardModel) {
    let count = model
        .form
        .as_ref()
        .filter(|form| !form.is_task())
        .map(|form| form.steps.pending_adds.len())
        .unwrap_or(0);
    if count == 0 {
        select_add_step(model);
    } else if let Some(form) = model.form.as_mut() {
        form.steps.cursor = Some(0);
        form.steps.add_selected = false;
        model.input_mode = BoardInputMode::CapturePage;
    }
}

fn select_last_capture_step(model: &mut BoardModel) -> bool {
    let count = model
        .form
        .as_ref()
        .filter(|form| !form.is_task())
        .map(|form| form.steps.pending_adds.len())
        .unwrap_or(0);
    let Some(index) = count.checked_sub(1) else {
        return false;
    };
    if let Some(form) = model.form.as_mut() {
        form.steps.cursor = Some(index);
        form.steps.add_selected = false;
        model.input_mode = BoardInputMode::CapturePage;
    }
    true
}

/// Move among locally staged capture steps without wrapping. The caller handles the field at
/// either end of the group, matching task-page step traversal.
fn move_capture_step_with_tab(model: &mut BoardModel, forward: bool) -> bool {
    let state = model.form.as_ref().and_then(|form| {
        (!form.is_task()).then_some((form.steps.cursor, form.steps.pending_adds.len()))
    });
    let Some((Some(index), count)) = state else {
        return false;
    };
    if forward && index + 1 == count {
        select_add_step(model);
        return true;
    }
    if !forward && index == 0 {
        return false;
    }
    let next = if forward { index + 1 } else { index - 1 };
    if let Some(form) = model.form.as_mut() {
        form.steps.cursor = Some(next);
        form.steps.add_selected = false;
        model.input_mode = BoardInputMode::CapturePage;
    }
    true
}

fn select_last_step_for_edit(model: &mut BoardModel) {
    let target = model.form.as_ref().and_then(|form| {
        let task = model
            .tasks
            .iter()
            .find(|task| Some(task.id) == form.task_id())?;
        task.steps
            .iter()
            .enumerate()
            .rev()
            .find(|(_, step)| !form.steps.removals.contains(&step.id))
            .map(|(source_index, step)| (source_index, step.id, step.text.clone()))
    });
    let Some((index, step_id, text)) = target else {
        model.focus_form_field(CaptureField::Notes);
        return;
    };
    if let Some(form) = model.form.as_mut() {
        form.steps.cursor = Some(index);
        form.steps.add_selected = false;
        steps_scroll_to_cursor(form, index);
    }
    open_step_editor(model, &text, Some(step_id));
}

/// Resolve the page's selected step to live task and step ids, declining a stale index after
/// a domain change. Task view and task edit sessions share this selection.
fn selected_step(domain: &DomainState, model: &BoardModel) -> Option<(Uuid, Uuid)> {
    if model.focused_surface() != FocusedSurface::Task
        || !matches!(
            model.input_mode,
            BoardInputMode::TaskPage | BoardInputMode::EditStep
        )
    {
        return None;
    }
    let form = model.form.as_ref().filter(|form| form.is_task())?;
    if form.steps.add_selected {
        return None;
    }
    let task_id = form.task_id()?;
    let index = form.steps.cursor?;
    let step_id = domain.get(task_id)?.steps.get(index).map(|step| step.id)?;
    (!form.steps.removals.contains(&step_id)).then_some((task_id, step_id))
}

/// Keep the selected step in the renderer-recorded shared content viewport.
fn steps_scroll_to_cursor(form: &mut BoardForm, cursor: usize) {
    let counts = form.steps.row_counts.borrow();
    let wrapped_before: usize = if counts.len() >= cursor {
        counts.iter().take(cursor).copied().sum()
    } else {
        cursor
    };
    let target = form
        .steps
        .content_start
        .get()
        .saturating_add(1)
        .saturating_add(wrapped_before);
    let rows = form.steps.window_rows.get().max(1);
    if target < form.notes_scroll {
        form.notes_scroll = target;
    } else if target >= form.notes_scroll.saturating_add(rows) {
        form.notes_scroll = target + 1 - rows;
    }
    form.notes_scroll = form.notes_scroll.min(form.notes_max_scroll.get());
}

/// Outcome of routing the delete verb through the page's step cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageStepDelete {
    /// The cursor is not active on the page: the verb is the task soft delete.
    NotApplicable,
    /// The edit session owns this removal until its final save.
    Staged,
    /// First view-mode press: the cursor's step is visibly marked; nothing was removed.
    Marked,
    /// Second view-mode press: the marked step was removed through the domain command.
    Removed,
}

/// Mark-then-confirm delete for the step under the page's cursor (AC-11). Any
/// intervening intent already cleared the mark in [`apply_intent`], so a mark found
/// here equal to the cursor's step can only be this verb's own first press.
fn page_step_delete(
    domain: &mut DomainState,
    model: &mut BoardModel,
) -> Result<PageStepDelete, DomainError> {
    let Some((task_id, step_id)) = selected_step(domain, model) else {
        return Ok(PageStepDelete::NotApplicable);
    };
    let form = model
        .form
        .as_ref()
        .filter(|form| form.is_task())
        .expect("selected_step checked a task form");
    let index = form
        .steps
        .cursor
        .expect("selected_step checked an active cursor");
    if model.task_editing() {
        let task = domain
            .get(task_id)
            .expect("selected_step checked a live task");
        let form = model.form.as_mut().expect("task form checked above");
        form.steps.removals.insert(step_id);
        form.steps.drafts.remove(&step_id);
        form.steps.editor = None;
        form.steps.delete_mark = None;
        // Keep the selector on the nearest remaining stored row. Its source index remains
        // stable while the renderer filters staged removals, so it cannot point at the row
        // that just vanished or silently select a different draft.
        let remaining: Vec<usize> = task
            .steps
            .iter()
            .enumerate()
            .filter(|(_, step)| !form.steps.removals.contains(&step.id))
            .map(|(source_index, _)| source_index)
            .collect();
        form.steps.cursor = remaining
            .iter()
            .copied()
            .find(|source_index| *source_index >= index)
            .or_else(|| remaining.last().copied());
        form.steps.add_selected = false;
        if let Some(cursor) = form.steps.cursor {
            steps_scroll_to_cursor(form, cursor);
        }
        model.input_mode = BoardInputMode::TaskPage;
        return Ok(PageStepDelete::Staged);
    }
    if form.steps.delete_mark != Some(index) {
        model
            .form
            .as_mut()
            .expect("task form checked above")
            .steps
            .delete_mark = Some(index);
        // AC-23: the footer's message slot carries the press-again hint for exactly
        // as long as the mark is armed — the removal press and every intervening
        // intent clear it with the mark. The verb's own modifier names the key, so
        model.set_message("press ctrl+x again to remove");
        return Ok(PageStepDelete::Marked);
    }
    domain.remove_step(task_id, step_id)?;
    let len = domain
        .get(task_id)
        .map(|task| task.steps.len())
        .unwrap_or(0);
    let form = model
        .form
        .as_mut()
        .filter(|form| form.is_task())
        .expect("task form checked above");
    form.steps.delete_mark = None;
    if len == 0 {
        form.steps.cursor = None;
    } else {
        let cursor = form.steps.cursor.unwrap_or(0).min(len - 1);
        form.steps.cursor = Some(cursor);
        steps_scroll_to_cursor(form, cursor);
    }
    Ok(PageStepDelete::Removed)
}

/// Open an in-place step row seeded with `text`, renaming `step` when given and adding
/// a transient row when `None`.
fn open_step_editor(model: &mut BoardModel, text: &str, rename: Option<Uuid>) {
    if let Some(form) = model.form.as_mut() {
        form.steps.add_selected = false;
        if form
            .steps
            .editor
            .as_ref()
            .is_some_and(|editor| editor.rename == rename)
        {
            model.input_mode = BoardInputMode::EditStep;
            model.clear_message();
            return;
        }
        let buffer = rename
            .and_then(|step_id| form.steps.drafts.remove(&step_id))
            .unwrap_or_else(|| crate::ui::edit::seeded_draft(text));
        form.steps.editor = Some(StepEditor {
            buffer,
            rename,
            pending_index: None,
            refusal: None,
        });
        model.input_mode = BoardInputMode::EditStep;
        model.clear_message();
    }
}

/// Close the inline step editor back to page view, discarding its draft. Any pending editor
/// save goes with it: a closed row has nothing left for the save boundary to release (the only
/// path that can be here with one pending is a defensive direct
/// intent, never the keyboard).
fn open_capture_pending_step_editor(model: &mut BoardModel, index: usize) {
    let text = model
        .form
        .as_ref()
        .filter(|form| !form.is_task())
        .and_then(|form| form.steps.pending_adds.get(index))
        .cloned();
    let Some(text) = text else {
        return;
    };
    if let Some(form) = model.form.as_mut() {
        form.steps.add_selected = false;
        form.steps.editor = Some(StepEditor {
            buffer: crate::ui::edit::seeded_draft(&text),
            rename: None,
            pending_index: Some(index),
            refusal: None,
        });
        model.input_mode = BoardInputMode::EditStep;
        model.clear_message();
    }
}

fn close_step_editor(model: &mut BoardModel) {
    if let Some(form) = model.form.as_mut() {
        form.steps.pending_save = None;
        form.steps.editor = None;
    }
    model.input_mode = BoardInputMode::TaskPage;
    model.clear_message();
}

/// What the inline step row paints when its draft is empty after trim: a short dim refusal
/// on the row itself, never the board status row.
const STEP_TEXT_REQUIRED: &str = "text required";

/// Enter saves a new step and opens the next empty row. Shift+Enter saves it and
/// then the enclosing form (task session or expanded capture).
fn confirm_add_step(
    domain: &mut DomainState,
    model: &mut BoardModel,
    snapshot: Option<&InvocationSnapshot>,
    save_session: bool,
) -> Result<IntentOutcome, DomainError> {
    if !save_session {
        return confirm_step_editor(domain, model, true, false);
    }
    if model.form.as_ref().is_some_and(|form| !form.is_task()) {
        let outcome = confirm_step_editor(domain, model, false, false)?;
        if model
            .form
            .as_ref()
            .is_some_and(|form| form.steps.editor.is_some())
        {
            return Ok(outcome);
        }
        return apply_intent(domain, model, BoardIntent::ConfirmEdit, snapshot);
    }
    let extra_step = model.form.as_ref().and_then(|form| {
        form.steps
            .editor
            .as_ref()
            .filter(|editor| editor.rename.is_none())
            .map(|editor| editor.buffer.value().trim().to_string())
    });
    let Some(text) = extra_step.filter(|text| !text.is_empty()) else {
        return confirm_step_editor(domain, model, false, true);
    };
    confirm_edit(domain, model, Some(&text))
}

fn stage_capture_step(
    model: &mut BoardModel,
    keep_open: bool,
) -> Result<IntentOutcome, DomainError> {
    let Some(form) = model.form.as_mut().filter(|form| !form.is_task()) else {
        return Ok(IntentOutcome::None);
    };
    let Some(editor) = form.steps.editor.as_ref() else {
        return Ok(IntentOutcome::None);
    };
    let text = editor.buffer.value().trim().to_string();
    let pending_index = editor.pending_index;
    if text.is_empty() {
        if let Some(editor) = form.steps.editor.as_mut() {
            editor.refusal = Some(STEP_TEXT_REQUIRED.to_string());
        }
        return Ok(IntentOutcome::None);
    }
    if let Some(index) = pending_index {
        if let Some(existing) = form.steps.pending_adds.get_mut(index) {
            *existing = text;
        }
    } else {
        form.steps.pending_adds.push(text);
    }
    if keep_open {
        form.steps.editor = Some(StepEditor {
            buffer: crate::ui::edit::seeded_draft(""),
            rename: None,
            pending_index: None,
            refusal: None,
        });
        model.input_mode = BoardInputMode::EditStep;
    } else {
        form.steps.editor = None;
        model.input_mode = form.parent_mode();
    }
    Ok(IntentOutcome::None)
}

/// Persist a new-step editor draft through the domain command.
///
/// Existing-step editors are parked into the enclosing task session before this function is
/// reached. A successful add records the touched step on the page's pending-save slot and
/// leaves the line exactly as the user left it. Only the persistence boundary's confirmed
/// sync releases it, closing for Enter or reopening empty for Shift+Enter. A failed save
/// therefore holds the line behind SaveRecovery until Retry/Cancel resolve it.
///
/// An empty-after-trim draft is the line's own refusal (AC-13), painted on the line
/// and cleared when it closes or its buffer changes; it never reaches the board
/// message. Every other domain refusal — an step another actor removed — propagates
/// before anything is cleared, exactly as the task form's edit does.
fn confirm_step_editor(
    domain: &mut DomainState,
    model: &mut BoardModel,
    keep_open: bool,
    exit_editing: bool,
) -> Result<IntentOutcome, DomainError> {
    if model.form.as_ref().is_some_and(|form| !form.is_task()) {
        return stage_capture_step(model, keep_open);
    }
    let Some(form) = model.form.as_ref().filter(|form| form.is_task()) else {
        return Ok(IntentOutcome::None);
    };
    let Some(task_id) = form.task_id() else {
        return Ok(IntentOutcome::None);
    };
    let Some(editor) = form.steps.editor.as_ref() else {
        return Ok(IntentOutcome::None);
    };
    let text = editor.buffer.value().to_string();
    debug_assert!(
        editor.rename.is_none(),
        "existing-step drafts are staged by the task session"
    );
    let touched = match domain.add_step(task_id, &text) {
        Ok(step) => step,
        Err(DomainError::EmptyStepText) => {
            if let Some(editor) = model
                .form
                .as_mut()
                .and_then(|form| form.steps.editor.as_mut())
            {
                editor.refusal = Some(STEP_TEXT_REQUIRED.to_string());
            }
            return Ok(IntentOutcome::None);
        }
        Err(other) => return Err(other),
    };
    let form = model.form.as_mut().expect("task form checked above");
    form.steps.pending_save = Some(StepEditorSave {
        step: touched,
        text: text.trim().to_string(),
        reopen: keep_open,
        exit_editing,
    });
    Ok(IntentOutcome::Persist)
}
