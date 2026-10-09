//! Blocks on the board: the block card (`ctrl+b`), the task page's BLOCKED section ring, and
//! its reply box.

use std::cell::{Cell, RefCell};

use uuid::Uuid;

use crate::domain::{
    Block, BlockDraft, BlockField, BlockKey, BlockOn, BlockPatch, DomainError, DomainState,
    HumanStatus, Task, BLOCK_TEXT_MAX, OWNER,
};
use crate::ui::edit::{seeded_draft, EditBuffer};
use crate::ui::mouse::BoardPopup;

use super::model::{BoardInputMode, BoardModel, IntentOutcome};

/// One Tab stop of the task page's BLOCKED section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockTarget {
    Heading,
    Option(usize),
    Reply(usize),
}

/// Session state of the BLOCKED section, carried by the task page form.
#[derive(Debug, Clone, Default)]
pub(crate) struct BlockPageState {
    /// The selected ring stop. A selected step outranks it, so a stale target is inert.
    pub(crate) target: Option<BlockTarget>,
    /// The open reply box.
    pub(crate) reply: Option<ReplyEditor>,
    /// Absolute content row of each ring stop at the last painted width, recorded by the
    /// renderer so Tab can keep the selected stop inside the page viewport.
    pub(crate) rows: RefCell<Vec<(BlockTarget, usize)>>,
}

/// The reply box under the last reply.
#[derive(Debug, Clone)]
pub(crate) struct ReplyEditor {
    pub(crate) buffer: EditBuffer,
    /// The block the box was opened on. A refresh that closed or replaced it refuses the save
    /// and keeps the draft, so an answer never lands on a different question.
    pub(crate) block: BlockKey,
    /// The index of the owner's reply this box rewrites, or `None` for a new reply.
    pub(crate) edit: Option<usize>,
    pub(crate) refusal: Option<String>,
    /// A stored reply the save boundary has not confirmed yet. The box and its mode are held
    /// until the synced task carries it.
    pub(crate) pending: Option<ReplySave>,
    /// The wrap width the last painted frame used, for vertical caret movement.
    pub(crate) width: Cell<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReplySave {
    pub(crate) task: Uuid,
    pub(crate) index: usize,
    pub(crate) text: String,
    pub(crate) unblock: bool,
}

/// The block card's fields, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockCardField {
    Why,
    On,
    Needs,
}

/// The card's waiting-on choice; `task` and `other` take the typed text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OnKind {
    #[default]
    You,
    Task,
    Other,
}

impl OnKind {
    pub fn label(self) -> &'static str {
        match self {
            OnKind::You => "you",
            OnKind::Task => "task",
            OnKind::Other => "other",
        }
    }

    fn cycle(self, forward: bool) -> Self {
        match (self, forward) {
            (OnKind::You, true) | (OnKind::Other, false) => OnKind::Task,
            (OnKind::Task, true) | (OnKind::You, false) => OnKind::Other,
            (OnKind::Other, true) | (OnKind::Task, false) => OnKind::You,
        }
    }
}

/// The block card: why, on and needs for one task or a marked set.
#[derive(Debug, Clone)]
pub struct BlockCard {
    /// Tasks the card blocks, captured when it opened.
    pub(crate) targets: Vec<Uuid>,
    /// Editing this task's open block instead of blocking.
    pub(crate) edit: Option<Uuid>,
    /// The block the edit card was opened on.
    pub(crate) block: Option<BlockKey>,
    /// The blocks the save boundary has not confirmed yet. The card, its mode and the marks
    /// are held until the synced tasks carry them; a cancelled save releases the hold.
    pub(crate) pending: Option<Vec<(Uuid, Block)>>,
    pub(crate) why: EditBuffer,
    pub(crate) on_kind: OnKind,
    pub(crate) on_text: EditBuffer,
    pub(crate) needs: EditBuffer,
    pub(crate) field: BlockCardField,
    pub(crate) refusal: Option<String>,
}

impl BlockCard {
    fn new(targets: Vec<Uuid>) -> Self {
        Self {
            targets,
            edit: None,
            block: None,
            pending: None,
            why: seeded_draft(""),
            on_kind: OnKind::You,
            on_text: seeded_draft(""),
            needs: seeded_draft(""),
            field: BlockCardField::Why,
            refusal: None,
        }
    }

    fn editing(task: &Task) -> Self {
        let block = task.block.as_ref();
        let (on_kind, on_text) = match block.map(|block| &block.on) {
            Some(BlockOn::Task(number)) => (OnKind::Task, number.to_string()),
            Some(BlockOn::Other(text)) => (OnKind::Other, text.clone()),
            _ => (OnKind::You, String::new()),
        };
        Self {
            edit: Some(task.id),
            block: block.map(Block::key),
            why: seeded_draft(block.and_then(|block| block.why.as_deref()).unwrap_or("")),
            on_kind,
            on_text: seeded_draft(&on_text),
            needs: seeded_draft(block.and_then(|block| block.needs.as_deref()).unwrap_or("")),
            ..Self::new(vec![task.id])
        }
    }

    pub fn targets(&self) -> &[Uuid] {
        &self.targets
    }

    pub fn is_edit(&self) -> bool {
        self.edit.is_some()
    }

    pub fn field(&self) -> BlockCardField {
        self.field
    }

    pub fn on_kind(&self) -> OnKind {
        self.on_kind
    }

    pub fn refusal(&self) -> Option<&str> {
        self.refusal.as_deref()
    }

    /// Whether a confirmed card waits on the save boundary.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn why(&self) -> &EditBuffer {
        &self.why
    }

    pub(crate) fn on_text(&self) -> &EditBuffer {
        &self.on_text
    }

    pub(crate) fn needs(&self) -> &EditBuffer {
        &self.needs
    }

    /// The buffer of the focused field, or `None` while a save holds the card.
    pub(crate) fn focused_buffer_mut(&mut self) -> Option<&mut EditBuffer> {
        if self.pending.is_some() {
            return None;
        }
        self.refusal = None;
        Some(match self.field {
            BlockCardField::Why => &mut self.why,
            BlockCardField::On => &mut self.on_text,
            BlockCardField::Needs => &mut self.needs,
        })
    }

    fn move_field(&mut self, forward: bool) {
        self.field = match (self.field, forward) {
            (BlockCardField::Why, true) | (BlockCardField::Needs, false) => BlockCardField::On,
            (BlockCardField::On, true) | (BlockCardField::Why, false) => BlockCardField::Needs,
            (BlockCardField::Needs, true) | (BlockCardField::On, false) => BlockCardField::Why,
        };
    }

    /// Resolve the typed waiting-on value against the board. A task must exist and must not
    /// be one of the card's own targets.
    fn resolve_on(&self, domain: &DomainState) -> Result<BlockOn, String> {
        let text = self.on_text.value().trim();
        match self.on_kind {
            OnKind::You => Ok(BlockOn::You),
            OnKind::Other if text.is_empty() => Err("say what it waits on".to_string()),
            OnKind::Other => Ok(BlockOn::Other(text.to_string())),
            OnKind::Task => {
                let BlockOn::Task(number) = BlockOn::parse_input(text) else {
                    return Err("type a task number, like T12".to_string());
                };
                let found = domain.tasks().iter().find(|task| {
                    task.number == Some(number) && !task.soft_deleted && !task.is_notice()
                });
                match found {
                    Some(task) if self.targets.contains(&task.id) => {
                        Err(format!("T{number} cannot wait on itself"))
                    }
                    Some(_) => Ok(BlockOn::Task(number)),
                    None => Err(format!("T{number} is not on the board")),
                }
            }
        }
    }
}

/// The reply box's block was closed or replaced elsewhere; the draft stays.
pub(crate) const BLOCK_REPLACED: &str = "this block was closed or replaced elsewhere; reply kept";

fn too_long(field: BlockField) -> String {
    format!("{} is longer than {BLOCK_TEXT_MAX} bytes", field.name())
}

/// `ctrl+b` on non-blocked work: open the card over `targets`. Marks stay until it confirms.
pub(super) fn open_block_card(model: &mut BoardModel, targets: Vec<Uuid>) {
    model.block_card = Some(BlockCard::new(targets));
    model.set_popup(BoardPopup::BlockCard);
    model.clear_message();
}

/// `ctrl+e` on the BLOCKED heading: the same card, prefilled, editing why, on and needs.
pub(super) fn open_block_edit_card(model: &mut BoardModel, task: &Task) {
    model.block_card = Some(BlockCard::editing(task));
    model.set_popup(BoardPopup::BlockCard);
    model.clear_message();
}

/// `Esc` on the card. A card held by a save stays until the save resolves.
pub(super) fn cancel_block_card(model: &mut BoardModel) {
    if !model.block_card.as_ref().is_some_and(BlockCard::is_pending) {
        close_block_card(model);
    }
}

pub(super) fn close_block_card(model: &mut BoardModel) {
    model.block_card = None;
    if model.popup() == BoardPopup::BlockCard {
        model.set_popup(BoardPopup::None);
    }
}

pub(super) fn block_card_field(model: &mut BoardModel, forward: bool) {
    if let Some(card) = model.block_card.as_mut().filter(|card| !card.is_pending()) {
        card.refusal = None;
        card.move_field(forward);
    }
}

/// `←`/`→`: cycle the waiting-on choice on the On field, move the caret elsewhere.
pub(super) fn block_card_arrow(model: &mut BoardModel, forward: bool) {
    let Some(card) = model.block_card.as_mut().filter(|card| !card.is_pending()) else {
        return;
    };
    card.refusal = None;
    match card.field {
        BlockCardField::On => card.on_kind = card.on_kind.cycle(forward),
        _ => {
            let Some(buffer) = card.focused_buffer_mut() else {
                return;
            };
            if forward {
                buffer.move_right();
            } else {
                buffer.move_left();
            }
        }
    }
}

/// Enter on the card: block every target with one draft as one batch and one undo entry, or
/// edit the open block in place. A refusal keeps the card open with its own line. A stored
/// change holds the card and the marks until the synced tasks carry it.
pub(super) fn confirm_block_card(
    domain: &mut DomainState,
    model: &mut BoardModel,
) -> Result<IntentOutcome, DomainError> {
    let Some(card) = model.block_card.as_ref().filter(|card| !card.is_pending()) else {
        return Ok(IntentOutcome::None);
    };
    let on = match card.resolve_on(domain) {
        Ok(on) => on,
        Err(refusal) => {
            if let Some(card) = model.block_card.as_mut() {
                card.refusal = Some(refusal);
            }
            return Ok(IntentOutcome::None);
        }
    };
    let draft =
        match BlockDraft::from_input(Some(card.why.value()), Some(card.needs.value()), &[], on) {
            Ok(draft) => draft,
            Err(field) => {
                if let Some(card) = model.block_card.as_mut() {
                    card.refusal = Some(too_long(field));
                }
                return Ok(IntentOutcome::None);
            }
        };
    let targets = card.targets.clone();
    let edit = card.edit;
    let opened_on = card.block.clone();
    let changed = match edit {
        Some(id) => {
            let current = domain
                .get(id)
                .and_then(|task| task.block.as_ref())
                .map(Block::key);
            if current.is_none() || current != opened_on {
                close_block_card(model);
                model.set_message("that block was closed elsewhere");
                return Ok(IntentOutcome::None);
            }
            // The card has no options field: edit why, on and needs only.
            domain.edit_block(
                id,
                BlockPatch {
                    why: Some(draft.why),
                    needs: Some(draft.needs),
                    options: None,
                    on: Some(draft.on),
                },
            )?
        }
        None => {
            let live: Vec<Uuid> = targets
                .iter()
                .copied()
                .filter(|id| domain.get(*id).is_some())
                .collect();
            domain.block_batch(&live, &draft, OWNER)?
        }
    };
    if !changed {
        close_block_card(model);
        if edit.is_none() {
            model.clear_marks();
        }
        return Ok(IntentOutcome::None);
    }
    let stored = targets
        .iter()
        .filter_map(|id| {
            let block = domain.get(*id)?.block.clone()?;
            Some((*id, block))
        })
        .collect();
    if let Some(card) = model.block_card.as_mut() {
        card.refusal = None;
        card.pending = Some(stored);
    }
    Ok(IntentOutcome::Persist)
}

/// Close a held card once the synced tasks carry its blocks (the confirmed landing), and
/// clear the marked set it blocked.
pub(super) fn finish_block_card_save(model: &mut BoardModel) {
    let Some(card) = model.block_card.as_ref() else {
        return;
    };
    let Some(pending) = card.pending.as_ref() else {
        return;
    };
    let landed = pending.iter().all(|(id, block)| {
        model
            .tasks
            .iter()
            .find(|task| task.id == *id)
            .is_some_and(|task| task.block.as_ref() == Some(block))
    });
    if !landed {
        return;
    }
    let edit = card.edit.is_some();
    close_block_card(model);
    if !edit {
        model.clear_marks();
    }
}

/// A cancelled failed save rolled the blocks back: reopen the card with its drafts and keep
/// the marks, so Enter tries the same set again.
pub(super) fn release_cancelled_block_card(model: &mut BoardModel) {
    let Some(card) = model.block_card.as_mut() else {
        return;
    };
    if card.pending.take().is_some() {
        model.set_popup(BoardPopup::BlockCard);
    }
}

/// The task the page shows, when it has an open block and no edit session owns the page.
fn page_block_task(model: &BoardModel) -> Option<&Task> {
    let form = model.form.as_ref().filter(|form| form.is_task())?;
    if form.editing {
        return None;
    }
    let id = form.task_id()?;
    model
        .tasks
        .iter()
        .find(|task| task.id == id)
        .filter(|task| task.status == HumanStatus::Blocked && task.block.is_some())
}

/// The ring stops of the page's open block, in order.
pub(super) fn block_ring(task: &Task) -> Vec<BlockTarget> {
    let Some(block) = task.block.as_ref() else {
        return Vec::new();
    };
    let mut ring = vec![BlockTarget::Heading];
    ring.extend((0..block.options.len()).map(BlockTarget::Option));
    ring.extend(
        block
            .replies
            .iter()
            .enumerate()
            .filter(|(_, reply)| !reply.deleted)
            .map(|(index, _)| BlockTarget::Reply(index)),
    );
    ring
}

/// The selected BLOCKED stop. A selected step or `+ step` outranks it.
pub(super) fn selected_block_target(model: &BoardModel) -> Option<BlockTarget> {
    let form = model.form.as_ref()?;
    if form.steps.cursor.is_some() || form.steps.add_selected {
        return None;
    }
    let target = form.block.target?;
    let task = page_block_task(model)?;
    block_ring(task).contains(&target).then_some(target)
}

fn select_block_target(model: &mut BoardModel, target: Option<BlockTarget>) {
    let Some(form) = model.form.as_mut() else {
        return;
    };
    form.block.target = target;
    if target.is_some() {
        form.steps.cursor = None;
        form.steps.add_selected = false;
    }
    let Some(target) = target else {
        return;
    };
    let row = form
        .block
        .rows
        .borrow()
        .iter()
        .find(|(stop, _)| *stop == target)
        .map(|(_, row)| *row);
    if let Some(row) = row {
        let rows = form.steps.window_rows.get().max(1);
        if row < form.notes_scroll {
            form.notes_scroll = row;
        } else if row >= form.notes_scroll.saturating_add(rows) {
            form.notes_scroll = row + 1 - rows;
        }
        form.notes_scroll = form.notes_scroll.min(form.notes_max_scroll.get());
    }
}

/// Where Tab goes next on a blocked task page.
pub(super) enum PageTab {
    /// The ring moved within the BLOCKED section.
    Moved,
    /// Leave the section for the first step (forward) or nothing (backward).
    LeaveBlock,
    /// Not on the section: enter it at its last stop (backward from the first step) or its
    /// heading (forward from nothing or from `+ step`).
    NotHandled,
}

/// Move the page's Tab ring through the BLOCKED section. Steps and `+ step` follow it.
pub(super) fn move_block_tab(model: &mut BoardModel, forward: bool) -> PageTab {
    let Some(task) = page_block_task(model) else {
        return PageTab::NotHandled;
    };
    let ring = block_ring(task);
    let Some(current) = selected_block_target(model) else {
        return PageTab::NotHandled;
    };
    let position = ring.iter().position(|stop| *stop == current).unwrap_or(0);
    let next = if forward {
        ring.get(position + 1).copied()
    } else {
        position
            .checked_sub(1)
            .and_then(|index| ring.get(index))
            .copied()
    };
    match next {
        Some(stop) => {
            select_block_target(model, Some(stop));
            PageTab::Moved
        }
        None => {
            select_block_target(model, None);
            PageTab::LeaveBlock
        }
    }
}

/// Enter the ring at its heading (forward) or its last stop (backward). False without a block.
pub(super) fn enter_block_ring(model: &mut BoardModel, forward: bool) -> bool {
    let Some(task) = page_block_task(model) else {
        return false;
    };
    let ring = block_ring(task);
    let stop = if forward { ring.first() } else { ring.last() }.copied();
    if stop.is_none() {
        return false;
    }
    select_block_target(model, stop);
    true
}

/// `r`, or `Enter` on an option (prefilled), or `ctrl+e` on one of your replies (`edit`).
pub(super) fn begin_reply(model: &mut BoardModel, prefill: &str, edit: Option<usize>) -> bool {
    let Some(block) = page_block_task(model)
        .and_then(|task| task.block.as_ref())
        .map(Block::key)
    else {
        return false;
    };
    let Some(form) = model.form.as_mut() else {
        return false;
    };
    form.block.reply = Some(ReplyEditor {
        buffer: seeded_draft(prefill),
        block,
        edit,
        refusal: None,
        pending: None,
        width: Cell::new(0),
    });
    model.input_mode = BoardInputMode::EditReply;
    model.clear_message();
    true
}

/// The text of the selected option, for `Enter`.
pub(super) fn selected_option_text(model: &BoardModel) -> Option<String> {
    let BlockTarget::Option(index) = selected_block_target(model)? else {
        return None;
    };
    page_block_task(model)?
        .block
        .as_ref()?
        .options
        .get(index)
        .cloned()
}

/// The selected reply's index when it is the owner's and not deleted.
pub(super) fn selected_own_reply(model: &BoardModel) -> Option<(usize, String)> {
    let BlockTarget::Reply(index) = selected_block_target(model)? else {
        return None;
    };
    let reply = page_block_task(model)?.block.as_ref()?.replies.get(index)?;
    (reply.is_owner() && !reply.deleted).then(|| (index, reply.text.clone()))
}

/// The page's task, when the selected stop is the BLOCKED heading.
pub(super) fn heading_selected_task(model: &BoardModel) -> Option<Task> {
    (selected_block_target(model)? == BlockTarget::Heading)
        .then(|| page_block_task(model).cloned())
        .flatten()
}

/// Whether the selected stop is an agent's reply (not editable).
pub(super) fn agent_reply_selected(model: &BoardModel) -> bool {
    let Some(BlockTarget::Reply(index)) = selected_block_target(model) else {
        return false;
    };
    page_block_task(model)
        .and_then(|task| task.block.as_ref())
        .and_then(|block| block.replies.get(index))
        .is_some_and(|reply| !reply.is_owner())
}

/// `ctrl+x` on one of your replies: soft-delete it, keeping a stub.
pub(super) fn delete_selected_reply(
    domain: &mut DomainState,
    model: &mut BoardModel,
) -> Result<IntentOutcome, DomainError> {
    let Some((index, _)) = selected_own_reply(model) else {
        return Ok(IntentOutcome::None);
    };
    let Some(id) = model.form.as_ref().and_then(|form| form.task_id()) else {
        return Ok(IntentOutcome::None);
    };
    domain.delete_reply(id, index)?;
    // The stub is not a ring stop: the heading keeps the cursor on the section.
    if let Some(form) = model.form.as_mut() {
        form.block.target = Some(BlockTarget::Heading);
    }
    Ok(IntentOutcome::Persist)
}

/// `shift+enter` stores the reply; `ctrl+s` stores it and unblocks the task to ready. The box
/// and its mode stay until the synced task carries the reply.
pub(super) fn save_reply(
    domain: &mut DomainState,
    model: &mut BoardModel,
    unblock: bool,
) -> Result<IntentOutcome, DomainError> {
    let Some(form) = model.form.as_ref() else {
        return Ok(IntentOutcome::None);
    };
    let Some(editor) = form.block.reply.as_ref() else {
        return Ok(IntentOutcome::None);
    };
    if editor.pending.is_some() {
        return Ok(IntentOutcome::None);
    }
    let Some(id) = form.task_id() else {
        return Ok(IntentOutcome::None);
    };
    let text = editor.buffer.value().trim().to_string();
    let edit = editor.edit;
    let opened_on = editor.block.clone();
    let refuse = |model: &mut BoardModel, refusal: String| {
        if let Some(editor) = model
            .form
            .as_mut()
            .and_then(|form| form.block.reply.as_mut())
        {
            editor.refusal = Some(refusal);
        }
        Ok(IntentOutcome::None)
    };
    // A refresh may have closed the block the box was opened on, or replaced it with a new
    // one: the answer (or the reply index being edited) belongs to that block only.
    let current = domain
        .get(id)
        .and_then(|task| task.block.as_ref())
        .map(Block::key);
    if current.as_ref() != Some(&opened_on) {
        model.set_message(BLOCK_REPLACED);
        return refuse(model, BLOCK_REPLACED.to_string());
    }
    if text.is_empty() {
        return refuse(model, "type a reply first".to_string());
    }
    if text.len() > BLOCK_TEXT_MAX {
        return refuse(model, too_long(BlockField::Reply));
    }
    // The reply lands on the open block before the unblock closes it, as one change.
    let stored = domain.as_one_change(id, |domain| {
        let index = match edit {
            Some(index) => domain.edit_reply(id, index, &text).map(|()| index)?,
            None => domain.reply(id, &text, OWNER)?,
        };
        if unblock {
            domain.set_status(id, HumanStatus::Ready)?;
        }
        Ok(index)
    });
    let index = match stored {
        Ok(index) => index,
        Err(DomainError::NotBlocked(_) | DomainError::UnknownReply(_)) => {
            return refuse(model, "this block changed elsewhere".to_string());
        }
        Err(error) => return Err(error),
    };
    if let Some(editor) = model
        .form
        .as_mut()
        .and_then(|form| form.block.reply.as_mut())
    {
        editor.refusal = None;
        editor.pending = Some(ReplySave {
            task: id,
            index,
            text,
            unblock,
        });
    }
    Ok(IntentOutcome::Persist)
}

/// Esc in the reply box: discard the draft and return to the page.
pub(super) fn cancel_reply(model: &mut BoardModel) {
    if let Some(form) = model.form.as_mut() {
        form.block.reply = None;
    }
    model.input_mode = BoardInputMode::TaskPage;
    model.clear_message();
}

/// Release a held reply box once the synced task carries its reply (the confirmed landing).
pub(super) fn finish_reply_save(model: &mut BoardModel) {
    let Some(pending) = model
        .form
        .as_ref()
        .and_then(|form| form.block.reply.as_ref())
        .and_then(|editor| editor.pending.clone())
    else {
        return;
    };
    let landed = model
        .tasks
        .iter()
        .find(|task| task.id == pending.task)
        .and_then(|task| {
            if pending.unblock {
                (task.status != HumanStatus::Blocked)
                    .then(|| task.past_blocks.last())
                    .flatten()
            } else {
                task.block.as_ref()
            }
        })
        .and_then(|block| block.replies.get(pending.index))
        .is_some_and(|reply| reply.text == pending.text && !reply.deleted);
    if !landed {
        return;
    }
    if let Some(form) = model.form.as_mut() {
        form.block.reply = None;
        form.block.target = (!pending.unblock).then_some(BlockTarget::Reply(pending.index));
    }
    if model.input_mode == BoardInputMode::EditReply {
        model.input_mode = BoardInputMode::TaskPage;
    }
}

/// A cancelled failed save rolled the reply back: keep the box and its text, drop the hold.
pub(super) fn release_cancelled_reply(model: &mut BoardModel) {
    if let Some(editor) = model
        .form
        .as_mut()
        .and_then(|form| form.block.reply.as_mut())
    {
        editor.pending = None;
    }
}

/// The reply box's draft, for the shared edit intents.
pub(super) fn reply_buffer_mut(model: &mut BoardModel) -> Option<&mut EditBuffer> {
    let editor = model.form.as_mut()?.block.reply.as_mut()?;
    if editor.pending.is_some() {
        return None;
    }
    editor.refusal = None;
    Some(&mut editor.buffer)
}
