//! Mono markdown subset for task notes (view / peek).
//!
//! Markers are styled with bold / dim / underline only — never color. Edit mode
//! keeps the raw source; this module paints the reading surface.
//!
//! Supported inline: `**strong**`, `*em*` / `_em_`, `` `code` `` (dim, ticks kept).
//! Supported line starts: `#`…`######` headings (bold+underline body), `-` / `*` lists
//! (dim marker), fenced ` ``` ` blocks (dim fence, plain body). Task-list markers
//! like `- [ ]` stay literal text (structured steps own checklists).

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::present_line;
use super::render::{style_dim, style_heading, style_plain};

/// A line that opens or closes a fenced code block (` ``` ` after indent).
pub fn is_fence_line(text: &str) -> bool {
    text.trim_start().starts_with("```")
}

/// Paint one already-wrapped notes row, tracking fenced-code state.
///
/// Fence lines and body are dim. Body does not run inline md.
/// Other lines use [`paint_md_line`].
pub fn paint_notes_line(
    text: &str,
    width: usize,
    base: Style,
    in_fence: &mut bool,
) -> Line<'static> {
    if width == 0 {
        return Line::from("");
    }
    if is_fence_line(text) {
        *in_fence = !*in_fence;
        return bound_styled_line(
            vec![Span::styled(text.trim_end().to_string(), style_dim())],
            width,
        );
    }
    if *in_fence {
        return bound_styled_line(
            vec![Span::styled(text.trim_end().to_string(), style_dim())],
            width,
        );
    }
    paint_md_line(text, width, base)
}

/// Same styles as view, with dim on every span (peek).
pub fn dim_line(line: Line<'static>) -> Line<'static> {
    Line::from(
        line.spans
            .into_iter()
            .map(|span| Span::styled(span.content, span.style.add_modifier(Modifier::DIM)))
            .collect::<Vec<_>>(),
    )
}

/// Paint one already-wrapped notes row with mono markdown styling.
pub fn paint_md_line(text: &str, width: usize, base: Style) -> Line<'static> {
    if width == 0 {
        return Line::from("");
    }
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return Line::from(Span::styled(" ".repeat(width.min(1)), base));
    }

    let (prefix_spans, body, heading) = line_prefix(trimmed);
    let mut spans = prefix_spans;
    let body_base = if heading { style_heading() } else { base };
    spans.extend(inline_spans(body, body_base));
    bound_styled_line(spans, width)
}

/// Dim list / heading markers; return the remainder and whether it is a heading.
fn line_prefix(text: &str) -> (Vec<Span<'static>>, &str, bool) {
    let lead = text.len() - text.trim_start().len();
    let (indent, rest) = text.split_at(lead);
    let mut spans = Vec::new();
    if !indent.is_empty() {
        spans.push(Span::styled(indent.to_string(), style_plain()));
    }
    let bytes = rest.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && bytes[i] == b'#' && i < 6 {
        i += 1;
    }
    if i > 0 && i < bytes.len() && bytes[i] == b' ' {
        spans.push(Span::styled(rest[..i + 1].to_string(), style_dim()));
        return (spans, &rest[i + 1..], true);
    }
    if bytes.len() >= 2 && matches!(bytes[0], b'-' | b'*') && bytes[1] == b' ' {
        spans.push(Span::styled(rest[..2].to_string(), style_dim()));
        return (spans, &rest[2..], false);
    }
    (spans, rest, false)
}

#[derive(Clone, Copy)]
enum InlineKind {
    Plain,
    Strong,
    /// Emphasis opened by `*` or `_`; close only on the same character.
    Em(char),
    Code,
}

fn inline_spans(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    let mut kind = InlineKind::Plain;
    let mut buf = String::new();

    let flush = |buf: &mut String, kind: InlineKind, spans: &mut Vec<Span<'static>>| {
        if buf.is_empty() {
            return;
        }
        let style = match kind {
            InlineKind::Plain => base,
            InlineKind::Strong => base.add_modifier(Modifier::BOLD),
            InlineKind::Em(_) => base.add_modifier(Modifier::UNDERLINED),
            InlineKind::Code => style_dim(),
        };
        spans.push(Span::styled(std::mem::take(buf), style));
    };

    while i < chars.len() {
        match kind {
            InlineKind::Plain => {
                if chars[i] == '`' {
                    flush(&mut buf, kind, &mut spans);
                    kind = InlineKind::Code;
                    buf.push('`');
                    i += 1;
                } else if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
                    if rest_has_starstar(&chars, i + 2) {
                        flush(&mut buf, kind, &mut spans);
                        kind = InlineKind::Strong;
                        i += 2;
                    } else {
                        buf.push('*');
                        i += 1;
                    }
                } else if chars[i] == '*' && rest_has_char(&chars, i + 1, '*') {
                    flush(&mut buf, kind, &mut spans);
                    kind = InlineKind::Em('*');
                    i += 1;
                } else if chars[i] == '_' && can_open_underscore(&chars, i) {
                    flush(&mut buf, kind, &mut spans);
                    kind = InlineKind::Em('_');
                    i += 1;
                } else {
                    buf.push(chars[i]);
                    i += 1;
                }
            }
            InlineKind::Strong => {
                if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
                    flush(&mut buf, kind, &mut spans);
                    kind = InlineKind::Plain;
                    i += 2;
                } else {
                    buf.push(chars[i]);
                    i += 1;
                }
            }
            InlineKind::Em(delim) => {
                if chars[i] == delim && (delim != '_' || can_close_underscore(&chars, i)) {
                    flush(&mut buf, kind, &mut spans);
                    kind = InlineKind::Plain;
                    i += 1;
                } else {
                    buf.push(chars[i]);
                    i += 1;
                }
            }
            InlineKind::Code => {
                if chars[i] == '`' {
                    buf.push('`');
                    flush(&mut buf, kind, &mut spans);
                    kind = InlineKind::Plain;
                    i += 1;
                } else {
                    buf.push(chars[i]);
                    i += 1;
                }
            }
        }
    }
    flush(&mut buf, kind, &mut spans);
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base));
    }
    spans
}

fn rest_has_starstar(chars: &[char], from: usize) -> bool {
    chars[from..].windows(2).any(|w| w[0] == '*' && w[1] == '*')
}

fn rest_has_char(chars: &[char], from: usize, needle: char) -> bool {
    chars[from..].contains(&needle)
}

fn can_open_underscore(chars: &[char], i: usize) -> bool {
    let left_ok = i == 0 || !chars[i - 1].is_alphanumeric();
    left_ok && (i + 1..chars.len()).any(|j| chars[j] == '_' && can_close_underscore(chars, j))
}

fn can_close_underscore(chars: &[char], i: usize) -> bool {
    i + 1 >= chars.len() || !chars[i + 1].is_alphanumeric()
}

fn bound_styled_line(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let flat: String = spans.iter().map(|s| s.content.as_ref()).collect();
    let shown = present_line(&flat, width);
    if shown == flat {
        return Line::from(spans);
    }
    Line::from(Span::styled(shown, style_plain()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    fn has_mod(line: &Line<'_>, m: Modifier) -> bool {
        line.spans.iter().any(|s| s.style.add_modifier.contains(m))
    }

    #[test]
    fn strong_and_em_and_code() {
        let line = paint_md_line("a **b** *c* `d`", 40, style_plain());
        assert!(has_mod(&line, Modifier::BOLD));
        assert!(has_mod(&line, Modifier::UNDERLINED));
        let code = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "`d`")
            .expect("code span");
        assert!(code.style.add_modifier.contains(Modifier::DIM));
        assert!(!code.style.add_modifier.contains(Modifier::REVERSED));
        let flat: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(flat, "a b c `d`");
        let strong = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "b")
            .expect("strong span");
        assert!(strong.style.add_modifier.contains(Modifier::BOLD));
        assert!(!strong.style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn heading_and_list_markers_dim() {
        let h = paint_md_line("# Title", 40, style_plain());
        assert_eq!(h.spans[0].content.as_ref(), "# ");
        assert!(h.spans[0].style.add_modifier.contains(Modifier::DIM));
        let title = h
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "Title")
            .expect("heading body");
        assert!(title.style.add_modifier.contains(Modifier::BOLD));
        assert!(title.style.add_modifier.contains(Modifier::UNDERLINED));
        let indented = paint_md_line("  # Title", 40, style_plain());
        assert!(indented.spans.iter().any(|s| s.content.as_ref() == "# "));
        let list = paint_md_line("- item", 40, style_plain());
        assert_eq!(list.spans[0].content.as_ref(), "- ");
        assert!(list.spans[0].style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn task_list_stays_literal_text() {
        let line = paint_md_line("- [ ] not a step", 40, style_plain());
        let flat: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(flat.contains("[ ] not a step"));
    }

    #[test]
    fn fenced_block_dims_the_fence_and_leaves_body_unparsed() {
        let mut in_fence = false;
        let open = paint_notes_line("```rust", 40, style_plain(), &mut in_fence);
        assert!(in_fence);
        assert!(has_mod(&open, Modifier::DIM));
        let body = paint_notes_line("**not bold**", 40, style_plain(), &mut in_fence);
        let flat: String = body.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(flat, "**not bold**");
        assert!(!has_mod(&body, Modifier::BOLD));
        assert!(has_mod(&body, Modifier::DIM));
        let close = paint_notes_line("```", 40, style_plain(), &mut in_fence);
        assert!(!in_fence);
        assert!(has_mod(&close, Modifier::DIM));
        let after = paint_notes_line("**bold**", 40, style_plain(), &mut in_fence);
        assert!(has_mod(&after, Modifier::BOLD));
    }

    #[test]
    fn dim_line_keeps_heading_and_code_and_adds_dim() {
        let mut in_fence = false;
        let heading = dim_line(paint_notes_line(
            "# Title",
            40,
            style_plain(),
            &mut in_fence,
        ));
        let title = heading
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "Title")
            .expect("heading");
        assert!(title.style.add_modifier.contains(Modifier::BOLD));
        assert!(title.style.add_modifier.contains(Modifier::UNDERLINED));
        assert!(title.style.add_modifier.contains(Modifier::DIM));
        let code = dim_line(paint_md_line("`x`", 40, style_plain()));
        let span = code
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "`x`")
            .expect("code");
        assert!(!span.style.add_modifier.contains(Modifier::REVERSED));
        assert!(span.style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn unmatched_stars_and_word_underscores_stay_literal() {
        let glob = paint_md_line("*.rs", 40, style_plain());
        let flat: String = glob.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(flat, "*.rs");
        let ident = paint_md_line("test_foo", 40, style_plain());
        let flat: String = ident.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(flat, "test_foo");
    }

    #[test]
    fn em_keeps_matching_delimiter_across_inner_underscores() {
        for sample in ["*file_name*", "_file_name_", "*foo*", "_foo_"] {
            let line = paint_md_line(sample, 40, style_plain());
            let flat: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let expected = sample.trim_matches(|c| c == '*' || c == '_');
            assert_eq!(flat, expected, "{sample}");
            let body = line
                .spans
                .iter()
                .find(|s| s.content.as_ref() == expected)
                .unwrap_or_else(|| panic!("em body for {sample}"));
            assert!(
                body.style.add_modifier.contains(Modifier::UNDERLINED),
                "{sample}"
            );
        }
    }

    #[test]
    fn underscore_without_a_valid_closer_stays_literal() {
        let line = paint_md_line("_file_name and **bold** `code`", 60, style_plain());
        let flat: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(flat, "_file_name and bold `code`");
        assert!(!has_mod(&line, Modifier::UNDERLINED));
        let bold = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "bold")
            .expect("strong span");
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
        let code = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "`code`")
            .expect("code span");
        assert!(code.style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn heading_keeps_bold_when_the_body_is_emphasized() {
        let h = paint_md_line("# *Release*", 40, style_plain());
        let body = h
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "Release")
            .expect("heading em");
        assert!(body.style.add_modifier.contains(Modifier::BOLD));
        assert!(body.style.add_modifier.contains(Modifier::UNDERLINED));
    }
}
