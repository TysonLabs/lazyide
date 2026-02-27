use super::App;
use std::io;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::keybinds::KeyScope;
use crate::types::{Focus, PendingAction};
use crate::util::{inside, to_u16_saturating};

impl App {
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> io::Result<()> {
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }

        if self.keybind_editor.open {
            return self.handle_keybind_editor_key(key);
        }
        if self.file_picker_open {
            return self.handle_file_picker_key(key);
        }
        if self.active_tab().is_some_and(|t| t.recovery_prompt_open) {
            return self.handle_recovery_prompt_key(key);
        }
        if self.active_tab().is_some_and(|t| t.conflict_prompt_open) {
            return self.handle_conflict_prompt_key(key);
        }
        if self.prompt.is_some() {
            return self.handle_prompt_key(key);
        }
        if self.completion.open {
            return self.handle_completion_key(key);
        }
        if self.search_results.open {
            return self.handle_search_results_key(key);
        }
        if self.editor_context_menu_open {
            return self.handle_editor_context_menu_key(key);
        }
        if self.context_menu.open {
            return self.handle_context_menu_key(key);
        }
        if self.theme_browser_open {
            return self.handle_theme_browser_key(key);
        }
        if self.menu_open {
            return self.handle_menu_key(key);
        }
        if self.help_open {
            return self.handle_help_key(key);
        }

        if self.handle_pending_key(key)? {
            return Ok(());
        }

        // Global keybind lookup
        if let Some(action) = self.keybinds.lookup(&key, KeyScope::Global) {
            return self.run_key_action(action);
        }

        // Non-remappable keys
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) => {
                if self.open_path().is_some() && self.is_dirty() {
                    self.pending = PendingAction::ClosePrompt;
                    self.set_status("Unsaved changes: Enter save+close | Esc discard | C cancel");
                    return Ok(());
                }
                if self.focus == Focus::Editor && self.open_path().is_some() {
                    self.close_file();
                    return Ok(());
                }
            }
            (KeyModifiers::NONE, KeyCode::Tab) => {
                if self.focus == Focus::Editor {
                    // Keep Tab in editor so inline/popup completion can work.
                } else if self.files_view_open {
                    self.focus = Focus::Tree;
                    self.set_status("Focus: files");
                } else {
                    self.focus = Focus::Editor;
                    self.set_status("Files view is hidden");
                }
                if self.focus != Focus::Editor {
                    return Ok(());
                }
            }
            (KeyModifiers::NONE, KeyCode::Delete) => {
                if self.focus == Focus::Tree {
                    if let Some(item) = self.selected_item().cloned() {
                        if item.path == self.root {
                            self.set_status("Cannot delete project root");
                            return Ok(());
                        }
                        self.pending = PendingAction::Delete(item.path.clone());
                        self.set_status(format!(
                            "Delete {} ? Press Enter to confirm, Esc to cancel.",
                            item.name,
                        ));
                    }
                    return Ok(());
                }
            }
            _ => {}
        }

        match self.focus {
            Focus::Tree => self.handle_tree_key(key),
            Focus::Editor => self.handle_editor_key(key),
        }
    }
    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) -> io::Result<()> {
        if self.help_open {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                self.help_open = false;
            }
            return Ok(());
        }

        // Modal states: allow left-click to dismiss, block everything else
        if self.prompt.is_some()
            || matches!(
                self.pending,
                PendingAction::ClosePrompt | PendingAction::Delete(_)
            )
            || self
                .active_tab()
                .is_some_and(|t| t.recovery_prompt_open || t.conflict_prompt_open)
        {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                // Dismiss the modal on click outside (Esc-equivalent)
                if self.prompt.is_some() {
                    self.prompt = None;
                } else if matches!(self.pending, PendingAction::Delete(_)) {
                    self.pending = PendingAction::None;
                    self.set_status("Delete cancelled");
                } else if matches!(self.pending, PendingAction::ClosePrompt) {
                    self.pending = PendingAction::None;
                    self.set_status("Close cancelled");
                } else if let Some(tab) = self.active_tab_mut() {
                    if tab.recovery_prompt_open {
                        tab.recovery_prompt_open = false;
                    } else if tab.conflict_prompt_open {
                        tab.conflict_prompt_open = false;
                    }
                }
            }
            return Ok(());
        }

        if self.search_results.open {
            return self.handle_search_results_mouse(mouse);
        }
        if self.completion.open {
            return self.handle_completion_mouse(mouse);
        }

        if self.editor_context_menu_open {
            return self.handle_editor_context_menu_mouse(mouse);
        }

        if self.context_menu.open {
            return self.handle_context_menu_mouse(mouse);
        }

        if self.menu_open {
            return self.handle_menu_mouse(mouse);
        }

        if self.theme_browser_open {
            return self.handle_theme_browser_mouse(mouse);
        }

        if self.files_view_open {
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if inside(mouse.column, mouse.row, self.divider_rect) {
                        self.divider_dragging = true;
                        return Ok(());
                    }
                }
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Moved => {
                    if self.divider_dragging {
                        // Convert absolute X to pane width by using content frame start.
                        let desired = mouse.column.saturating_sub(self.tree_rect.x);
                        self.files_pane_width = desired.max(Self::MIN_FILES_PANE_WIDTH);
                        self.clamp_files_pane_width(
                            self.editor_rect.width + self.tree_rect.width + self.divider_rect.width,
                        );
                        return Ok(());
                    }
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    if self.divider_dragging {
                        self.divider_dragging = false;
                        self.persist_state();
                        self.set_status(format!("Files pane width: {}", self.files_pane_width));
                        return Ok(());
                    }
                }
                _ => {}
            }
        }

        if inside(mouse.column, mouse.row, self.tree_rect) {
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(idx) = self.tree_index_from_mouse(mouse.row) {
                        self.selected = idx;
                        let path = self.tree[idx].path.clone();
                        if path.is_dir() {
                            self.tree_activate_selected()?;
                            self.focus = Focus::Tree;
                        } else {
                            // Double-click detection (400ms threshold)
                            let is_double_click =
                                self.last_tree_click.as_ref().is_some_and(|(t, prev_idx)| {
                                    *prev_idx == idx && t.elapsed() < Duration::from_millis(400)
                                });
                            self.last_tree_click = Some((Instant::now(), idx));
                            if is_double_click {
                                // Double-click opens as sticky
                                self.open_file_as(path, false)?;
                            } else {
                                // Single-click opens as preview
                                self.open_file_as(path, true)?;
                            }
                        }
                    }
                }
                MouseEventKind::Down(MouseButton::Right) => {
                    self.open_tree_context_menu_at(mouse.column, mouse.row);
                }
                MouseEventKind::ScrollDown => {
                    self.selected = (self.selected + Self::SCROLL_LINES)
                        .min(self.tree.len().saturating_sub(1));
                }
                MouseEventKind::ScrollUp => {
                    self.selected = self.selected.saturating_sub(Self::SCROLL_LINES);
                }
                _ => {}
            }
            return Ok(());
        }

        // Tab bar click detection (title bar row of editor block)
        if mouse.row == self.editor_rect.y && inside(mouse.column, mouse.row, self.editor_rect) {
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    for (i, (name_rect, close_rect)) in self.tab_rects.iter().enumerate() {
                        if inside(mouse.column, mouse.row, *close_rect) {
                            // Click on [x] — close this tab
                            if self.tabs[i].dirty {
                                self.switch_to_tab(i);
                                self.pending = PendingAction::ClosePrompt;
                                self.set_status(
                                    "Unsaved changes: Enter save+close | Esc discard | C cancel",
                                );
                            } else {
                                self.close_tab_at(i);
                            }
                            return Ok(());
                        }
                        if inside(mouse.column, mouse.row, *name_rect) {
                            // Click on tab name — switch to it
                            self.switch_to_tab(i);
                            return Ok(());
                        }
                    }
                    return Ok(());
                }
                // Scroll events on the tab bar fall through to the editor scroll handler
                MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {}
                _ => return Ok(()),
            }
        }

        if inside(mouse.column, mouse.row, self.editor_rect) {
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    self.focus = Focus::Editor;
                    let inner_x = mouse
                        .column
                        .saturating_sub(self.editor_rect.x.saturating_add(1));
                    if inner_x < Self::EDITOR_GUTTER_WIDTH {
                        if inner_x < 6 {
                            // Line number area → select full line
                            if let Some(row) = self.gutter_row_from_mouse(mouse.row) {
                                self.select_line(row);
                                self.gutter_drag_anchor = Some(row);
                                self.editor_dragging = true;
                            }
                        } else {
                            // Fold/marker area → toggle fold
                            if let Some(row) = self.gutter_row_from_mouse(mouse.row) {
                                self.toggle_fold_at_row(row);
                            }
                        }
                        return Ok(());
                    }
                    if let Some((row, col)) = self.editor_pos_from_mouse(mouse.column, mouse.row) {
                        if let Some(tab) = self.active_tab_mut() {
                            tab.editor.move_cursor(ratatui_textarea::CursorMove::Jump(
                                to_u16_saturating(row),
                                to_u16_saturating(col),
                            ));
                            tab.editor.cancel_selection();
                        }
                        self.editor_dragging = true;
                        self.editor_drag_anchor = Some((row, col));
                    }
                }
                MouseEventKind::Drag(MouseButton::Left) => {
                    if let Some(anchor) = self.gutter_drag_anchor {
                        if let Some(target) = self.gutter_row_from_mouse(mouse.row) {
                            self.select_line_range(anchor, target);
                        }
                    } else {
                        self.extend_mouse_selection(mouse.column, mouse.row);
                    }
                }
                MouseEventKind::Moved => {
                    if self.editor_dragging {
                        if let Some(anchor) = self.gutter_drag_anchor {
                            if let Some(target) = self.gutter_row_from_mouse(mouse.row) {
                                self.select_line_range(anchor, target);
                            }
                        } else {
                            self.extend_mouse_selection(mouse.column, mouse.row);
                        }
                    } else {
                        return Ok(());
                    }
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.editor_dragging = false;
                    self.editor_drag_anchor = None;
                    self.gutter_drag_anchor = None;
                    return Ok(());
                }
                MouseEventKind::Down(MouseButton::Right) => {
                    self.focus = Focus::Editor;
                    self.editor_context_menu_pos = (mouse.column, mouse.row);
                    self.editor_context_menu_index = 0;
                    self.editor_context_menu_open = true;
                    return Ok(());
                }
                MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                    if self
                        .active_tab()
                        .is_some_and(|t| t.visible_rows_map.is_empty())
                    {
                        self.rebuild_visible_rows();
                    }
                    let viewport_h = self.editor_rect.height.saturating_sub(2) as usize;
                    if let Some(tab) = self.active_tab_mut() {
                        let max_scroll = tab
                            .visible_rows_map
                            .len()
                            .saturating_sub(viewport_h.max(1));
                        match mouse.kind {
                            MouseEventKind::ScrollDown => {
                                tab.editor_scroll_row = tab
                                    .editor_scroll_row
                                    .saturating_add(Self::SCROLL_LINES)
                                    .min(max_scroll)
                            }
                            MouseEventKind::ScrollUp => {
                                tab.editor_scroll_row = tab
                                    .editor_scroll_row
                                    .saturating_sub(Self::SCROLL_LINES)
                            }
                            _ => {}
                        }
                    }
                    // Move cursor to stay within the visible viewport so that
                    // subsequent actions don't snap the scroll back to the old
                    // cursor position.
                    self.clamp_cursor_to_viewport();
                    return Ok(());
                }
                MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => {
                    if !self.word_wrap {
                        if let Some(tab) = self.active_tab_mut() {
                            match mouse.kind {
                                MouseEventKind::ScrollLeft => {
                                    tab.editor_scroll_col = tab
                                        .editor_scroll_col
                                        .saturating_sub(Self::SCROLL_LINES);
                                }
                                MouseEventKind::ScrollRight => {
                                    tab.editor_scroll_col = tab
                                        .editor_scroll_col
                                        .saturating_add(Self::SCROLL_LINES);
                                }
                                _ => {}
                            }
                        }
                    }
                    return Ok(());
                }
                _ => return Ok(()),
            }
            self.sync_editor_scroll_guess();
            self.refresh_inline_ghost();
            return Ok(());
        }

        Ok(())
    }
}
