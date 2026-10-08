//! Chrome-row composition and edit-state status helpers.

use crate::ui::present_line;

use super::model::{BoardInputMode, BoardModel};

impl BoardModel {
    /// The chrome row an open field edit owns, composed for `width` columns.
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
        BoardInputMode::SelectBase => [
            "Enter choose base · Tab next · Esc cancel",
            "Enter choose · Tab next · Esc",
            "Enter choose · Esc",
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
        | BoardInputMode::CleanupConfirm
        | BoardInputMode::CleanupDirtyConfirm
        | BoardInputMode::DispatchConfirm
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

fn edit_chrome_line(mode: BoardInputMode, lead: &[ChromeRowPart<'_>], width: usize) -> String {
    use ratatui::text::Line;

    /// Narrowest lead worth painting; below this only the legend is left.
    const REASON_FLOOR: usize = 8;
    /// Columns the row spends on padding and the separator: `" " + " · "`.
    const FRAME: usize = 7;

    let idle = [ChromeRowPart::Message("editing…")];
    let lead = if lead.is_empty() { &idle[..] } else { lead };
    let full = fit_chrome_row(lead, usize::MAX);
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
    format!("  {}  ·  {}", fit_chrome_row(lead, room), shortest)
}

/// The Undo control on the delete recovery notice: the key and its label.
pub const DELETE_NOTICE_UNDO: &str = "ctrl+u undo";
/// Undo control text for a bulk delete notice.
pub const BULK_DELETE_NOTICE_UNDO: &str = "ctrl+u restores";

const CHROME_ROW_SEPARATOR: &str = "  ·  ";
const CHROME_ROW_OMITTED: &str = "…";
const NOTICE_TITLE_FLOOR: usize = 4;
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

#[derive(Debug, Clone, Copy)]
enum ChromeRowPart<'a> {
    Notice { title: &'a str, undo: bool },
    Message(&'a str),
}

impl ChromeRowPart<'_> {
    fn want(&self) -> usize {
        match self {
            Self::Notice { title, undo } => row_width(&notice_framed(title, *undo)),
            Self::Message(message) => row_width(message),
        }
    }

    fn floor(&self) -> usize {
        match self {
            Self::Notice { undo, .. } => {
                (row_width(&notice_framed("", *undo)) + NOTICE_TITLE_FLOOR).min(self.want())
            }
            Self::Message(message) => MESSAGE_FLOOR.min(row_width(message)),
        }
    }

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

fn fit_chrome_row(parts: &[ChromeRowPart<'_>], width: usize) -> String {
    let separator = row_width(CHROME_ROW_SEPARATOR);
    let wants: Vec<usize> = parts.iter().map(ChromeRowPart::want).collect();
    let floors: Vec<usize> = parts.iter().map(ChromeRowPart::floor).collect();

    let fit = |room: usize| {
        let mut kept = parts.len();
        while kept > 1 {
            let last = kept - 1;
            let above: usize = floors[..last].iter().sum();
            if above + floors[last] + separator * last <= room {
                break;
            }
            kept -= 1;
        }
        kept
    };
    let mut kept = fit(width);

    let marker = separator + row_width(CHROME_ROW_OMITTED);
    let marked = kept < parts.len() && floors.first().is_some_and(|floor| floor + marker <= width);
    let room = if marked { width - marker } else { width };
    if marked {
        kept = fit(room);
    }

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

    for (budget, want) in budgets.iter_mut().zip(&wants) {
        let extra = want.saturating_sub(*budget).min(spare);
        *budget += extra;
        spare -= extra;
    }

    let mut text = String::new();
    for (index, (part, budget)) in parts.iter().zip(&budgets).enumerate() {
        if index > 0 {
            text.push_str(CHROME_ROW_SEPARATOR);
        }
        text.push_str(&part.render(*budget));
    }
    if marked {
        text.push_str(CHROME_ROW_SEPARATOR);
        text.push_str(CHROME_ROW_OMITTED);
    }

    if row_width(&text) > width {
        present_line(&text, width)
    } else {
        text
    }
}
