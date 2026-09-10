mod helpers;
mod overlays;
mod status_bar;

#[cfg(test)]
pub(crate) use helpers::centered_rect;

use std::collections::HashSet;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Padding, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::lsp_client::LspDiagnostic;
use crate::syntax::{highlight_line, syntax_lang_for_path};
use crate::tab::{FoldRange, GitLineStatus};
use crate::types::Focus;
use crate::types::PendingAction;
use crate::util::segment_has_selection;
use helpers::{apply_indent_guides, apply_selection_to_spans, clip_spans_by_columns};
use overlays::*;

fn slice_chars(s: &str, start: usize, end: usize) -> String {
    let count = end.saturating_sub(start);
    s.chars().skip(start).take(count).collect()
}

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
        let main = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(app.files_pane_width),
                Constraint::Min(App::MIN_EDITOR_PANE_WIDTH),
            ])
            .split(vertical[1]);
        // The divider is the editor's left border column, shared with the tree.
        app.divider_rect = Rect::new(main[1].x, main[1].y, 1, main[1].height);
        (Some(main[0]), main[1])
    } else {
        app.divider_rect = Rect::default();
        (None, vertical[1])
    };
    app.tree_rect = tree_area.unwrap_or_default();
    app.editor_rect = editor_area;

    let top_text = format!("lazyide   root: {}", app.root.display());
    let top = Paragraph::new(top_text)
        .style(Style::default().fg(theme.fg).bg(theme.bg_alt))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
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
                        Some(crate::tab::GitFileStatus::Modified) => theme.git_modified,
                        Some(crate::tab::GitFileStatus::Added) => theme.git_added,
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
        app.tree_state.select(Some(app.selected));
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
                    .borders(Borders::TOP | Borders::LEFT | Borders::BOTTOM)
                    .border_type(BorderType::Rounded)
                    .padding(Padding::horizontal(1))
                    .border_style(Style::default().fg(left_border))
                    .style(Style::default().bg(theme.bg_alt).fg(theme.fg)),
            );
        frame.render_stateful_widget(tree, tree_area, &mut app.tree_state);
        // Render [+][-] buttons right-aligned in the title bar row
        {
            let btn_width = 6u16; // "[+][-]"
            // Place buttons inside the right border: 1 char from right edge
            let btn_x = tree_area
                .x
                .saturating_add(tree_area.width)
                .saturating_sub(btn_width + 1);
            let btn_y = tree_area.y;
            app.tree_expand_btn_rect = Rect::new(btn_x, btn_y, 3, 1);
            app.tree_collapse_btn_rect = Rect::new(btn_x + 3, btn_y, 3, 1);
            let btns = Paragraph::new(Line::from(vec![
                Span::styled("[+]", Style::default().fg(theme.accent)),
                Span::styled("[-]", Style::default().fg(theme.accent)),
            ]));
            frame.render_widget(btns, Rect::new(btn_x, btn_y, btn_width, 1));
        }
    }

    // Tab bar gets its own row above the editor block.
    let editor_column = editor_area;
    let tab_row = Rect::new(editor_column.x, editor_column.y, editor_column.width, 1);
    let editor_area = Rect::new(
        editor_column.x,
        editor_column.y.saturating_add(1),
        editor_column.width,
        editor_column.height.saturating_sub(1),
    );
    app.tab_bar_rect = tab_row;
    app.editor_rect = editor_area;
    render_tab_bar(app, frame, tab_row, &theme);

    let editor_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(right_border))
        .style(Style::default().bg(theme.bg_alt).fg(theme.fg));
    frame.render_widget(editor_block, editor_area);
    if app.files_view_open && app.divider_rect.width > 0 {
        // Redraw the shared border column as a proper split: the tree's corner
        // beside the tab row, a junction where the editor's top border meets it,
        // and a junction at the bottom. Colored for whichever pane has focus.
        let color = if app.focus == Focus::Tree {
            theme.accent
        } else {
            right_border
        };
        let d = app.divider_rect;
        let style = Style::default().fg(color).bg(theme.bg_alt);
        let buf = frame.buffer_mut();
        for y in d.y..d.bottom() {
            let sym = if y == d.y {
                "╮"
            } else if y == d.y.saturating_add(1) {
                "├"
            } else if y + 1 == d.bottom() {
                "┴"
            } else {
                "│"
            };
            buf[(d.x, y)].set_symbol(sym).set_style(style);
        }
    }
    let inner = app.editor_inner_rect();

    frame.render_widget(Clear, inner);
    let wrap_width = inner.width.saturating_sub(App::EDITOR_GUTTER_WIDTH) as usize;
    if app.wrap_width_cache != wrap_width {
        app.wrap_width_cache = wrap_width;
        if app.word_wrap {
            // Debounce: schedule a rebuild for all tabs after resizing settles.
            // Rebuild the active tab immediately so the current view stays correct.
            app.wrap_rebuild_deadline =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(80));
            app.rebuild_visible_rows();
        }
    }
    let lang = syntax_lang_for_path(app.open_path().map(|p| p.as_path()));
    let visible_rows = inner.height as usize;
    if app
        .active_tab()
        .is_some_and(|t| t.visible_rows_map.is_empty())
    {
        app.rebuild_visible_rows();
    }
    // Extract scalar values; all tab data is referenced directly via the tab index
    // to avoid expensive per-frame clones.
    let tab_idx = app.active_tab;
    let has_tab = tab_idx < app.tabs.len();
    let (start_row, selection, cursor_row, cursor_col, scroll_col) = if has_tab {
        let tab = &app.tabs[tab_idx];
        let sr = tab
            .editor_scroll_row
            .min(tab.visible_rows_map.len().saturating_sub(1));
        (
            sr,
            tab.editor.selection_range(),
            tab.editor.cursor().0,
            tab.editor.cursor().1,
            tab.editor_scroll_col,
        )
    } else {
        (0, None, 0, 0, 0)
    };
    // Provide empty fallbacks for the no-tab case
    let empty_lines: Vec<String> = vec![String::new()];
    let empty_diagnostics: Vec<LspDiagnostic> = Vec::new();
    let empty_fold_ranges: Vec<FoldRange> = Vec::new();
    let empty_folded_starts: HashSet<usize> = HashSet::new();
    let empty_visible_rows: Vec<usize> = vec![0usize];
    let empty_visible_row_starts: Vec<usize> = vec![0usize];
    let empty_visible_row_ends: Vec<usize> = vec![0usize];
    let empty_bracket_depths: Vec<u16> = Vec::new();
    let empty_git_line_status: Vec<GitLineStatus> = Vec::new();
    let lines_ref: &[String] = if has_tab {
        app.tabs[tab_idx].editor.lines()
    } else {
        &empty_lines
    };
    let diagnostics_ref: &[LspDiagnostic] = if has_tab {
        &app.tabs[tab_idx].diagnostics
    } else {
        &empty_diagnostics
    };
    let fold_ranges_ref: &[FoldRange] = if has_tab {
        &app.tabs[tab_idx].fold_ranges
    } else {
        &empty_fold_ranges
    };
    let folded_starts_ref: &HashSet<usize> = if has_tab {
        &app.tabs[tab_idx].folded_starts
    } else {
        &empty_folded_starts
    };
    let visible_rows_map_ref: &[usize] = if has_tab {
        &app.tabs[tab_idx].visible_rows_map
    } else {
        &empty_visible_rows
    };
    let visible_row_starts_ref: &[usize] = if has_tab {
        &app.tabs[tab_idx].visible_row_starts
    } else {
        &empty_visible_row_starts
    };
    let visible_row_ends_ref: &[usize] = if has_tab {
        &app.tabs[tab_idx].visible_row_ends
    } else {
        &empty_visible_row_ends
    };
    let bracket_depths_ref: &[u16] = if has_tab {
        &app.tabs[tab_idx].bracket_depths
    } else {
        &empty_bracket_depths
    };
    let git_line_status_ref: &[GitLineStatus] = if has_tab {
        &app.tabs[tab_idx].git_line_status
    } else {
        &empty_git_line_status
    };
    let inner_w = inner.width as usize;
    let blank_line = Line::from(Span::styled(
        " ".repeat(inner_w),
        Style::default().bg(theme.bg),
    ));
    // Precompute indent depths for visible rows (for indent guides)
    let indent_depths: Vec<usize> = {
        let total = lines_ref.len();
        let mut depths = vec![0usize; total];
        let mut is_blank = vec![false; total];
        // First pass: compute depth for non-blank lines, mark blanks
        for i in 0..total {
            let line = &lines_ref[i];
            let expanded = line.replace('\t', "    ");
            let leading = expanded.len() - expanded.trim_start_matches(' ').len();
            if expanded.trim().is_empty() {
                is_blank[i] = true;
                depths[i] = 0;
            } else {
                depths[i] = leading / 4;
            }
        }
        // O(n) two-pass for blank lines: propagate nearest non-blank above/below
        let mut above_depth = vec![0usize; total];
        let mut last = 0usize;
        for i in 0..total {
            if !is_blank[i] {
                last = depths[i];
            }
            above_depth[i] = last;
        }
        let mut below_depth = vec![0usize; total];
        last = 0;
        for i in (0..total).rev() {
            if !is_blank[i] {
                last = depths[i];
            }
            below_depth[i] = last;
        }
        for i in 0..total {
            if is_blank[i] {
                depths[i] = above_depth[i].min(below_depth[i]);
            }
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
        let seg_start = visible_row_starts_ref
            .get(visible_idx)
            .copied()
            .unwrap_or(0);
        let seg_end = visible_row_ends_ref
            .get(visible_idx)
            .copied()
            .unwrap_or(seg_start);
        let is_first_segment = seg_start == 0;
        if row >= lines_ref.len() {
            lines_out.push(blank_line.clone());
            continue;
        }
        let mut spans = Vec::new();
        let line_num = if is_first_segment {
            format!("{:>5}", row + 1)
        } else {
            "     ".to_string()
        };
        let line_num_style = if row == cursor_row {
            Style::default().fg(theme.accent)
        } else {
            Style::default().fg(theme.fg_muted)
        };
        spans.push(Span::styled(line_num, line_num_style));

        let fold_indicator = if is_first_segment {
            if let Some(fr) = fold_ranges_ref.iter().find(|fr| fr.start_line == row) {
                if folded_starts_ref.contains(&fr.start_line) {
                    "▸ "
                } else {
                    "▾ "
                }
            } else {
                "  "
            }
        } else {
            "↪ "
        };
        spans.push(Span::styled(
            fold_indicator,
            Style::default()
                .fg(theme.fg_muted)
                .add_modifier(Modifier::BOLD),
        ));

        let diag_for_row = diagnostics_ref.iter().find(|d| d.line == row + 1);
        if is_first_segment {
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
        } else {
            spans.push(Span::raw(" "));
        }
        let git_status = if is_first_segment {
            git_line_status_ref
                .get(row)
                .copied()
                .unwrap_or(GitLineStatus::None)
        } else {
            GitLineStatus::None
        };
        match git_status {
            GitLineStatus::Added => {
                spans.push(Span::styled("+", Style::default().fg(theme.git_added)));
            }
            GitLineStatus::Modified => {
                spans.push(Span::styled("~", Style::default().fg(theme.git_modified)));
            }
            GitLineStatus::Deleted => {
                spans.push(Span::styled("-", Style::default().fg(theme.git_deleted)));
            }
            GitLineStatus::None => {
                spans.push(Span::raw(" "));
            }
        }
        // Gutter separator, then one space before the code.
        spans.push(Span::styled("│", Style::default().fg(theme.border)));
        spans.push(Span::raw(" "));
        let segment_text = slice_chars(&lines_ref[row], seg_start, seg_end).replace('\t', "    ");
        let bracket_colors = [theme.bracket_1, theme.bracket_2, theme.bracket_3];
        let bd = bracket_depths_ref.get(row).copied().unwrap_or(0);
        let hl = highlight_line(&segment_text, lang, &theme, bd, &bracket_colors);
        let guide_depth = indent_depths.get(row).copied().unwrap_or(0);
        let content_spans = if is_first_segment {
            apply_indent_guides(hl.spans, guide_depth, guide_style)
        } else {
            hl.spans
        };
        let content_width = inner_w.saturating_sub(App::EDITOR_GUTTER_WIDTH as usize);
        let content_spans = if !app.word_wrap && scroll_col > 0 {
            clip_spans_by_columns(content_spans, scroll_col, content_width)
        } else if !app.word_wrap {
            clip_spans_by_columns(content_spans, 0, content_width)
        } else {
            content_spans
        };
        // Apply character-level selection highlighting to content spans
        let (content_spans, sel_extends_to_eol) =
            if segment_has_selection(row, seg_start, seg_end, selection) {
                let Some(((mut sr, mut sc), (mut er, mut ec))) = selection else {
                    unreachable!()
                };
                if (sr, sc) > (er, ec) {
                    std::mem::swap(&mut sr, &mut er);
                    std::mem::swap(&mut sc, &mut ec);
                }
                let sel_start_col = if row == sr { sc } else { 0 };
                let sel_end_col = if row == er { ec } else { usize::MAX };
                // Clamp to segment boundaries
                let clamped_start = sel_start_col.max(seg_start).min(seg_end);
                let clamped_end = sel_end_col.min(seg_end).max(seg_start);
                // Convert original char positions to display columns (tab=4 cols)
                let orig_chars: Vec<char> = lines_ref[row]
                    .chars()
                    .skip(seg_start)
                    .take(seg_end - seg_start)
                    .collect();
                let char_to_display = |n: usize| -> usize {
                    orig_chars.iter().take(n).fold(0, |acc, ch| {
                        acc + if *ch == '\t' {
                            4
                        } else {
                            unicode_width::UnicodeWidthChar::width(*ch).unwrap_or(0)
                        }
                    })
                };
                let display_start = char_to_display(clamped_start - seg_start);
                let display_end = if sel_end_col >= seg_end {
                    char_to_display(orig_chars.len())
                } else {
                    char_to_display(clamped_end - seg_start)
                };
                let effective_scroll = if !app.word_wrap { scroll_col } else { 0 };
                let clipped_start = display_start.saturating_sub(effective_scroll);
                let clipped_end = display_end.saturating_sub(effective_scroll);
                let sel_style = Style::default().bg(theme.selection);
                (
                    apply_selection_to_spans(content_spans, clipped_start, clipped_end, sel_style),
                    sel_end_col >= seg_end,
                )
            } else {
                (content_spans, false)
            };
        spans.extend(content_spans);
        // Pad line to full width so stale characters from previous frame are overwritten
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        if used < inner_w {
            let pad_bg = if sel_extends_to_eol {
                theme.selection
            } else {
                theme.bg
            };
            spans.push(Span::styled(
                " ".repeat(inner_w - used),
                Style::default().bg(pad_bg),
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
        let line_len_chars = lines_ref[row].chars().count();
        let cursor_on_segment = row == cursor_row
            && cursor_col >= seg_start
            && (cursor_col < seg_end || (cursor_col == seg_end && seg_end == line_len_chars));
        let has_selection = segment_has_selection(row, seg_start, seg_end, selection);
        let hl = if cursor_on_segment && !has_selection {
            hl.patch_style(Style::default().bg(theme.bg_alt))
        } else {
            hl
        };
        if is_first_segment
            && let Some(fr) = fold_ranges_ref
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
    if app.focus == Focus::Editor && has_tab {
        let cursor_visible = app.visible_index_of_source_position(cursor_row, cursor_col);
        let cursor_y = cursor_visible.saturating_sub(start_row);
        if cursor_y < visible_rows {
            let seg_start = visible_row_starts_ref
                .get(cursor_visible)
                .copied()
                .unwrap_or(0);
            let seg_end = visible_row_ends_ref
                .get(cursor_visible)
                .copied()
                .unwrap_or(seg_start);
            let max_x = inner
                .width
                .saturating_sub(1)
                .saturating_sub(App::EDITOR_GUTTER_WIDTH) as usize;
            let logical_x = cursor_col
                .clamp(seg_start, seg_end)
                .saturating_sub(seg_start);
            // When not wrapping, compute display-width offset for cursor and
            // subtract the horizontal scroll so it renders at the right screen column.
            let logical_x = if !app.word_wrap {
                // Compute display-width of chars before cursor on this line
                let line_chars: Vec<char> = lines_ref
                    .get(cursor_row)
                    .map(|l| l.replace('\t', "    ").chars().collect())
                    .unwrap_or_default();
                let mut dw = 0usize;
                for &ch in &line_chars[..cursor_col.min(line_chars.len())] {
                    dw += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                }
                dw.saturating_sub(scroll_col)
            } else {
                logical_x
            };
            let cursor_x = logical_x.min(max_x);
            // If cursor would be off-screen horizontally (scrolled past), skip rendering
            if !app.word_wrap && cursor_x > max_x {
                // cursor off-screen — don't render
            } else if let Some(ghost) = app.completion.ghost.as_ref()
                && !ghost.is_empty()
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
                let ghost_span = Span::styled(ghost.clone(), Style::default().fg(theme.fg_muted));
                frame.render_widget(Paragraph::new(Line::from(vec![ghost_span])), ghost_area);
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

    status_bar::render_status_bar(&*app, frame, vertical[2]);

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
    if matches!(app.pending, PendingAction::Delete(_)) {
        render_delete_prompt(app, frame);
    }
    if app.active_tab().is_some_and(|t| t.conflict_prompt_open) {
        render_conflict_prompt(app, frame);
    }
    if app.active_tab().is_some_and(|t| t.recovery_prompt_open) {
        render_recovery_prompt(app, frame);
    }
}

/// One-row tab strip. Column 0 is left blank so labels line up with the
/// editor's left border (or the shared divider) below it.
fn render_tab_bar(app: &mut App, frame: &mut Frame<'_>, area: Rect, theme: &crate::theme::Theme) {
    app.tab_rects.clear();
    let base = Style::default().bg(theme.bg_alt);
    let mut spans: Vec<Span> = vec![Span::styled(" ", base)];
    let mut x = area.x.saturating_add(1);
    if app.tabs.is_empty() {
        spans.push(Span::styled(" Working View", base.fg(theme.fg_muted)));
    }
    for (i, tab) in app.tabs.iter().enumerate() {
        let fname = tab
            .path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "untitled".to_string());
        let marker = if tab.dirty { "● " } else { "" };
        let name_text = format!(" {marker}{fname} ");
        let close_text = "× ";
        let active = i == app.active_tab;
        let mut name_style = if active {
            base.fg(theme.accent)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            base.fg(theme.fg_muted)
        };
        if tab.is_preview {
            name_style = name_style.add_modifier(Modifier::ITALIC);
        }
        let close_style = if active {
            base.fg(theme.accent)
        } else {
            base.fg(theme.fg_muted)
        };
        let name_w = name_text.width() as u16;
        let close_w = close_text.width() as u16;
        app.tab_rects.push((
            Rect::new(x, area.y, name_w, 1),
            Rect::new(x.saturating_add(name_w), area.y, close_w, 1),
        ));
        spans.push(Span::styled(name_text, name_style));
        spans.push(Span::styled(close_text, close_style));
        spans.push(Span::styled(" ", base));
        x = x.saturating_add(name_w + close_w + 1);
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).style(base), area);
}
