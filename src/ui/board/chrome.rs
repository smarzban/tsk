//! Chrome-row composition and edit-state status helpers.

use crate::ui::present_line;

use super::model::{BoardInputMode, BoardModel};

impl BoardModel {
    /// The chrome row an open field edit owns, composed for `width` columns.
    ///
    /// Same order as [`Self::chrome_row`], with two differences the mode forces: the edit's
    /// own legend is never dropped (it is the only statement of the save chord and `Esc` in
    /// Browse), and the notice is stated **without** its `u Undo` control, because `u` types
    /// a `u` into the draft here and there is no hit region on this row. The deletion stays
    /// visible; the route it names comes back with the row when the edit closes.
    pub(super) fn edit_chrome_row(&self, width: usize) -> String {
        let mut parts = Vec::new();
        let bulk_notice = self.visible_delete_notice_count().map(|count| {
            format!(
                "deleted {count} {}",
                if count == 1 { "task" } else { "tasks" }
            )
        });
        if let Some(notice) = bulk_notice.as_deref() {
            parts.push(ChromeRowPart::Message(notice));
        } else if let Some(title) = self.visible_delete_notice() {
            parts.push(ChromeRowPart::Notice { title, undo: false });
        }
        if let Some(message) = self.message.as_deref().filter(|msg| !msg.is_empty()) {
            parts.push(ChromeRowPart::Message(message));
        }
        edit_chrome_line(self.input_mode, &parts, width)
    }
}

/// The same legend at three widths: full, then shortened, then shortest.
///
/// N2 removed the standalone legacy `draw_browse_edit_band` (and the
/// `EDIT_LABEL_WIDTH`/`notes_block`/`edit_line_window` helpers it alone used) because the
/// queue overlay above already paints the open title/notes editor and the two live copies
/// fought over the terminal cursor. This legend composer survives: it still feeds the
/// open-edit toast row painted in [`draw_board`].
///
/// Each tier drops wording, never an element the user cannot do without: the parenthetical
/// alternative chord goes first (a second way to do what the first entry already names),
/// then `Enter newline` (Notes only, and never painted in Browse even before this: the full
/// legend has always overflowed that row). The save chord and `Esc` survive every tier.
fn edit_chrome_legends(mode: BoardInputMode) -> [&'static str; 3] {
    match mode {
        BoardInputMode::EditNotes => [
            "Shift+Enter save · Enter newline · Esc cancel",
            "Shift+Enter save · Esc cancel",
            "Shift+Enter save · Esc",
        ],
        BoardInputMode::EditScope => [
            "Space cycle · Enter scopes · Esc cancel",
            "Space · Enter scopes · Esc",
            "Enter scopes · Esc",
        ],
        BoardInputMode::EditAssignee => [
            "Space / arrows cycle · Enter pick · Esc cancel",
            "Space cycle · Enter pick · Esc",
            "Enter pick · Esc",
        ],
        BoardInputMode::SelectThread => [
            "Enter edit thread · Tab next · Esc cancel",
            "Enter edit · Tab next · Esc",
            "Enter edit · Esc",
        ],
        BoardInputMode::EditTitle => [
            "Shift+Enter save · Esc cancel",
            "Shift+Enter save · Esc cancel",
            "Shift+Enter save · Esc",
        ],
        BoardInputMode::EditThread => [
            "Enter close · Shift+Enter save · Esc cancel",
            "Enter close · Shift+Enter save · Esc",
            "Enter close · Esc",
        ],
        BoardInputMode::EditStep => [
            "Enter next · Shift+Enter save · Esc cancel",
            "Enter next · Shift+Enter save · Esc",
            "Enter · Shift+Enter · Esc",
        ],
        BoardInputMode::QuickAdd
        | BoardInputMode::FormDropdown
        | BoardInputMode::Normal
        | BoardInputMode::ProjectPicker
        | BoardInputMode::ListPicker
        | BoardInputMode::Search
        | BoardInputMode::SaveRecovery
        | BoardInputMode::LaunchCard
        | BoardInputMode::Palette
        | BoardInputMode::Help
        | BoardInputMode::TaskPage
        | BoardInputMode::CapturePage => [
            "Enter save · Esc cancel",
            "Enter save · Esc cancel",
            "Enter save · Esc",
        ],
    }
}

/// The whole chrome row an open edit owns: what wants the row, followed by the legend.
///
/// Both halves have to survive the narrowest painted board. Browse is 50 columns, so 48
/// inside the border, and Browse paints no detail panel — this row is then the *only*
/// statement of `Shift+Enter save` on screen, and a Notes draft cannot be committed without
/// it. So the row degrades in a deliberate order:
///
/// 1. **dropped first** — the legend's extra wording, tier by tier
///    (see [`edit_chrome_legends`]);
/// 2. **dropped second** — the lead, composed and clipped by [`fit_chrome_row`] in its own
///    order of importance, once even the shortest legend leaves it no room;
/// 3. **never dropped** — the save chord and `Esc`.
///
/// Below [`REASON_FLOOR`](self) columns a clipped lead says nothing anyway, so at that
/// point the legend takes the row alone rather than both halves becoming unreadable. Dropping
/// the lead whole is still a drop, so it leaves [`CHROME_ROW_OMITTED`] behind on
/// [`fit_chrome_row`]'s terms: out of the columns the legend does not need, and skipped where
/// those columns will not seat the lead's floor and the marker both, because the way out of
/// the field is what never gives. No board the product paints is this narrow (50 columns
/// leaves this row 48), so this is the guard rather than a width a user reaches. What the
/// narrowest board *does* reach is the tightest mark in the product: a Notes edit spends 21
/// of those 48 on `Shift+Enter save · Esc` and 7 on the frame, leaving [`fit_chrome_row`] 20
/// columns against a lead floor of 14 -- exactly enough to mark a dropped refusal, and no
/// more.
fn edit_chrome_line(mode: BoardInputMode, lead: &[ChromeRowPart<'_>], width: usize) -> String {
    use ratatui::text::Line;

    /// Narrowest lead worth painting; below this only the legend is left.
    const REASON_FLOOR: usize = 8;
    /// Columns the row spends on padding and the separator: `" " + " · "`.
    const FRAME: usize = 7;

    let idle = [ChromeRowPart::Message("editing…")];
    let lead = if lead.is_empty() { &idle[..] } else { lead };
    let full = fit_chrome_row(lead, usize::MAX).text;
    let legends = edit_chrome_legends(mode);
    let lead_width = Line::from(full.as_str()).width();
    for legend in legends {
        if FRAME + lead_width + Line::from(legend).width() <= width {
            return format!("  {full}  ·  {legend}");
        }
    }

    let shortest = legends[2];
    let room = width.saturating_sub(FRAME + Line::from(shortest).width());
    if room < REASON_FLOOR {
        let marked = format!("  {CHROME_ROW_OMITTED}{CHROME_ROW_SEPARATOR}{shortest}");
        if row_width(&marked) <= width {
            return marked;
        }
        return present_line(shortest, width);
    }
    format!("  {}  ·  {}", fit_chrome_row(lead, room).text, shortest)
}

/// The Undo control on the delete recovery notice: the key and its label.
///
/// One string for the row and the hit region, so a click always lands on the words the
/// board painted.
pub const DELETE_NOTICE_UNDO: &str = "ctrl+u undo";
/// Undo control text for a bulk delete notice.
pub const BULK_DELETE_NOTICE_UNDO: &str = "ctrl+u restores";

/// Columns between two things sharing the chrome row.
const CHROME_ROW_SEPARATOR: &str = "  ·  ";

/// What the row says in place of a part it could not seat at all: the same `…` marker
/// [`present_line`](crate::ui::present_line) leaves behind where it clipped, standing on its
/// own as a row element instead of inline inside one.
///
/// One convention for both kinds of omission, so a row that has had to leave something out
/// never reads as a row that had everything to say.
const CHROME_ROW_OMITTED: &str = "…";

/// Narrowest deleted-task title worth painting.
const NOTICE_TITLE_FLOOR: usize = 4;

/// Narrowest message worth painting: the first words of a reason (`changed since t…`), which
/// is what tells the user their action was refused rather than ignored.
///
/// It is bounded above by the narrowest board there is: 46 content columns, less the
/// notice's own 23-column floor and the 5-column separator, leaves 18. Sixteen keeps that
/// case fitting with two columns to spare for the deleted title.
const MESSAGE_FLOOR: usize = 16;

/// Display width of a presented string, in terminal cells.
pub(super) fn row_width(text: &str) -> usize {
    ratatui::text::Line::from(text).width()
}

/// The delete recovery notice around a title: `Deleted "title" · ctrl+u undo`.
pub(super) fn notice_framed(title: &str, undo: bool) -> String {
    if undo {
        format!("Deleted \"{title}\" · {DELETE_NOTICE_UNDO}")
    } else {
        format!("Deleted \"{title}\"")
    }
}

/// One thing competing for the chrome row.
///
/// The variants are listed in the order they matter; see [`fit_chrome_row`].
#[derive(Debug, Clone, Copy)]
enum ChromeRowPart<'a> {
    /// The delete recovery notice, which outlives its action on purpose.
    ///
    /// `undo` is false where the key it would name types a character instead (an open field
    /// edit), so the row never advertises a route the user cannot take from where they are.
    Notice { title: &'a str, undo: bool },
    /// Feedback about the action the user just took.
    Message(&'a str),
}

impl ChromeRowPart<'_> {
    /// Everything this part has to say, in columns.
    fn want(&self) -> usize {
        match self {
            Self::Notice { title, undo } => row_width(&notice_framed(title, *undo)),
            Self::Message(message) => row_width(message),
        }
    }

    fn joins_only_on_surplus(&self) -> bool {
        false
    }

    /// Narrowest form still worth painting. Below this the row drops the part instead of
    /// clipping it further.
    ///
    /// Never wider than [`Self::want`]: a floor above what the part would actually render
    /// reserves columns the row then leaves blank, and the budget drops a part it had the
    /// room to seat. A deleted title of one to three characters is exactly that case.
    fn floor(&self) -> usize {
        match self {
            Self::Notice { undo, .. } => {
                (row_width(&notice_framed("", *undo)) + NOTICE_TITLE_FLOOR).min(self.want())
            }
            Self::Message(message) => MESSAGE_FLOOR.min(row_width(message)),
        }
    }

    /// This part in `budget` columns, which is between [`Self::floor`] and [`Self::want`].
    fn render(&self, budget: usize) -> String {
        match self {
            Self::Notice { title, undo } => {
                let frame = row_width(&notice_framed("", *undo));
                notice_framed(&present_line(title, budget.saturating_sub(frame)), *undo)
            }
            Self::Message(message) => present_line(message, budget),
        }
    }
}

/// The chrome row as it is painted, and where its Undo control landed on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChromeRow {
    /// The row's text, without the padding the renderer puts either side of it.
    pub(crate) text: String,
    /// Column offset of [`DELETE_NOTICE_UNDO`] inside [`Self::text`], when the notice put a
    /// clickable one there. `None` whenever the row carries no Undo control, so the hit test
    /// and the renderer cannot disagree about whether one exists.
    pub(crate) undo_offset: Option<usize>,
    /// How many of the parts it was offered the row actually seated, counted from the most
    /// important. The renderer colours the row from what it *shows* rather than from what it
    /// was offered, so a row that had to drop a refusal is not painted as one.
    pub(crate) painted: usize,
}

/// Compose one chrome row out of the things competing for it.
///
/// `parts` are given most important first, and that is the order they are painted in:
///
/// 1. the delete recovery notice, when one is armed;
/// 2. the newest message, when there is one;
/// 3. the mode's legend.
///
/// Every part declares a **floor** (the least it can say and still mean something) and a
/// **want**, which is its full text. The row is budgeted from the floors, never from the
/// wants:
///
/// 1. the least important part is dropped, one at a time, until everything left can be
///    seated at its floor;
/// 2. what is left is seated at its floor;
/// 3. the surplus is handed out in importance order, up to each part's want.
///
/// So a part that is present is **clipped** with the shared `…` marker, never dropped, and a
/// part is dropped only when the row truly cannot seat it: after a drop the unused columns
/// are always fewer than the dropped part's floor plus its separator, so no width leaves
/// usable room unpainted while something is missing. Clipping the notice takes the deleted
/// *title* only: the Undo affordance is not part of what is clipped, and the notice is never
/// dropped, so's way back and's visibility hold at every width.
///
/// A part that *is* dropped leaves [`CHROME_ROW_OMITTED`] behind as the row's last element,
/// so a dropped part is marked on the same convention a clipped one is and the row never
/// reads as complete when it is not. This is what rests on where the row cannot seat a
/// refusal: the reason is missing, but the *fact* that there is one is not. The marker is
/// taken out of the row's own budget before the parts are seated, so it costs columns rather
/// than contents.
///
/// The mark is skipped entirely below `floor + marker` columns for the *leading* part (the
/// marker being six: the separator plus the `…`), rather than bought by making a worse
/// omission than the one it would announce. **Below that width the drop is not marked at all,
/// and it is not necessarily visible either.** [`ChromeRowPart::floor`] is clamped to
/// [`ChromeRowPart::want`], so a leading part the row can seat whole *is* seated whole and no
/// `…` is painted anywhere: a notice carrying a three-character title, for one, drops the
/// refusal silently at every width from 22 to 27. Narrower still the leading part is clipped
/// and an inline `…` does appear, but it marks that clip, not the drop.
///
/// That silent band is a guard, not a width a user reaches. Below 50x18 the board paints its
/// Resize screen instead of a chrome row, and 50 columns leave this row 44; a row that has
/// dropped something is always led by the notice (a message leads only when there is no
/// notice, and then the only part under it is the legend, which goes unmarked by design), so
/// the test asks for at most 23 + 6 = 29 of those 44. The tightest case the product can
/// reach is the edit-owned row under a Notes edit, where the longer save chord leaves
/// [`edit_chrome_line`] 20 columns to pass on and the notice, stripped of its `u Undo`, floors
/// at 14: 14 + 6 = 20 exactly, affordable with nothing to spare.
///
/// The legend is the exception twice over, and it is what makes the other two fit: it is a
/// hint about the mode rather than feedback about an action, so it joins only on **surplus**
/// (only when everything above it can already say everything) and is therefore the first
/// thing to go, before a character of a message or a deleted title is clipped. When it is
/// seated but short of its full text it falls back to the way out of the modal state and no
/// further. And when it is the only thing dropped the row is **not** marked: a hint the row
/// was never owed is not omitted feedback, and marking it would spend six columns of the
/// notice or the message to announce the absence of the very thing that was dropped to give
/// them those columns.
///
/// The narrowest board there is, 50 columns (46 inside the padding), carrying a deleted task
/// and the refusal of a stale Undo, paints `Deleted "Delet…" · u Undo · changed since t…`:
/// the way back whole, the reason legible, the legend gone (and unmarked). The same row
/// under an open field edit has fewer columns to spend, because the edit's legend is not
/// droppable, and it reads `Deleted "Delete t…" · …`: the refusal is gone, and says so.
///
/// Postcondition: the row is never wider than `width`, at any width, for any parts.
fn fit_chrome_row(parts: &[ChromeRowPart<'_>], width: usize) -> ChromeRow {
    let separator = row_width(CHROME_ROW_SEPARATOR);
    let wants: Vec<usize> = parts.iter().map(ChromeRowPart::want).collect();
    let floors: Vec<usize> = parts.iter().map(ChromeRowPart::floor).collect();

    // Drop from the least important end until what is left fits at its floors in `room`. The
    // test is against the **floors** of everything more important, so a part is never dropped
    // to let a more important one say everything -- shortage comes out of the widths, not out
    // of the row's contents. A part that joins only on surplus (the legend) is the one tested
    // against those wants instead, which is what puts it first in the queue to go.
    let fit = |room: usize| {
        let mut kept = parts.len();
        while kept > 1 {
            let last = kept - 1;
            let above: usize = if parts[last].joins_only_on_surplus() {
                wants[..last].iter().sum()
            } else {
                floors[..last].iter().sum()
            };
            if above + floors[last] + separator * last <= room {
                break;
            }
            kept -= 1;
        }
        kept
    };
    let mut kept = fit(width);

    // Does the row owe the user a mark? Only for a part that carries feedback about what the
    // user did; the legend is a hint that joins on surplus and goes unmarked (see above). The
    // marker is charged to the row before anything is seated, and re-running the fit inside
    // what is left can only drop further, never fewer, so the mark it pays for stays true.
    let marker = separator + row_width(CHROME_ROW_OMITTED);
    let marked = parts[kept..]
        .iter()
        .any(|part| !part.joins_only_on_surplus())
        && floors.first().is_some_and(|floor| floor + marker <= width);
    let room = if marked { width - marker } else { width };
    if marked {
        kept = fit(room);
    }

    // Seat what is kept at its floor, never spending room the row does not have: with a
    // single part left there is nothing to drop, so it is clipped to the row instead.
    let mut budgets: Vec<usize> = Vec::with_capacity(kept);
    let mut spare = room;
    for (index, floor) in floors[..kept].iter().enumerate() {
        if index > 0 {
            spare = spare.saturating_sub(separator);
        }
        let budget = (*floor).min(spare);
        spare -= budget;
        budgets.push(budget);
    }

    // Then hand the room that is left out in importance order, so the least important part
    // is the one still sitting at its floor.
    for (budget, want) in budgets.iter_mut().zip(&wants) {
        let extra = want.saturating_sub(*budget).min(spare);
        *budget += extra;
        spare -= extra;
    }

    let mut text = String::new();
    let mut undo_offset = None;
    for (index, (part, budget)) in parts.iter().zip(&budgets).enumerate() {
        if index > 0 {
            text.push_str(CHROME_ROW_SEPARATOR);
        }
        let offset = row_width(&text);
        let rendered = part.render(*budget);
        if matches!(part, ChromeRowPart::Notice { undo: true, .. }) {
            undo_offset =
                Some(offset + row_width(&rendered).saturating_sub(row_width(DELETE_NOTICE_UNDO)));
        }
        text.push_str(&rendered);
    }
    if marked {
        text.push_str(CHROME_ROW_SEPARATOR);
        text.push_str(CHROME_ROW_OMITTED);
    }

    // The postcondition, enforced where it is stated. Reachable only below the narrowest
    // board, where even the most important part's floor does not fit and there is nothing
    // left to drop: the row then says what it can, and reports no control it did not paint
    // whole, so the hit test cannot offer a click the row does not show.
    if row_width(&text) > width {
        text = present_line(&text, width);
        undo_offset =
            undo_offset.filter(|offset| offset + row_width(DELETE_NOTICE_UNDO) <= row_width(&text));
    }

    ChromeRow {
        text,
        undo_offset,
        painted: kept,
    }
}
