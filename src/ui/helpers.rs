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
