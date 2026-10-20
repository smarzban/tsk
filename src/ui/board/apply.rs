//! Board intent reducer.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::context::InvocationSnapshot;
use crate::domain::{
    normalize_thread, status_word, thread_refusal_message, DomainError, DomainState, HumanStatus,
    TaskScope, ThreadError, UndoEntry,
};
use crate::scope::paths_equivalent;
use crate::ui::capture::{CaptureField, TITLE_REQUIRED_MESSAGE};
use crate::ui::edit::{delete_last_word, flatten_line_breaks, EditBuffer};
use crate::ui::input::{BoardIntent, MarkDirection};
use crate::ui::mouse::BoardPopup;
use crate::ui::queue::{GroupTarget, NavTab, ARCHIVED_HEADER_ROW_ID, INBOX_HEADER_ROW_ID};
use crate::ui::tier::{FocusedSurface, WideStage};

use super::commands::{resolve_board_command, CommandSurface};
use super::model::{steps_scroll_to_cursor, steps_scroll_to_row};
use super::model::{
    BoardForm, BoardInputMode, BoardLocation, BoardModel, IntentOutcome, ListPickerValue,
    PickerTab, ProjectPickerState, ProjectScopeOption, ProjectsView, StepEditor, StepEditorSave,
    TaskEditSave, DIRTY_TASK_SWITCH_REFUSAL,
};

/// What the row says when an action that aims at the selection is asked for on a board that
/// has none. One wording, so the same refusal always reads the same way.
const NO_SELECTION: &str = "select a task first";
/// `@` with no `config.toml` profile: the picker would offer only **none**.
pub const NO_AGENT_PROFILES: &str = "no agent profiles · add one to config.toml";

fn base_target_project(domain: &DomainState, ids: &[Uuid]) -> Result<PathBuf, String> {
    for id in ids {
        let task = domain
            .get(*id)
            .ok_or_else(|| format!("unknown task {id}"))?;
        if let TaskScope::Project { path } = &task.scope {
            return Ok(PathBuf::from(path));
        }
    }
    Err("base requires a project task".to_string())
}

/// What the row says when Shift+Enter reaches a form as Ctrl+J (#125).
pub const SHIFT_ENTER_AS_CTRL_J_HINT: &str = "Shift+Enter sent Ctrl+J: check terminal keys";

fn take_verb_targets(model: &mut BoardModel) -> (Vec<Uuid>, bool) {
    let bulk = model.task_list_owns_input() && model.mark_mode_active() && model.marked_count() > 0;
    let targets = model.verb_target_ids();
    model.clear_marks();
    (targets, bulk)
}

/// Block and review targets without their done tasks, and how many were dropped.
fn without_done(domain: &DomainState, targets: Vec<Uuid>) -> (Vec<Uuid>, usize) {
    let count = targets.len();
    let rest: Vec<Uuid> = targets
        .into_iter()
        .filter(|id| {
            !domain
                .get(*id)
                .is_some_and(|task| task.status == HumanStatus::Done)
        })
        .collect();
    let skipped = count - rest.len();
    (rest, skipped)
}

/// The status row's note for a marked set whose done tasks a block or review skipped.
fn note_skipped_done(model: &mut BoardModel, skipped: usize) {
    if skipped > 0 {
        model.set_message(format!(
            "skipped {skipped} done (ctrl+n or ctrl+o reopens them)"
        ));
    }
}

/// What one `ctrl+u` reverts, read from the top entry before it is undone: the action word, the
/// tasks it is about, and, for a done, the tasks it started that go back to ready.
struct UndoNote {
    action: &'static str,
    subjects: Vec<Uuid>,
    released: Vec<Uuid>,
    /// Every other task the entry touches (a deletion's unlinked `after` lists).
    others: Vec<Uuid>,
    /// Tasks that were started before the undo: only these can leave a running agent behind.
    was_started: Vec<Uuid>,
    restores_status: bool,
}

/// The board's word for the action an undo leaf recorded. `domain` is the state before the
/// undo, which still shows what the action did (a block card versus a review card).
fn undo_action_word(domain: &DomainState, leaf: &UndoEntry) -> &'static str {
    match leaf {
        UndoEntry::Complete { .. } => "done",
        UndoEntry::SoftDelete { .. } => "delete",
        UndoEntry::Archive {
            previous: false, ..
        } => "archive",
        UndoEntry::Archive { previous: true, .. } => "unarchive",
        UndoEntry::Start { .. } => "start",
        UndoEntry::Status { .. } => "status",
        UndoEntry::Block { id, .. } => {
            if domain
                .get(*id)
                .is_some_and(|task| task.status == HumanStatus::Review)
            {
                "review"
            } else {
                "block"
            }
        }
        UndoEntry::Assign { .. } => "assignee",
        UndoEntry::SetBase { .. } => "base",
        UndoEntry::SetAfter { .. } => "after",
        UndoEntry::Steps { .. } => "steps",
        UndoEntry::Batch { .. } => "undo",
    }
}

fn leaf_target(leaf: &UndoEntry) -> Option<Uuid> {
    leaf.targets().first().map(|(id, _)| *id)
}

impl UndoNote {
    fn read(domain: &DomainState, entry: &UndoEntry) -> Option<Self> {
        let leaves = entry.leaf_entries();
        let first = *leaves.first()?;
        let kind = std::mem::discriminant(first);
        let mut subjects = Vec::new();
        let mut released = Vec::new();
        let mut others = Vec::new();
        for leaf in &leaves {
            let Some(id) = leaf_target(leaf) else {
                continue;
            };
            if std::mem::discriminant(*leaf) == kind {
                if !subjects.contains(&id) {
                    subjects.push(id);
                }
            } else if matches!(first, UndoEntry::Complete { .. })
                && matches!(leaf, UndoEntry::Start { .. })
            {
                if !released.contains(&id) {
                    released.push(id);
                }
            } else if !others.contains(&id) {
                others.push(id);
            }
        }
        others.retain(|id| !subjects.contains(id) && !released.contains(id));
        let was_started = subjects
            .iter()
            .chain(&released)
            .copied()
            .filter(|id| {
                domain
                    .get(*id)
                    .is_some_and(|task| task.status == HumanStatus::Started)
            })
            .collect();
        Some(Self {
            action: undo_action_word(domain, first),
            subjects,
            released,
            others,
            was_started,
            restores_status: matches!(
                first,
                UndoEntry::Complete { .. }
                    | UndoEntry::Start { .. }
                    | UndoEntry::Status { .. }
                    | UndoEntry::Block { .. }
            ),
        })
    }

    /// `done undone · T202 back to started · T203 back to ready · @claude kept running`, read
    /// from the state after the undo.
    fn message(&self, domain: &DomainState) -> String {
        let mut message = format!(
            "{} undone · {}",
            self.action,
            task_names(domain, &self.subjects)
        );
        if self.restores_status {
            if let [id] = self.subjects.as_slice() {
                if let Some(task) = domain.get(*id) {
                    message.push_str(&format!(" back to {}", status_word(task.status)));
                }
            }
        }
        if !self.released.is_empty() {
            message.push_str(&format!(
                " · {} back to ready",
                task_names(domain, &self.released)
            ));
        }
        // Undo never stops an agent: a task the undo takes out of started (the board's `◉`
        // rule: started with a live dispatch) leaves its agent running.
        let mut running: Vec<String> = self
            .was_started
            .iter()
            .filter(|_| self.restores_status)
            .filter_map(|id| domain.get(*id))
            .filter(|task| task.status != HumanStatus::Started)
            .filter(|task| task.dispatch.as_ref().is_some_and(|record| !record.cleaned))
            .filter_map(|task| task.assignee.clone())
            .collect();
        running.sort();
        running.dedup();
        for assignee in running {
            message.push_str(&format!(" · @{assignee} kept running"));
        }
        message
    }
}

/// `T3`, `T3, T4, T5`, or `5 tasks`: the tasks an undo is about, the way the board names them.
fn task_names(domain: &DomainState, ids: &[Uuid]) -> String {
    if ids.len() > 3 {
        return format!("{} tasks", ids.len());
    }
    let mut ids = ids.to_vec();
    ids.sort_by_key(|id| {
        domain
            .get(*id)
            .and_then(|task| task.number)
            .unwrap_or(u64::MAX)
    });
    let names: Vec<String> = ids
        .iter()
        .map(|id| {
            domain
                .get(*id)
                .and_then(crate::domain::Task::board_identifier)
                .unwrap_or_else(|| "a task".to_string())
        })
        .collect();
    if names.is_empty() {
        "nothing".to_string()
    } else {
        names.join(", ")
    }
}

/// The status row when the undo stack is empty.
const NOTHING_TO_UNDO: &str = "nothing to undo";

/// The refusal when **chain in order** has fewer than two marked tasks to chain.
const CHAIN_NEEDS_MARKS: &str = "mark two or more tasks, in order, to chain them";

/// `T202, T205`.
fn after_list(numbers: &[u64]) -> String {
    numbers
        .iter()
        .map(|number| format!("T{number}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the row says when an Undo is refused because its target moved on.
///
/// The domain's [`DomainError::StaleUndo`] names the task by uuid, which is right for a log
/// line but means nothing on the board. The row names the task the way the board paints it,
/// says which recorded action went stale, adds the visible change that most likely caused it
/// (an archive is the common one), and states that nothing moved, naming every task of a
/// marked set's entry.
fn stale_undo_message(domain: &DomainState, id: Uuid) -> String {
    let task = domain.get(id);
    let subject = task
        .and_then(crate::domain::Task::board_identifier)
        .unwrap_or_else(|| "this task".to_string());
    let entry = domain.last_undo();
    let leaf = entry.and_then(|entry| {
        entry
            .leaf_entries()
            .into_iter()
            .find(|leaf| leaf_target(leaf) == Some(id))
    });
    let since = match leaf {
        Some(UndoEntry::Complete { .. }) => "since it was completed",
        Some(UndoEntry::SoftDelete { .. }) => "since it was deleted",
        Some(UndoEntry::Archive {
            previous: false, ..
        }) => "since it was archived",
        Some(UndoEntry::Archive { previous: true, .. }) => "since it was unarchived",
        Some(UndoEntry::Start { .. }) => "since it was started",
        Some(UndoEntry::Status { .. }) => "since its status change",
        Some(UndoEntry::Block { .. }) => "since its block or review",
        Some(UndoEntry::Assign { .. }) => "since it was assigned",
        Some(UndoEntry::SetBase { .. }) => "since its base change",
        Some(UndoEntry::SetAfter { .. }) => "since its after change",
        Some(UndoEntry::Steps { .. }) => "since its step change",
        Some(UndoEntry::Batch { .. }) | None => "since the undoable action",
    };
    let cause = task.and_then(|task| match leaf {
        Some(UndoEntry::Archive {
            previous: false, ..
        }) if !task.archived => Some("unarchived"),
        Some(UndoEntry::Archive { .. }) => None,
        _ if task.archived => Some("archived"),
        Some(UndoEntry::Complete { .. }) if task.soft_deleted => Some("deleted"),
        Some(UndoEntry::Complete { .. }) if task.status != HumanStatus::Done => Some("reopened"),
        Some(UndoEntry::SoftDelete { .. }) if !task.soft_deleted => Some("restored"),
        _ => None,
    });
    let cause = cause.map(|cause| format!(" ({cause})")).unwrap_or_default();
    let note = entry.and_then(|entry| UndoNote::read(domain, entry));
    match note {
        // An entry about more than the task that moved on names what it would have undone:
        // `done of T1 not undone: T2 changed since it was started`.
        Some(note)
            if note.subjects != [id] || !note.released.is_empty() || !note.others.is_empty() =>
        {
            let extras: Vec<Uuid> = note
                .released
                .iter()
                .chain(&note.others)
                .copied()
                .filter(|extra| *extra != id)
                .collect();
            let with = if extras.is_empty() {
                String::new()
            } else {
                format!(" (with {})", task_names(domain, &extras))
            };
            format!(
                "{} of {}{with} not undone: {subject} changed {since}{cause}",
                note.action,
                task_names(domain, &note.subjects)
            )
        }
        _ => format!("{subject} changed {since}{cause}; nothing undone"),
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
    // The mouse route recurses into ConfirmListPicker inside the reducer, so the outer
    // intent is the one the save baseline is decided on.
    let picker_assignment = matches!(
        intent,
        BoardIntent::ConfirmListPicker | BoardIntent::SelectListOption(_)
    ) && model.input_mode == BoardInputMode::ListPicker
        && matches!(
            model.list_picker_kind(),
            Some(
                crate::ui::board::ListPickerKind::Assignee
                    | crate::ui::board::ListPickerKind::Base
                    | crate::ui::board::ListPickerKind::After
            )
        );
    picker_assignment
        || matches!(
            intent,
            BoardIntent::ConfirmEdit
                | BoardIntent::ConfirmEditNext
                | BoardIntent::SetStatus(_)
                | BoardIntent::Complete
                | BoardIntent::Reopen
                | BoardIntent::SoftDelete
                | BoardIntent::Undo
                | BoardIntent::File
                | BoardIntent::LaunchUnarchive
                | BoardIntent::PrimaryVerb
                | BoardIntent::DispatchAgain
                | BoardIntent::ConfirmDispatch
                | BoardIntent::StartWithoutRelaunch
                | BoardIntent::ConfirmCleanup
                | BoardIntent::KeepCleanup
                | BoardIntent::ToggleBlock
                | BoardIntent::BlockCardConfirm
                | BoardIntent::ReplySave
                | BoardIntent::ReplySaveUnblock
                | BoardIntent::PageReplyUnblock
                | BoardIntent::ReplySaveStart
                | BoardIntent::ReplySaveBeforeLaunch
                | BoardIntent::ToggleReview
                | BoardIntent::CycleCheck
                | BoardIntent::ClickCheck(_)
                | BoardIntent::ReplyApprove
                | BoardIntent::ApproveReview(_)
                | BoardIntent::ToggleStep
                | BoardIntent::MoveStepUp
                | BoardIntent::MoveStepDown
                | BoardIntent::QuickAddSave
                | BoardIntent::QuickAddSaveNext
                | BoardIntent::ChainAfter
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
/// - **the cursor or the lens moves** (a different selected row or project, another
///   destination or View, the done drawer opening or closing) — the notice is cleared: it
///   reported the delete, and the user has moved on. The message is left alone. Applied by
///   [`apply_intent`] from the state before and after a non-mutating intent, so a mouse
///   click and a key move are the same event. The footer's undo seat
///   ([`BoardModel::undo_hint`]) is not the notice and stays while its entry is live.
/// - **a few seconds pass** — the notice is cleared ([`BoardModel::expire_delete_notice`],
///   on the idle tick), so a delete with nothing after it does not hold the row forever.
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
            | BoardIntent::OpenAssigneePicker
            | BoardIntent::OpenBasePicker
            | BoardIntent::OpenAfterPicker
            | BoardIntent::BeginAddStep
            | BoardIntent::BeginAppendStep
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
                | BoardIntent::EditDeleteWordLeft
                | BoardIntent::EditDeleteWordRight
                | BoardIntent::EditDeleteToLineStart
                | BoardIntent::EditDeleteToLineEnd
                | BoardIntent::ConfirmEdit
                | BoardIntent::ConfirmEditNext
                | BoardIntent::CancelEdit
                | BoardIntent::ShiftEnterAsCtrlJ
                | BoardIntent::BeginAddStep
                | BoardIntent::BeginAppendStep
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
        model.delete_notice.clone(),
        model.delete_notice_count,
        model.delete_notice_label.clone(),
        model.delete_notice_expires_at,
    );
    let mutating = board_intent_may_persist(model, &intent);
    if mutating {
        model.undo_hint_before_save = model.undo_hint.clone();
    }
    let place_before = BoardPlace::of(model);
    let result = apply_board_intent(domain, model, intent, snapshot);
    // A command confirmation carries no lifetime of its own: it recurses with the command it
    // resolved to, and that intent is classified on the way through, so it is the recursion
    // that restores.
    if mutating && !matches!(result, Ok(IntentOutcome::Persist)) {
        model.delete_notice = notice_before.0;
        model.delete_notice_count = notice_before.1;
        model.delete_notice_label = notice_before.2;
        model.delete_notice_expires_at = notice_before.3;
    }
    if !mutating && BoardPlace::of(model) != place_before {
        model.clear_delete_notice();
    }
    result
}

/// Where the user is looking: the cursor and the lens it moves in. A change between two
/// intents is "moving on" for the chrome-row lifetime rule ([`apply_chrome_row_lifetime`]).
#[derive(PartialEq)]
struct BoardPlace {
    selection: Option<Uuid>,
    location: BoardLocation,
    view: ProjectsView,
    project_row: usize,
    drawer_open: bool,
}

impl BoardPlace {
    fn of(model: &BoardModel) -> Self {
        Self {
            selection: model.selection_id,
            location: model.board_location.clone(),
            view: model.projects_view.clone(),
            project_row: model.projects_selected,
            drawer_open: model.drawer_open,
        }
    }
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
                | BoardInputMode::SelectBase
                | BoardInputMode::SelectAfter
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
            | BoardIntent::CommandQueryDeleteWord
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
        BoardIntent::CommandQueryBackspace | BoardIntent::CommandQueryDeleteWord => {
            if model.surface == CommandSurface::Palette {
                if intent == BoardIntent::CommandQueryDeleteWord {
                    delete_last_word(&mut model.command_query);
                } else {
                    model.command_query.pop();
                }
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
            // The palette reaches this from a task page too: never drop a dirty session.
            if model.form.is_some() && model.has_unsaved_work() {
                model.set_message(DIRTY_TASK_SWITCH_REFUSAL);
                return Ok(IntentOutcome::None);
            }
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
        BoardIntent::QuickAddDeleteWordLeft
        | BoardIntent::QuickAddDeleteWordRight
        | BoardIntent::QuickAddDeleteToLineStart
        | BoardIntent::QuickAddDeleteToLineEnd => {
            model.invalidate_quick_add_stash();
            if let Some(quick_add) = model.quick_add.as_mut() {
                match intent {
                    BoardIntent::QuickAddDeleteWordLeft => quick_add.title.delete_word_left(),
                    BoardIntent::QuickAddDeleteWordRight => quick_add.title.delete_word_right(),
                    BoardIntent::QuickAddDeleteToLineStart => {
                        quick_add.title.delete_to_line_start()
                    }
                    _ => quick_add.title.delete_to_line_end(),
                }
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
            let lifted = match crate::capture::lift_quick_add_tokens(
                quick_add.title.value(),
                domain,
                quick_add.snapshot.as_ref().as_ref(),
                &quick_add.default,
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
            form.base = lifted.base;
            form.after = lifted.after;
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
                } else if let Some(trail) = trail_tab(model, true) {
                    // The PAPER TRAIL closes the ring: past its last record Tab wraps to the
                    // BLOCKED section, the first step, or `+ step`.
                    if matches!(trail, super::block::PageTab::LeaveBlock)
                        && !super::block::enter_block_ring(model, true)
                        && !select_first_step_from_page(model)
                    {
                        model.enter_page_field_focus();
                    }
                } else {
                    match super::block::move_block_tab(model, true) {
                        super::block::PageTab::Moved => {}
                        super::block::PageTab::LeaveBlock => {
                            if !select_first_step_from_page(model) {
                                model.enter_page_field_focus();
                            }
                        }
                        super::block::PageTab::NotHandled => {
                            let on_add = model
                                .form
                                .as_ref()
                                .is_some_and(|form| form.steps.add_selected);
                            let nothing_selected = model.form.as_ref().is_some_and(|form| {
                                form.steps.cursor.is_none() && !form.steps.add_selected
                            });
                            // `+ step` leads into the PAPER TRAIL's records. The ring leads with
                            // the BLOCKED section: from nothing, and after them it wraps.
                            if !(on_add && super::block::enter_trail(model, true))
                                && !((nothing_selected || on_add)
                                    && super::block::enter_block_ring(model, true))
                                && !move_step_with_tab(model, true)
                                && !select_first_step_from_page(model)
                            {
                                model.enter_page_field_focus();
                            }
                        }
                    }
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
            } else if model.input_mode == BoardInputMode::EditAssignee && model.form.is_some() {
                model.focus_form_field(CaptureField::Base);
            } else if model.input_mode == BoardInputMode::SelectBase && model.form.is_some() {
                model.focus_form_field(CaptureField::After);
            } else if model.input_mode == BoardInputMode::SelectAfter && model.form.is_some() {
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
                } else if let Some(trail) = trail_tab(model, false) {
                    // Shift+Tab from the newest record climbs back to `+ step`.
                    if matches!(trail, super::block::PageTab::LeaveBlock) {
                        select_add_step(model);
                    }
                } else {
                    match super::block::move_block_tab(model, false) {
                        super::block::PageTab::Moved => {}
                        // Shift+Tab before the top line wraps to the ring's last stop.
                        super::block::PageTab::LeaveBlock => select_last_page_stop(model),
                        super::block::PageTab::NotHandled
                            if model.page_section_open()
                                && model.form.as_ref().is_some_and(|form| {
                                    form.steps.cursor.is_none() && !form.steps.add_selected
                                }) =>
                        {
                            // Nothing selected yet (a blocked page opens so): the last stop.
                            select_last_page_stop(model);
                        }
                        super::block::PageTab::NotHandled => {
                            let no_steps = model
                                .form
                                .as_ref()
                                .and_then(|form| form.task_id())
                                .and_then(|id| model.tasks.iter().find(|task| task.id == id))
                                .is_some_and(|task| task.steps.is_empty());
                            let first_step = model.form.as_ref().is_some_and(|form| {
                                (form.steps.cursor == Some(0) && !form.steps.add_selected)
                                    || (form.steps.add_selected && no_steps)
                            });
                            // Shift+Tab from the first step (or `+ step` with none) climbs back
                            // into the section.
                            let climbed =
                                first_step && super::block::enter_block_ring(model, false);
                            if !(climbed || move_step_with_tab(model, false)) {
                                model.enter_page_field_focus();
                            }
                        }
                    }
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
                model.focus_form_field(CaptureField::After);
            } else if model.input_mode == BoardInputMode::SelectAfter && model.form.is_some() {
                model.focus_form_field(CaptureField::Base);
            } else if model.input_mode == BoardInputMode::SelectBase && model.form.is_some() {
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
            if model.input_mode == BoardInputMode::FormDropdown {
                model.close_form_dropdown(true);
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
            model.select_form_dropdown_option(index);
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
            // Shift+M on a sub-header marks that whole group as one set, whether or not
            // multi-select is already on.
            if model.task_list_owns_input() && model.group_header_selected() {
                if let Some(header_id) = model.selection_id {
                    model.mark_group(header_id);
                }
                model.clear_message();
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
        BoardIntent::BlockCardConfirm => {
            return super::block::confirm_block_card(domain, model);
        }
        BoardIntent::BlockCardCancel => {
            // Esc keeps the marks: the card returns to the same marked set.
            super::block::cancel_block_card(model);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::BlockCardNextField | BoardIntent::BlockCardPrevField => {
            super::block::block_card_field(model, intent == BoardIntent::BlockCardNextField);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::BlockCardLeft | BoardIntent::BlockCardRight => {
            super::block::block_card_arrow(model, intent == BoardIntent::BlockCardRight);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::BlockCardNewline => {
            super::block::block_card_newline(model);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CycleCheck => {
            return super::block::cycle_selected_check(domain, model);
        }
        BoardIntent::TogglePassedChecks => {
            super::block::toggle_passed_checks(model);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ClickCheck(index) => {
            if !super::block::select_stop(model, super::block::BlockTarget::Check(index)) {
                return Ok(IntentOutcome::None);
            }
            return super::block::cycle_selected_check(domain, model);
        }
        BoardIntent::ClickPassedFold => {
            if super::block::select_stop(model, super::block::BlockTarget::PassedFold) {
                super::block::toggle_passed_checks(model);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ToggleTrail => {
            super::block::toggle_trail(model);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ToggleTrailRecord(index) => {
            super::block::toggle_trail_record(model, index);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ApproveReview(id) => {
            model.close_popup();
            if !domain
                .get(id)
                .is_some_and(|task| task.status == HumanStatus::Review)
            {
                return Ok(IntentOutcome::None);
            }
            domain.complete(id)?;
        }
        BoardIntent::ReplyApprove => {
            // The application boundary approves a review; anything else lands here.
            if model.input_mode == BoardInputMode::EditReply {
                model.set_message("only a task in review can be approved");
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::BeginReply => {
            if model.input_mode() == BoardInputMode::Normal {
                // `r` on a board row answers the cursor task inline, whatever is marked.
                if !super::block::begin_row_reply(model) {
                    model.set_message("not blocked or in review");
                }
            } else if model.input_mode == BoardInputMode::TaskPage
                && !super::block::begin_reply(model, "", None)
            {
                model.set_message("only a blocked or review task takes replies");
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::PickOption(index) => {
            // A number picks a block option only on a blocked page; anywhere else it is inert.
            if model.input_mode == BoardInputMode::TaskPage {
                if let Some(text) = super::block::option_text(model, index) {
                    super::block::begin_reply(model, &text, None);
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::PageReplyUnblock => {
            // The application boundary opens the box and saves it; a bare reducer only opens it.
            if model.input_mode == BoardInputMode::TaskPage {
                super::block::begin_reply(model, "", None);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ReplyWithOption => {
            if let Some(text) = super::block::selected_option_text(model) {
                super::block::begin_reply(model, &text, None);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ReplySave
        | BoardIntent::ReplySaveUnblock
        | BoardIntent::ReplySaveStart
        | BoardIntent::ReplySaveBeforeLaunch => {
            if model.input_mode != BoardInputMode::EditReply {
                return Ok(IntentOutcome::None);
            }
            let unblock = match intent {
                BoardIntent::ReplySaveUnblock => Some(HumanStatus::Ready),
                BoardIntent::ReplySaveStart => Some(HumanStatus::Started),
                _ => None,
            };
            return super::block::save_reply(
                domain,
                model,
                unblock,
                intent == BoardIntent::ReplySaveBeforeLaunch,
            );
        }
        BoardIntent::BeginAddStep | BoardIntent::BeginAppendStep => {
            model.close_popup();
            // Ctrl+A and the trailing target both open the independent add row from a task
            // page or an expanded capture. Park a rename first, so Ctrl+A never drops an
            // existing staged rename while replacing its cursor with the add row.
            // A selected stored step anchors the row: the new step lands directly below it.
            // A click on the trailing `+ step` row means the end of the list.
            let anchor = (intent == BoardIntent::BeginAddStep)
                .then(|| selected_step(domain, model).map(|(_, step_id)| step_id))
                .flatten();
            if model.form.is_some() && model.park_rename_step_draft() {
                open_step_editor(model, "", None);
                if let Some(editor) = model
                    .form
                    .as_mut()
                    .filter(|form| form.is_task())
                    .and_then(|form| form.steps.editor.as_mut())
                    .filter(|editor| editor.rename.is_none())
                {
                    editor.insert_after = anchor;
                }
                // Keep the new row, painted directly below its anchor, inside the viewport.
                let anchor_index = anchor.and_then(|anchor| {
                    let task = domain.get(model.form.as_ref()?.task_id()?)?;
                    task.steps.iter().position(|step| step.id == anchor)
                });
                if let (Some(index), Some(form)) = (anchor_index, model.form.as_mut()) {
                    let rows_through_anchor =
                        form.steps.row_counts.borrow().iter().take(index + 1).sum();
                    steps_scroll_to_row(form, rows_through_anchor, 1);
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::BeginEditTitle | BoardIntent::BeginEditNotes | BoardIntent::BeginEditScope => {
            model.close_popup();
            model.clear_marks();
            // Ctrl+E on the BLOCKED section: the heading edits why, on and needs in the
            // block card; one of your replies opens the reply box on it.
            if intent == BoardIntent::BeginEditTitle && model.input_mode == BoardInputMode::TaskPage
            {
                if let Some(task) = super::block::heading_selected_task(model) {
                    super::block::open_block_edit_card(model, &task);
                    return Ok(IntentOutcome::None);
                }
                if let Some((index, text)) = super::block::selected_own_reply(model) {
                    super::block::begin_reply(model, &text, Some(index));
                    return Ok(IntentOutcome::None);
                }
                if super::block::agent_reply_selected(model) {
                    model.set_message("agent replies cannot be edited");
                    return Ok(IntentOutcome::None);
                }
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
                || model.input_mode() == BoardInputMode::BlockCard
                || (model.input_mode != BoardInputMode::EditReply
                    && model.form.as_ref().is_some_and(|form| {
                        matches!(form.focus, CaptureField::Title | CaptureField::Thread)
                    }));
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
            if model.input_mode == BoardInputMode::EditReply
                || model
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
            let reply = super::block::active_reply(model)
                .filter(|_| model.input_mode == BoardInputMode::EditReply)
                .and_then(|(_, editor)| {
                    crate::ui::edit::wrapped_vertical_move(
                        &editor.buffer,
                        editor.width.get(),
                        delta,
                    )
                });
            let target = reply.or_else(|| {
                model
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
                    })
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
        BoardIntent::EditDeleteWordLeft => {
            edit_draft(model, EditBuffer::delete_word_left);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditDeleteWordRight => {
            edit_draft(model, EditBuffer::delete_word_right);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditDeleteToLineStart => {
            edit_draft(model, EditBuffer::delete_to_line_start);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditDeleteToLineEnd => {
            edit_draft(model, EditBuffer::delete_to_line_end);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ShiftEnterAsCtrlJ => {
            model.set_ephemeral_message(SHIFT_ENTER_AS_CTRL_J_HINT, Duration::from_secs(5));
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CancelEdit => {
            // AC-45: Esc leaves the read-only archived focus, which hides that project's
            // tasks again. Nothing else on the board is open in that state.
            if model.focus_is_archived() && model.input_mode == BoardInputMode::Normal {
                model.leave_archived_focus();
                return Ok(IntentOutcome::None);
            }
            // The reply box cancels to page view (or the board), its draft discarded.
            if model.input_mode == BoardInputMode::EditReply {
                if super::block::active_reply(model)
                    .is_some_and(|(_, editor)| editor.pending.is_none())
                {
                    super::block::cancel_reply(model);
                }
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
                    | BoardInputMode::SelectThread
                    | BoardInputMode::EditThread
                    | BoardInputMode::EditScope
                    | BoardInputMode::EditAssignee
                    | BoardInputMode::SelectBase
                    | BoardInputMode::SelectAfter
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
                let base = form.base.clone();
                let after = form.after.clone();
                if let Err(error) = domain.check_new_after(&after) {
                    model.set_message(error.to_string());
                    return Ok(IntentOutcome::None);
                }
                let pending_adds = form.steps.pending_adds.clone();
                return match crate::capture::capture_save_configured(
                    domain,
                    None,
                    &snap,
                    title,
                    notes,
                    scope_override,
                    thread,
                    assignee,
                    base,
                ) {
                    Ok(id) => {
                        domain.set_after_on_create(id, &after)?;
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
                    .position(|option| match option {
                        ProjectScopeOption::Project(option) => {
                            paths_equivalent(&option.to_string_lossy(), &path.to_string_lossy())
                        }
                        ProjectScopeOption::Home => false,
                    })
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
            if matches!(model.board_location, BoardLocation::Desk) {
                model.open_desk_filter_picker(crate::ui::board::FilterTab::Threads);
            } else {
                model.open_thread_filter_picker();
            }
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
        BoardIntent::OpenAssigneePicker => {
            if model.project_picker.is_some() || model.popup == BoardPopup::SaveRecovery {
                return Ok(IntentOutcome::None);
            }
            model.close_popup();
            model.close_help();
            // An open edit session owns its assignee draft: `@` opens that field's dropdown
            // rather than writing behind the draft.
            if model.input_mode == BoardInputMode::TaskPage && model.task_editing() {
                return apply_board_intent(
                    domain,
                    model,
                    BoardIntent::OpenFormDropdown(CaptureField::Assignee),
                    snapshot,
                );
            }
            let task_page = model.input_mode == BoardInputMode::TaskPage
                && model.form.as_ref().is_some_and(BoardForm::is_task);
            if !(task_page || model.input_mode == BoardInputMode::Normal)
                || model.projects_overview()
            {
                return Ok(IntentOutcome::None);
            }
            let ids: Vec<Uuid> = model
                .verb_target_ids()
                .into_iter()
                .filter(|id| domain.get(*id).is_some_and(|task| !task.is_notice()))
                .collect();
            if ids.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            if model.agent_names.is_empty() {
                model.set_message(NO_AGENT_PROFILES);
                return Ok(IntentOutcome::None);
            }
            let current = model
                .selected_id()
                .filter(|id| ids.contains(id))
                .or_else(|| ids.first().copied())
                .and_then(|id| domain.get(id))
                .and_then(|task| task.assignee.clone());
            model.clear_message();
            model.open_assignee_picker(ids, current);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::OpenBasePicker => {
            if model.project_picker.is_some() || model.popup == BoardPopup::SaveRecovery {
                return Ok(IntentOutcome::None);
            }
            model.close_popup();
            model.close_help();
            let task_page = model.input_mode == BoardInputMode::TaskPage
                && model.form.as_ref().is_some_and(BoardForm::is_task);
            let edit_draft =
                (model.task_editing() || model.capture_draft_open()) && model.form.is_some();
            if !(task_page || edit_draft || model.input_mode == BoardInputMode::Normal)
                || model.projects_overview()
            {
                return Ok(IntentOutcome::None);
            }
            let ids: Vec<Uuid> = if edit_draft {
                model
                    .form
                    .as_ref()
                    .and_then(BoardForm::task_id)
                    .into_iter()
                    .collect()
            } else {
                model
                    .verb_target_ids()
                    .into_iter()
                    .filter(|id| domain.get(*id).is_some_and(|task| !task.is_notice()))
                    .collect()
            };
            if ids.is_empty() && !edit_draft {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            let project = if edit_draft {
                match model.form.as_ref().map(|form| &form.scope) {
                    Some(TaskScope::Project { path }) => PathBuf::from(path),
                    _ => {
                        model.set_message("base requires a project task");
                        return Ok(IntentOutcome::None);
                    }
                }
            } else {
                match base_target_project(domain, &ids) {
                    Ok(project) => project,
                    Err(message) => {
                        model.set_message(message);
                        return Ok(IntentOutcome::None);
                    }
                }
            };
            let current = if edit_draft {
                model.form.as_ref().and_then(|form| form.base.clone())
            } else {
                ids.first()
                    .and_then(|id| domain.get(*id))
                    .and_then(|task| task.base.clone())
            };
            model.clear_message();
            model.open_base_picker(ids, current, &project, edit_draft);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::OpenAfterPicker => {
            if model.project_picker.is_some() || model.popup == BoardPopup::SaveRecovery {
                return Ok(IntentOutcome::None);
            }
            model.close_popup();
            model.close_help();
            let task_page = model.input_mode == BoardInputMode::TaskPage
                && model.form.as_ref().is_some_and(BoardForm::is_task);
            let edit_draft =
                (model.task_editing() || model.capture_draft_open()) && model.form.is_some();
            if !(task_page || edit_draft || model.input_mode == BoardInputMode::Normal)
                || model.projects_overview()
            {
                return Ok(IntentOutcome::None);
            }
            let ids: Vec<Uuid> = if edit_draft {
                model
                    .form
                    .as_ref()
                    .and_then(BoardForm::task_id)
                    .into_iter()
                    .collect()
            } else {
                model
                    .verb_target_ids()
                    .into_iter()
                    .filter(|id| domain.get(*id).is_some_and(|task| !task.is_notice()))
                    .collect()
            };
            if ids.is_empty() && !edit_draft {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            let first = ids.first().and_then(|id| domain.get(*id));
            let (current, scope) = if edit_draft {
                let form = model.form.as_ref().expect("edit draft has a form");
                (form.after.clone(), form.scope.clone())
            } else {
                (
                    first
                        .filter(|_| ids.len() == 1)
                        .map(|task| task.after.clone())
                        .unwrap_or_default(),
                    first.map_or(TaskScope::Global, |task| task.scope.clone()),
                )
            };
            let own = first
                .and_then(|task| task.number)
                .filter(|_| ids.len() == 1);
            model.clear_message();
            model.open_after_picker(ids, current, &scope, edit_draft, own);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ListPickerQueryInsert(' ')
            if model.list_picker_kind() == Some(crate::ui::board::ListPickerKind::After) =>
        {
            model.toggle_after_tick();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ConfirmListPicker
            if model.list_picker_kind() == Some(crate::ui::board::ListPickerKind::After) =>
        {
            let Some(after) = model.after_picker_choice() else {
                return Ok(IntentOutcome::None);
            };
            let Some(target) = model
                .list_picker
                .as_ref()
                .and_then(|picker| picker.after_target.clone())
            else {
                return Ok(IntentOutcome::None);
            };
            // A refusal (a loop, or a task done meanwhile) keeps the picker open.
            let checked = match target.ids.as_slice() {
                [] => domain.check_new_after(&after),
                ids if target.edit_draft => domain.check_after(ids[0], &after),
                ids => {
                    let mut work = domain.clone();
                    work.set_after_batch(ids, &after).map(|_| ())
                }
            };
            if let Err(error) = checked {
                model.set_message(error.to_string());
                return Ok(IntentOutcome::None);
            }
            model.close_after_picker();
            if target.edit_draft {
                if let Some(form) = model.form.as_mut() {
                    form.after = after;
                    form.focus = CaptureField::After;
                    form.editing = true;
                    model.input_mode = BoardInputMode::SelectAfter;
                }
                return Ok(IntentOutcome::None);
            }
            let changed = domain.set_after_batch(&target.ids, &after)?;
            model.clear_marks();
            if !changed {
                return Ok(IntentOutcome::None);
            }
            model.pending_form_after_sync = Some(target.ids.clone());
            let subject = match target.ids.as_slice() {
                [id] => domain
                    .get(*id)
                    .and_then(|task| task.number)
                    .map_or_else(|| "task".to_string(), |number| format!("T{number}")),
                ids => format!("{} tasks", ids.len()),
            };
            model.set_message(if after.is_empty() {
                format!("{subject} runs after nothing")
            } else {
                format!("{subject} runs after {}", after_list(&after))
            });
            return Ok(IntentOutcome::Persist);
        }
        BoardIntent::ChainAfter => {
            model.close_popup();
            let ids = if model.bulk_verb_active() {
                model.marked_in_order()
            } else {
                Vec::new()
            };
            if ids.len() < 2 {
                model.set_message(CHAIN_NEEDS_MARKS);
                return Ok(IntentOutcome::None);
            }
            if let Err(error) = domain.chain_after(&ids) {
                model.set_message(error.to_string());
                return Ok(IntentOutcome::None);
            }
            model.clear_marks();
            let chain = ids
                .iter()
                .filter_map(|id| domain.get(*id).and_then(|task| task.number))
                .map(|number| format!("T{number}"))
                .collect::<Vec<_>>()
                .join(" → ");
            model.sync_from_domain(domain);
            model.set_message(format!("chained {chain}"));
            return Ok(IntentOutcome::Persist);
        }
        BoardIntent::ListPickerNext => {
            model.move_list_picker(true);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ListPickerPrev => {
            model.move_list_picker(false);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ListPickerTabNext | BoardIntent::ListPickerTabPrev => {
            if !model.switch_list_picker_tab(None, intent == BoardIntent::ListPickerTabNext) {
                model.move_list_picker(intent == BoardIntent::ListPickerTabNext);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SelectListPickerTab(tab) => {
            model.switch_list_picker_tab(Some(tab), true);
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
        BoardIntent::ListPickerQueryDeleteWord => {
            model.list_picker_query_delete_word();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SelectListOption(index) => {
            // Mouse-only jump onto a visible picker row, same discipline as
            // `SelectCommand`/`SelectProjectOption`: name the row directly.
            if model.select_list_picker_option(index) {
                return apply_board_intent(domain, model, BoardIntent::ConfirmListPicker, snapshot);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ConfirmListPicker
            if model.list_picker_kind() == Some(crate::ui::board::ListPickerKind::Base) =>
        {
            let Some((_, option)) = model.selected_list_picker_option() else {
                return Ok(IntentOutcome::None);
            };
            let ListPickerValue::Base(base) = option.value else {
                return Ok(IntentOutcome::None);
            };
            let Some(target) = model.close_base_picker() else {
                return Ok(IntentOutcome::None);
            };
            if target.edit_draft {
                if let Some(form) = model.form.as_mut() {
                    form.base = base;
                    form.focus = CaptureField::Base;
                    form.editing = true;
                    model.input_mode = BoardInputMode::SelectBase;
                }
                return Ok(IntentOutcome::None);
            }
            if let Some(branch) = base.as_deref() {
                for id in &target.ids {
                    let Some(task) = domain.get(*id) else {
                        model.set_message("base requires a project task");
                        return Ok(IntentOutcome::None);
                    };
                    if let Err(message) =
                        crate::git_base::validate_task_base(&task.scope, branch, false)
                    {
                        model.set_message(message);
                        return Ok(IntentOutcome::None);
                    }
                }
            }
            let changed = domain.set_base_batch(&target.ids, base.clone())?;
            model.clear_marks();
            if !changed {
                return Ok(IntentOutcome::None);
            }
            model.pending_form_base_sync = Some(target.ids.clone());
            let subject = match target.ids.as_slice() {
                [id] => domain
                    .get(*id)
                    .and_then(|task| task.number)
                    .map_or_else(|| "task".to_string(), |number| format!("T{number}")),
                ids => format!("{} tasks", ids.len()),
            };
            model.set_message(match base {
                Some(branch) => format!("set {subject} base to {branch}"),
                None => format!("set {subject} base to default"),
            });
            return Ok(IntentOutcome::Persist);
        }
        BoardIntent::ConfirmListPicker
            if model.list_picker_kind() == Some(crate::ui::board::ListPickerKind::Assignee) =>
        {
            let Some((_, option)) = model.selected_list_picker_option() else {
                return Ok(IntentOutcome::None);
            };
            let ListPickerValue::Assignee(assignee) = option.value else {
                return Ok(IntentOutcome::None);
            };
            let Some(target) = model.close_assignee_picker() else {
                return Ok(IntentOutcome::None);
            };
            let changed = domain.assign_batch(&target.ids, assignee.clone())?;
            model.clear_marks();
            if !changed {
                return Ok(IntentOutcome::None);
            }
            model.pending_form_assignee_sync = Some(target.ids.clone());
            let subject = match target.ids.as_slice() {
                [id] => domain
                    .get(*id)
                    .and_then(|task| task.number)
                    .map_or_else(|| "task".to_string(), |number| format!("T{number}")),
                ids => format!("{} tasks", ids.len()),
            };
            model.set_message(match assignee {
                Some(name) => format!("assigned {subject} to @{name}"),
                None => format!("unassigned {subject}"),
            });
            return Ok(IntentOutcome::Persist);
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
                            ListPickerValue::ProjectsOverview
                                | ListPickerValue::ProjectsThread(_)
                                | ListPickerValue::ProjectsAssignee(_)
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
                    ListPickerValue::ProjectsOverview
                        | ListPickerValue::ProjectsThread(_)
                        | ListPickerValue::ProjectsAssignee(_)
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
        BoardIntent::SearchQueryBackspace | BoardIntent::SearchQueryDeleteWord => {
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
            if intent == BoardIntent::SearchQueryDeleteWord {
                delete_last_word(&mut model.search_query);
            } else {
                model.search_query.pop();
            }
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
        BoardIntent::FoldGroup => {
            if !model.task_list_owns_input() {
                return Ok(IntentOutcome::None);
            }
            let previous_visible = model.visible_ids();
            match model.fold_cursor_group() {
                Some(target) => {
                    model.clear_marks();
                    model.select_row(target);
                    model.reanchor_selection(Some(target), &previous_visible);
                    model.clear_message();
                }
                None => model.set_message("nothing to fold here"),
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FoldAllGroups => {
            if !model.task_list_owns_input() {
                return Ok(IntentOutcome::None);
            }
            let previous_visible = model.visible_ids();
            let previous = model.selection_id;
            if model.toggle_all_folds() {
                model.clear_marks();
                // A row the fold hid hands the selection to its own group's header.
                let hidden_into = previous
                    .filter(|id| !model.visible_ids().contains(id))
                    .and_then(|id| model.fold_landing(id));
                model.reanchor_selection(hidden_into.or(previous), &previous_visible);
                model.clear_message();
            } else {
                model.set_message("nothing to fold here");
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ClickHeader(header_id) => {
            // One click only selects the header. A double-click opens what it names; a
            // header that opens nothing (inbox, archived, `no thread`, the desk on the desk)
            // folds or unfolds instead.
            let target = model.group_header_target(header_id);
            if crate::ui::queue::is_group_header_id(header_id) && target.is_none() {
                return Ok(IntentOutcome::None);
            }
            let now = Instant::now();
            let is_double = model.last_group_header_click.is_some_and(|(at, last)| {
                last == header_id && now.duration_since(at) <= ROW_DOUBLE_CLICK_WINDOW
            });
            let previous_visible = model.visible_ids();
            model.select_row(header_id);
            model.reanchor_selection(Some(header_id), &previous_visible);
            if !is_double {
                model.last_group_header_click = Some((now, header_id));
                return Ok(IntentOutcome::None);
            }
            model.last_group_header_click = None;
            if let Some(target) = target.filter(|target| model.group_target_opens(target)) {
                return open_group_target(model, target);
            }
            let Some(folded) = model.selected_header_folded() else {
                return Ok(IntentOutcome::None);
            };
            let previous_visible = model.visible_ids();
            if model.set_selected_header_folded(!folded) {
                model.clear_marks();
                model.reanchor_selection(Some(header_id), &previous_visible);
                model.clear_message();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FoldHeader | BoardIntent::UnfoldHeader => {
            if !model.task_list_owns_input() {
                return Ok(IntentOutcome::None);
            }
            let previous_visible = model.visible_ids();
            let header = model.selection_id;
            if model.set_selected_header_folded(intent == BoardIntent::FoldHeader) {
                model.clear_marks();
                model.reanchor_selection(header, &previous_visible);
                model.clear_message();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::OpenGroupTarget(target) => {
            return open_group_target(model, target);
        }
        BoardIntent::OpenGroupPicker | BoardIntent::OpenSortPicker => {
            if model.project_picker.is_some()
                || model.popup == BoardPopup::SaveRecovery
                || !model.task_list_owns_input()
            {
                return Ok(IntentOutcome::None);
            }
            model.close_command_surface();
            model.close_popup();
            model.close_help();
            if matches!(intent, BoardIntent::OpenGroupPicker) {
                model.open_group_picker();
            } else {
                model.open_sort_picker();
            }
            if model.list_picker.is_some() {
                model.input_mode = BoardInputMode::ListPicker;
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SwitchListPickerTab(forward) => {
            model.switch_list_picker_tab(None, forward);
            return Ok(IntentOutcome::None);
        }
        // The app-level save-recovery boundary handles these while unresolved.
        BoardIntent::RetrySave | BoardIntent::CancelSave => return Ok(IntentOutcome::None),
        BoardIntent::ToggleStep => {
            model.close_popup();
            let Some((task_id, step_id)) = selected_step(domain, model) else {
                return Ok(IntentOutcome::None);
            };
            domain.with_steps_undo(task_id, |domain| domain.toggle_step(task_id, step_id))?;
        }
        BoardIntent::DispatchAgain => {
            // Host work and its one durable save are owned by the application boundary.
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ConfirmCleanup | BoardIntent::KeepCleanup | BoardIntent::MergeCleanup => {
            // Cleanup host work and completion persistence are owned by the app boundary.
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CancelCleanup => {
            model.cancel_cleanup_card();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ConfirmDispatch | BoardIntent::StartWithoutRelaunch => {
            // Launches, starts, and their saves are owned by the application boundary.
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CancelDispatch => {
            // The card keeps the marks: Esc returns to the same marked set.
            model.close_popup();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CleanupScrollUp | BoardIntent::CleanupScrollDown => {
            model.scroll_cleanup(intent == BoardIntent::CleanupScrollDown);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CleanupSelectUp | BoardIntent::CleanupSelectDown => {
            model.step_cleanup_selection(intent == BoardIntent::CleanupSelectDown);
            return Ok(IntentOutcome::None);
        }
        // The application boundary resolves these to the option they run.
        BoardIntent::CleanupRunSelected | BoardIntent::CleanupClickOption(_) => {
            return Ok(IntentOutcome::None);
        }
        BoardIntent::MoveStepUp | BoardIntent::MoveStepDown => {
            model.close_popup();
            let Some((task_id, step_id)) = selected_step(domain, model) else {
                return Ok(IntentOutcome::None);
            };
            let Some(form) = model.form.as_mut().filter(|form| form.is_task()) else {
                return Ok(IntentOutcome::None);
            };
            let steps = &domain.get(task_id).expect("selected_step checked").steps;
            let from = steps
                .iter()
                .position(|step| step.id == step_id)
                .expect("selected_step checked");
            // Swap with the nearest painted neighbour; staged removals are already gone from
            // the page, so a move never seems to stall behind an invisible row.
            let visible = |index: &usize| !form.steps.removals.contains(&steps[*index].id);
            let target = if intent == BoardIntent::MoveStepUp {
                (0..from).rev().find(visible)
            } else {
                (from + 1..steps.len()).find(visible)
            };
            let Some(target) = target else {
                return Ok(IntentOutcome::None);
            };
            // One undo step per move, like every other step change (T241).
            domain.with_steps_undo(task_id, |domain| domain.move_step(task_id, step_id, target))?;
            // The renderer-recorded heights are source-aligned; reorder them with the step so
            // the scroll below measures the new layout, not the one painted before the move.
            {
                let mut counts = form.steps.row_counts.borrow_mut();
                if from < counts.len() && target < counts.len() {
                    let moved = counts.remove(from);
                    counts.insert(target, moved);
                }
            }
            form.steps.cursor = Some(target);
            form.steps.add_selected = false;
            steps_scroll_to_cursor(form, target);
        }
        BoardIntent::PrimaryVerb => {
            model.close_popup();
            // Status verbs always act on tasks, even with a step selected: Enter owns steps.
            let (targets, bulk) = take_verb_targets(model);
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            let mut moving = Vec::with_capacity(targets.len());
            let mut skipped_done = 0usize;
            for id in targets {
                let Some(status) = domain.get(id).map(|task| task.status) else {
                    model.set_message("that task is no longer here");
                    return Ok(IntentOutcome::None);
                };
                if matches!(status, HumanStatus::Open | HumanStatus::Ready) {
                    moving.push(id);
                } else if status == HumanStatus::Done {
                    skipped_done += 1;
                }
            }
            // A marked set that held done tasks says so: start never reopens them.
            if bulk && skipped_done > 0 {
                model.set_message(format!(
                    "started {} · skipped {skipped_done} done (ctrl+n or ctrl+o reopens them)",
                    moving.len()
                ));
            }
            if !domain.change_status(&moving, HumanStatus::Started, bulk)? {
                return Ok(IntentOutcome::None);
            }
        }
        BoardIntent::ToggleBlock => {
            model.close_popup();
            let bulk = model.task_list_owns_input()
                && model.mark_mode_active()
                && model.marked_count() > 0;
            let targets = model.verb_target_ids();
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            // A done task alone refuses; a marked set skips its done tasks, as start does.
            let (targets, skipped_done) = without_done(domain, targets);
            if targets.is_empty() {
                model.set_message("completed tasks cannot be blocked");
                return Ok(IntentOutcome::None);
            }
            let all_blocked = targets.iter().all(|id| {
                domain
                    .get(*id)
                    .is_some_and(|task| task.status == HumanStatus::Blocked)
            });
            if !all_blocked {
                // Blocking asks why first. The marks stay until the card confirms.
                super::block::open_block_card(model, targets);
                note_skipped_done(model, skipped_done);
                return Ok(IntentOutcome::None);
            }
            model.clear_marks();
            note_skipped_done(model, skipped_done);
            domain.change_status(&targets, HumanStatus::Ready, bulk)?;
        }
        BoardIntent::ToggleReview => {
            model.close_popup();
            let bulk = model.task_list_owns_input()
                && model.mark_mode_active()
                && model.marked_count() > 0;
            let targets = model.verb_target_ids();
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            let (targets, skipped_done) = without_done(domain, targets);
            if targets.is_empty() {
                model.set_message("completed tasks cannot go to review");
                return Ok(IntentOutcome::None);
            }
            let all_review = targets.iter().all(|id| {
                domain
                    .get(*id)
                    .is_some_and(|task| task.status == HumanStatus::Review)
            });
            if !all_review {
                // Review asks what was done first. The marks stay until the card confirms.
                super::block::open_review_card(model, targets);
                note_skipped_done(model, skipped_done);
                return Ok(IntentOutcome::None);
            }
            model.clear_marks();
            note_skipped_done(model, skipped_done);
            domain.change_status(&targets, HumanStatus::Ready, bulk)?;
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
            // Headers are chrome, never a task. Enter on a group sub-header opens the
            // group's board; on the inbox or archived header it does nothing (←/→ fold).
            if model.archived_header_selected() || model.inbox_header_selected() {
                return Ok(IntentOutcome::None);
            }
            if let Some(target) = model
                .selection_id
                .and_then(|id| model.group_header_target(id))
            {
                return open_group_target(model, target);
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
            // Flip that one group and select its header. The drawer is already open,
            // because the header only paints inside it.
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
        BoardIntent::HelpQueryBackspace | BoardIntent::HelpQueryDeleteWord => {
            if model.input_mode == BoardInputMode::Help {
                if intent == BoardIntent::HelpQueryDeleteWord {
                    delete_last_word(&mut model.help_query);
                } else {
                    model.help_query.pop();
                }
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
                    // Still the same task: the PAPER TRAIL keeps how it was expanded.
                    let trail_expanded = form.block.trail_expanded;
                    let trail_open = std::mem::take(&mut form.block.trail_open);
                    *form = BoardForm::task(
                        &task,
                        model.this_repo.as_deref(),
                        &model.tasks,
                        CaptureField::Title,
                        &model.archived_projects,
                        &model.agent_names,
                    );
                    form.block.trail_expanded = trail_expanded;
                    form.block.trail_open = trail_open;
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
        // Palette **set status: blocked** asks why like `ctrl+b`, over the targets not yet
        // blocked; it never unblocks.
        BoardIntent::SetStatus(HumanStatus::Blocked) => {
            model.close_popup();
            let targets = model.verb_target_ids();
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            let targets: Vec<Uuid> = targets
                .into_iter()
                .filter(|id| {
                    domain
                        .get(*id)
                        .is_some_and(|task| task.status != HumanStatus::Blocked)
                })
                .collect();
            if !targets.is_empty() {
                super::block::open_block_card(model, targets);
            }
            return Ok(IntentOutcome::None);
        }
        // Palette **set status: review** asks what was done like `ctrl+r`, over the targets not
        // yet in review; it never returns a review to ready.
        BoardIntent::SetStatus(HumanStatus::Review) => {
            model.close_popup();
            let targets = model.verb_target_ids();
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            let targets: Vec<Uuid> = targets
                .into_iter()
                .filter(|id| {
                    domain
                        .get(*id)
                        .is_some_and(|task| task.status != HumanStatus::Review)
                })
                .collect();
            if !targets.is_empty() {
                super::block::open_review_card(model, targets);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SetStatus(status) => {
            let (targets, bulk) = take_verb_targets(model);
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            if !domain.change_status(&targets, status, bulk)? {
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
            let (targets, bulk) = take_verb_targets(model);
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            if !domain.change_status(&targets, HumanStatus::Open, bulk)? {
                return Ok(IntentOutcome::None);
            }
        }
        BoardIntent::SoftDelete => {
            model.close_popup();
            // On the BLOCKED section a selected reply is the target, never the task: yours
            // soft-deletes to a stub, an agent's refuses.
            if model.input_mode == BoardInputMode::TaskPage {
                if super::block::selected_own_reply(model).is_some() {
                    model.pending_delete = None;
                    model.clear_marks();
                    return super::block::delete_selected_reply(domain, model);
                }
                if super::block::agent_reply_selected(model) {
                    model.pending_delete = None;
                    model.set_message("agent replies cannot be deleted");
                    return Ok(IntentOutcome::None);
                }
            }
            // In a new-step row the delete verb discards that draft row, as Esc does. It
            // never reaches the step the row was opened below, nor the task.
            if model.input_mode == BoardInputMode::EditStep
                && model
                    .form
                    .as_ref()
                    .and_then(|form| form.steps.editor.as_ref())
                    .is_some_and(|editor| editor.rename.is_none())
            {
                let capture = model.form.as_ref().is_some_and(|form| !form.is_task());
                close_step_editor(model);
                if capture {
                    if let Some(form) = model.form.as_ref() {
                        model.input_mode = form.parent_mode();
                    }
                }
                return Ok(IntentOutcome::None);
            }
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
                    // A numbered task is named by its `T` number too; a notice task (`N…`)
                    // keeps its title alone.
                    let named = targets.first().and_then(|id| domain.get(*id)).map(|task| {
                        let label = task
                            .notice
                            .is_none()
                            .then(|| task.board_identifier())
                            .flatten();
                        (label, task.title.clone())
                    });
                    // A ready task the delete leaves waiting on nothing stays ready: say so.
                    let unwaited = domain.unwaited_by_delete(&targets);
                    if bulk {
                        domain.soft_delete_batch(&targets)?;
                        model.arm_bulk_delete_notice(targets.len());
                    } else {
                        let id = targets[0];
                        domain.soft_delete(id)?;
                        if let Some((label, title)) = named {
                            model.arm_delete_notice(label.as_deref(), &title);
                        }
                    }
                    if !unwaited.is_empty() {
                        let waiting = unwaited
                            .iter()
                            .map(|(waiting, _)| format!("T{waiting}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let mut deleted: Vec<u64> =
                            unwaited.iter().map(|(_, deleted)| *deleted).collect();
                        deleted.sort_unstable();
                        deleted.dedup();
                        let deleted = deleted
                            .iter()
                            .map(|number| format!("T{number}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        model.set_message(format!("{waiting} no longer waits ({deleted} deleted)"));
                    }
                    model.undo_hint = domain.last_undo().cloned();
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
            let Some(path) = model.launch_card.as_ref().map(|card| card.path.clone()) else {
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
            let Some(path) = model.launch_card.take().map(|card| card.path) else {
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
            let (targets, bulk) = take_verb_targets(model);
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            if !domain.file_tasks(&targets, bulk)? {
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
            // ctrl+u right after an archive undoes it. Otherwise ctrl+u on an archived
            // selection is the unarchive route, never an undo: the stack is not popped (AC-3).
            let archive_on_top = domain.top_undo_is_live()
                && domain.last_undo().is_some_and(|entry| {
                    entry
                        .leaf_entries()
                        .iter()
                        .all(|leaf| matches!(leaf, UndoEntry::Archive { .. }))
                });
            if let Some(id) = model.selected_id().filter(|_| !archive_on_top) {
                if domain.get(id).is_some_and(|task| task.archived) {
                    domain.unarchive_task(id)?;
                    let name = task_names(domain, &[id]);
                    model.sync_from_domain(domain);
                    model.set_message(format!("{name} unarchived"));
                    return Ok(IntentOutcome::Persist);
                }
            }
            let Some(entry) = domain.last_undo().cloned() else {
                model.set_message(NOTHING_TO_UNDO);
                return Ok(IntentOutcome::None);
            };
            let note = UndoNote::read(domain, &entry);
            if let Err(error) = domain.undo() {
                if let DomainError::StaleUndo(id) = error {
                    model.sync_from_domain(domain);
                    // Nothing moved, so this reports nothing to persist -- which is what
                    // puts the way back on the row beside the refusal that explains why
                    // this attempt did not take.
                    model.set_message(stale_undo_message(domain, id));
                    return Ok(IntentOutcome::None);
                }
                return Err(error);
            }
            model.sync_from_domain(domain);
            if let Some(note) = note {
                model.set_message(note.message(domain));
            }
            return Ok(IntentOutcome::Persist);
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
    if model.input_mode() == BoardInputMode::BlockCard {
        if let Some(buffer) = model
            .block_card
            .as_mut()
            .and_then(|card| card.focused_buffer_mut())
        {
            operation(buffer);
        }
        return;
    }
    if model.input_mode == BoardInputMode::EditReply {
        if let Some(buffer) = super::block::reply_buffer_mut(model) {
            operation(buffer);
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
        CaptureField::Scope | CaptureField::Assignee | CaptureField::Base | CaptureField::After => {
        }
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

/// Enter or a double-click on a group sub-header: open its project's board, the desk, or
/// its thread's or assignee's cross-project view. A preview seat cannot leave the projects tab itself, so
/// it hands the request to the outer board.
fn open_group_target(
    model: &mut BoardModel,
    target: GroupTarget,
) -> Result<IntentOutcome, DomainError> {
    // Nothing to open leaves the search, marks, and fold untouched.
    if !model.group_target_opens(&target) {
        return Ok(IntentOutcome::None);
    }
    if model.preview_seat {
        model.outer_request = Some(BoardIntent::OpenGroupTarget(target));
        return Ok(IntentOutcome::None);
    }
    if model
        .right_seat()
        .is_some_and(|right| right.has_unsaved_work())
    {
        model.set_message(DIRTY_TASK_SWITCH_REFUSAL);
        return Ok(IntentOutcome::None);
    }
    model.clear_marks();
    match target {
        GroupTarget::Project(path) => {
            model.set_board_scope(ProjectScopeOption::Project(PathBuf::from(path)))
        }
        GroupTarget::Desk => model.set_board_scope(ProjectScopeOption::Home),
        GroupTarget::Thread(name) => model.open_thread_view(name),
        GroupTarget::Assignee(name) => model.open_assignee_view(name),
        GroupTarget::None => {}
    }
    model.clear_message();
    Ok(IntentOutcome::None)
}

/// The row double-click window: a second click on the same row within it opens the page.
pub const ROW_DOUBLE_CLICK_WINDOW: std::time::Duration = std::time::Duration::from_millis(400);

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
    let lifted = crate::capture::lift_quick_add_tokens(
        quick_add.title.value(),
        domain,
        quick_add.snapshot.as_ref().as_ref(),
        &quick_add.default,
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
    let lifted = match crate::capture::lift_quick_add_tokens(
        quick_add.title.value(),
        domain,
        quick_add.snapshot.as_ref().as_ref(),
        &quick_add.default,
        &model.agent_names,
    ) {
        Ok(lifted) => lifted,
        Err(message) => {
            model.set_message(message);
            return Ok(IntentOutcome::None);
        }
    };
    let scope = lifted.scope.unwrap_or_else(|| quick_add.scope.clone());
    match crate::capture::capture_save_configured(
        domain,
        None,
        &snapshot,
        lifted.title,
        None,
        Some(scope.clone()),
        lifted.thread,
        lifted.assignee,
        lifted.base,
    ) {
        Ok(id) => {
            domain.set_after_on_create(id, &lifted.after)?;
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
    extra_step: Option<(&str, Option<Uuid>)>,
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
    let base = form.base.clone();
    let after = form.after.clone();
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
    let task = domain.get(id).ok_or(DomainError::UnknownId(id))?.clone();
    // An existing step emptied in this session is refused the way a new empty step is: on
    // its own row, with its editor open, never by a status-row message beside a blank row.
    let emptied = task.steps.iter().enumerate().find(|(_, step)| {
        step_renames
            .get(&step.id)
            .is_some_and(|text| text.is_empty())
    });
    if let Some((index, step)) = emptied {
        refuse_empty_step_rename(model, index, step.id);
        return Ok(IntentOutcome::None);
    }
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
    if let Some(branch) = base
        .as_deref()
        .filter(|_| base != task.base || scope != task.scope)
    {
        if let Err(message) = crate::git_base::validate_task_base(&scope, branch, false) {
            model.set_message(message);
            return Ok(IntentOutcome::None);
        }
    }

    // Refused before anything changes: the draft stays for another choice.
    if after != task.after {
        if let Err(error) = domain.check_after(id, &after) {
            model.set_message(error.to_string());
            return Ok(IntentOutcome::None);
        }
    }

    // The full page session changes under one revision, then the app persists exactly once.
    // A chained `edit` plus `rename_step` sequence would advance the merge base after each
    // draft and make the store correctly refuse the later revision as a conflicting writer.
    let step_rename_list = step_renames
        .iter()
        .map(|(step_id, text)| (*step_id, text.clone()))
        .collect::<Vec<_>>();
    let extra_steps = extra_step
        .map(|(text, _)| vec![text.trim().to_string()])
        .unwrap_or_default();
    let extra_after = extra_step.and_then(|(_, insert_after)| insert_after);
    // Only the steps part of the save is undoable.
    domain.with_steps_undo(id, |domain| {
        domain.edit_with_step_changes_and_base(
            id,
            &title,
            notes.clone(),
            scope.clone(),
            thread.clone(),
            assignee.clone(),
            base.clone(),
            &step_rename_list,
            &step_removals.iter().copied().collect::<Vec<_>>(),
            &extra_steps,
            extra_after,
        )?;
        domain.stage_after_in_edit(id, &after)
    })?;

    // Retain the complete form and mode until the persistence boundary confirms this exact
    // task-session save. A failed save can then Retry or Cancel without orphaning drafts.
    model.task_edit_save = Some(TaskEditSave {
        id,
        title,
        notes,
        scope,
        thread,
        assignee,
        base,
        after,
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

/// The page ring's last stop, where Shift+Tab wraps from its first: the PAPER TRAIL's last stop
/// (its heading, or its last record while expanded), else `+ step`.
fn select_last_page_stop(model: &mut BoardModel) {
    if !super::block::enter_trail(model, false) {
        select_add_step(model);
    }
}

/// Move Tab within the PAPER TRAIL when one of its records is selected; `None` otherwise.
fn trail_tab(model: &mut BoardModel, forward: bool) -> Option<super::block::PageTab> {
    match super::block::move_trail_tab(model, forward) {
        super::block::PageTab::NotHandled => None,
        moved => Some(moved),
    }
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
    domain.with_steps_undo(task_id, |domain| domain.remove_step(task_id, step_id))?;
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
            insert_after: None,
            refusal: None,
        });
        model.input_mode = BoardInputMode::EditStep;
        model.clear_message();
    }
}

/// Reopen the emptied existing step at `index` with the inline `text required` refusal.
///
/// A typed new-step row that is open when the session save is refused would lose its text if
/// the emptied step took its place, so it stays open and the status row says what is wrong.
fn refuse_empty_step_rename(model: &mut BoardModel, index: usize, step_id: Uuid) {
    let typed_add_open = model.form.as_ref().is_some_and(|form| {
        form.steps.editor.as_ref().is_some_and(|editor| {
            editor.rename.is_none() && !editor.buffer.value().trim().is_empty()
        })
    });
    if typed_add_open {
        model.set_message(EMPTIED_STEP_REFUSAL);
        return;
    }
    if let Some(form) = model.form.as_mut() {
        form.steps.cursor = Some(index);
        steps_scroll_to_cursor(form, index);
    }
    open_step_editor(model, "", Some(step_id));
    if let Some(editor) = model
        .form
        .as_mut()
        .and_then(|form| form.steps.editor.as_mut())
    {
        editor.refusal = Some(STEP_TEXT_REQUIRED.to_string());
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
            insert_after: None,
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

/// The status-row form of the same refusal, for when an open new-step row keeps the editor.
const EMPTIED_STEP_REFUSAL: &str = "a cleared step needs text";

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
            .map(|editor| {
                (
                    editor.buffer.value().trim().to_string(),
                    editor.insert_after,
                )
            })
    });
    let Some((text, insert_after)) = extra_step.filter(|(text, _)| !text.is_empty()) else {
        return confirm_step_editor(domain, model, false, true);
    };
    confirm_edit(domain, model, Some((&text, insert_after)))
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
            insert_after: None,
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
    let index = editor
        .insert_after
        .and_then(|anchor| {
            domain
                .get(task_id)?
                .steps
                .iter()
                .position(|step| step.id == anchor)
        })
        .map_or(usize::MAX, |anchor| anchor + 1);
    let touched =
        match domain.with_steps_undo(task_id, |domain| domain.insert_step(task_id, &text, index)) {
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
