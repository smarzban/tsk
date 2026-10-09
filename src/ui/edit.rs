//! Shared text editing: the cursor-carrying edit buffer, the one-line viewport that
//! presents it, and the edit-row render geometry both editing surfaces paint through.
//!
//! The board's and the Capture form's Title and Notes field editors are the consumers: the
//! reducer drives [`EditBuffer`], and the renderer turns a buffer into a painted row via
//! [`edit_block_region`] (label columns off the row), [`escaped_line_window`] (escape once,
//! then window around the cursor) and [`place_edit_cursor`] (the terminal's own cursor).
//! Everything both surfaces need identically lives here rather than in either of them.

use ratatui::layout::{Position, Rect};
use ratatui::text::Line;
use ratatui::Frame;

use super::{present_line, split_line_breaks, terminal_text};

/// An editable field value carrying a logical cursor.
///
/// The cursor is an insertion index measured in Unicode scalar values (ADR 0005):
/// `0` sits before the first character and `char_count()` sits after the last.
#[derive(Clone, Debug)]
pub(crate) struct EditBuffer {
    value: String,
    cursor: usize,
}

impl EditBuffer {
    /// Build a buffer over `value`, clamping `cursor` into the value's character range.
    pub(crate) fn new(value: &str, cursor: usize) -> Self {
        let count = value.chars().count();
        Self {
            value: value.to_string(),
            cursor: cursor.min(count),
        }
    }

    pub(crate) fn value(&self) -> &str {
        &self.value
    }

    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    /// Park the cursor at `index`, clamped into the value's character range.
    pub(crate) fn set_cursor(&mut self, index: usize) {
        self.cursor = index.min(self.char_count());
    }

    // SHORTCUT: character indexing rescans the value -- fine for one-line titles and
    // short notes; carry a cached char index if this ever backs a large document.
    pub(crate) fn char_count(&self) -> usize {
        self.value.chars().count()
    }

    /// Byte offset of the character at `index`, or the value's length past the last one.
    fn byte_offset(&self, index: usize) -> usize {
        self.value
            .char_indices()
            .nth(index)
            .map_or(self.value.len(), |(offset, _)| offset)
    }

    /// insert one character at the cursor, which advances past it.
    pub(crate) fn insert_char(&mut self, character: char) {
        let at = self.byte_offset(self.cursor);
        self.value.insert(at, character);
        self.cursor += 1;
    }

    /// insert a run at the cursor, which ends immediately after it.
    pub(crate) fn insert_text(&mut self, text: &str) {
        let at = self.byte_offset(self.cursor);
        self.value.insert_str(at, text);
        self.cursor += text.chars().count();
    }

    /// remove the character before the cursor; a no-op at position 0.
    pub(crate) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let at = self.byte_offset(self.cursor - 1);
        self.value.remove(at);
        self.cursor -= 1;
    }

    /// remove the character at the cursor; a no-op at the end of the value.
    pub(crate) fn delete_forward(&mut self) {
        if self.cursor == self.char_count() {
            return;
        }
        let at = self.byte_offset(self.cursor);
        self.value.remove(at);
    }

    /// one character toward the start, stopping at position 0.
    pub(crate) fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// one character toward the end, stopping at the character count.
    pub(crate) fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.char_count());
    }

    /// the start of the cursor's own line, bounded by [`is_line_break`].
    pub(crate) fn move_line_start(&mut self) {
        let chars: Vec<char> = self.value.chars().collect();
        let from = line_cursor(&chars, self.cursor);
        self.cursor = chars[..from]
            .iter()
            .rposition(|character| is_line_break(*character))
            .map_or(0, |index| index + 1);
    }

    /// the end of the cursor's own line, bounded by [`is_line_break`].
    pub(crate) fn move_line_end(&mut self) {
        let chars: Vec<char> = self.value.chars().collect();
        let from = line_cursor(&chars, self.cursor);
        self.cursor = chars[from..]
            .iter()
            .position(|character| is_line_break(*character))
            .map_or(chars.len(), |offset| from + offset);
    }

    /// the nearest word boundary strictly before the cursor, stopping at 0.
    pub(crate) fn move_word_left(&mut self) {
        let chars: Vec<char> = self.value.chars().collect();
        if let Some(target) = (0..self.cursor)
            .rev()
            .find(|position| is_word_boundary(&chars, *position))
        {
            self.cursor = target;
        }
    }

    /// the nearest word boundary strictly after the cursor, stopping at the end.
    pub(crate) fn move_word_right(&mut self) {
        let chars: Vec<char> = self.value.chars().collect();
        if let Some(target) =
            (self.cursor + 1..=chars.len()).find(|position| is_word_boundary(&chars, *position))
        {
            self.cursor = target;
        }
    }
}

/// per character: a line break is a `\r\n` pair, a lone `\n`, or a lone `\r`, so
/// either character ends a line. The one definition of a break ([`super::split_line_breaks`])
/// read from the cursor's side of it: the current line is the maximal run around the
/// cursor holding no break, and the presenters cut their rows on the very same characters,
/// so the line Home and End move along is the row the draft is painted on.
fn is_line_break(character: char) -> bool {
    character == '\n' || character == '\r'
}

/// The position line movement measures from: the cursor, except inside a `\r\n` pair, where
/// it is the position just past the pair.
///
/// A cursor parked between a `\r` and its `\n` is on no line at all by the letter of the
/// definition: it sits inside the break, which is one break rather than two. It is pinned
/// to the line that **follows** the pair, because that is the row
/// [`escaped_draft_rows`] already paints it on (column 0 of the following row). Home from
/// there is therefore visibly a no-op instead of a jump to the previous row, End runs to
/// the end of the row the cursor is seen on, and both carry the cursor out of the pair's
/// interior, where no character position of the rendered text exists.
fn line_cursor(chars: &[char], cursor: usize) -> usize {
    if cursor > 0 && chars[cursor - 1] == '\r' && chars.get(cursor) == Some(&'\n') {
        cursor + 1
    } else {
        cursor
    }
}

/// A cursor position is a word boundary at either end of the value, or wherever the
/// characters around it cross between whitespace and non-whitespace.
fn is_word_boundary(chars: &[char], position: usize) -> bool {
    if position == 0 || position == chars.len() {
        return true;
    }
    chars[position - 1].is_whitespace() != chars[position].is_whitespace()
}

/// window a field value into `width` display columns, keeping the cursor visible.
///
/// Returns the window to render and the cursor's column offset within it. For an
/// allocation of `width` columns, the returned `(window, column)` satisfies all of:
///
/// - the window's display width is at most `width`, and it under-fills rather than
///   splitting a double-width character;
/// - the window is a contiguous run of the value's characters spanning the cursor
///   position, which may sit at either edge of that run;
/// - `column` is the display width of the window's characters before the cursor, and
///   `column <= width - 1`: the cursor cell is always inside the allocation, so one
///   column stays reserved for it when the value would otherwise fill the allocation
///   exactly (`width == 0` returns `("", 0)` instead);
/// - the window begins at the *smallest* start index for which all of the above hold,
///   which is the minimal scroll. A value that fits is returned whole, unless it fills the
///   allocation exactly and the cursor sits at its end, where the reserved cursor column
///   wins and the window scrolls by one character; a cursor near the start of a long value
///   shows the value from its first character with trailing context filling the rest of the
///   window; the window scrolls only once the cursor would otherwise fall outside it.
///
/// Every width above is measured over a whole run of the value, never summed from its
/// characters: `unicode-width` is explicit that a string's width is not the sum of its
/// characters' widths for variation-selector sequences, ZWJ sequences, `"\r\n"` and some
/// ligatures, and summing would both overflow the allocation and misplace the column.
///
/// The returned column indexes the returned window *as given*. A caller that escapes its
/// text for display must escape **before** calling, so that the window and the column
/// describe the same string it paints; escaping afterwards changes character counts and
/// misplaces the cursor.
pub(crate) fn field_viewport(value: &str, cursor: usize, width: usize) -> (String, usize) {
    if width == 0 {
        return (String::new(), 0);
    }

    // Byte offset of every character index, plus the value's length, so a run can be
    // measured as a borrowed slice of `value` rather than rebuilt as a fresh `String`.
    let mut byte_of: Vec<usize> = value.char_indices().map(|(offset, _)| offset).collect();
    byte_of.push(value.len());
    let count = byte_of.len() - 1;
    let cursor = cursor.min(count);

    // Each run is measured whole, never summed from per-character widths. Both loops below
    // advance one index per measurement, so the number of measurements stays linear in the
    // value's length, and measuring a borrowed slice allocates nothing.
    let run_width = |from: usize, to: usize| Line::from(&value[byte_of[from]..byte_of[to]]).width();

    // One column is reserved for the cursor cell itself, so the run before the cursor may
    // occupy at most `width - 1`. Advance to the smallest start that fits: minimal scroll.
    let before_cursor = width - 1;
    let mut start = 0;
    while run_width(start, cursor) > before_cursor {
        start += 1;
    }

    // Fill the allocation forward, stopping at the first character that would overflow it.
    // Since the run before the cursor fits in `width - 1`, this always reaches the cursor.
    let mut end = start;
    while end < count && run_width(start, end + 1) <= width {
        end += 1;
    }

    (
        value[byte_of[start]..byte_of[end]].to_string(),
        run_width(start, cursor),
    )
}

/// Open a draft on `value` with the cursor parked after its last character.
pub(crate) fn seeded_draft(value: &str) -> EditBuffer {
    EditBuffer::new(value, value.chars().count())
}

/// fold a pasted run onto one line, keeping the word boundary each break carried.
///
/// One break becomes one space, over the crate's single definition of a line break
/// ([`super::split_line_breaks`]), so a CRLF pair is one break rather than two: mapping its
/// two characters separately would double-space every line boundary of a Windows-flavoured
/// paste. Flattening and the multiline presenters read that one definition, so a note
/// created by paste and a note created by Enter break in exactly the same places.
pub(crate) fn flatten_line_breaks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, line) in split_line_breaks(text).enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(line);
    }
    out
}

/// One active edit line: the draft escaped and windowed around its cursor, plus the display
/// column inside the region where the cursor sits.
///
/// Escaping order is pinned by [`field_viewport`]'s contract: escape the raw draft once,
/// translate the cursor into the escaped string, window that, and paint the window
/// directly. `present_line` is deliberately not used on the window: it is already escaped
/// and already width-bounded, and a second pass would escape twice and re-truncate.
///
/// Callers paint the window as **one unsplit run**. Slicing it to style a block-cursor cell
/// tears grapheme clusters apart: a combining mark or a ZWJ joiner occupies no column, so
/// a column-derived split lands inside the cluster, the cursor cell becomes a zero-width
/// symbol that paints nothing at all, and the cluster's remainder renders as separate
/// glyphs. Re-joining the pieces would need text segmentation, which ADR 0005 rules out.
/// The terminal's own cursor carries the position instead ([`place_edit_cursor`]), so no
/// cell on the row is ever rewritten.
///
/// A multi-line draft is spread over rows by [`escaped_draft_rows`], which calls this once
/// per row for the row the cursor is on; a caller with one row of its own passes the whole
/// draft here, and every break in it escapes like any other control character.
pub(crate) fn escaped_line_window(draft: &EditBuffer, width: usize) -> (String, u16) {
    let escaped = terminal_text(draft.value());
    let escaped_cursor = terminal_text(
        &draft
            .value()
            .chars()
            .take(draft.cursor())
            .collect::<String>(),
    )
    .chars()
    .count();
    let (window, column) = field_viewport(&escaped, escaped_cursor, width);

    (window, u16::try_from(column).unwrap_or(u16::MAX))
}

/// the draft spread over the rows of an edit region, one draft line per row, plus
/// the row and column the terminal cursor belongs on.
///
/// Splitting on line breaks happens first and escaping second, exactly as
/// [`super::present_lines`] does it for a stored note and over the same definition
/// ([`super::split_line_breaks`]), so a break becomes a row boundary while every other
/// control character keeps its escape. The cursor's own line is windowed by
/// [`escaped_line_window`], so it scrolls horizontally to keep the cursor visible; the
/// other rows are presented from their first column, since no cursor anchors them.
///
/// A draft with more lines than the region has rows scrolls vertically by the minimum that
/// keeps the cursor's line visible, which is [`field_viewport`]'s horizontal rule applied
/// to rows. No line is dropped: unlike a stored note, every line here is reachable by
/// moving the cursor, so the rows carry no omission marker.
pub(crate) fn escaped_draft_rows(
    draft: &EditBuffer,
    width: usize,
    height: usize,
) -> (Vec<String>, u16, u16) {
    if height == 0 {
        return (Vec::new(), 0, 0);
    }
    let value = draft.value();
    let lines: Vec<&str> = split_line_breaks(value).collect();

    // The cursor's line and its column within that line, measured over the same split: the
    // text before the cursor has one more line than it has breaks, and its last line is
    // what the cursor sits on. (A cursor parked between a `\r` and its `\n` therefore reads
    // as column 0 of the following line, which is where it visibly belongs.)
    let before: String = value.chars().take(draft.cursor()).collect();
    let (mut cursor_line, mut cursor_column) = (0, 0);
    for (index, line) in split_line_breaks(&before).enumerate() {
        cursor_line = index;
        cursor_column = line.chars().count();
    }

    // The smallest first row that still shows the cursor's line: minimal vertical scroll.
    // Since `cursor_line < lines.len()`, the cursor's line is always inside the window.
    let first_row = cursor_line.saturating_sub(height - 1);
    let (cursor_window, column) = escaped_line_window(
        &EditBuffer::new(
            lines.get(cursor_line).copied().unwrap_or_default(),
            cursor_column,
        ),
        width,
    );
    let mut rows: Vec<String> = lines
        .iter()
        .skip(first_row)
        .take(height)
        .map(|line| present_line(line, width))
        .collect();
    if let Some(row) = rows.get_mut(cursor_line - first_row) {
        *row = cursor_window;
    }

    (
        rows,
        u16::try_from(cursor_line - first_row).unwrap_or(u16::MAX),
        column,
    )
}

/// One character's width in terminal cells, measured the same way the painter measures a row.
fn char_cells(character: char) -> usize {
    Line::from(character.to_string()).width()
}

/// One wrapped row of a value: the escaped text as painted, plus everything cursor
/// mapping and vertical navigation need to translate between raw character positions
/// and painted cells.
#[derive(Debug, Clone)]
pub(crate) struct WrappedRow {
    /// Escaped text as painted.
    pub text: String,
    /// Raw scalar index of this row's first and last characters (inclusive).
    /// A blank line's row carries no characters: both equal the line's start.
    pub first_raw: usize,
    pub last_raw: usize,
    /// Cell offset of each raw character on this row, in row order; length equals
    /// the number of raw characters living here.
    pub cell_of: Vec<usize>,
    /// Total display width of the row in terminal cells.
    pub width: usize,
    /// The row opens a logical line (its own line's first row).
    pub starts_line: bool,
    /// The row closes a logical line: the next row begins a new line.
    pub ends_line: bool,
    /// Width of the break following a line-closing row: 0 mid-line, 1 for a lone
    /// `\n`/`\r`, 2 for a consumed `\r\n` pair.
    pub break_width: usize,
    /// Insertion index just past this row: `last_raw + 1`, or past the whole break
    /// when the row closes its line (a pair counts as two).
    pub end_cursor: usize,
}

impl WrappedRow {
    /// Raw cursor index for a target column on this row, clamped into it. A
    /// column past the row's end resolves past its last character -- and past
    /// its whole break when the row closes the line, so a caret parked at a
    /// row's end never sits inside a `\r\n` pair. A row that continues the
    /// line instead clamps ONTO its last character: its `end_cursor` names the
    /// NEXT row's first character, and returning it would make locate paint the
    /// caret on that row, skipping the one the move targeted.
    pub(crate) fn cursor_at(&self, column: usize) -> usize {
        if self.cell_of.is_empty() || column < self.cell_of[0] {
            return self.first_raw;
        }
        let offset = self
            .cell_of
            .iter()
            .rposition(|start| *start <= column)
            .unwrap_or(0);
        if offset == self.cell_of.len() - 1 && column >= self.width {
            if self.ends_line {
                self.end_cursor
            } else {
                self.last_raw
            }
        } else {
            self.first_raw + offset
        }
    }
}

/// Wrap `value` into rows of at most `width` display cells, preferring word
/// boundaries: a row that cannot fit the next character breaks after its last
/// whitespace character, and only a run with no whitespace at all (or a single
/// character wider than the whole allocation) hard-breaks at the cell edge.
///
/// Splitting on breaks happens first and escaping second, over the crate's one
/// definition of a break ([`super::split_line_breaks`]); wrapping measures
/// accumulated DISPLAY WIDTH, not scalar count, so double-width glyphs move whole
/// to the next row instead of being clipped away.
pub(crate) fn wrap_text(value: &str, width: usize) -> Vec<WrappedRow> {
    let mut out: Vec<WrappedRow> = Vec::new();
    if width == 0 {
        return out;
    }
    let chars: Vec<char> = value.chars().collect();
    // Cut into logical lines over chars, consuming a `\r\n` pair as one break:
    // the same lines [`super::split_line_breaks`] yields, kept as char slices so
    // raw cursor indices survive without byte/char translation.
    let mut lines: Vec<&[char]> = Vec::new();
    let mut from = 0usize;
    let mut scan = 0usize;
    while scan < chars.len() {
        if chars[scan] == '\r' || chars[scan] == '\n' {
            lines.push(&chars[from..scan]);
            scan += if chars[scan] == '\r' && chars.get(scan + 1) == Some(&'\n') {
                2
            } else {
                1
            };
            from = scan;
        } else {
            scan += 1;
        }
    }
    lines.push(&chars[from..]);

    let mut line_start = 0usize;
    for line in &lines {
        let line_opened_at = out.len();
        // (raw index, character) pairs in line order.
        let items: Vec<(usize, char)> = line
            .iter()
            .enumerate()
            .map(|(offset, character)| (line_start + offset, *character))
            .collect();
        let mut next = 0usize;
        while next < items.len() {
            // Pass one finds this row's exclusive end: fill until the next
            // character would overflow, then prefer the last whitespace already
            // scanned -- a run with none hard-breaks, excluding the overflowing
            // character. A single character wider than the whole allocation keeps
            // its own row (an empty row is never flushed); the painter clips it.
            let mut end = next;
            let mut used = 0usize;
            let mut soft_break: Option<usize> = None;
            while end < items.len() {
                let character = items[end].1;
                let add: usize = terminal_text(&character.to_string())
                    .chars()
                    .map(char_cells)
                    .sum();
                if used + add > width && end > next {
                    end = soft_break.unwrap_or(end);
                    break;
                }
                used += add;
                if character.is_whitespace() {
                    soft_break = Some(end + 1);
                }
                end += 1;
                if used > width {
                    break;
                }
            }
            // Pass two builds the escaped text, per-character cell offsets, and the
            // row's PAINTED width: pass one's `used` keeps scanning past the soft
            // break it rewinds to, so only the text actually emitted here is a
            // truthful width for `cursor_at` and past-end columns.
            let mut text = String::new();
            let mut cell_of: Vec<usize> = Vec::new();
            let mut column = 0usize;
            for (_, character) in &items[next..end] {
                cell_of.push(column);
                for painted in terminal_text(&character.to_string()).chars() {
                    text.push(painted);
                    column += char_cells(painted);
                }
            }
            let last_raw = items[end - 1].0;
            out.push(WrappedRow {
                text,
                first_raw: items[next].0,
                last_raw,
                cell_of,
                width: column,
                starts_line: next == 0,
                ends_line: false,
                break_width: 0,
                end_cursor: last_raw + 1,
            });
            next = end;
        }
        if out.len() == line_opened_at {
            // A blank logical line paints its own empty row.
            out.push(WrappedRow {
                text: String::new(),
                first_raw: line_start,
                last_raw: line_start,
                cell_of: Vec::new(),
                width: 0,
                starts_line: true,
                ends_line: false,
                break_width: 0,
                end_cursor: line_start,
            });
        }
        // Close the line: mark its last row and advance past content plus break.
        let end = line_start + line.len();
        let break_width = if end < chars.len() {
            usize::from(chars[end] == '\r' && chars.get(end + 1) == Some(&'\n')) + 1
        } else {
            0
        };
        if let Some(last) = out.last_mut() {
            last.ends_line = true;
            last.break_width = break_width;
            last.end_cursor = end + break_width;
        }
        line_start = end + break_width;
    }
    out
}

/// Map a raw cursor index onto the wrapped layout: the row it paints on and the
/// cell column within that row.
///
/// Conventions shared with the line movements and [`escaped_draft_rows`]: a cursor
/// sitting ON a break reads as the line before it at its past-end column, except
/// inside a `\r\n` pair, where it is pinned to the line that follows at column 0.
pub(crate) fn locate_wrapped_cursor(rows: &[WrappedRow], cursor: usize) -> (usize, usize) {
    // 1. A character carries its cursor: the first row owning this raw index.
    for (index, row) in rows.iter().enumerate() {
        if !row.cell_of.is_empty() && row.first_raw <= cursor && cursor <= row.last_raw {
            let offset = cursor - row.first_raw;
            return (index, row.cell_of[offset]);
        }
    }
    // 2. A cursor sitting on the break itself -- on a lone `\n`, a lone `\r`, or
    // the `\r` that opens a consumed pair -- reads as the line's past-end column.
    for (index, row) in rows.iter().enumerate() {
        if row.ends_line && cursor == row.last_raw + 1 {
            return (index, row.width);
        }
    }
    // 3. At or before a line's start -- value start, right after a break, blank
    // line, or inside a `\r\n` pair (pinned forward): that line's first row, column 0.
    for (index, row) in rows.iter().enumerate() {
        if row.starts_line && cursor <= row.first_raw {
            return (index, 0);
        }
    }
    // 4. The value's very end.
    let last = rows.len().saturating_sub(1);
    (last, rows.get(last).map_or(0, |row| row.width))
}

/// The whole draft wrapped to `width` display cells, escaped, one row per wrapped
/// line, plus the wrapped row and column where the draft's cursor belongs. Both are
/// absolute: the row counts from the first returned row, and the column counts
/// terminal cells from that row's first painted cell.
///
/// Wrapping prefers word boundaries ([`wrap_text`]) and nothing is dropped; an
/// empty draft yields one empty row.
pub(crate) fn wrapped_edit_rows(draft: &EditBuffer, width: usize) -> (Vec<String>, usize, usize) {
    let rows = wrap_text(draft.value(), width);
    let (row, column) = locate_wrapped_cursor(&rows, draft.cursor());
    (
        rows.into_iter().map(|wrapped| wrapped.text).collect(),
        row,
        column,
    )
}

/// The whole draft wrapped to `width` display cells, escaped, one row per wrapped
/// line. View-only presentation: no cursor anchors any line, so this is
/// [`wrapped_edit_rows`] without its cursor report.
pub(crate) fn wrapped_draft_rows(draft: &EditBuffer, width: usize) -> Vec<String> {
    wrap_text(draft.value(), width)
        .into_iter()
        .map(|row| row.text)
        .collect()
}

/// Move the draft's cursor vertically by `delta` wrapped rows, preserving the
/// target column in cells as far as the destination row allows. Clamps at the
/// first and last wrapped rows; `None` means nothing changed (zero width).
pub(crate) fn wrapped_vertical_move(
    draft: &EditBuffer,
    width: usize,
    delta: isize,
) -> Option<usize> {
    if width == 0 {
        return None;
    }
    let rows = wrap_text(draft.value(), width);
    let (current_row, column) = locate_wrapped_cursor(&rows, draft.cursor());
    let target = current_row as isize + delta;
    let target = target.clamp(0, rows.len().saturating_sub(1) as isize) as usize;
    let cursor = rows
        .get(target)
        .map(|row| row.cursor_at(column))
        .unwrap_or_else(|| draft.cursor());
    (cursor != draft.cursor()).then_some(cursor)
}

/// The label columns off the front of every row of an edit region, keeping the region's
/// full height for a draft spread over rows.
///
/// N2 removed this module's single-row sibling (`edit_region`) along with its one caller,
/// the retired legacy Browse edit band; Capture is the sole remaining caller of this form.
pub(crate) fn edit_block_region(row: Rect, label: u16) -> Rect {
    let label = label.min(row.width);
    Rect {
        x: row.x.saturating_add(label),
        y: row.y,
        width: row.width - label,
        height: row.height,
    }
}

/// Put the terminal's own cursor on the edit position, clamped inside the region so a row
/// too narrow for its label can never push the cursor off the row.
pub(crate) fn place_edit_cursor(frame: &mut Frame, region: Rect, column: u16) {
    place_edit_cursor_at(frame, region, 0, column);
}

/// [`place_edit_cursor`] for a region of more than one row: `row` counts from the region's
/// first row and is clamped inside it, as `column` already was.
pub(crate) fn place_edit_cursor_at(frame: &mut Frame, region: Rect, row: u16, column: u16) {
    if region.width == 0 || region.height == 0 {
        return;
    }
    let last_column = region.x.saturating_add(region.width - 1);
    let last_row = region.y.saturating_add(region.height - 1);
    frame.set_cursor_position(Position::new(
        region.x.saturating_add(column).min(last_column),
        region.y.saturating_add(row).min(last_row),
    ));
}

#[cfg(test)]
mod tests {
    use super::{
        escaped_draft_rows, field_viewport, seeded_draft, wrapped_draft_rows, wrapped_edit_rows,
        EditBuffer,
    };
    use ratatui::text::Line;

    /// `wrapped_edit_rows` wraps exactly as view mode does (it is the same
    /// function with a cursor report), so the wide-character sweep covers both.
    #[test]
    fn wrapped_edit_rows_wrap_wide_characters_and_drop_nothing() {
        let text = "日".repeat(30);
        let width = 20;
        let draft = seeded_draft(&text);
        let (rows, row, column) = wrapped_edit_rows(&draft, width);

        for wrapped in &rows {
            assert!(
                Line::from(wrapped.as_str()).width() <= width,
                "row wider than {width} cells: {wrapped:?}"
            );
        }
        assert_eq!(rows.concat(), text, "wide-character text was lost");
        assert!(
            rows.len() > 1,
            "expected the wide run to wrap, got {rows:?}"
        );

        // The end cursor sits past the final glyph on the last wrapped row: ten
        // double-width glyphs per row fill 20 cells exactly.
        assert_eq!((row, column), (2, 20));
        // And view mode is precisely this wrapping without the cursor report.
        assert_eq!(wrapped_draft_rows(&draft, width), rows);
    }

    /// A long single-line draft wraps onto continuation rows and the cursor maps
    /// into wrapped coordinates: an interior cursor lands on its own cell of the
    /// continuation row, and the end cursor on the final chunk's past-end column.
    #[test]
    fn wrapped_edit_rows_map_the_cursor_onto_its_wrapped_row() {
        let value = format!("{}xyz", "a".repeat(25));

        // Cursor at 22: two characters into the second wrapped chunk (20 + 2).
        let (rows, row, column) = wrapped_edit_rows(&EditBuffer::new(&value, 22), 20);
        assert_eq!(rows, vec!["a".repeat(20), "aaaaaxyz".to_string()]);
        assert_eq!((row, column), (1, 2));

        // Cursor at the value's end: the last chunk's past-end column.
        let (rows, row, column) = wrapped_edit_rows(&seeded_draft(&value), 20);
        assert_eq!(rows, vec!["a".repeat(20), "aaaaaxyz".to_string()]);
        assert_eq!((row, column), (1, 8));
    }

    /// Breaks spread over rows over one definition, and a cursor sitting ON a break
    /// reads as the line before it at its past-end column -- except inside a CRLF
    /// pair, pinned to the following line, matching [`escaped_draft_rows`].
    #[test]
    fn wrapped_edit_rows_read_one_definition_of_a_line_break() {
        for value in [
            "first\nsecond\nthird",
            "first\r\nsecond\r\nthird",
            "first\rsecond\rthird",
        ] {
            let (rows, row, column) = wrapped_edit_rows(&seeded_draft(value), 20);
            assert_eq!(rows, vec!["first", "second", "third"], "{value:?}");
            assert_eq!((row, column), (2, 5), "{value:?}");
        }

        // A cursor parked between a `\r` and its `\n` paints where the renderer
        // already puts it: column 0 of the following row.
        let (_, row, column) = wrapped_edit_rows(&EditBuffer::new("one\r\ntwo", 4), 20);
        assert_eq!((row, column), (1, 0));

        // A trailing break yields the final empty line, and the end cursor is on it.
        let (rows, row, column) = wrapped_edit_rows(&seeded_draft("ab\n"), 20);
        assert_eq!(rows, vec!["ab", ""]);
        assert_eq!((row, column), (1, 0));

        // While a cursor before that break stays on "ab" at its past-end column.
        let (_, row, column) = wrapped_edit_rows(&EditBuffer::new("ab\n", 2), 20);
        assert_eq!((row, column), (0, 2));
    }

    /// Degenerate shapes stay defined: an empty draft is one empty row with the
    /// cursor at its origin; a zero-width allocation paints nothing.
    #[test]
    fn wrapped_edit_rows_handle_empty_drafts_and_zero_widths() {
        assert_eq!(
            wrapped_edit_rows(&seeded_draft(""), 20),
            (vec![String::new()], 0, 0)
        );
        assert_eq!(
            wrapped_edit_rows(&seeded_draft("abc"), 0),
            (Vec::<String>::new(), 0, 0)
        );
    }

    /// A row shortened by a soft word break reports its PAINTED width, not the
    /// pre-rewind scan total: `cursor_at` and past-end columns read that width,
    /// and an inflated one both misplaces the caret and lets Down/Up skip rows.
    #[test]
    fn a_soft_broken_row_reports_its_painted_width() {
        // "aa bb cc" at 5 cells: pass one scans 5 cells before rewinding to the
        // space, so the row paints "aa " -- three cells, not five.
        let rows = super::wrap_text("aa bb cc", 5);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].text, "aa ");
        assert_eq!(rows[0].width, 3, "the soft-broken row over-reported width");
        assert_eq!(rows[1].text, "bb cc");
        assert_eq!(rows[1].width, 5);

        // Down from the end (col 5) clamps onto row 0's last character rather
        // than resolving past it to row 1's first char, which would pin the
        // caret to row 1 and skip row 0 on the way back up.
        let at_end = EditBuffer::new("aa bb cc", 8);
        assert_eq!(super::wrapped_vertical_move(&at_end, 5, -1), Some(2));
        assert_eq!(super::wrapped_vertical_move(&at_end, 5, -1), Some(2));
        let on_row0 = EditBuffer::new("aa bb cc", 2);
        assert_eq!(
            super::wrapped_vertical_move(&on_row0, 5, -1),
            None,
            "already on the first row"
        );
    }

    /// The same rewind applies across the break: a second line's first row keeps
    /// an honest width after its own soft break (the F-8 shape).
    #[test]
    fn wrapped_widths_stay_honest_after_every_soft_break() {
        let rows = super::wrap_text("12345\nabc defgh", 6);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].text, "12345");
        assert!(rows[0].ends_line);
        assert_eq!(rows[1].text, "abc ");
        assert_eq!(rows[1].width, 4, "row kept the pre-rewind total of 6");
        assert_eq!(rows[2].text, "defgh");
        assert_eq!(rows[2].width, 5);
    }

    /// Words are not cut: a row that cannot fit its next character breaks after
    /// the last whitespace it already holds, and only a whitespace-free run
    /// hard-breaks at the cell edge. The break's trailing space stays row-end.
    #[test]
    fn wrap_text_prefers_word_boundaries_over_cell_edges() {
        let rows_of = |value: &str, width| {
            super::wrap_text(value, width)
                .into_iter()
                .map(|row| row.text)
                .collect::<Vec<_>>()
        };

        assert_eq!(
            rows_of("the quick brown fox", 10),
            vec!["the quick ", "brown fox"]
        );
        // A word longer than the whole allocation still hard-breaks...
        assert_eq!(
            rows_of("aaaaaaaaaaaa bb", 5),
            vec!["aaaaa", "aaaaa", "aa bb"]
        );
        // ...and a whitespace-free value behaves exactly as before.
        assert_eq!(rows_of("abcdefgh", 3), vec!["abc", "def", "gh"]);
        // Multiple spaces collapse onto the row they end.
        assert_eq!(rows_of("aa    bb", 4), vec!["aa  ", "  bb"]);
    }

    /// Vertical movement walks WRAPPED rows, preserving the target column in cells
    /// and clamping at the first and last row.
    #[test]
    fn wrapped_vertical_move_walks_wrapped_rows_and_preserves_the_column() {
        // "alpha beta gamma" at 6 cells wraps as "alpha ", "beta ", "gamma".
        let value = "alpha beta gamma";
        let rows = || {
            super::wrap_text(value, 6)
                .into_iter()
                .map(|row| row.text)
                .collect::<Vec<String>>()
        };
        assert_eq!(rows(), vec!["alpha ", "beta ", "gamma"]);

        // Down from (0, col 3) lands on raw index 9 (the 'a' of "beta"); up again
        // restores (0, col 3).
        assert_eq!(
            super::wrapped_vertical_move(&EditBuffer::new(value, 3), 6, 1),
            Some(9)
        );
        assert_eq!(
            super::wrapped_vertical_move(&EditBuffer::new(value, 9), 6, -1),
            Some(3)
        );

        // Clamped at both ends rather than wrapping around.
        assert_eq!(
            super::wrapped_vertical_move(&EditBuffer::new(value, 1), 6, -1),
            None
        );
        let at_end = EditBuffer::new(value, value.chars().count());
        assert_eq!(
            super::wrapped_vertical_move(&at_end, 6, 1),
            None,
            "the caret already sits on the last wrapped row"
        );

        // Blank logical lines are real rows: down crosses them column-zero.
        assert_eq!(
            super::wrapped_vertical_move(&EditBuffer::new("a\n\nb", 0), 10, 1),
            Some(2)
        );
        assert_eq!(
            super::wrapped_vertical_move(&EditBuffer::new("a\n\nb", 2), 10, 1),
            Some(3)
        );
    }

    /// Wide characters must WRAP, never be clipped away.
    ///
    /// `width` is terminal cells, so chunking by scalar count let a double-width run overflow
    /// its row; the painter then ellipsized the overflow instead of carrying it to the next
    /// row, losing that text from the page entirely rather than merely misdrawing it.
    #[test]
    fn wrapped_draft_rows_wrap_wide_characters_by_cells_and_drop_nothing() {
        let text = "日".repeat(30);
        let width = 20;
        let rows = wrapped_draft_rows(&seeded_draft(&text), width);

        // No row may exceed the column, measured the way the painter measures it.
        for row in &rows {
            assert!(
                Line::from(row.as_str()).width() <= width,
                "row wider than {width} cells: {row:?} ({} cells)",
                Line::from(row.as_str()).width()
            );
        }
        // Every character survives, in order: wrapping moves text, it never discards it.
        assert_eq!(
            rows.concat(),
            text,
            "wide-character text was lost while wrapping"
        );
        // 30 double-width glyphs at 20 cells is 10 per row, so it must actually have wrapped.
        assert!(
            rows.len() > 1,
            "expected the wide run to wrap, got {rows:?}"
        );
    }

    /// ASCII, a two-byte letter, a double-width glyph, a four-byte emoji, a space, all
    /// three spellings of a line break, a trailing letter, a variation-selector sequence
    /// and a ZWJ family: every shape an edit has to survive. Carrying a lone `\n`, a `\r\n`
    /// pair and a lone `\r` puts the sweep's cursor inside a pair as well as on either side
    /// of each break. The two sequences' display width is not the sum of their characters'
    /// widths, so they also guard the viewport's measurement; editing itself stays per
    /// scalar value (ADR 0005), which they do not change.
    const MIXED: &str =
        "aé你🎉 b\nz\r\n\u{263A}\u{FE0F}w\r\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F466}";

    /// `U+263A U+FE0F`: two scalar values that paint as one two-column glyph, while their
    /// per-character widths sum to one.
    const VARIATION_SELECTOR: &str = "\u{263A}\u{FE0F}";

    /// A five-scalar ZWJ family emoji: two columns painted, six columns summed.
    const ZWJ_FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F466}";

    /// One cursor-movement operation, so the sweep can drive all six from one table.
    type Movement = fn(&mut EditBuffer);

    fn width_of(value: &str) -> usize {
        Line::from(value).width()
    }

    /// the edit rows read the same definition of a line break the stored-note
    /// presenter and the paste flattening read. A draft whose breaks are `\r\n` (which
    /// keeps verbatim) or lone `\r` spreads over rows exactly as the `\n` draft does, and
    /// the cursor lands on the same row and column in every one of them.
    #[test]
    fn escaped_draft_rows_reads_one_definition_of_a_line_break() {
        for value in [
            "first\nsecond\nthird",
            "first\r\nsecond\r\nthird",
            "first\rsecond\rthird",
            "first\r\nsecond\rthird",
        ] {
            let (rows, cursor_row, column) = escaped_draft_rows(&seeded_draft(value), 20, 5);
            assert_eq!(
                rows,
                vec!["first", "second", "third"],
                "{value:?} did not spread over three rows"
            );
            assert_eq!(
                (cursor_row, column),
                (2, 5),
                "{value:?} misplaced the cursor at the draft's end"
            );
        }
    }

    /// splitting consumes the break and nothing else. A tab, an ESC and a C1 control
    /// inside an edited draft keep exactly the escape they have always had.
    #[test]
    fn escaped_draft_rows_leaves_every_other_control_character_escaped() {
        let draft = EditBuffer::new("a\tb\nc\u{1b}[2Jd\ne\u{85}f", 0);
        let (rows, cursor_row, column) = escaped_draft_rows(&draft, 40, 5);
        assert_eq!(
            rows,
            vec!["a\\u{0009}b", "c\\u{001b}[2Jd", "e\\u{0085}f"],
            "an escaped control character changed shape"
        );
        assert_eq!((cursor_row, column), (0, 0));
    }

    /// a draft with more lines than the region has rows scrolls by the minimum that
    /// keeps the cursor's line visible, and the cursor sits on the region's last row.
    #[test]
    fn escaped_draft_rows_scrolls_the_minimum_that_keeps_the_cursor_line_visible() {
        let draft = seeded_draft("one\ntwo\nthree\nfour");
        let (rows, cursor_row, column) = escaped_draft_rows(&draft, 20, 2);
        assert_eq!(rows, vec!["three", "four"], "the window did not scroll");
        assert_eq!(cursor_row, 1, "the cursor left the region");
        assert_eq!(column, 4, "the cursor missed the end of its own line");

        // A cursor on an earlier line pulls the window back up by the same rule.
        let (rows, cursor_row, column) =
            escaped_draft_rows(&EditBuffer::new(draft.value(), 5), 20, 2);
        assert_eq!(rows, vec!["one", "two"]);
        assert_eq!((cursor_row, column), (1, 1));
    }

    /// typing at a mid-value cursor keeps the tail and advances the cursor by one.
    #[test]
    fn insertion_at_a_mid_value_cursor_keeps_the_tail_and_advances_the_cursor() {
        let mut buffer = EditBuffer::new("hello world", 5);
        buffer.insert_char(',');
        assert_eq!(buffer.value(), "hello, world");
        assert_eq!(buffer.cursor(), 6);
    }

    /// the two bound cases change nothing at all.
    #[test]
    fn backspace_at_the_start_and_delete_at_the_end_are_no_ops() {
        let mut at_start = EditBuffer::new("hello", 0);
        at_start.backspace();
        assert_eq!(at_start.value(), "hello");
        assert_eq!(at_start.cursor(), 0);

        let mut at_end = EditBuffer::new("hello", 5);
        at_end.delete_forward();
        assert_eq!(at_end.value(), "hello");
        assert_eq!(at_end.cursor(), 5);
    }

    /// Backspace removes the one character before the cursor and steps back.
    #[test]
    fn backspace_removes_exactly_the_character_before_the_cursor() {
        let mut buffer = EditBuffer::new("hello", 3);
        buffer.backspace();
        assert_eq!(buffer.value(), "helo");
        assert_eq!(buffer.cursor(), 2);
    }

    /// forward Delete removes the one character at the cursor and stays put.
    #[test]
    fn forward_delete_removes_exactly_the_character_at_the_cursor() {
        let mut buffer = EditBuffer::new("hello", 3);
        buffer.delete_forward();
        assert_eq!(buffer.value(), "helo");
        assert_eq!(buffer.cursor(), 3);
    }

    /// one character per step, stopping at both bounds instead of wrapping.
    #[test]
    fn character_movement_steps_once_and_clamps_at_both_bounds() {
        let mut buffer = EditBuffer::new("abc", 1);
        buffer.move_right();
        assert_eq!(buffer.cursor(), 2);
        buffer.move_left();
        assert_eq!(buffer.cursor(), 1);

        let mut at_start = EditBuffer::new("abc", 0);
        at_start.move_left();
        assert_eq!(at_start.cursor(), 0, "left wrapped past the start");

        let mut at_end = EditBuffer::new("abc", 3);
        at_end.move_right();
        assert_eq!(at_end.cursor(), 3, "right wrapped past the end");
    }

    /// line-start and line-end target the cursor's own line, not the whole value.
    #[test]
    fn line_movement_targets_the_current_line_of_a_multiline_value() {
        // "one\ntwo\nthree": the middle line spans positions 4..=7.
        let mut to_start = EditBuffer::new("one\ntwo\nthree", 6);
        to_start.move_line_start();
        assert_eq!(to_start.cursor(), 4);

        let mut to_end = EditBuffer::new("one\ntwo\nthree", 6);
        to_end.move_line_end();
        assert_eq!(to_end.cursor(), 7);

        // The last line has no trailing break: line-end is the end of the value.
        let mut last_line = EditBuffer::new("one\ntwo\nthree", 10);
        last_line.move_line_end();
        assert_eq!(last_line.cursor(), 13);

        // The first line has no leading break: line-start is the start of the value.
        let mut first_line = EditBuffer::new("one\ntwo\nthree", 2);
        first_line.move_line_start();
        assert_eq!(first_line.cursor(), 0);
    }

    /// a lone `\r` ends a line exactly as `\n` does, because line movement
    /// reads the crate's one definition of a line break. A draft broken that way (reachable
    /// by paste, whose breaks stores verbatim) already renders as several rows, so
    /// Home and End have to stay inside the row the cursor is painted on.
    #[test]
    fn line_movement_treats_a_lone_carriage_return_as_a_line_break() {
        // "one\rtwo\rthree": lines "one" 0..=3, "two" 4..=7, "three" 8..=13.
        let value = "one\rtwo\rthree";

        for (cursor, start, end, case) in [
            (6, 4, 7, "mid-line"),
            (4, 4, 7, "at the line's start"),
            (7, 4, 7, "at the line's end"),
            (3, 0, 3, "immediately before a break"),
            (8, 8, 13, "immediately after a break"),
            (1, 0, 3, "on the first line"),
            (11, 8, 13, "on the last line"),
        ] {
            let mut to_start = EditBuffer::new(value, cursor);
            to_start.move_line_start();
            assert_eq!(
                to_start.cursor(),
                start,
                "line start from {cursor} ({case})"
            );

            let mut to_end = EditBuffer::new(value, cursor);
            to_end.move_line_end();
            assert_eq!(to_end.cursor(), end, "line end from {cursor} ({case})");
        }
    }

    /// a `\r\n` pair ends exactly one line, so the line after it starts past
    /// both characters rather than between them.
    #[test]
    fn line_movement_treats_a_crlf_pair_as_one_line_break() {
        // "one\r\ntwo\r\nthree": lines "one" 0..=3, "two" 5..=8, "three" 10..=15.
        let value = "one\r\ntwo\r\nthree";

        for (cursor, start, end, case) in [
            (6, 5, 8, "mid-line"),
            (5, 5, 8, "at the line's start"),
            (8, 5, 8, "at the line's end"),
            (3, 0, 3, "immediately before a break"),
            (10, 10, 15, "immediately after a break"),
            (1, 0, 3, "on the first line"),
            (13, 10, 15, "on the last line"),
        ] {
            let mut to_start = EditBuffer::new(value, cursor);
            to_start.move_line_start();
            assert_eq!(
                to_start.cursor(),
                start,
                "line start from {cursor} ({case})"
            );

            let mut to_end = EditBuffer::new(value, cursor);
            to_end.move_line_end();
            assert_eq!(to_end.cursor(), end, "line end from {cursor} ({case})");
        }
    }

    /// a cursor parked between a `\r` and its `\n` is on no line by the letter of the
    /// definition -- it sits inside the break. It is pinned to the line that *follows* the
    /// pair, which is the row the draft renderer already paints it on (column 0 of the
    /// following row), so neither Home nor End moves it across a rendered row boundary.
    #[test]
    fn line_movement_from_inside_a_crlf_pair_targets_the_following_line() {
        // Position 4 of "one\r\ntwo\r\nthree" sits between the first `\r` and its `\n`.
        let value = "one\r\ntwo\r\nthree";

        let mut to_start = EditBuffer::new(value, 4);
        to_start.move_line_start();
        assert_eq!(to_start.cursor(), 5, "line start left the following line");

        let mut to_end = EditBuffer::new(value, 4);
        to_end.move_line_end();
        assert_eq!(to_end.cursor(), 8, "line end crossed back over the pair");

        // The renderer agrees on which row that cursor is on, and line-start leaves the
        // painted position exactly where it already was.
        let painted = |cursor| {
            let (_, row, column) = escaped_draft_rows(&EditBuffer::new(value, cursor), 20, 5);
            (row, column)
        };
        assert_eq!(painted(4), (1, 0), "the pair's interior paints elsewhere");
        assert_eq!(
            painted(5),
            painted(4),
            "line start moved the painted cursor"
        );
    }

    /// word movement lands on the nearest boundary strictly in the asked direction.
    #[test]
    fn word_movement_lands_on_the_nearest_boundary_in_the_requested_direction() {
        // " alpha beta " boundaries: 0, 2, 7, 8, 12, 14.
        let value = "  alpha beta  ";

        let mut right = EditBuffer::new(value, 0);
        right.move_word_right();
        assert_eq!(right.cursor(), 2);
        right.move_word_right();
        assert_eq!(right.cursor(), 7);
        right.move_word_right();
        assert_eq!(right.cursor(), 8);

        let mut left = EditBuffer::new(value, 9);
        left.move_word_left();
        assert_eq!(left.cursor(), 8);
        left.move_word_left();
        assert_eq!(left.cursor(), 7);

        // Strictly in the requested direction: sitting on a boundary still moves off it.
        let mut on_boundary = EditBuffer::new(value, 7);
        on_boundary.move_word_right();
        assert_eq!(on_boundary.cursor(), 8);
    }

    /// word movement stops at the value bounds rather than wrapping.
    #[test]
    fn word_movement_stops_at_the_value_bounds() {
        let mut at_start = EditBuffer::new("alpha beta", 0);
        at_start.move_word_left();
        assert_eq!(at_start.cursor(), 0);

        let mut at_end = EditBuffer::new("alpha beta", 10);
        at_end.move_word_right();
        assert_eq!(at_end.cursor(), 10);
    }

    /// a pasted run goes in at the cursor, which ends immediately after it.
    #[test]
    fn inserting_a_text_run_places_the_cursor_immediately_after_it() {
        let mut buffer = EditBuffer::new("ab", 1);
        buffer.insert_text("你ξz");
        assert_eq!(buffer.value(), "a你ξzb");
        assert_eq!(buffer.cursor(), 4);
    }

    /// (buffer half): a line break splits the value at the cursor.
    #[test]
    fn inserting_a_line_break_splits_the_value_at_the_cursor() {
        let mut buffer = EditBuffer::new("abcd", 2);
        buffer.insert_char('\n');
        assert_eq!(buffer.value(), "ab\ncd");
        assert_eq!(buffer.cursor(), 3);
    }

    /// every operation at every cursor position of a mixed-width value moves whole
    /// characters only, never panics, and never leaves a partial character behind.
    #[test]
    fn every_operation_at_every_position_of_a_mixed_width_value_stays_whole() {
        let original: Vec<char> = MIXED.chars().collect();
        let count = original.len();

        for k in 0..=count {
            // Insertion of one wide character.
            let mut inserted = EditBuffer::new(MIXED, k);
            inserted.insert_char('世');
            let mut expected = original.clone();
            expected.insert(k, '世');
            assert_eq!(inserted.value(), expected.iter().collect::<String>());
            assert_eq!(inserted.cursor(), k + 1);

            // Insertion of a multi-character run.
            let mut pasted = EditBuffer::new(MIXED, k);
            pasted.insert_text("界ß");
            let mut expected_paste = original.clone();
            expected_paste.insert(k, 'ß');
            expected_paste.insert(k, '界');
            assert_eq!(pasted.value(), expected_paste.iter().collect::<String>());
            assert_eq!(pasted.cursor(), k + 2);

            // Backspace.
            let mut back = EditBuffer::new(MIXED, k);
            back.backspace();
            if k == 0 {
                assert_eq!(back.value(), MIXED);
                assert_eq!(back.cursor(), 0);
            } else {
                let mut expected_back = original.clone();
                expected_back.remove(k - 1);
                assert_eq!(back.value(), expected_back.iter().collect::<String>());
                assert_eq!(back.cursor(), k - 1);
            }

            // Forward delete.
            let mut forward = EditBuffer::new(MIXED, k);
            forward.delete_forward();
            if k == count {
                assert_eq!(forward.value(), MIXED);
                assert_eq!(forward.cursor(), count);
            } else {
                let mut expected_forward = original.clone();
                expected_forward.remove(k);
                assert_eq!(forward.value(), expected_forward.iter().collect::<String>());
                assert_eq!(forward.cursor(), k);
            }

            // Every movement leaves the value untouched and lands on the position the
            // pinned rules give, computed here from the same character vector: the current
            // line is the maximal break-free run around the cursor, and a word boundary is
            // the value's end or a whitespace/non-whitespace transition.

            // The runs, cut on the one definition of a break: a `\r\n` pair, a lone `\n`,
            // or a lone `\r`. Segmenting the whole value is independent of how the buffer
            // scans outward from one cursor.
            let mut runs: Vec<(usize, usize)> = Vec::new();
            let mut run_from = 0;
            let mut position = 0;
            while position < count {
                let break_width = match original[position] {
                    '\r' if original.get(position + 1) == Some(&'\n') => 2,
                    '\r' | '\n' => 1,
                    _ => {
                        position += 1;
                        continue;
                    }
                };
                runs.push((run_from, position));
                position += break_width;
                run_from = position;
            }
            runs.push((run_from, count));

            // A cursor inside a `\r\n` pair is on no run at all; it belongs to the run that
            // follows the pair, which is the row the draft renderer paints it on. So: the
            // first run that either contains the cursor or begins after it.
            let (line_start, line_end) = runs
                .iter()
                .find(|(start, end)| (k >= *start && k <= *end) || *start > k)
                .copied()
                .expect("every cursor position has a line");
            let boundaries: Vec<usize> = (0..=count)
                .filter(|position| {
                    *position == 0
                        || *position == count
                        || original[*position - 1].is_whitespace()
                            != original[*position].is_whitespace()
                })
                .collect();
            // Strictly in the asked direction, and a no-op when there is none that way.
            let word_left = boundaries
                .iter()
                .rev()
                .find(|position| **position < k)
                .copied()
                .unwrap_or(k);
            let word_right = boundaries
                .iter()
                .find(|position| **position > k)
                .copied()
                .unwrap_or(k);

            let movements: [(Movement, usize, &str); 6] = [
                (EditBuffer::move_left, k.saturating_sub(1), "move_left"),
                (EditBuffer::move_right, (k + 1).min(count), "move_right"),
                (EditBuffer::move_line_start, line_start, "move_line_start"),
                (EditBuffer::move_line_end, line_end, "move_line_end"),
                (EditBuffer::move_word_left, word_left, "move_word_left"),
                (EditBuffer::move_word_right, word_right, "move_word_right"),
            ];

            for (movement, expected_cursor, name) in movements {
                let mut moved = EditBuffer::new(MIXED, k);
                movement(&mut moved);
                assert_eq!(moved.value(), MIXED, "{name} altered the value at {k}");
                assert_eq!(
                    moved.cursor(),
                    expected_cursor,
                    "{name} from {k} missed its target"
                );
            }
        }
    }

    /// a value that fits is windowed whole.
    #[test]
    fn a_value_narrower_than_its_allocation_is_returned_whole() {
        let (window, column) = field_viewport("hello", 3, 20);
        assert_eq!(window, "hello");
        assert_eq!(column, 3);
    }

    /// a wider value yields a window that holds the cursor within the allocation.
    #[test]
    fn a_value_wider_than_its_allocation_keeps_the_cursor_inside_the_window() {
        let value = "the quick brown fox jumps over the lazy dog";
        let allocation = 10;

        for cursor in 0..=value.chars().count() {
            let (window, column) = field_viewport(value, cursor, allocation);
            assert!(
                width_of(&window) <= allocation,
                "window {window:?} overflowed at cursor {cursor}"
            );
            // The cursor cell itself must sit inside the allocation, whose columns are
            // `0..=allocation - 1`; `column == allocation` is the first cell past the edge.
            assert!(
                column < allocation,
                "cursor column {column} fell outside the {allocation}-column allocation at cursor {cursor}"
            );
            assert!(
                column <= width_of(&window),
                "cursor column {column} fell outside window {window:?}"
            );
            assert!(
                value.contains(window.as_str()),
                "window {window:?} is not a run of the value"
            );
        }
    }

    /// the cursor cell is reserved, so a cursor at the end of an overflowing value
    /// still reports a column inside the allocation rather than one cell past its edge.
    #[test]
    fn a_cursor_at_the_end_of_an_overflowing_value_stays_inside_the_allocation() {
        let (window, column) = field_viewport("abcdef", 6, 3);
        assert!(
            column < 3,
            "cursor column {column} fell outside a 3-column allocation, window {window:?}"
        );
        assert!(width_of(&window) <= 3, "window {window:?} overflowed");
    }

    /// minimal scroll -- a cursor at the start of a long value shows the value from
    /// its first character, with the rest of the window filled by trailing context.
    #[test]
    fn a_cursor_at_the_start_of_a_long_value_shows_the_value_from_its_beginning() {
        let value = "the quick brown fox jumps over the lazy dog";
        let (window, column) = field_viewport(value, 0, 10);
        assert!(
            value.starts_with(window.as_str()),
            "window {window:?} did not start at the value's first character"
        );
        assert_eq!(column, 0);
        assert_eq!(
            width_of(&window),
            10,
            "trailing context did not fill {window:?}"
        );
    }

    /// a cursor in the middle of a long value keeps context on both sides of it.
    #[test]
    fn a_cursor_mid_value_keeps_context_on_both_sides() {
        let value = "the quick brown fox jumps over the lazy dog";
        let (window, column) = field_viewport(value, 20, 10);
        assert!(column > 0, "no context before the cursor in {window:?}");
        assert!(
            width_of(&window) > column,
            "no context after the cursor in {window:?} (column {column})"
        );
    }

    /// the cursor-inside-the-allocation invariant holds for double-width content too.
    #[test]
    fn double_width_content_keeps_the_cursor_column_inside_the_allocation() {
        let value = "你好世界宇宙";
        for allocation in [1, 2, 3, 4, 5] {
            for cursor in 0..=value.chars().count() {
                let (window, column) = field_viewport(value, cursor, allocation);
                assert!(
                    width_of(&window) <= allocation,
                    "window {window:?} overflowed {allocation} columns at cursor {cursor}"
                );
                assert!(
                    column < allocation,
                    "cursor column {column} fell outside the {allocation}-column allocation at cursor {cursor}"
                );
                assert!(
                    value.contains(window.as_str()),
                    "window {window:?} is not a run of the value"
                );
            }
        }
    }

    /// a value that fills its allocation exactly still reserves the cursor cell, so
    /// the window scrolls by one character rather than being returned whole.
    #[test]
    fn a_value_that_exactly_fills_its_allocation_still_reserves_the_cursor_cell() {
        assert_eq!(field_viewport("abc", 3, 3), ("bc".to_string(), 2));
    }

    /// a variation-selector sequence is measured as a run, not as a sum of
    /// its characters' widths, so the window cannot overflow its allocation.
    #[test]
    fn a_variation_selector_sequence_does_not_overflow_its_allocation() {
        let value = format!("a{VARIATION_SELECTOR}b");
        let (window, column) = field_viewport(&value, 0, 2);
        assert!(
            width_of(&window) <= 2,
            "window {window:?} (width {}) overflowed a 2-column allocation",
            width_of(&window)
        );
        assert_eq!(column, 0);
    }

    /// a ZWJ family emoji paints two columns, not the six its characters sum
    /// to, so a cursor just after it reports the run's real width and nothing scrolls.
    #[test]
    fn a_zwj_sequence_before_the_cursor_is_measured_as_one_run() {
        let value = format!("{ZWJ_FAMILY}ab");
        let cursor = ZWJ_FAMILY.chars().count();
        let allocation = 6;

        let (window, column) = field_viewport(&value, cursor, allocation);
        assert_eq!(
            column,
            width_of(ZWJ_FAMILY),
            "column {column} disagrees with the measured width of the text before the cursor"
        );
        assert_eq!(
            window, value,
            "a value narrower than its allocation scrolled anyway"
        );
        assert!(
            width_of(&window) <= allocation,
            "window {window:?} overflowed {allocation} columns"
        );
    }

    /// a zero allocation paints nothing and parks the cursor at column zero.
    #[test]
    fn a_zero_allocation_returns_an_empty_window_at_column_zero() {
        assert_eq!(field_viewport("hello", 3, 0), (String::new(), 0));
    }

    /// double-width content under-fills its allocation rather than overflowing.
    #[test]
    fn a_window_over_double_width_content_under_fills_rather_than_overflowing() {
        // Four double-width glyphs are eight columns; five columns can hold only two.
        let (window, column) = field_viewport("你好世界", 0, 5);
        assert_eq!(window, "你好");
        assert_eq!(width_of(&window), 4);
        assert_eq!(column, 0);

        // Scrolled to the tail, the same under-fill rule holds.
        let (tail, tail_column) = field_viewport("你好世界", 4, 5);
        assert!(width_of(&tail) <= 5, "tail window overflowed: {tail:?}");
        assert!(tail_column <= width_of(&tail));
    }
}
