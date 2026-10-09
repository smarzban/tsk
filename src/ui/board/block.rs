//! Blocks and reviews on the board: the block card (`ctrl+b`) and review card (`ctrl+r`), the
//! task page's BLOCKED and REVIEW section rings, and their reply (feedback) box.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;

use uuid::Uuid;

use crate::domain::{
    Block, BlockDraft, BlockField, BlockKey, BlockKind, BlockOn, BlockPatch, CheckState,
    DomainError, DomainState, HumanStatus, ReviewDraft, ReviewPatch, Task, BLOCK_TEXT_MAX, OWNER,
};
use crate::ui::edit::{seeded_draft, EditBuffer};
use crate::ui::mouse::BoardPopup;

use super::model::{BoardInputMode, BoardModel, IntentOutcome};

/// One Tab stop of the task page's BLOCKED or REVIEW section, or of its PAPER TRAIL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockTarget {
    /// The PAPER TRAIL's `PAPER TRAIL · N ▸` heading: Enter expands or collapses the trail.
    TrailHeading,
    /// A closed block or review round on the PAPER TRAIL, by its index in `past_blocks`.
    Trail(usize),
    /// The section's top line, the row's live line: `ctrl+e` edits the block or review round.
    Heading,
    Option(usize),
    /// A review check, by its index in the round.
    Check(usize),
    /// The dim `N passed ▸` line the passed checks fold into.
    PassedFold,
    Reply(usize),
}

/// Session state of the BLOCKED section, carried by the task page form.
#[derive(Debug, Clone, Default)]
pub(crate) struct BlockPageState {
    /// The selected ring stop. A selected step outranks it, so a stale target is inert.
    pub(crate) target: Option<BlockTarget>,
    /// The open reply box.
    pub(crate) reply: Option<ReplyEditor>,
    /// The passed checks are unfolded under their `N passed ▾` line.
    pub(crate) passed_open: bool,
    /// Which checks sit under the `N passed` line, frozen when the page is painted fresh.
    pub(crate) fold: Option<CheckFold>,
    /// Absolute content row of each ring stop at the last painted width, recorded by the
    /// renderer so Tab can keep the selected stop inside the page viewport.
    pub(crate) rows: RefCell<Vec<(BlockTarget, usize)>>,
    /// The PAPER TRAIL is expanded (`g`). Every page opens with it collapsed.
    pub(crate) trail_expanded: bool,
    /// Closed records expanded in place on the PAPER TRAIL, by `past_blocks` index.
    pub(crate) trail_open: BTreeSet<usize>,
}

/// The checks folded under the `N passed` line, as they stood when the page was painted fresh
/// (opened, or folded with `Enter`). A check cycled on the page keeps its row until the page is
/// left, so it can go passed → failed in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckFold {
    /// The review round the fold was taken on. A different round paints fresh.
    key: BlockKey,
    folded: BTreeSet<usize>,
}

impl CheckFold {
    /// The fresh fold of a review round: its passed checks. `None` for anything else.
    pub(crate) fn fresh(task: &Task) -> Option<Self> {
        let block = task.block.as_ref().filter(|block| block.is_review())?;
        Some(Self {
            key: block.key(),
            folded: block
                .checks
                .iter()
                .enumerate()
                .filter(|(_, check)| check.state == CheckState::Passed)
                .map(|(index, _)| index)
                .collect(),
        })
    }
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
    /// Feedback on a review round rather than a reply to a block.
    pub(crate) review: bool,
    /// The empty draft's hint: `reply…`, or `feedback to @claude…`.
    pub(crate) placeholder: String,
}

impl ReplyEditor {
    fn open(block: &Block, task: &Task, prefill: &str, edit: Option<usize>) -> Self {
        let review = block.is_review();
        let agent = task
            .assignee
            .clone()
            .or_else(|| (block.by != OWNER).then(|| block.by.clone()));
        let placeholder = match (review, agent) {
            (true, Some(agent)) => format!("feedback to @{agent}…"),
            (true, None) => "feedback…".to_string(),
            (false, _) => "reply…".to_string(),
        };
        Self {
            buffer: seeded_draft(prefill),
            block: block.key(),
            edit,
            refusal: None,
            pending: None,
            width: Cell::new(0),
            review,
            placeholder,
        }
    }
}

/// The reply box opened inline under a blocked board row (`r` on the board).
#[derive(Debug, Clone)]
pub(crate) struct RowReply {
    pub(crate) task: Uuid,
    pub(crate) editor: ReplyEditor,
    /// The mode the board was in when the box opened, restored when it closes.
    pub(crate) return_mode: BoardInputMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReplySave {
    pub(crate) task: Uuid,
    /// The stored reply's index, or `None` for `ctrl+s` on an empty box (unblock only).
    pub(crate) index: Option<usize>,
    pub(crate) text: String,
    pub(crate) unblock: bool,
}

/// The card's fields. A block card tabs why → on → needs; a review card done → check → next →
/// on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockCardField {
    Why,
    On,
    Needs,
    Done,
    Check,
    Next,
}

/// The card's waiting-on choice; `task`, `agent` and `other` take the typed text. A block card
/// offers you · task · other, a review card you · agent · other.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OnKind {
    #[default]
    You,
    Task,
    Agent,
    Other,
}

impl OnKind {
    pub fn label(self) -> &'static str {
        match self {
            OnKind::You => "you",
            OnKind::Task => "task",
            OnKind::Agent => "agent",
            OnKind::Other => "other",
        }
    }

    fn cycle(self, forward: bool, kind: BlockKind) -> Self {
        let middle = match kind {
            BlockKind::Blocked => OnKind::Task,
            BlockKind::Review => OnKind::Agent,
        };
        let ring = [OnKind::You, middle, OnKind::Other];
        let position = ring.iter().position(|on| *on == self).unwrap_or(0);
        let next = if forward { position + 1 } else { position + 2 };
        ring[next % ring.len()]
    }
}

/// The block card (why, on and needs) or review card (done, checks, next and on) for one task
/// or a marked set.
#[derive(Debug, Clone)]
pub struct BlockCard {
    /// What the card opens or edits: a block or a review round.
    pub(crate) kind: BlockKind,
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
    pub(crate) done: EditBuffer,
    /// One check per line.
    pub(crate) checks: EditBuffer,
    pub(crate) next: EditBuffer,
    pub(crate) field: BlockCardField,
    pub(crate) refusal: Option<String>,
}

impl BlockCard {
    fn new(targets: Vec<Uuid>) -> Self {
        Self {
            kind: BlockKind::Blocked,
            targets,
            edit: None,
            block: None,
            pending: None,
            why: seeded_draft(""),
            on_kind: OnKind::You,
            on_text: seeded_draft(""),
            needs: seeded_draft(""),
            done: seeded_draft(""),
            checks: seeded_draft(""),
            next: seeded_draft(""),
            field: BlockCardField::Why,
            refusal: None,
        }
    }

    fn new_review(targets: Vec<Uuid>) -> Self {
        Self {
            kind: BlockKind::Review,
            field: BlockCardField::Done,
            ..Self::new(targets)
        }
    }

    fn editing(task: &Task) -> Self {
        let block = task.block.as_ref();
        let (on_kind, on_text) = match block.map(|block| &block.on) {
            Some(BlockOn::Task(number)) => (OnKind::Task, number.to_string()),
            Some(BlockOn::Agent(name)) => (OnKind::Agent, name.clone()),
            Some(BlockOn::Other(text)) => (OnKind::Other, text.clone()),
            Some(BlockOn::You) | None => (OnKind::You, String::new()),
        };
        let text = |value: Option<&String>| seeded_draft(value.map(String::as_str).unwrap_or(""));
        let base = if block.is_some_and(Block::is_review) {
            Self::new_review(vec![task.id])
        } else {
            Self::new(vec![task.id])
        };
        Self {
            edit: Some(task.id),
            block: block.map(Block::key),
            why: text(block.and_then(|block| block.why.as_ref())),
            on_kind,
            on_text: seeded_draft(&on_text),
            needs: text(block.and_then(|block| block.needs.as_ref())),
            done: text(block.and_then(|block| block.done.as_ref())),
            checks: seeded_draft(
                &block
                    .map(|block| {
                        block
                            .checks
                            .iter()
                            .map(|check| check.text.as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default(),
            ),
            next: text(block.and_then(|block| block.next.as_ref())),
            ..base
        }
    }

    pub fn kind(&self) -> BlockKind {
        self.kind
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

    pub(crate) fn done(&self) -> &EditBuffer {
        &self.done
    }

    pub(crate) fn checks(&self) -> &EditBuffer {
        &self.checks
    }

    pub(crate) fn next(&self) -> &EditBuffer {
        &self.next
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
            BlockCardField::Done => &mut self.done,
            BlockCardField::Check => &mut self.checks,
            BlockCardField::Next => &mut self.next,
        })
    }

    fn move_field(&mut self, forward: bool) {
        let order: &[BlockCardField] = match self.kind {
            BlockKind::Blocked => &[
                BlockCardField::Why,
                BlockCardField::On,
                BlockCardField::Needs,
            ],
            BlockKind::Review => &[
                BlockCardField::Done,
                BlockCardField::Check,
                BlockCardField::Next,
                BlockCardField::On,
            ],
        };
        let position = order
            .iter()
            .position(|field| *field == self.field)
            .unwrap_or(0);
        let next = if forward {
            position + 1
        } else {
            position + order.len() - 1
        };
        self.field = order[next % order.len()];
    }

    /// Resolve the typed waiting-on value against the board. A task must exist and must not
    /// be one of the card's own targets; an agent must be a defined profile.
    fn resolve_on(&self, domain: &DomainState, agents: &[String]) -> Result<BlockOn, String> {
        let text = self.on_text.value().trim();
        match self.on_kind {
            OnKind::You => Ok(BlockOn::You),
            OnKind::Other if text.is_empty() => Err("say what it waits on".to_string()),
            OnKind::Other => Ok(BlockOn::Other(text.to_string())),
            OnKind::Agent => {
                let name = text.strip_prefix('@').unwrap_or(text).to_ascii_lowercase();
                if name.is_empty() {
                    return Err("type an agent profile, like pi".to_string());
                }
                if agents.contains(&name) {
                    Ok(BlockOn::Agent(name))
                } else {
                    Err(format!("no agent profile named {name}"))
                }
            }
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

/// `ctrl+s` on an empty feedback box with no failed check.
pub(crate) const NOTHING_TO_SEND_BACK: &str = "type feedback or fail a check first";

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

/// `ctrl+r` on work not in review: open the review card over `targets`. Marks stay until it
/// confirms.
pub(super) fn open_review_card(model: &mut BoardModel, targets: Vec<Uuid>) {
    model.block_card = Some(BlockCard::new_review(targets));
    model.set_popup(BoardPopup::BlockCard);
    model.clear_message();
}

/// `ctrl+e` on the BLOCKED or REVIEW heading: the same card, prefilled, editing the open record.
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
        BlockCardField::On => card.on_kind = card.on_kind.cycle(forward, card.kind),
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
    if card.kind == BlockKind::Review {
        return confirm_review_card(domain, model);
    }
    let on = match card.resolve_on(domain, &model.agent_names) {
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

/// Enter on the review card: put every target up for review with one draft as one batch and
/// one undo entry, or edit the open round in place. An empty card puts the work up for review
/// at once.
fn confirm_review_card(
    domain: &mut DomainState,
    model: &mut BoardModel,
) -> Result<IntentOutcome, DomainError> {
    let Some(card) = model.block_card.as_ref() else {
        return Ok(IntentOutcome::None);
    };
    let refuse = |model: &mut BoardModel, refusal: String| {
        if let Some(card) = model.block_card.as_mut() {
            card.refusal = Some(refusal);
        }
        Ok(IntentOutcome::None)
    };
    let on = match card.resolve_on(domain, &model.agent_names) {
        Ok(on) => on,
        Err(refusal) => return refuse(model, refusal),
    };
    let checks: Vec<String> = card.checks.value().lines().map(str::to_string).collect();
    let draft = match ReviewDraft::from_input(
        Some(card.done.value()),
        &checks,
        Some(card.next.value()),
        on,
    ) {
        Ok(draft) => draft,
        Err(field) => return refuse(model, too_long(field)),
    };
    let targets = card.targets.clone();
    let edit = card.edit;
    let opened_on = card.block.clone();
    let changed = match edit {
        Some(id) => {
            let current = domain
                .get(id)
                .and_then(|task| task.block.as_ref())
                .filter(|block| block.is_review())
                .map(Block::key);
            if current.is_none() || current != opened_on {
                close_block_card(model);
                model.set_message("that review round was closed elsewhere");
                return Ok(IntentOutcome::None);
            }
            domain.edit_review(id, ReviewPatch::replace_with(draft))?
        }
        None => {
            let live: Vec<Uuid> = targets
                .iter()
                .copied()
                .filter(|id| domain.get(*id).is_some())
                .collect();
            domain.review_batch(&live, &draft, OWNER)?
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

/// Shift+Enter in the review card's check field starts the next check.
pub(super) fn block_card_newline(model: &mut BoardModel) {
    let Some(card) = model.block_card.as_mut().filter(|card| !card.is_pending()) else {
        return;
    };
    if card.field == BlockCardField::Check {
        card.refusal = None;
        card.checks.insert_char('\n');
    }
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

/// Whether `task` shows a BLOCKED or REVIEW section: blocked or in review, with the open
/// record of that kind.
pub(crate) fn has_open_section(task: &Task) -> bool {
    task.block
        .as_ref()
        .is_some_and(|block| BlockKind::for_status(task.status) == Some(block.kind))
}

/// The task the page shows, when it has an open block or review round and no edit session
/// owns the page.
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
        .filter(|task| has_open_section(task))
}

/// Whether the page shows its BLOCKED or REVIEW section.
pub(super) fn page_section_open(model: &BoardModel) -> bool {
    page_block_task(model).is_some()
}

/// The review checks in page order: the checks shown in place, then the ones folded under their
/// `N passed` line. The page's frozen fold decides which fold; without one for this round, the
/// passed checks do (a fresh paint).
pub(crate) fn review_check_order(
    block: &Block,
    fold: Option<&CheckFold>,
) -> (Vec<usize>, Vec<usize>) {
    let frozen = fold.filter(|fold| fold.key == block.key());
    (0..block.checks.len()).partition(|index| match frozen {
        Some(fold) => !fold.folded.contains(index),
        None => block.checks[*index].state != CheckState::Passed,
    })
}

/// Where a freshly opened page's cursor rests: a review round's first unmarked check in page
/// order (all marked: its first check shown, else the `N passed` line). A blocked page, and any
/// other, opens with nothing selected.
pub(crate) fn initial_target(task: &Task, fold: Option<&CheckFold>) -> Option<BlockTarget> {
    let block = task
        .block
        .as_ref()
        .filter(|block| block.is_review() && has_open_section(task))?;
    let (shown, passed) = review_check_order(block, fold);
    shown
        .iter()
        .find(|index| block.checks[**index].state == CheckState::Open)
        .or(shown.first())
        .map(|index| BlockTarget::Check(*index))
        .or_else(|| (!passed.is_empty()).then_some(BlockTarget::PassedFold))
}

/// The ring stops of the page's open block or review round, in order.
pub(super) fn block_ring(
    task: &Task,
    passed_open: bool,
    fold: Option<&CheckFold>,
) -> Vec<BlockTarget> {
    let Some(block) = task.block.as_ref() else {
        return Vec::new();
    };
    let mut ring = vec![BlockTarget::Heading];
    ring.extend((0..block.options.len()).map(BlockTarget::Option));
    if block.is_review() {
        let (shown, passed) = review_check_order(block, fold);
        ring.extend(shown.into_iter().map(BlockTarget::Check));
        if !passed.is_empty() {
            ring.push(BlockTarget::PassedFold);
            if passed_open {
                ring.extend(passed.into_iter().map(BlockTarget::Check));
            }
        }
    }
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

/// The selected BLOCKED or PAPER TRAIL stop. A selected step or `+ step` outranks it.
pub(super) fn selected_block_target(model: &BoardModel) -> Option<BlockTarget> {
    let form = model.form.as_ref()?;
    if form.steps.cursor.is_some() || form.steps.add_selected {
        return None;
    }
    let target = form.block.target?;
    if matches!(target, BlockTarget::Trail(_) | BlockTarget::TrailHeading) {
        return trail_stops(model).contains(&target).then_some(target);
    }
    let task = page_block_task(model)?;
    block_ring(task, form.block.passed_open, form.block.fold.as_ref())
        .contains(&target)
        .then_some(target)
}

/// Select `target` when the page's open section has it as a stop (a click). False otherwise.
pub(super) fn select_stop(model: &mut BoardModel, target: BlockTarget) -> bool {
    let Some(task) = page_block_task(model) else {
        return false;
    };
    if !page_ring(model, task).contains(&target) {
        return false;
    }
    select_block_target(model, Some(target));
    true
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

/// The task whose PAPER TRAIL the page shows: any bound task but a notice row. It stays painted
/// through an edit session, so the page body never jumps as one starts or ends.
pub(crate) fn page_trail_task(model: &BoardModel) -> Option<&Task> {
    let form = model.form.as_ref().filter(|form| form.is_task())?;
    let id = form.task_id()?;
    model
        .tasks
        .iter()
        .find(|task| task.id == id)
        .filter(|task| !task.is_notice())
}

/// The PAPER TRAIL's Tab stops: its heading, then the closed records it shows, newest first,
/// while it is expanded. An edit session owns the ring, so it has none then.
fn trail_stops(model: &BoardModel) -> Vec<BlockTarget> {
    let editing = model
        .form
        .as_ref()
        .is_some_and(|form| form.editing || model.open_field_edit().is_some());
    let Some(task) = page_trail_task(model).filter(|_| !editing) else {
        return Vec::new();
    };
    let entries = crate::activity::paper_trail(task);
    if entries.is_empty() {
        return Vec::new();
    }
    let mut stops = vec![BlockTarget::TrailHeading];
    if model
        .form
        .as_ref()
        .is_some_and(|form| form.block.trail_expanded)
    {
        stops.extend(
            entries
                .iter()
                .filter_map(|entry| entry.record.map(BlockTarget::Trail)),
        );
    }
    stops
}

/// Move Tab within the PAPER TRAIL. `LeaveBlock` past either end, `NotHandled` off the trail.
pub(super) fn move_trail_tab(model: &mut BoardModel, forward: bool) -> PageTab {
    let Some(current @ (BlockTarget::Trail(_) | BlockTarget::TrailHeading)) =
        selected_block_target(model)
    else {
        return PageTab::NotHandled;
    };
    let stops = trail_stops(model);
    let position = stops.iter().position(|stop| *stop == current).unwrap_or(0);
    let next = if forward {
        stops.get(position + 1).copied()
    } else {
        position
            .checked_sub(1)
            .and_then(|index| stops.get(index))
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

/// Enter the PAPER TRAIL at its first (forward) or last stop. False when it has none.
pub(super) fn enter_trail(model: &mut BoardModel, forward: bool) -> bool {
    let stops = trail_stops(model);
    let stop = if forward { stops.first() } else { stops.last() }.copied();
    if stop.is_none() {
        return false;
    }
    select_block_target(model, stop);
    true
}

/// `g`, `Enter` on the heading, or a click on it: expand or collapse the PAPER TRAIL. A record
/// the collapse hides hands its selection to the heading.
pub(super) fn toggle_trail(model: &mut BoardModel) {
    if page_trail_task(model).is_none() {
        return;
    }
    let selected = selected_block_target(model);
    if let Some(form) = model.form.as_mut() {
        form.block.trail_expanded = !form.block.trail_expanded;
    }
    if let Some(BlockTarget::Trail(_)) = selected {
        select_block_target(model, Some(BlockTarget::TrailHeading));
    }
}

/// `Enter` on a selected closed record, or a click on one (`index`): expand it in place or fold
/// it back.
pub(super) fn toggle_trail_record(model: &mut BoardModel, index: Option<usize>) {
    let target = match index {
        Some(index) => BlockTarget::Trail(index),
        None => match selected_block_target(model) {
            Some(target @ BlockTarget::Trail(_)) => target,
            _ => return,
        },
    };
    if !trail_stops(model).contains(&target) {
        return;
    }
    select_block_target(model, Some(target));
    let BlockTarget::Trail(index) = target else {
        return;
    };
    if let Some(form) = model.form.as_mut() {
        if !form.block.trail_open.remove(&index) {
            form.block.trail_open.insert(index);
        }
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
    let ring = page_ring(model, task);
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
    let ring = page_ring(model, task);
    let stop = if forward { ring.first() } else { ring.last() }.copied();
    if stop.is_none() {
        return false;
    }
    select_block_target(model, stop);
    true
}

fn page_ring(model: &BoardModel, task: &Task) -> Vec<BlockTarget> {
    let form = model.form.as_ref();
    block_ring(
        task,
        form.is_some_and(|form| form.block.passed_open),
        form.and_then(|form| form.block.fold.as_ref()),
    )
}

/// Take a fresh fold when the page has none for the task's current review round: the round
/// first appears on an open page, or a new round replaces it. Later refreshes keep the fold, so
/// a check marked passed elsewhere keeps its row until the page is painted fresh.
pub(super) fn freeze_check_fold(model: &mut BoardModel) {
    let Some(id) = model
        .form
        .as_ref()
        .filter(|form| form.is_task())
        .and_then(|form| form.task_id())
    else {
        return;
    };
    let Some(fresh) = model
        .tasks
        .iter()
        .find(|task| task.id == id)
        .and_then(CheckFold::fresh)
    else {
        return;
    };
    if let Some(form) = model.form.as_mut() {
        if form.block.fold.as_ref().map(|fold| &fold.key) != Some(&fresh.key) {
            form.block.fold = Some(fresh);
        }
    }
}

/// `r`, or `Enter` on an option (prefilled), or `ctrl+e` on one of your replies (`edit`).
pub(super) fn begin_reply(model: &mut BoardModel, prefill: &str, edit: Option<usize>) -> bool {
    let Some(editor) = page_block_task(model).and_then(|task| {
        task.block
            .as_ref()
            .map(|block| ReplyEditor::open(block, task, prefill, edit))
    }) else {
        return false;
    };
    let Some(form) = model.form.as_mut() else {
        return false;
    };
    form.block.reply = Some(editor);
    model.input_mode = BoardInputMode::EditReply;
    model.clear_message();
    true
}

/// `r` on a board row: open the reply box inline under the cursor task. A marked set is
/// ignored; the box answers the cursor row only. False when the row is not blocked or in
/// review.
pub(super) fn begin_row_reply(model: &mut BoardModel) -> bool {
    let Some(id) = model.selected_id() else {
        return false;
    };
    let Some(editor) = model
        .tasks
        .iter()
        .find(|task| task.id == id)
        .filter(|task| has_open_section(task) && !task.archived)
        .and_then(|task| {
            task.block
                .as_ref()
                .map(|block| ReplyEditor::open(block, task, "", None))
        })
    else {
        return false;
    };
    model.row_reply = Some(RowReply {
        task: id,
        editor,
        return_mode: model.input_mode,
    });
    model.input_mode = BoardInputMode::EditReply;
    model.clear_message();
    true
}

/// The open reply box and its task: the task page's, or the board row's.
pub(crate) fn active_reply(model: &BoardModel) -> Option<(Uuid, &ReplyEditor)> {
    if let Some(form) = model.form.as_ref() {
        if let (Some(editor), Some(id)) = (form.block.reply.as_ref(), form.task_id()) {
            return Some((id, editor));
        }
    }
    model.row_reply.as_ref().map(|row| (row.task, &row.editor))
}

fn active_reply_mut(model: &mut BoardModel) -> Option<&mut ReplyEditor> {
    let page = model
        .form
        .as_ref()
        .is_some_and(|form| form.block.reply.is_some());
    if page {
        return model.form.as_mut()?.block.reply.as_mut();
    }
    model.row_reply.as_mut().map(|row| &mut row.editor)
}

/// Close the open reply box: the page's returns to the page, the row's to the board mode it
/// opened from.
fn close_reply(model: &mut BoardModel, page_target: Option<BlockTarget>) {
    if let Some(form) = model
        .form
        .as_mut()
        .filter(|form| form.block.reply.is_some())
    {
        form.block.reply = None;
        form.block.target = page_target;
        if model.input_mode == BoardInputMode::EditReply {
            model.input_mode = BoardInputMode::TaskPage;
        }
        return;
    }
    if let Some(row) = model.row_reply.take() {
        if model.input_mode == BoardInputMode::EditReply {
            model.input_mode = row.return_mode;
        }
    }
}

/// `Enter` on a review check: cycle it open → passed → failed → open. The check keeps its row
/// and the cursor through the whole cycle; it folds only when the page is next painted fresh.
pub(super) fn cycle_selected_check(
    domain: &mut DomainState,
    model: &mut BoardModel,
) -> Result<IntentOutcome, DomainError> {
    let Some(BlockTarget::Check(index)) = selected_block_target(model) else {
        return Ok(IntentOutcome::None);
    };
    let Some(task) = page_block_task(model) else {
        return Ok(IntentOutcome::None);
    };
    let id = task.id;
    let Some(state) = task
        .block
        .as_ref()
        .and_then(|block| block.checks.get(index))
        .map(|check| check.state.cycle())
    else {
        return Ok(IntentOutcome::None);
    };
    freeze_check_fold(model);
    match domain.set_check(id, index, state) {
        Ok(true) => {}
        Ok(false) => return Ok(IntentOutcome::None),
        Err(DomainError::NotInReview(_) | DomainError::UnknownCheck(_)) => {
            model.set_message("this review changed elsewhere");
            return Ok(IntentOutcome::None);
        }
        Err(error) => return Err(error),
    }
    Ok(IntentOutcome::Persist)
}

/// `Enter` on the `N passed` line: show or fold the passed checks. Folding paints the fold
/// fresh, so it takes in the checks passed in place and lets go of any no longer passed. When
/// none is still passed the line goes away, and the cursor moves to the check that left the fold
/// (else the stop before the line), never onto a row the page does not paint.
pub(super) fn toggle_passed_checks(model: &mut BoardModel) {
    if selected_block_target(model) != Some(BlockTarget::PassedFold) {
        return;
    }
    let Some(task) = page_block_task(model) else {
        return;
    };
    let fresh = CheckFold::fresh(task);
    let before = page_ring(model, task);
    let Some(form) = model.form.as_mut() else {
        return;
    };
    form.block.passed_open = !form.block.passed_open;
    if form.block.passed_open {
        return;
    }
    let old = std::mem::replace(&mut form.block.fold, fresh);
    if form
        .block
        .fold
        .as_ref()
        .is_some_and(|fold| !fold.folded.is_empty())
    {
        return;
    }
    let left = old.and_then(|fold| fold.folded.first().copied());
    let previous = before
        .iter()
        .position(|stop| *stop == BlockTarget::PassedFold)
        .and_then(|at| at.checked_sub(1))
        .map(|at| before[at]);
    form.block.target = left.map(BlockTarget::Check).or(previous);
}

/// The text of the selected option, for `Enter`.
pub(super) fn selected_option_text(model: &BoardModel) -> Option<String> {
    let BlockTarget::Option(index) = selected_block_target(model)? else {
        return None;
    };
    option_text(model, index)
}

/// The text of the page's open block option `index` (from 0), for its number key.
pub(super) fn option_text(model: &BoardModel, index: usize) -> Option<String> {
    page_block_task(model)?
        .block
        .as_ref()
        .filter(|block| !block.is_review())?
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

/// `shift+enter` stores the reply; `ctrl+s` stores it and unblocks the task to `unblock`
/// (ready, or started when its agent is still running; the application boundary decides).
/// `send_back` is a review's `ctrl+s` whose start (a dispatch or relaunch) follows this save:
/// like an unblock, it may store nothing. A review sent back with an empty box needs a failed
/// check. The box and its mode stay until the synced task carries the reply.
pub(super) fn save_reply(
    domain: &mut DomainState,
    model: &mut BoardModel,
    unblock: Option<HumanStatus>,
    send_back: bool,
) -> Result<IntentOutcome, DomainError> {
    let Some((id, editor)) = active_reply(model) else {
        return Ok(IntentOutcome::None);
    };
    if editor.pending.is_some() {
        return Ok(IntentOutcome::None);
    }
    let text = editor.buffer.value().trim().to_string();
    let edit = editor.edit;
    let opened_on = editor.block.clone();
    let editor_review = editor.review;
    let refuse = |model: &mut BoardModel, refusal: String| {
        if let Some(editor) = active_reply_mut(model) {
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
    // `ctrl+s` on an empty new reply only unblocks; saving an empty reply is refused.
    let unblock_only = text.is_empty() && (unblock.is_some() || send_back) && edit.is_none();
    if text.is_empty() && !unblock_only {
        return refuse(model, "type a reply first".to_string());
    }
    // An empty send-back says nothing unless a check failed.
    let failed = domain
        .get(id)
        .and_then(|task| task.block.as_ref())
        .is_some_and(|block| !block.failed_checks().is_empty());
    if unblock_only && editor_review && !failed {
        return refuse(model, NOTHING_TO_SEND_BACK.to_string());
    }
    if text.len() > BLOCK_TEXT_MAX {
        return refuse(model, too_long(BlockField::Reply));
    }
    // Sending a review back always restarts the work, assigned or not.
    let unblock = unblock.map(|status| {
        if editor_review {
            HumanStatus::Started
        } else {
            status
        }
    });
    // The reply lands on the open block before the unblock closes it, as one change.
    let stored = domain.as_one_change(id, |domain| {
        let index = match edit {
            _ if unblock_only => None,
            Some(index) => Some(domain.edit_reply(id, index, &text).map(|()| index)?),
            None => Some(domain.reply(id, &text, OWNER)?),
        };
        if let Some(status) = unblock {
            domain.set_status(id, status)?;
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
    if let Some(editor) = active_reply_mut(model) {
        editor.refusal = None;
        editor.pending = Some(ReplySave {
            task: id,
            index,
            text,
            unblock: unblock.is_some(),
        });
    }
    Ok(IntentOutcome::Persist)
}

/// Whether the open reply box still answers the task's open block or review round. When a
/// refresh closed or replaced it, the box says so and keeps its draft.
pub(super) fn reply_box_current(domain: &DomainState, model: &mut BoardModel) -> bool {
    let Some((id, editor)) = active_reply(model) else {
        return false;
    };
    let current = domain
        .get(id)
        .and_then(|task| task.block.as_ref())
        .map(Block::key);
    if current.as_ref() == Some(&editor.block) {
        return true;
    }
    model.set_message(BLOCK_REPLACED);
    if let Some(editor) = active_reply_mut(model) {
        editor.refusal = Some(BLOCK_REPLACED.to_string());
    }
    false
}

/// Esc in the reply box: discard the draft and return to the page, or to the board.
pub(super) fn cancel_reply(model: &mut BoardModel) {
    let target = model.form.as_ref().and_then(|form| form.block.target);
    close_reply(model, target);
    model.clear_message();
}

/// Release a held reply box once the synced task carries its reply (the confirmed landing).
pub(super) fn finish_reply_save(model: &mut BoardModel) {
    let Some(pending) = active_reply(model).and_then(|(_, editor)| editor.pending.clone()) else {
        return;
    };
    let landed = model
        .tasks
        .iter()
        .find(|task| task.id == pending.task)
        .and_then(|task| {
            if pending.unblock {
                task.block
                    .is_none()
                    .then(|| task.past_blocks.last())
                    .flatten()
            } else {
                task.block.as_ref()
            }
        })
        .is_some_and(|block| match pending.index {
            Some(index) => block
                .replies
                .get(index)
                .is_some_and(|reply| reply.text == pending.text && !reply.deleted),
            None => true,
        });
    if !landed {
        return;
    }
    close_reply(
        model,
        pending
            .index
            .filter(|_| !pending.unblock)
            .map(BlockTarget::Reply),
    );
}

/// A refresh closed or replaced the block an open row reply box answers: say so in the box
/// at once, keeping the draft. Saving would refuse the same way.
pub(super) fn flag_stale_row_reply(model: &mut BoardModel) {
    let Some(row) = model.row_reply.as_ref() else {
        return;
    };
    if row.editor.pending.is_some() {
        return;
    }
    let current = model
        .tasks
        .iter()
        .find(|task| task.id == row.task)
        .and_then(|task| task.block.as_ref())
        .map(Block::key);
    if current.as_ref() != Some(&row.editor.block) {
        if let Some(row) = model.row_reply.as_mut() {
            row.editor.refusal = Some(BLOCK_REPLACED.to_string());
        }
    }
}

/// A cancelled failed save rolled the reply back: keep the box and its text, drop the hold.
pub(super) fn release_cancelled_reply(model: &mut BoardModel) {
    if let Some(editor) = active_reply_mut(model) {
        editor.pending = None;
    }
}

/// The reply box's draft, for the shared edit intents.
pub(super) fn reply_buffer_mut(model: &mut BoardModel) -> Option<&mut EditBuffer> {
    let editor = active_reply_mut(model)?;
    if editor.pending.is_some() {
        return None;
    }
    // A closed or replaced block stays said while the draft is kept; typing cannot fix it.
    if editor.refusal.as_deref() != Some(BLOCK_REPLACED) {
        editor.refusal = None;
    }
    Some(&mut editor.buffer)
}
