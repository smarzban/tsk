//! Interactive terminal presentation (Board UI / Capture UI).

pub mod board;
pub mod capture;
pub mod edit;
pub mod input;
pub mod markdown;
pub mod mouse;
pub mod queue;
pub mod render;
pub mod scheduler;
pub mod scrollbar;
pub mod selection;
pub mod text_select;
pub mod tier;

/// Render untrusted values without allowing C0/C1 terminal control sequences through.
pub fn terminal_text(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| {
            if character.is_control() {
                format!("\\u{{{:04x}}}", character as u32)
                    .chars()
                    .collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect()
}

/// Safely present a one-line value within its terminal display width.
///
/// This deliberately preserves the existing control-character treatment and does
/// not reinterpret Unicode bidi or format controls.
pub(crate) fn present_line(value: &str, max_width: usize) -> String {
    use ratatui::text::Line;

    let safe = terminal_text(value);
    if Line::from(safe.as_str()).width() <= max_width {
        return safe;
    }
    if max_width == 0 {
        return String::new();
    }

    let content_width = max_width.saturating_sub(Line::from("…").width());
    let mut out = String::new();
    let mut width: usize = 0;
    for character in safe.chars() {
        let character_width = Line::from(character.to_string()).width();
        if width.saturating_add(character_width) > content_width {
            break;
        }
        out.push(character);
        width += character_width;
    }
    out.push('…');
    out
}

/// The one definition of a line break this crate uses: a `\r\n` pair, a lone `\n`, or a
/// lone `\r`, each ending one line.
///
/// Every place that has to know where a line ends reads this: the two multiline presenters
/// ([`present_lines`] and [`edit::escaped_draft_rows`]), the paste flattening
/// ([`edit::flatten_line_breaks`]), and the line movements, which bound the current
/// line by the same characters so Home and End stay on the row the draft is painted on.
/// They have to agree, because the store keeps a pasted break
/// **verbatim** in a Notes draft: a note pasted from a CRLF source is stored with its pairs,
/// so a presenter that only knew `'\n'` would leave a `\u{000d}` escape dangling at the end
/// of every rendered line, and would not split a lone-`\r` note at all.
///
/// A `\r\n` pair is consumed whole, so no `\r` ever survives into a rendered line. This is
/// the only control character whose presentation differs from [`terminal_text`]'s escape:
/// a `\r` that is not a line break cannot exist under this definition.
///
/// Like `str::split`, a trailing break yields a final empty line, and an empty value yields
/// one empty line.
pub(crate) fn split_line_breaks(value: &str) -> impl Iterator<Item = &str> {
    let mut rest = Some(value);
    std::iter::from_fn(move || {
        let current = rest?;
        match current.find(['\n', '\r']) {
            Some(at) => {
                let width = if current[at..].starts_with("\r\n") {
                    2
                } else {
                    1
                };
                rest = Some(&current[at + width..]);
                Some(&current[..at])
            }
            None => {
                rest = None;
                Some(current)
            }
        }
    })
}

/// present a stored value across the rows of a `max_width` x `max_height` region.
///
/// One stored line becomes one rendered line, in order, splitting on [`split_line_breaks`].
/// Splitting happens *before* the escaping in [`present_line`], which is what turns a line
/// break into a line boundary rather than the `\u{000a}` escape every other control
/// character still gets: the split consumes the break, and no other character's treatment
/// changes.
///
/// Each rendered line goes through [`present_line`], so it stays inside `max_width` and
/// keeps that presenter's `…` omission marker. When the value has more lines than the
/// region has rows, the last rendered row carries the same marker: the overflow is visibly
/// omitted rather than silently dropped, and nothing is reflowed into the missing rows. An
/// omitted line with no content is not worth a marker, so a value ending in a line break
/// does not claim to be hiding something.
pub(crate) fn present_lines(value: &str, max_width: usize, max_height: usize) -> Vec<String> {
    if max_height == 0 {
        return Vec::new();
    }
    let mut rest = split_line_breaks(value);
    let mut out: Vec<String> = Vec::with_capacity(max_height);
    let mut last_visible = "";
    for line in rest.by_ref().take(max_height) {
        last_visible = line;
        out.push(present_line(line, max_width));
    }

    // Content left over: re-present the last visible line with the marker appended, so the
    // row ends in `…` whether or not that line was itself long enough to be truncated.
    if rest.any(|line| !line.is_empty()) {
        let last = out.len() - 1;
        out[last] = present_line(&format!("{last_visible}…"), max_width);
    }
    out
}

pub use board::{
    apply_intent, board_hit_map, board_verb_items, draw_board, BoardInputMode, BoardModel,
    IntentOutcome, BOARD_TITLE,
};
pub use capture::{
    apply_capture_intent, draw_capture, format_scope, CaptureField, CaptureModel, CaptureOutcome,
    CaptureScopeChoice, CAPTURE_SCOPE_CONTROLS, CAPTURE_TITLE,
};
pub use input::{
    intent_primary_action, intent_primary_capture_action, map_capture_key, map_capture_paste_state,
    map_edit_paste, map_key, primary_action_sample_key, primary_capture_action_sample_focus,
    primary_capture_action_sample_key, BoardIntent, CaptureIntent, PrimaryBoardAction,
    PrimaryCaptureAction, BOARD_HELP_LINE, CAPTURE_HELP_LINE, PRIMARY_BOARD_ACTIONS,
    PRIMARY_CAPTURE_ACTIONS,
};
pub use mouse::{
    capture_layout, capture_layout_for_model, capture_layout_state, left_click, map_board_mouse,
    map_capture_mouse, primary_capture_action_sample_mouse, BoardPopup, CaptureLayout, Chip,
};

#[cfg(test)]
mod tests {
    use super::{present_line, present_lines, terminal_text};
    use ratatui::text::Line;

    /// one stored line becomes one rendered line, in order, with no escape left
    /// where the break was.
    #[test]
    fn present_lines_gives_one_rendered_line_per_stored_line() {
        assert_eq!(
            present_lines("first\nsecond\nthird", 20, 5),
            vec!["first", "second", "third"]
        );
        // A value with no break at all is still exactly one line.
        assert_eq!(present_lines("only", 20, 5), vec!["only"]);
    }

    /// each rendered line keeps the one-line presenter's width bound and marker.
    #[test]
    fn present_lines_bounds_every_line_by_the_allocated_width() {
        let lines = present_lines("abcdefgh\nij\nklmnopqr", 4, 5);
        assert_eq!(lines, vec!["abc…", "ij", "klm…"]);
        for line in &lines {
            assert!(
                Line::from(line.as_str()).width() <= 4,
                "line {line:?} overflowed its allocation"
            );
        }
    }

    /// more stored lines than rows means the overflow is visibly omitted on the last
    /// rendered row, never silently dropped and never reflowed into the missing rows.
    #[test]
    fn present_lines_marks_the_last_row_when_lines_are_omitted() {
        let lines = present_lines("one\ntwo\nthree\nfour", 20, 2);
        assert_eq!(lines, vec!["one", "two…"]);

        // The marker also fits when the last visible line already fills the width.
        let narrow = present_lines("aaaa\nbbbb\ncccc", 4, 2);
        assert_eq!(narrow.len(), 2);
        assert!(narrow[1].ends_with('…'), "{narrow:?}");
        for line in &narrow {
            assert!(
                Line::from(line.as_str()).width() <= 4,
                "line {line:?} overflowed its allocation"
            );
        }

        // No rows at all paints nothing; exactly enough rows spends no marker.
        assert!(present_lines("one\ntwo", 20, 0).is_empty());
        assert_eq!(present_lines("one\ntwo", 20, 2), vec!["one", "two"]);
    }

    /// one definition of a line break. A note pasted from a CRLF source is
    /// stored with its pair verbatim, and a note from a classic-Mac source carries lone
    /// `\r`; both must render exactly as the `\n` note does, with no `\u{000d}` left over.
    #[test]
    fn present_lines_reads_one_definition_of_a_line_break() {
        for value in [
            "first\nsecond\nthird",
            "first\r\nsecond\r\nthird",
            "first\rsecond\rthird",
            "first\r\nsecond\rthird",
        ] {
            let lines = present_lines(value, 20, 5);
            assert_eq!(
                lines,
                vec!["first", "second", "third"],
                "{value:?} did not render as three plain lines"
            );
        }
        // The overflow marker counts the same breaks the rows do.
        assert_eq!(
            present_lines("one\r\ntwo\r\nthree", 20, 2),
            vec!["one", "two…"]
        );
    }

    /// only the line break becomes a line boundary. Every other control character
    /// keeps exactly the escaping [`terminal_text`] has always given it.
    #[test]
    fn present_lines_leaves_every_other_control_character_escaped_as_before() {
        // Spelled out, so a change to the escape form cannot pass unnoticed.
        assert_eq!(
            present_lines("a\tb\nc\u{1b}[2Jd\ne\u{85}f", 40, 5),
            vec!["a\\u{0009}b", "c\\u{001b}[2Jd", "e\\u{0085}f"],
            "a tab, an ESC and a C1 control must still escape exactly as they do today"
        );
        assert_eq!(present_lines("x\u{7}y", 40, 5), vec!["x\\u{0007}y"]);
        // The helper the presenter delegates to is unchanged for those characters too.
        assert_eq!(terminal_text("a\tb"), "a\\u{0009}b");
    }

    #[test]
    fn terminal_text_visibly_encodes_control_sequences() {
        assert_eq!(
            terminal_text("safe\u{1b}]52;clipboard\u{7}"),
            "safe\\u{001b}]52;clipboard\\u{0007}"
        );
    }

    /// boundary contract: the presenter never exceeds its allocation, and it only
    /// spends a marker when it actually dropped content.
    #[test]
    fn present_line_boundaries_never_exceed_the_allocation_or_mark_a_fitting_line() {
        // Zero allocated width paints nothing at all, marker included.
        assert_eq!(present_line("abcdef", 0), "");
        assert_eq!(present_line("", 0), "");

        // Empty content stays empty however much room it is given.
        assert_eq!(present_line("", 8), "");

        // An exact fit is returned verbatim: no marker for nothing omitted.
        let exact = present_line("abcd", 4);
        assert_eq!(exact, "abcd");
        assert!(!exact.contains('…'), "{exact:?}");
        assert_eq!(Line::from(exact).width(), 4);

        // A one-column allocation of longer content can only be the marker.
        assert_eq!(present_line("abcdef", 1), "…");
        assert_eq!(Line::from(present_line("abcdef", 1)).width(), 1);

        // Double-width content at an odd boundary must under-fill, never overflow: a
        // second wide glyph plus the marker would be 5 columns in a 4-column slot.
        let wide = present_line("你好世界", 4);
        assert!(
            Line::from(wide.as_str()).width() <= 4,
            "wide glyph overflowed its allocation: {wide:?}"
        );
        assert!(wide.contains('…'), "{wide:?}");
        assert_eq!(wide, "你…");

        // Control-sequence escaping is unchanged by any of the above.
        assert!(present_line("safe\u{1b}]52;clipboard", 40).contains("\\u{001b}"));
        assert_eq!(present_line("a\u{7}b", 40), "a\\u{0007}b");
    }
}
