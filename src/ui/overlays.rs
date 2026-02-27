use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, Paragraph, Wrap};

use crate::app::App;
use crate::keybinds::KeyAction;
use crate::types::PendingAction;
use crate::util::{
    command_action_label, context_actions, context_label, editor_context_actions,
    editor_context_label, primary_mod_label, relative_path,
};

use super::helpers::{centered_rect, help_keybind_line, list_item_style, themed_block};

pub(crate) fn render_menu(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme().clone();
    let area = centered_rect(62, 62, frame.area());
    app.menu_rect = area;
    frame.render_widget(Clear, area);
    let mut items: Vec<ListItem> = Vec::new();
    items.push(ListItem::new(Line::from(vec![
        Span::styled("Query: ", Style::default().fg(theme.fg_muted)),
        Span::styled(app.menu_query.clone(), Style::default().fg(theme.fg)),
    ])));
    if app.menu_results.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "No commands",
            Style::default().fg(theme.fg_muted),
        ))));
    }
    let list_items: Vec<ListItem> = app
        .menu_results
        .iter()
        .enumerate()
        .map(|(idx, action)| {
            let style = if idx == app.menu_index {
                list_item_style(true, &theme)
            } else {
                list_item_style(false, &theme)
            };
            ListItem::new(Line::from(Span::styled(
                command_action_label(*action),
                style,
            )))
        })
        .collect();
    items.extend(list_items);
    let list = List::new(items).block(themed_block(&theme).title("Command Palette"));
    frame.render_widget(list, area);
}

pub(crate) fn render_theme_browser(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme().clone();
    let area = centered_rect(62, 70, frame.area());
    app.theme_browser_rect = area;
    frame.render_widget(Clear, area);
    let list_items: Vec<ListItem> = app
        .themes
        .iter()
        .enumerate()
        .map(|(idx, t)| {
            let label = format!("{} [{}]", t.name, t.theme_type);
            let style = if idx == app.theme_index {
                list_item_style(true, &theme)
            } else {
                list_item_style(false, &theme)
            };
            ListItem::new(Line::from(Span::styled(label, style)))
        })
        .collect();
    let list =
        List::new(list_items).block(themed_block(&theme).title("Theme Picker (Live Preview)"));
    frame.render_widget(list, area);
}

pub(crate) fn render_file_picker(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme().clone();
    let area = centered_rect(72, 65, frame.area());
    app.file_picker_rect = area;
    frame.render_widget(Clear, area);
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("Query: ", Style::default().fg(theme.fg_muted)),
        Span::styled(app.file_picker_query.clone(), Style::default().fg(theme.fg)),
    ]));
    lines.push(Line::from(""));
    if app.file_picker_results.is_empty() {
        lines.push(Line::from(Span::styled(
            "No matching files",
            Style::default().fg(theme.fg_muted),
        )));
    } else {
        for (idx, path) in app.file_picker_results.iter().take(25).enumerate() {
            let rel = relative_path(&app.root, path).display().to_string();
            let style = if idx == app.file_picker_index {
                list_item_style(true, &theme)
            } else {
                list_item_style(false, &theme)
            };
            lines.push(Line::from(Span::styled(rel, style)));
        }
    }
    let paragraph = Paragraph::new(lines)
        .style(Style::default().fg(theme.fg).bg(theme.bg_alt))
        .wrap(Wrap { trim: false })
        .block(
            themed_block(&theme)
                .title(format!("Quick Open ({}+P)", primary_mod_label()))
                .style(Style::default().bg(theme.bg_alt)),
        );
    frame.render_widget(paragraph, area);
}

pub(crate) fn render_search_results(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme().clone();
    let area = centered_rect(78, 72, frame.area());
    app.search_results_rect = area;
    frame.render_widget(Clear, area);
    let list_items: Vec<ListItem> = if app.search_results.results.is_empty() {
        vec![ListItem::new(Line::from("No results"))]
    } else {
        app.search_results
            .results
            .iter()
            .enumerate()
            .map(|(idx, hit)| {
                let rel = relative_path(&app.root, &hit.path);
                let label = format!("{}:{}  {}", rel.display(), hit.line, hit.preview);
                let style = if idx == app.search_results.index {
                    list_item_style(true, &theme)
                } else {
                    list_item_style(false, &theme)
                };
                ListItem::new(Line::from(Span::styled(label, style)))
            })
            .collect()
    };
    let title = format!("Search Results: {}", app.search_results.query);
    let list = List::new(list_items).block(themed_block(&theme).title(title));
    frame.render_widget(list, area);
}

pub(crate) fn render_completion_popup(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme().clone();
    let width = 56;
    let visible = app.completion.items.len().min(10);
    let height = visible as u16 + 2;
    let max_x = frame.area().width.saturating_sub(width);
    let max_y = frame.area().height.saturating_sub(height);
    let x = app.editor_rect.x.saturating_add(3).min(max_x);
    let y = app.editor_rect.y.saturating_add(2).min(max_y);
    let area = Rect::new(x, y, width, height);
    app.completion.rect = area;
    frame.render_widget(Clear, area);
    let list_items: Vec<ListItem> = app
        .completion
        .items
        .iter()
        .take(10)
        .enumerate()
        .map(|(idx, item)| {
            let label = if let Some(detail) = &item.detail {
                format!("{}  {}", item.label, detail)
            } else {
                item.label.clone()
            };
            let style = if idx == app.completion.index {
                list_item_style(true, &theme)
            } else {
                list_item_style(false, &theme)
            };
            ListItem::new(Line::from(Span::styled(label, style)))
        })
        .collect();
    let list = List::new(list_items).block(themed_block(&theme).title("Completion"));
    frame.render_widget(list, area);
}

pub(crate) fn render_keybind_editor(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme().clone();
    let area = centered_rect(72, 78, frame.area());
    frame.render_widget(Clear, area);
    let heading = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("Filter: ", Style::default().fg(theme.fg_muted)),
        Span::styled(
            app.keybind_editor.query.clone(),
            Style::default().fg(theme.fg),
        ),
    ]));
    lines.push(Line::from(""));
    if app.keybind_editor.actions.is_empty() {
        lines.push(Line::from(Span::styled(
            "No matching actions",
            Style::default().fg(theme.fg_muted),
        )));
    } else {
        // Build display rows: section headers (None) + action rows (Some(index))
        let mut display_rows: Vec<Option<usize>> = Vec::new();
        let has_global = app.keybind_editor.actions.iter().any(|a| a.is_global());
        let has_editor = app.keybind_editor.actions.iter().any(|a| a.is_editor());
        let mut entered_editor = false;
        if has_global {
            display_rows.push(None); // "Global" header
        }
        for (idx, action) in app.keybind_editor.actions.iter().enumerate() {
            if action.is_editor() && !entered_editor {
                entered_editor = true;
                if has_editor {
                    display_rows.push(None); // "Editor" header
                }
            }
            display_rows.push(Some(idx));
        }

        // Find which display row the selected action maps to, for scrolling
        let selected_display_row = display_rows
            .iter()
            .position(|r| *r == Some(app.keybind_editor.index))
            .unwrap_or(0);

        let max_visible = (area.height as usize).saturating_sub(7);
        let start = if selected_display_row >= max_visible {
            selected_display_row - max_visible + 1
        } else {
            0
        };

        // Track which headers we've passed to know what None means
        let mut header_labels: Vec<&str> = Vec::new();
        if has_global {
            header_labels.push("Global");
        }
        if has_editor {
            header_labels.push("Editor");
        }
        let mut header_count = 0;

        for display_row in display_rows.iter().skip(start).take(max_visible) {
            match display_row {
                None => {
                    if header_count > 0 {
                        lines.push(Line::from(""));
                    }
                    if let Some(label) = header_labels.get(header_count) {
                        lines.push(Line::from(Span::styled(*label, heading)));
                        lines.push(Line::from(""));
                    }
                    header_count += 1;
                }
                Some(action_idx) => {
                    let action = app.keybind_editor.actions[*action_idx];
                    let label = action.label();
                    let bind_str = app.keybinds.display_for(action);
                    let is_selected = *action_idx == app.keybind_editor.index;
                    let style = if is_selected {
                        list_item_style(true, &theme)
                    } else {
                        list_item_style(false, &theme)
                    };
                    let bind_style = if is_selected {
                        Style::default().fg(theme.bg).bg(theme.accent)
                    } else {
                        Style::default().fg(theme.accent_secondary)
                    };
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {label:<30}"), style),
                        Span::styled(bind_str, bind_style),
                    ]));
                }
            }
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Enter: rebind | Ctrl+Del: clear | Ctrl+R: reset | Esc: close",
        Style::default().fg(theme.fg_muted),
    )));
    if app.keybind_editor.recording {
        lines.push(Line::from(Span::styled(
            ">> Press a key to bind... (Esc to cancel)",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
    }
    if let Some((bind, for_action)) = app.keybind_editor.conflict.as_ref() {
        let other = app
            .keybinds
            .find_conflict(bind, *for_action)
            .map(|a| a.label())
            .unwrap_or("another action");
        lines.push(Line::from(Span::styled(
            format!(
                "Conflict: {} is bound to {}. Enter overwrite | Esc cancel | any other key to retry",
                bind.display(),
                other
            ),
            Style::default().fg(theme.accent_secondary),
        )));
    }
    let paragraph = Paragraph::new(lines)
        .style(Style::default().fg(theme.fg).bg(theme.bg_alt))
        .block(
            themed_block(&theme)
                .title(" Keybind Editor ")
                .style(Style::default().bg(theme.bg_alt)),
        );
    frame.render_widget(paragraph, area);
}

pub(crate) fn render_help(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme();
    let area = centered_rect(78, 80, frame.area());
    frame.render_widget(Clear, area);

    let kb = &app.keybinds;
    let heading = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let key_s = Style::default().fg(theme.accent_secondary);
    let desc_s = Style::default().fg(theme.fg);
    let sep_s = Style::default().fg(theme.fg_muted);
    let muted = Style::default().fg(theme.fg_muted);

    let lines: Vec<Line> = vec![
        Line::from(Span::styled("Keyboard", heading)),
        Line::from(""),
        help_keybind_line(
            &[
                (&kb.display_for(KeyAction::Save), "save"),
                (&kb.display_for(KeyAction::CloseTab), "close tab"),
                (&kb.display_for(KeyAction::NewFile), "new file"),
                (&kb.display_for(KeyAction::Quit), "quit"),
            ],
            key_s,
            desc_s,
            sep_s,
        ),
        help_keybind_line(
            &[
                (
                    &kb.display_for(KeyAction::CommandPalette),
                    "command palette",
                ),
                (&kb.display_for(KeyAction::QuickOpen), "quick open"),
                (&kb.display_for(KeyAction::GoToLine), "go to line"),
            ],
            key_s,
            desc_s,
            sep_s,
        ),
        help_keybind_line(
            &[
                (&kb.display_for(KeyAction::ToggleFiles), "toggle files"),
                (&kb.display_for(KeyAction::RefreshTree), "refresh tree"),
                (&kb.display_for(KeyAction::ToggleWordWrap), "toggle wrap"),
            ],
            key_s,
            desc_s,
            sep_s,
        ),
        help_keybind_line(
            &[
                (&kb.display_for(KeyAction::Find), "find"),
                (&kb.display_for(KeyAction::FindReplace), "find & replace"),
                (&kb.display_for(KeyAction::SearchFiles), "search files"),
            ],
            key_s,
            desc_s,
            sep_s,
        ),
        help_keybind_line(
            &[(
                &kb.display_for(KeyAction::GoToDefinition),
                "go to definition",
            )],
            key_s,
            desc_s,
            sep_s,
        ),
        help_keybind_line(
            &[
                (&kb.display_for(KeyAction::FoldToggle), "toggle fold"),
                (&kb.display_for(KeyAction::FoldAllToggle), "toggle fold all"),
            ],
            key_s,
            desc_s,
            sep_s,
        ),
        help_keybind_line(
            &[
                (&kb.display_for(KeyAction::Fold), "fold"),
                (&kb.display_for(KeyAction::Unfold), "unfold"),
                (&kb.display_for(KeyAction::FoldAll), "fold all"),
                (&kb.display_for(KeyAction::UnfoldAll), "unfold all"),
            ],
            key_s,
            desc_s,
            sep_s,
        ),
        help_keybind_line(
            &[
                (&kb.display_for(KeyAction::DupLineDown), "dup line down"),
                (&kb.display_for(KeyAction::DupLineUp), "dup line up"),
            ],
            key_s,
            desc_s,
            sep_s,
        ),
        help_keybind_line(
            &[
                (&kb.display_for(KeyAction::FindNext), "find next"),
                (&kb.display_for(KeyAction::FindPrev), "find prev"),
                (&kb.display_for(KeyAction::Dedent), "dedent"),
            ],
            key_s,
            desc_s,
            sep_s,
        ),
        help_keybind_line(
            &[
                (&kb.display_for(KeyAction::PageUp), "page up"),
                (&kb.display_for(KeyAction::PageDown), "page down"),
                (&kb.display_for(KeyAction::GoToStart), "start of file"),
                (&kb.display_for(KeyAction::GoToEnd), "end of file"),
            ],
            key_s,
            desc_s,
            sep_s,
        ),
        help_keybind_line(
            &[
                ("Tab", "completion"),
                (&kb.display_for(KeyAction::Completion), "completion"),
            ],
            key_s,
            desc_s,
            sep_s,
        ),
        help_keybind_line(
            &[
                (&kb.display_for(KeyAction::Undo), "undo"),
                (&kb.display_for(KeyAction::Redo), "redo"),
            ],
            key_s,
            desc_s,
            sep_s,
        ),
        help_keybind_line(
            &[
                (&kb.display_for(KeyAction::SelectAll), "select all"),
                (&kb.display_for(KeyAction::Copy), "copy"),
                (&kb.display_for(KeyAction::Cut), "cut"),
                (&kb.display_for(KeyAction::CutLine), "cut line"),
                (&kb.display_for(KeyAction::Paste), "paste"),
                (&kb.display_for(KeyAction::ToggleComment), "toggle comment"),
            ],
            key_s,
            desc_s,
            sep_s,
        ),
        help_keybind_line(
            &[
                (&kb.display_for(KeyAction::PrevTab), "prev tab"),
                (&kb.display_for(KeyAction::NextTab), "next tab"),
                (&kb.display_for(KeyAction::Help), "help"),
            ],
            key_s,
            desc_s,
            sep_s,
        ),
        Line::from(""),
        Line::from(Span::styled("Tree", heading)),
        Line::from(""),
        help_keybind_line(
            &[
                ("Up/Down/K/J", "move"),
                ("Left/H", "collapse"),
                ("Right/L/Enter", "open"),
            ],
            key_s,
            desc_s,
            sep_s,
        ),
        help_keybind_line(
            &[
                (&kb.display_for(KeyAction::TreeExpandRecursive), "expand recursive"),
                (&kb.display_for(KeyAction::TreeCollapseRecursive), "collapse recursive"),
            ],
            key_s,
            desc_s,
            sep_s,
        ),
        help_keybind_line(
            &[
                (&kb.display_for(KeyAction::TreeExpandAll), "expand all"),
                (&kb.display_for(KeyAction::TreeCollapseAll), "collapse all"),
            ],
            key_s,
            desc_s,
            sep_s,
        ),
        help_keybind_line(&[("Delete", "delete selected item")], key_s, desc_s, sep_s),
        Line::from(""),
        Line::from(Span::styled("Mouse", heading)),
        Line::from(""),
        Line::from(vec![
            Span::styled("Click", key_s),
            Span::styled(" file: preview tab", desc_s),
            Span::styled("  |  ", sep_s),
            Span::styled("Double-click", key_s),
            Span::styled(" sticky tab", desc_s),
        ]),
        Line::from(vec![
            Span::styled("Click", key_s),
            Span::styled(" tab to switch", desc_s),
            Span::styled("  |  ", sep_s),
            Span::styled("Click [x]", key_s),
            Span::styled(" to close", desc_s),
        ]),
        Line::from(Span::styled(
            "Drag divider to resize  |  Right-click: context menus  |  Gutter click: fold",
            muted,
        )),
        Line::from(Span::styled(
            "[+]/[-] buttons in tree header: expand/collapse all folders",
            muted,
        )),
        Line::from(""),
    ];

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(theme.fg).bg(theme.bg_alt))
        .block(
            themed_block(theme)
                .title(" Help ")
                .style(Style::default().bg(theme.bg_alt)),
        );
    frame.render_widget(paragraph, area);
}

pub(crate) fn render_context_menu(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme().clone();
    let width = 24;
    let height = context_actions().len() as u16 + 2;
    let max_x = frame.area().width.saturating_sub(width);
    let max_y = frame.area().height.saturating_sub(height);
    let x = app.context_menu.pos.0.min(max_x);
    let y = app.context_menu.pos.1.min(max_y);
    let area = Rect::new(x, y, width, height);
    app.context_menu.rect = area;
    frame.render_widget(Clear, area);
    let list_items: Vec<ListItem> = context_actions()
        .iter()
        .enumerate()
        .map(|(idx, action)| {
            let style = if idx == app.context_menu.index {
                list_item_style(true, &theme)
            } else {
                list_item_style(false, &theme)
            };
            ListItem::new(Line::from(Span::styled(context_label(*action), style)))
        })
        .collect();
    let title = app
        .context_menu
        .target
        .as_ref()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "Actions".to_string());
    let list = List::new(list_items).block(themed_block(&theme).title(title));
    frame.render_widget(list, area);
}

pub(crate) fn render_editor_context_menu(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme().clone();
    let width = 20;
    let height = editor_context_actions().len() as u16 + 2;
    let max_x = frame.area().width.saturating_sub(width);
    let max_y = frame.area().height.saturating_sub(height);
    let x = app.editor_context_menu_pos.0.min(max_x);
    let y = app.editor_context_menu_pos.1.min(max_y);
    let area = Rect::new(x, y, width, height);
    app.editor_context_menu_rect = area;
    frame.render_widget(Clear, area);
    let list_items: Vec<ListItem> = editor_context_actions()
        .iter()
        .enumerate()
        .map(|(idx, action)| {
            let style = if idx == app.editor_context_menu_index {
                list_item_style(true, &theme)
            } else {
                list_item_style(false, &theme)
            };
            ListItem::new(Line::from(Span::styled(
                editor_context_label(*action),
                style,
            )))
        })
        .collect();
    let list = List::new(list_items).block(themed_block(&theme).title("Edit"));
    frame.render_widget(list, area);
}

pub(crate) fn render_prompt(app: &mut App, frame: &mut Frame<'_>) {
    let Some(prompt) = app.prompt.as_ref() else {
        return;
    };
    let title = prompt.title.clone();
    let value = prompt.value.clone();
    let cursor_pos = prompt.cursor;
    let theme = app.active_theme().clone();
    let area = centered_rect(60, 20, frame.area());
    app.prompt_rect = area;
    frame.render_widget(Clear, area);
    let input = Paragraph::new(value).block(
        themed_block(&theme)
            .title(title.as_str())
            .border_style(Style::default().fg(theme.accent))
            .style(Style::default().bg(theme.bg_alt).fg(theme.fg)),
    );
    frame.render_widget(input, area);
    // Show a visible cursor at the current position in the input text
    let cursor_x = area.x + 1 + cursor_pos as u16;
    let cursor_y = area.y + 1;
    if cursor_x < area.right() {
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

fn render_dialog(
    area: Rect,
    title: &str,
    text: String,
    theme: &crate::theme::Theme,
    frame: &mut Frame<'_>,
) {
    frame.render_widget(Clear, area);
    let body = Paragraph::new(text)
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(theme.fg).bg(theme.bg_alt))
        .block(themed_block(theme).title(title));
    frame.render_widget(body, area);
}

pub(crate) fn render_close_prompt(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme();
    let area = centered_rect(60, 26, frame.area());
    let text = vec![
        "Unsaved changes".to_string(),
        "".to_string(),
        format!("Enter or {}+S: Save and close", primary_mod_label()),
        "Esc: Discard and close".to_string(),
        "C: Cancel".to_string(),
    ]
    .join("\n");
    render_dialog(area, "Close File", text, theme, frame);
}

pub(crate) fn render_delete_prompt(app: &mut App, frame: &mut Frame<'_>) {
    let PendingAction::Delete(path) = &app.pending else {
        return;
    };
    let theme = app.active_theme();
    let area = centered_rect(64, 28, frame.area());
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    let text = [
        format!("Delete '{}' ?", name),
        "".to_string(),
        "Enter or Y: Confirm delete".to_string(),
        "Esc or N: Cancel".to_string(),
    ]
    .join("\n");
    render_dialog(area, "Confirm Delete", text, theme, frame);
}

pub(crate) fn render_conflict_prompt(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme();
    let area = centered_rect(68, 30, frame.area());
    let text = [
        "File changed on disk while you have unsaved edits.",
        "",
        "R: Reload disk version (discard current edits)",
        "K: Keep local edits",
        "D or Esc: Decide later",
    ]
    .join("\n");
    render_dialog(area, "External Change Conflict", text, theme, frame);
}

pub(crate) fn render_recovery_prompt(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme();
    let area = centered_rect(62, 28, frame.area());
    let text = [
        "Autosave content found for this file.",
        "",
        "Enter or R: Recover autosave",
        "D: Discard autosave",
        "Esc or C: Cancel",
    ]
    .join("\n");
    render_dialog(area, "Recover Autosave", text, theme, frame);
}
