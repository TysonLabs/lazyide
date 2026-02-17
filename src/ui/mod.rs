mod helpers;
mod overlays;

#[cfg(test)]
pub(crate) use helpers::centered_rect;

use std::collections::HashSet;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::keybinds::KeyAction;
use crate::lsp_client::LspDiagnostic;
use crate::syntax::{highlight_line, syntax_lang_for_path};
use crate::tab::{FoldRange, GitLineStatus};
use crate::types::Focus;
use crate::types::PendingAction;
use crate::util::{relative_path, row_has_selection};
use helpers::apply_indent_guides;
use overlays::*;

pub(crate) fn draw(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme().clone();
    let size = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(size);
    let (tree_area, editor_area) = if app.files_view_open {
        app.clamp_files_pane_width(vertical[1].width);
        let divider_w = 1;
        let main = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(app.files_pane_width),
                Constraint::Length(divider_w),
                Constraint::Min(App::MIN_EDITOR_PANE_WIDTH),
            ])
            .split(vertical[1]);
        app.divider_rect = main[1];
        (Some(main[0]), main[2])
    } else {
        app.divider_rect = Rect::default();
        (None, vertical[1])
    };
    app.tree_rect = tree_area.unwrap_or_default();
    app.editor_rect = editor_area;

    let file_label = match app.open_path() {
        Some(path) => {
            let mut s = relative_path(&app.root, path).display().to_string();
            if app.is_dirty() {
                s.push_str(" *");
            }
            s
        }
        None => "no file".to_string(),
    };
    let branch_label = app.git_branch.as_deref().unwrap_or("");
    let git_label = if branch_label.is_empty() {
        String::new()
    } else if app.git_change_summary.is_clean() {
        format!("   git: {}", branch_label)
    } else {
        format!(
            "   git: {}   Δ: {} files +{} -{}",
            branch_label,
            app.git_change_summary.files_changed,
            app.git_change_summary.insertions,
            app.git_change_summary.deletions
        )
    };
    let top_text = format!(
        "lazyide   root: {}   file: {}{}",
        app.root.display(),
        file_label,
        git_label
    );
    let top = Paragraph::new(top_text)
        .style(Style::default().fg(theme.fg).bg(theme.bg_alt))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border)),
        );
    frame.render_widget(top, vertical[0]);

    let left_border = if app.focus == Focus::Tree && app.files_view_open {
        theme.accent
    } else {
        theme.border
    };
    let right_border = if app.focus == Focus::Editor {
        theme.accent
    } else {
        theme.border
    };

    if let Some(tree_area) = tree_area {
        let tree_items: Vec<ListItem> = app
            .tree
            .iter()
            .map(|item| {
                let indent = "  ".repeat(item.depth);
                let icon = if item.is_dir {
                    if item.expanded { "▾ " } else { "▸ " }
                } else {
                    "· "
                };
                let style = if item.is_dir {
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    let fg = match app.git_file_statuses.get(&item.path) {
                        Some(crate::tab::GitFileStatus::Modified) => Color::Yellow,
                        Some(crate::tab::GitFileStatus::Added) => Color::Green,
                        Some(crate::tab::GitFileStatus::Untracked) => theme.fg_muted,
                        None => theme.fg,
                    };
                    Style::default().fg(fg)
                };
                ListItem::new(Line::from(Span::styled(
                    format!("{indent}{icon}{}", item.name),
                    style,
                )))
            })
            .collect();
        let mut tree_state = ListState::default();
        tree_state.select(Some(app.selected));
        let tree = List::new(tree_items)
            .highlight_style(
                Style::default()
                    .fg(theme.fg)
                    .bg(theme.selection)
                    .add_modifier(Modifier::BOLD),
            )
            .block(
                Block::default()
                    .title("[1]-Files")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(left_border))
                    .style(Style::default().bg(theme.bg_alt).fg(theme.fg)),
            );
        frame.render_stateful_widget(tree, tree_area, &mut tree_state);
        if app.files_view_open && app.divider_rect.width > 0 {
            let divider =
                Paragraph::new("│").style(Style::default().fg(theme.border).bg(theme.bg_alt));
            frame.render_widget(divider, app.divider_rect);
        }
    }

    // Build tab bar title
    let tab_title: Line = if app.tabs.is_empty() {
        Line::from("Working View")
    } else {
        let mut spans = Vec::new();
        app.tab_rects.clear();
        for (i, tab) in app.tabs.iter().enumerate() {
            let fname = tab
                .path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "untitled".to_string());
            let prefix = if tab.dirty { "*" } else { "" };
            let label = format!(" {prefix}{fname} [x] ");
            let style = if i == app.active_tab {
                let mut s = Style::default().fg(theme.fg).bg(theme.bg);
                if tab.is_preview {
                    s = s.add_modifier(Modifier::ITALIC);
                }
                s
            } else {
                let mut s = Style::default().fg(theme.fg_muted);
                if tab.is_preview {
                    s = s.add_modifier(Modifier::ITALIC);
                }
                s
            };
            if !spans.is_empty() {
                spans.push(Span::styled("│", Style::default().fg(theme.border)));
            }
            spans.push(Span::styled(label, style));
        }
        Line::from(spans)
    };
    let editor_block = Block::default()
        .title(tab_title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(right_border))
        .style(Style::default().bg(theme.bg_alt).fg(theme.fg));
    frame.render_widget(editor_block, editor_area);
    let inner = Rect::new(
        editor_area.x.saturating_add(1),
        editor_area.y.saturating_add(1),
        editor_area.width.saturating_sub(2),
        editor_area.height.saturating_sub(2),
    );

    // Compute tab_rects for click detection (position within the title bar)
    {
        app.tab_rects.clear();
        let mut x_offset = editor_area.x + 1; // +1 for border
        for (i, tab) in app.tabs.iter().enumerate() {
            let fname = tab
                .path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "untitled".to_string());
            let prefix = if tab.dirty { "*" } else { "" };
            let label_text = format!(" {prefix}{fname} [x] ");
            let label_len = label_text.width() as u16;
            if i > 0 {
                x_offset += 1; // separator
            }
            // Name rect (clickable to switch)
            let close_len = 4u16; // " [x]" + trailing space
            let name_rect = Rect::new(
                x_offset,
                editor_area.y,
                label_len.saturating_sub(close_len),
                1,
            );
            // Close rect
            let close_rect = Rect::new(
                x_offset + label_len.saturating_sub(close_len),
                editor_area.y,
                close_len,
                1,
            );
            app.tab_rects.push((name_rect, close_rect));
            x_offset += label_len;
        }
    }

    frame.render_widget(Clear, inner);
    let lang = syntax_lang_for_path(app.open_path().map(|p| p.as_path()));
    let visible_rows = inner.height as usize;
    if app
        .active_tab()
        .is_some_and(|t| t.visible_rows_map.is_empty())
    {
        app.rebuild_visible_rows();
    }
    let (
        start_row,
        lines_src,
        selection,
        cursor_row,
        cursor_col,
        diagnostics_owned,
        fold_ranges_owned,
        folded_starts_owned,
        visible_rows_map_owned,
        bracket_depths_owned,
        git_line_status_owned,
    ) = if let Some(tab) = app.active_tab() {
        let sr = tab
            .editor_scroll_row
            .min(tab.visible_rows_map.len().saturating_sub(1));
        // Only clone lines up to the highest visible row, not the entire buffer
        let max_row = tab.visible_rows_map.iter().copied().max().unwrap_or(0);
        let all_lines = tab.editor.lines();
        let lines: Vec<String> = all_lines[..all_lines.len().min(max_row + 1)].to_vec();
        (
            sr,
            lines,
            tab.editor.selection_range(),
            tab.editor.cursor().0,
            tab.editor.cursor().1,
            tab.diagnostics.clone(),
            tab.fold_ranges.clone(),
            tab.folded_starts.clone(),
            tab.visible_rows_map.clone(),
            tab.bracket_depths.clone(),
            tab.git_line_status.clone(),
        )
    } else {
        (
            0,
            vec![String::new()],
            None,
            0,
            0,
            Vec::new(),
            Vec::new(),
            HashSet::new(),
            vec![0usize],
            Vec::new(),
            Vec::new(),
        )
    };
    let diagnostics_ref = &diagnostics_owned as &[LspDiagnostic];
    let fold_ranges_ref = &fold_ranges_owned as &[FoldRange];
    let folded_starts_ref = &folded_starts_owned;
    let visible_rows_map_ref = &visible_rows_map_owned as &[usize];
    let inner_w = inner.width as usize;
    let blank_line = Line::from(Span::styled(
        " ".repeat(inner_w),
        Style::default().bg(theme.bg),
    ));
    // Precompute indent depths for visible rows (for indent guides)
    let indent_depths: Vec<usize> = {
        let total = lines_src.len();
        let mut depths = vec![0usize; total];
        // First pass: compute depth for non-blank lines
        for i in 0..total {
            let line = &lines_src[i];
            let expanded = line.replace('\t', "    ");
            let leading = expanded.len() - expanded.trim_start_matches(' ').len();
            if expanded.trim().is_empty() {
                depths[i] = usize::MAX; // sentinel for blank
            } else {
                depths[i] = leading / 4;
            }
        }
        // Second pass: blank lines get min(nearest non-blank above, nearest non-blank below)
        for i in 0..total {
            if depths[i] != usize::MAX {
                continue;
            }
            let above = (0..i)
                .rev()
                .find(|&j| depths[j] != usize::MAX)
                .map(|j| depths[j])
                .unwrap_or(0);
            let below = ((i + 1)..total)
                .find(|&j| depths[j] != usize::MAX)
                .map(|j| depths[j])
                .unwrap_or(0);
            depths[i] = above.min(below);
        }
        depths
    };
    let guide_style = Style::default().fg(theme.fg_muted);

    let mut lines_out: Vec<Line> = Vec::with_capacity(visible_rows);
    for visual_row in 0..visible_rows {
        let visible_idx = start_row + visual_row;
        let Some(&row) = visible_rows_map_ref.get(visible_idx) else {
            lines_out.push(blank_line.clone());
            continue;
        };
        if row >= lines_src.len() {
            lines_out.push(blank_line.clone());
            continue;
        }
        let mut spans = Vec::new();
        let line_num = format!("{:>5} ", row + 1);
        let line_num_style = if row == cursor_row {
            Style::default().fg(theme.accent)
        } else {
            Style::default().fg(theme.fg_muted)
        };
        spans.push(Span::styled(line_num, line_num_style));

        let fold_indicator =
            if let Some(fr) = fold_ranges_ref.iter().find(|fr| fr.start_line == row) {
                if folded_starts_ref.contains(&fr.start_line) {
                    "▸ "
                } else {
                    "▾ "
                }
            } else {
                "  "
            };
        spans.push(Span::styled(
            fold_indicator,
            Style::default()
                .fg(theme.fg_muted)
                .add_modifier(Modifier::BOLD),
        ));

        let diag_for_row = diagnostics_ref.iter().find(|d| d.line == row + 1);
        if let Some(diag) = diag_for_row {
            let color = match diag.severity.as_str() {
                "error" => Color::Red,
                "warning" => Color::Yellow,
                "info" => Color::Cyan,
                _ => Color::Blue,
            };
            spans.push(Span::styled("●", Style::default().fg(color)));
        } else {
            spans.push(Span::raw(" "));
        }
        let git_status = git_line_status_owned
            .get(row)
            .copied()
            .unwrap_or(GitLineStatus::None);
        match git_status {
            GitLineStatus::Added => {
                spans.push(Span::styled("+", Style::default().fg(Color::Green)));
            }
            GitLineStatus::Modified => {
                spans.push(Span::styled("~", Style::default().fg(Color::Yellow)));
            }
            GitLineStatus::Deleted => {
                spans.push(Span::styled("-", Style::default().fg(Color::Red)));
            }
            GitLineStatus::None => {
                spans.push(Span::raw(" "));
            }
        }
        spans.push(Span::raw(" "));
        let display_line = lines_src[row].replace('\t', "    ");
        let bracket_colors = [theme.bracket_1, theme.bracket_2, theme.bracket_3];
        let bd = bracket_depths_owned.get(row).copied().unwrap_or(0);
        let hl = highlight_line(&display_line, lang, &theme, bd, &bracket_colors);
        let guide_depth = indent_depths.get(row).copied().unwrap_or(0);
        let content_spans = apply_indent_guides(hl.spans, guide_depth, guide_style);
        spans.extend(content_spans);
        // Pad line to full width so stale characters from previous frame are overwritten
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        if used < inner_w {
            spans.push(Span::styled(
                " ".repeat(inner_w - used),
                Style::default().bg(theme.bg),
            ));
        }
        let hl = Line::from(spans);
        let hl = if diagnostics_ref
            .iter()
            .any(|d| d.line == row + 1 && d.severity == "error")
        {
            hl.patch_style(Style::default().add_modifier(Modifier::UNDERLINED))
        } else {
            hl
        };
        let hl = if row == cursor_row {
            hl.patch_style(Style::default().bg(theme.bg_alt))
        } else {
            hl
        };
        let hl = if selection.is_some()
            && row_has_selection(row, lines_src[row].chars().count(), selection)
        {
            hl.patch_style(Style::default().bg(theme.selection))
        } else {
            hl
        };
        if let Some(fr) = fold_ranges_ref
            .iter()
            .find(|fr| fr.start_line == row && folded_starts_ref.contains(&fr.start_line))
        {
            let folded = fr.end_line.saturating_sub(fr.start_line);
            let mut spans = hl.spans;
            spans.push(Span::styled(
                format!("  ... [{} lines]", folded),
                Style::default().fg(theme.fg_muted),
            ));
            lines_out.push(Line::from(spans));
        } else {
            lines_out.push(hl);
        }
    }
    let editor_text = Paragraph::new(lines_out).style(Style::default().bg(theme.bg).fg(theme.fg));
    frame.render_widget(editor_text, inner);
    if app.focus == Focus::Editor {
        let cursor_visible = app.visible_index_of_source_row(cursor_row);
        let cursor_y = cursor_visible.saturating_sub(start_row);
        if cursor_y < visible_rows {
            let max_x = inner
                .width
                .saturating_sub(1)
                .saturating_sub(App::EDITOR_GUTTER_WIDTH) as usize;
            let cursor_x = cursor_col.min(max_x);
            if let Some(ghost) = app.completion.ghost.as_ref() {
                if !ghost.is_empty()
                    && (cursor_x as u16 + App::EDITOR_GUTTER_WIDTH) < inner.width.saturating_sub(1)
                {
                    let ghost_area = Rect::new(
                        inner
                            .x
                            .saturating_add(App::EDITOR_GUTTER_WIDTH)
                            .saturating_add(cursor_x as u16),
                        inner.y.saturating_add(cursor_y as u16),
                        inner
                            .width
                            .saturating_sub(App::EDITOR_GUTTER_WIDTH)
                            .saturating_sub(cursor_x as u16),
                        1,
                    );
                    let ghost_span =
                        Span::styled(ghost.clone(), Style::default().fg(theme.fg_muted));
                    frame.render_widget(Paragraph::new(Line::from(vec![ghost_span])), ghost_area);
                }
            }
            frame.set_cursor_position((
                inner
                    .x
                    .saturating_add(App::EDITOR_GUTTER_WIDTH)
                    .saturating_add(cursor_x as u16),
                inner.y.saturating_add(cursor_y as u16),
            ));
        }
    }

    let kb = &app.keybinds;
    let status = Paragraph::new(format!(
        "{} Cmd   {} Open   {} Help   {} Files   {} Close   {} Save   {} Quit",
        kb.display_for(KeyAction::CommandPalette),
        kb.display_for(KeyAction::QuickOpen),
        kb.display_for(KeyAction::Help),
        kb.display_for(KeyAction::ToggleFiles),
        kb.display_for(KeyAction::CloseTab),
        kb.display_for(KeyAction::Save),
        kb.display_for(KeyAction::Quit),
    ))
    .style(Style::default().fg(theme.fg).bg(theme.bg_alt))
    .wrap(Wrap { trim: true })
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border)),
    );
    frame.render_widget(status, vertical[2]);

    if app.menu_open {
        render_menu(app, frame);
    }
    if app.file_picker_open {
        render_file_picker(app, frame);
    }
    if app.theme_browser_open {
        render_theme_browser(app, frame);
    }
    if app.search_results.open {
        render_search_results(app, frame);
    }
    if app.completion.open {
        render_completion_popup(app, frame);
    }
    if app.help_open {
        render_help(app, frame);
    }
    if app.keybind_editor.open {
        render_keybind_editor(app, frame);
    }
    if app.context_menu.open {
        render_context_menu(app, frame);
    }
    if app.editor_context_menu_open {
        render_editor_context_menu(app, frame);
    }
    if app.prompt.is_some() {
        render_prompt(app, frame);
    }
    if matches!(app.pending, PendingAction::ClosePrompt) {
        render_close_prompt(app, frame);
    }
    if app.active_tab().is_some_and(|t| t.conflict_prompt_open) {
        render_conflict_prompt(app, frame);
    }
    if app.active_tab().is_some_and(|t| t.recovery_prompt_open) {
        render_recovery_prompt(app, frame);
    }
}
