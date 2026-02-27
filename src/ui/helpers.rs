use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders};

use crate::theme::Theme;

pub(crate) fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

pub(crate) fn help_keybind_line<'a>(
    entries: &[(&str, &str)],
    key_style: Style,
    desc_style: Style,
    sep_style: Style,
) -> Line<'a> {
    let mut spans = Vec::new();
    for (i, (key, desc)) in entries.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  |  ", sep_style));
        }
        spans.push(Span::styled(key.to_string(), key_style));
        spans.push(Span::styled(format!(" {desc}"), desc_style));
    }
    Line::from(spans)
}

pub(crate) fn list_item_style(selected: bool, theme: &Theme) -> Style {
    if selected {
        Style::default()
            .fg(theme.bg)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg)
    }
}

pub(crate) fn themed_block(theme: &Theme) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.bg_alt))
        .border_style(Style::default().fg(theme.accent))
}

/// Clip spans to a horizontal window: skip `skip` display columns, then collect up to `width`
/// display columns. Preserves per-char styles. Uses `UnicodeWidthChar` for display width.
pub(crate) fn clip_spans_by_columns(
    spans: Vec<Span<'static>>,
    skip: usize,
    width: usize,
) -> Vec<Span<'static>> {
    if skip == 0 && width == usize::MAX {
        return spans;
    }
    // Flatten into (char, style) pairs
    let mut chars: Vec<(char, Style)> = Vec::new();
    for span in &spans {
        let style = span.style;
        for ch in span.content.chars() {
            chars.push((ch, style));
        }
    }
    // Walk chars: skip `skip` display columns, collect `width` columns
    let mut col = 0usize;
    let mut start_idx = 0usize;
    // Skip phase
    for (i, &(ch, _)) in chars.iter().enumerate() {
        if col >= skip {
            start_idx = i;
            break;
        }
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        col += cw;
        start_idx = i + 1;
    }
    if start_idx >= chars.len() {
        return Vec::new();
    }
    // Collect phase
    let mut collected: Vec<(char, Style)> = Vec::new();
    let mut acc = 0usize;
    for &(ch, style) in &chars[start_idx..] {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if acc + cw > width {
            break;
        }
        collected.push((ch, style));
        acc += cw;
    }
    // Rebuild spans, merging consecutive chars with same style
    let mut result: Vec<Span<'static>> = Vec::new();
    if collected.is_empty() {
        return result;
    }
    let mut current_style = collected[0].1;
    let mut current_text = String::new();
    for (ch, style) in collected {
        if style == current_style {
            current_text.push(ch);
        } else {
            if !current_text.is_empty() {
                result.push(Span::styled(current_text, current_style));
                current_text = String::new();
            }
            current_style = style;
            current_text.push(ch);
        }
    }
    if !current_text.is_empty() {
        result.push(Span::styled(current_text, current_style));
    }
    result
}

/// Apply a style to a range of display columns within spans.
/// `sel_start` and `sel_end` are 0-based display column indices (inclusive start, exclusive end).
pub(crate) fn apply_selection_to_spans(
    spans: Vec<Span<'static>>,
    sel_start: usize,
    sel_end: usize,
    sel_style: Style,
) -> Vec<Span<'static>> {
    if sel_start >= sel_end {
        return spans;
    }
    let mut chars: Vec<(char, Style)> = Vec::new();
    for span in &spans {
        let style = span.style;
        for ch in span.content.chars() {
            chars.push((ch, style));
        }
    }
    let mut col = 0usize;
    for (ch, style) in &mut chars {
        let cw = unicode_width::UnicodeWidthChar::width(*ch).unwrap_or(0);
        if col >= sel_start && col < sel_end {
            *style = style.patch(sel_style);
        }
        col += cw;
    }
    // Rebuild spans, merging consecutive chars with same style
    let mut result: Vec<Span<'static>> = Vec::new();
    if chars.is_empty() {
        return result;
    }
    let mut current_style = chars[0].1;
    let mut current_text = String::new();
    for (ch, style) in chars {
        if style == current_style {
            current_text.push(ch);
        } else {
            if !current_text.is_empty() {
                result.push(Span::styled(current_text, current_style));
                current_text = String::new();
            }
            current_style = style;
            current_text.push(ch);
        }
    }
    if !current_text.is_empty() {
        result.push(Span::styled(current_text, current_style));
    }
    result
}

/// Replace spaces at indent guide columns (multiples of 4) with `│` within leading whitespace.
/// `guide_depth` is the number of indent levels to draw guides for.
pub(crate) fn apply_indent_guides(
    spans: Vec<Span<'static>>,
    guide_depth: usize,
    guide_style: Style,
) -> Vec<Span<'static>> {
    if guide_depth == 0 {
        return spans;
    }
    let max_col = guide_depth * 4;
    // Flatten spans into (char, style) pairs, then rebuild
    let mut chars: Vec<(char, Style)> = Vec::new();
    for span in &spans {
        let style = span.style;
        for ch in span.content.chars() {
            chars.push((ch, style));
        }
    }
    if chars.is_empty() {
        return spans;
    }
    // Find end of leading whitespace
    let ws_end = chars
        .iter()
        .position(|(ch, _)| *ch != ' ')
        .unwrap_or(chars.len());
    let limit = ws_end.min(max_col);
    // Replace spaces at guide columns (0, 4, 8, ...) with │
    for col in (0..limit).step_by(4) {
        if col < chars.len() && chars[col].0 == ' ' {
            chars[col] = ('│', guide_style);
        }
    }
    // Rebuild spans from chars, merging consecutive chars with same style
    let mut result: Vec<Span<'static>> = Vec::new();
    if chars.is_empty() {
        return result;
    }
    let mut current_style = chars[0].1;
    let mut current_text = String::new();
    for (ch, style) in chars {
        if style == current_style {
            current_text.push(ch);
        } else {
            if !current_text.is_empty() {
                result.push(Span::styled(current_text, current_style));
                current_text = String::new();
            }
            current_style = style;
            current_text.push(ch);
        }
    }
    if !current_text.is_empty() {
        result.push(Span::styled(current_text, current_style));
    }
    result
}

#[cfg(test)]
mod indent_guide_tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn test_no_guides_at_zero_depth() {
        let spans = vec![Span::raw("    hello")];
        let result = apply_indent_guides(spans.clone(), 0, Style::default());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content.as_ref(), "    hello");
    }

    #[test]
    fn test_guides_at_depth_one() {
        let guide_style = Style::default().fg(Color::Gray);
        let spans = vec![Span::raw("    code")];
        let result = apply_indent_guides(spans, 1, guide_style);
        // First char should be │
        let full: String = result.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(full, "│   code");
    }

    #[test]
    fn test_guides_at_depth_two() {
        let guide_style = Style::default().fg(Color::Gray);
        let spans = vec![Span::raw("        code")];
        let result = apply_indent_guides(spans, 2, guide_style);
        let full: String = result.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(full, "│   │   code");
    }

    #[test]
    fn test_no_guides_on_non_indented() {
        let guide_style = Style::default().fg(Color::Gray);
        let spans = vec![Span::raw("hello world")];
        let result = apply_indent_guides(spans, 3, guide_style);
        let full: String = result.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(full, "hello world");
    }

    #[test]
    fn test_empty_spans() {
        let result = apply_indent_guides(vec![], 2, Style::default());
        assert!(result.is_empty());
    }

    #[test]
    fn test_blank_line_with_guides() {
        let guide_style = Style::default().fg(Color::Gray);
        let spans = vec![Span::raw("        ")];
        let result = apply_indent_guides(spans, 2, guide_style);
        let full: String = result.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(full, "│   │   ");
    }
}

#[cfg(test)]
mod selection_span_tests {
    use super::*;
    use ratatui::style::Color;

    fn collect_text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn sel_style() -> Style {
        Style::default().bg(Color::Yellow)
    }

    #[test]
    fn test_empty_range_returns_unchanged() {
        let spans = vec![Span::raw("hello")];
        let result = apply_selection_to_spans(spans.clone(), 3, 3, sel_style());
        assert_eq!(result.len(), 1);
        assert_eq!(collect_text(&result), "hello");
        assert_eq!(result[0].style, Style::default());
    }

    #[test]
    fn test_inverted_range_returns_unchanged() {
        let spans = vec![Span::raw("hello")];
        let result = apply_selection_to_spans(spans, 4, 2, sel_style());
        assert_eq!(collect_text(&result), "hello");
    }

    #[test]
    fn test_select_entire_span() {
        let spans = vec![Span::raw("hello")];
        let result = apply_selection_to_spans(spans, 0, 5, sel_style());
        assert_eq!(collect_text(&result), "hello");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].style.bg, Some(Color::Yellow));
    }

    #[test]
    fn test_select_middle_of_span() {
        let spans = vec![Span::raw("hello world")];
        let result = apply_selection_to_spans(spans, 2, 7, sel_style());
        assert_eq!(collect_text(&result), "hello world");
        // Should split into 3 spans: "he" (plain), "llo w" (selected), "orld" (plain)
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].content.as_ref(), "he");
        assert_eq!(result[0].style.bg, None);
        assert_eq!(result[1].content.as_ref(), "llo w");
        assert_eq!(result[1].style.bg, Some(Color::Yellow));
        assert_eq!(result[2].content.as_ref(), "orld");
        assert_eq!(result[2].style.bg, None);
    }

    #[test]
    fn test_select_across_multiple_spans() {
        let kw = Style::default().fg(Color::Blue);
        let plain = Style::default();
        let spans = vec![
            Span::styled("fn ", kw),
            Span::styled("main()", plain),
        ];
        // Select "n main" (columns 1..7)
        let result = apply_selection_to_spans(spans, 1, 7, sel_style());
        assert_eq!(collect_text(&result), "fn main()");
        // "f" blue, "n " blue+sel, "main" plain+sel, "()" plain
        let selected_text: String = result
            .iter()
            .filter(|s| s.style.bg == Some(Color::Yellow))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(selected_text, "n main");
    }

    #[test]
    fn test_select_from_start() {
        let spans = vec![Span::raw("hello")];
        let result = apply_selection_to_spans(spans, 0, 3, sel_style());
        assert_eq!(collect_text(&result), "hello");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content.as_ref(), "hel");
        assert_eq!(result[0].style.bg, Some(Color::Yellow));
        assert_eq!(result[1].content.as_ref(), "lo");
        assert_eq!(result[1].style.bg, None);
    }

    #[test]
    fn test_select_to_end() {
        let spans = vec![Span::raw("hello")];
        let result = apply_selection_to_spans(spans, 3, 100, sel_style());
        assert_eq!(collect_text(&result), "hello");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content.as_ref(), "hel");
        assert_eq!(result[0].style.bg, None);
        assert_eq!(result[1].content.as_ref(), "lo");
        assert_eq!(result[1].style.bg, Some(Color::Yellow));
    }

    #[test]
    fn test_preserves_existing_styles() {
        let kw = Style::default().fg(Color::Blue);
        let spans = vec![Span::styled("keyword", kw)];
        let result = apply_selection_to_spans(spans, 0, 7, sel_style());
        assert_eq!(collect_text(&result), "keyword");
        // Should have both fg and bg
        assert_eq!(result[0].style.fg, Some(Color::Blue));
        assert_eq!(result[0].style.bg, Some(Color::Yellow));
    }

    #[test]
    fn test_empty_spans() {
        let result = apply_selection_to_spans(vec![], 0, 5, sel_style());
        assert!(result.is_empty());
    }
}
