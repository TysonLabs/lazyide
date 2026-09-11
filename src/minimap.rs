//! Pure helpers for the editor minimap: a narrow column that condenses the
//! whole file into one bar per row, highlights the visible viewport, and maps
//! clicks back to source lines.

/// Columns the minimap occupies (excluding the one-column gap to its left).
pub(crate) const MINIMAP_WIDTH: u16 = 12;
/// Panes narrower than this hide the minimap so code keeps room.
pub(crate) const MINIMAP_MIN_PANE_WIDTH: u16 = 70;
/// A source line this long (in display columns) fills the bar.
const FULL_BAR_COLUMNS: usize = 80;

/// How many source lines each minimap row summarizes (at least one).
pub(crate) fn lines_per_row(total_lines: usize, height: usize) -> usize {
    if height == 0 {
        return 1;
    }
    total_lines.div_ceil(height).max(1)
}

/// Bar length for a line of `len` display columns within `width` columns.
pub(crate) fn bar_width(len: usize, width: usize) -> usize {
    if len == 0 || width == 0 {
        return 0;
    }
    (len * width).div_ceil(FULL_BAR_COLUMNS).clamp(1, width)
}

/// Display width of a source line with tabs expanded to four columns.
pub(crate) fn line_display_len(line: &str) -> usize {
    line.chars()
        .map(|c| {
            if c == '\t' {
                4
            } else {
                unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
            }
        })
        .sum()
}

/// Longest display length among `lines[start..end]`.
pub(crate) fn max_len_in(lines: &[String], start: usize, end: usize) -> usize {
    lines[start.min(lines.len())..end.min(lines.len())]
        .iter()
        .map(|l| line_display_len(l))
        .max()
        .unwrap_or(0)
}

/// Scroll offset that centers `target_visible_idx` in a viewport of
/// `viewport_h` rows over `total_visible` rows.
pub(crate) fn centered_scroll(
    target_visible_idx: usize,
    viewport_h: usize,
    total_visible: usize,
) -> usize {
    let max_scroll = total_visible.saturating_sub(viewport_h.max(1));
    target_visible_idx
        .saturating_sub(viewport_h / 2)
        .min(max_scroll)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_per_row_rounds_up_and_never_zero() {
        assert_eq!(lines_per_row(0, 40), 1);
        assert_eq!(lines_per_row(40, 40), 1);
        assert_eq!(lines_per_row(41, 40), 2);
        assert_eq!(lines_per_row(1000, 40), 25);
        assert_eq!(lines_per_row(10, 0), 1);
    }

    #[test]
    fn bar_width_scales_and_clamps() {
        assert_eq!(bar_width(0, 12), 0);
        assert_eq!(bar_width(1, 12), 1);
        assert_eq!(bar_width(40, 12), 6);
        assert_eq!(bar_width(80, 12), 12);
        assert_eq!(bar_width(500, 12), 12);
        assert_eq!(bar_width(10, 0), 0);
    }

    #[test]
    fn display_len_expands_tabs_and_wide_chars() {
        assert_eq!(line_display_len("\tab"), 6);
        assert_eq!(line_display_len("日本"), 4);
        let lines = vec!["a".to_string(), "abcdef".to_string(), "abc".to_string()];
        assert_eq!(max_len_in(&lines, 0, 2), 6);
        assert_eq!(max_len_in(&lines, 2, 99), 3);
        assert_eq!(max_len_in(&lines, 5, 9), 0);
    }

    #[test]
    fn centered_scroll_clamps_to_range() {
        assert_eq!(centered_scroll(0, 20, 100), 0);
        assert_eq!(centered_scroll(50, 20, 100), 40);
        assert_eq!(centered_scroll(99, 20, 100), 80);
        assert_eq!(centered_scroll(5, 20, 10), 0);
    }
}
