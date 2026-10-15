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
            parts.push(ChromeRowPart::Notice {
                label: self.delete_notice_label(),
                title,
                undo: false,
            });
        }
        if let Some(message) = self.message.as_deref().filter(|msg| !msg.is_empty()) {
            parts.push(ChromeRowPart::Message(message));
        }
        let review = match self.input_mode {
            BoardInputMode::EditReply => self.reply_is_feedback(),
            BoardInputMode::BlockCard => self
                .block_card()
                .is_some_and(|card| card.kind() == crate::domain::BlockKind::Review),
            _ => false,
        };
        edit_chrome_line(self.input_mode, review, &parts, width)
    }
}

/// The reply (feedback) box's own keys, its widest legend, for the task page's action line.
pub(super) fn reply_box_keys(review: bool) -> &'static str {
    edit_chrome_legends(BoardInputMode::EditReply, review)[0]
}

/// The legends an edit owns, widest first. `review` picks the feedback box and review card.
fn edit_chrome_legends(mode: BoardInputMode, review: bool) -> [&'static str; 3] {
    match mode {
        BoardInputMode::EditReply if review => [
            "shift+enter save · ctrl+s send back · ctrl+d approve · esc cancel",
            "shift+enter save · ctrl+s send back · ctrl+d approve · esc",
            "shift+enter · ctrl+s · ctrl+d · esc",
        ],
        BoardInputMode::BlockCard if review => [
            "enter review · shift+enter new check · tab next field · esc cancel",
            "enter review · tab next · esc",
            "enter · tab · esc",
        ],
        BoardInputMode::EditNotes => [
            "shift+enter save · enter newline · esc cancel",
            "shift+enter save · esc cancel",
            "shift+enter save · esc",
        ],
        BoardInputMode::EditScope => [
            "space cycle · enter projects · esc cancel",
            "space · enter projects · esc",
            "enter projects · esc",
        ],
        BoardInputMode::EditAssignee => [
            "space / arrows cycle · enter choose · esc cancel",
            "space cycle · enter choose · esc",
            "enter choose · esc",
        ],
        BoardInputMode::SelectThread => [
            "enter edit thread · tab next · esc cancel",
            "enter edit · tab next · esc",
            "enter edit · esc",
        ],
        BoardInputMode::SelectBase => [
            "enter choose base · tab next · esc cancel",
            "enter choose · tab next · esc",
            "enter choose · esc",
        ],
        BoardInputMode::SelectAfter => [
            "enter choose tasks · tab next · esc cancel",
            "enter choose · tab next · esc",
            "enter choose · esc",
        ],
        BoardInputMode::EditTitle => [
            "shift+enter save · esc cancel",
            "shift+enter save · esc cancel",
            "shift+enter save · esc",
        ],
        BoardInputMode::EditThread => [
            "enter close · shift+enter save · esc cancel",
            "enter close · shift+enter save · esc",
            "enter close · esc",
        ],
        BoardInputMode::EditReply => [
            "shift+enter save · ctrl+s save and unblock · esc cancel",
            "shift+enter save · ctrl+s unblock · esc",
            "shift+enter · ctrl+s · esc",
        ],
        BoardInputMode::BlockCard => [
            "enter block · tab next field · esc cancel",
            "enter block · tab next · esc",
            "enter · tab · esc",
        ],
        BoardInputMode::EditStep => [
            "enter next · shift+enter save · esc cancel",
            "enter next · shift+enter save · esc",
            "enter · shift+enter · esc",
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
            "enter save · esc cancel",
            "enter save · esc cancel",
            "enter save · esc",
        ],
    }
}

fn edit_chrome_line(
    mode: BoardInputMode,
    review: bool,
    lead: &[ChromeRowPart<'_>],
    width: usize,
) -> String {
    use ratatui::text::Line;

    /// Narrowest lead worth painting; below this only the legend is left.
    const REASON_FLOOR: usize = 8;
    /// Columns the row spends on padding and the separator: `" " + " · "`.
    const FRAME: usize = 7;

    let idle = [ChromeRowPart::Message("editing…")];
    let lead = if lead.is_empty() { &idle[..] } else { lead };
    let full = fit_chrome_row(lead, usize::MAX);
    let legends = edit_chrome_legends(mode, review);
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

/// The Undo control on the delete recovery notice, single or bulk: the key and its label.
pub const DELETE_NOTICE_UNDO: &str = "ctrl+u undo";

const CHROME_ROW_SEPARATOR: &str = "  ·  ";
const CHROME_ROW_OMITTED: &str = "…";
const NOTICE_TITLE_FLOOR: usize = 4;
const MESSAGE_FLOOR: usize = 16;

/// Display width of a presented string, in terminal cells.
pub(super) fn row_width(text: &str) -> usize {
    ratatui::text::Line::from(text).width()
}

/// The delete recovery notice around a title: `Deleted T12 "title" · ctrl+u undo`. A task
/// without a `T` number (a notice task) passes no label and keeps the title alone.
pub(super) fn notice_framed(label: Option<&str>, title: &str, undo: bool) -> String {
    let subject = match label {
        Some(label) => format!("{label} \"{title}\""),
        None => format!("\"{title}\""),
    };
    if undo {
        format!("Deleted {subject} · {DELETE_NOTICE_UNDO}")
    } else {
        format!("Deleted {subject}")
    }
}

#[derive(Debug, Clone, Copy)]
enum ChromeRowPart<'a> {
    Notice {
        label: Option<&'a str>,
        title: &'a str,
        undo: bool,
    },
    Message(&'a str),
}

impl ChromeRowPart<'_> {
    fn want(&self) -> usize {
        match self {
            Self::Notice { label, title, undo } => row_width(&notice_framed(*label, title, *undo)),
            Self::Message(message) => row_width(message),
        }
    }

    fn floor(&self) -> usize {
        match self {
            Self::Notice { label, undo, .. } => {
                (row_width(&notice_framed(*label, "", *undo)) + NOTICE_TITLE_FLOOR).min(self.want())
            }
            Self::Message(message) => MESSAGE_FLOOR.min(row_width(message)),
        }
    }

    fn render(&self, budget: usize) -> String {
        match self {
            Self::Notice { label, title, undo } => {
                let frame = row_width(&notice_framed(*label, "", *undo));
                notice_framed(
                    *label,
                    &present_line(title, budget.saturating_sub(frame)),
                    *undo,
                )
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

#[cfg(test)]
mod tests {
    use super::*;

    /// T235: field-edit legends name keys in lowercase, like every footer and verb bar.
    #[test]
    fn edit_chrome_legends_name_keys_in_lowercase() {
        for mode in [
            BoardInputMode::Normal,
            BoardInputMode::EditTitle,
            BoardInputMode::EditNotes,
            BoardInputMode::SelectThread,
            BoardInputMode::SelectBase,
            BoardInputMode::SelectAfter,
            BoardInputMode::EditThread,
            BoardInputMode::EditScope,
            BoardInputMode::EditAssignee,
            BoardInputMode::EditReply,
            BoardInputMode::BlockCard,
            BoardInputMode::EditStep,
        ] {
            for review in [false, true] {
                for legend in edit_chrome_legends(mode, review) {
                    assert!(
                        !legend.chars().any(|c| c.is_ascii_uppercase()),
                        "{mode:?} legend {legend:?} must be lowercase"
                    );
                }
            }
        }
    }
}
