//! Structured status bar: mode chip and git branch on the left, breadcrumbs
//! (or a transient status message) in the center, cursor position,
//! diagnostics, and encoding on the right.

use std::path::Path;
use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::App;
use crate::keybinds::{KeyAction, KeyBindings};
use crate::theme::Theme;
use crate::types::Focus;
use crate::util::relative_path;

/// How long a status message stays in the bar before breadcrumbs return.
pub(crate) const STATUS_MESSAGE_TTL: Duration = Duration::from_secs(5);

pub(crate) fn render_status_bar(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let theme = app.active_theme();
    let inner_w = area.width.saturating_sub(2) as usize;

    // Left: mode chip + git branch + change summary
    let mut left: Vec<Span> = Vec::new();
    let (mode, mode_color) = match app.focus {
        Focus::Tree => (" FILES ", theme.accent_secondary),
        Focus::Editor => (" EDIT ", theme.accent),
    };
    left.push(Span::styled(
        mode,
        Style::default()
            .fg(theme.bg)
            .bg(mode_color)
            .add_modifier(Modifier::BOLD),
    ));
    if let Some(branch) = app.git_branch.as_deref().filter(|b| !b.is_empty()) {
        left.push(Span::styled(
            format!("  {branch}"),
            Style::default().fg(theme.fg),
        ));
        let s = &app.git_change_summary;
        if !s.is_clean() {
            left.push(Span::styled(
                format!(" +{}", s.insertions),
                Style::default().fg(theme.git_added),
            ));
            left.push(Span::styled(
                format!(" -{}", s.deletions),
                Style::default().fg(theme.git_deleted),
            ));
        }
    }

    // Right: cursor, diagnostics, encoding, wrap
    let mut right: Vec<Span> = Vec::new();
    if let Some(tab) = app.active_tab() {
        let (row, col) = tab.editor.cursor();
        right.push(Span::styled(
            format!("Ln {}, Col {}", row + 1, col + 1),
            Style::default().fg(theme.fg),
        ));
        let errors = tab
            .diagnostics
            .iter()
            .filter(|d| d.severity == "error")
            .count();
        let warnings = tab
            .diagnostics
            .iter()
            .filter(|d| d.severity == "warning")
            .count();
        if errors > 0 {
            right.push(Span::styled(
                format!("  ● {errors}"),
                Style::default().fg(theme.git_deleted),
            ));
        }
        if warnings > 0 {
            right.push(Span::styled(
                format!("  ▲ {warnings}"),
                Style::default().fg(theme.git_modified),
            ));
        }
        right.push(Span::styled("  UTF-8", Style::default().fg(theme.fg_muted)));
        if app.word_wrap {
            right.push(Span::styled("  Wrap", Style::default().fg(theme.fg_muted)));
        }
    }
    right.push(Span::raw(" "));

    // Center: whatever fits between the two sides
    let left_w: usize = left.iter().map(|s| s.content.as_ref().width()).sum();
    let right_w: usize = right.iter().map(|s| s.content.as_ref().width()).sum();
    let gap = 2usize;
    let center_w = inner_w.saturating_sub(left_w + right_w + 2 * gap);
    let (center_text, center_style) = center_content(app, theme);
    let center_text = fit_center(&center_text, center_w);
    let pad_total = center_w.saturating_sub(center_text.width());
    let pad_left = pad_total / 2;
    let pad_right = pad_total - pad_left;

    let mut spans = left;
    spans.push(Span::raw(" ".repeat(gap + pad_left)));
    spans.push(Span::styled(center_text, center_style));
    spans.push(Span::raw(" ".repeat(pad_right + gap)));
    spans.extend(right);

    let bar = Paragraph::new(Line::from(spans))
        .style(Style::default().fg(theme.fg).bg(theme.bg_alt))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme.border)),
        );
    frame.render_widget(bar, area);
}

fn center_content(app: &App, theme: &Theme) -> (String, Style) {
    let message_live = !app.status.is_empty()
        && app
            .status_set_at
            .is_some_and(|t| t.elapsed() < STATUS_MESSAGE_TTL);
    if message_live {
        return (
            app.status.clone(),
            Style::default().fg(theme.accent_secondary),
        );
    }
    if let Some(path) = app.open_path() {
        let mut crumbs = breadcrumbs(&app.root, path);
        if app.is_dirty() {
            crumbs.push_str(" ●");
        }
        return (crumbs, Style::default().fg(theme.fg));
    }
    (
        keybind_hints(&app.keybinds),
        Style::default().fg(theme.fg_muted),
    )
}

/// `src › app › input.rs` for a path under `root`.
pub(crate) fn breadcrumbs(root: &Path, path: &Path) -> String {
    relative_path(root, path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" › ")
}

/// Truncate from the left so the tail (usually the file name) survives.
pub(crate) fn fit_center(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width <= 1 {
        return String::new();
    }
    let budget = width - 1; // room for the ellipsis
    let mut kept: Vec<char> = Vec::new();
    let mut used = 0usize;
    for ch in text.chars().rev() {
        let cw = ch.width().unwrap_or(0);
        if used + cw > budget {
            break;
        }
        kept.push(ch);
        used += cw;
    }
    kept.reverse();
    let mut out = String::from("…");
    out.extend(kept);
    out
}

fn keybind_hints(kb: &KeyBindings) -> String {
    format!(
        "{} Cmd   {} Open   {} Help   {} Files   {} Save   {} Quit",
        kb.display_for(KeyAction::CommandPalette),
        kb.display_for(KeyAction::QuickOpen),
        kb.display_for(KeyAction::Help),
        kb.display_for(KeyAction::ToggleFiles),
        kb.display_for(KeyAction::Save),
        kb.display_for(KeyAction::Quit),
    )
}

#[cfg(test)]
mod tests {
    use super::{breadcrumbs, fit_center};
    use std::path::Path;

    #[test]
    fn breadcrumbs_join_components_relative_to_root() {
        let root = Path::new("/proj");
        assert_eq!(
            breadcrumbs(root, Path::new("/proj/src/app/input.rs")),
            "src › app › input.rs"
        );
        assert_eq!(breadcrumbs(root, Path::new("/proj/README.md")), "README.md");
        // Outside the root: fall back to the full path components
        assert_eq!(
            breadcrumbs(root, Path::new("/etc/hosts")),
            "/ › etc › hosts"
        );
    }

    #[test]
    fn fit_center_keeps_tail_and_prefixes_ellipsis() {
        assert_eq!(fit_center("abc", 5), "abc");
        assert_eq!(fit_center("src › app › input.rs", 10), "… input.rs");
        assert_eq!(fit_center("abcdef", 1), "");
        assert_eq!(fit_center("abcdef", 0), "");
        // wide chars count double
        assert_eq!(fit_center("ab日本", 5), "…日本");
        assert_eq!(fit_center("ab日本", 4), "…本");
    }
}
