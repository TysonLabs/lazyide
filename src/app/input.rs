use super::App;
use std::io;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use tui_textarea::Input;

use crate::keybinds::KeyScope;
use crate::types::{Focus, PendingAction};
use crate::util::{inside, primary_mod_label, to_u16_saturating};

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
                        self.pending = PendingAction::Delete(item.path.clone());
                        self.set_status(format!(
                            "Delete {} ? Press {}+D to confirm.",
                            item.name,
                            primary_mod_label()
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

        if self.prompt.is_some() {
            return Ok(());
        }
        if self
            .active_tab()
            .is_some_and(|t| t.recovery_prompt_open || t.conflict_prompt_open)
        {
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
                    if let Some(idx) = self.tree_index_from_mouse(mouse.row) {
                        self.selected = idx;
                        self.context_menu.target = Some(self.tree[idx].path.clone());
                        self.context_menu.index = 0;
                        self.context_menu.pos = (mouse.column, mouse.row);
                        self.context_menu.open = true;
                    }
                }
                MouseEventKind::ScrollDown => {
                    if self.selected + 1 < self.tree.len() {
                        self.selected += 1;
                    }
                }
                MouseEventKind::ScrollUp => {
                    if self.selected > 0 {
                        self.selected -= 1;
                    }
                }
                _ => {}
            }
            return Ok(());
        }

        // Tab bar click detection (title bar row of editor block)
        if mouse.row == self.editor_rect.y && inside(mouse.column, mouse.row, self.editor_rect) {
            if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
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
            }
            return Ok(());
        }

        if inside(mouse.column, mouse.row, self.editor_rect) {
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    self.focus = Focus::Editor;
                    let inner_x = mouse
                        .column
                        .saturating_sub(self.editor_rect.x.saturating_add(1));
                    if inner_x < Self::EDITOR_GUTTER_WIDTH {
                        let inner_y = mouse
                            .row
                            .saturating_sub(self.editor_rect.y.saturating_add(1))
                            as usize;
                        if let Some(tab) = self.active_tab() {
                            let visible_idx = tab.editor_scroll_row + inner_y;
                            if let Some(&row) = tab.visible_rows_map.get(visible_idx) {
                                self.toggle_fold_at_row(row);
                            }
                        }
                        return Ok(());
                    }
                    if let Some((row, col)) = self.editor_pos_from_mouse(mouse.column, mouse.row) {
                        if let Some(tab) = self.active_tab_mut() {
                            tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
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
                    self.extend_mouse_selection(mouse.column, mouse.row);
                }
                MouseEventKind::Moved => {
                    if self.editor_dragging {
                        self.extend_mouse_selection(mouse.column, mouse.row);
                    }
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.editor_dragging = false;
                    self.editor_drag_anchor = None;
                }
                MouseEventKind::Down(MouseButton::Right) => {
                    self.focus = Focus::Editor;
                    self.editor_context_menu_pos = (mouse.column, mouse.row);
                    self.editor_context_menu_index = 0;
                    self.editor_context_menu_open = true;
                }
                MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                    let modified = self
                        .active_tab_mut()
                        .is_some_and(|t| t.editor.input(Input::from(Event::Mouse(mouse))));
                    if modified {
                        self.mark_dirty();
                        self.notify_lsp_did_change();
                    }
                    if let Some(tab) = self.active_tab_mut() {
                        match mouse.kind {
                            MouseEventKind::ScrollDown => {
                                tab.editor_scroll_row = tab.editor_scroll_row.saturating_add(1)
                            }
                            MouseEventKind::ScrollUp => {
                                tab.editor_scroll_row = tab.editor_scroll_row.saturating_sub(1)
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            self.sync_editor_scroll_guess();
            self.refresh_inline_ghost();
            return Ok(());
        }

        Ok(())
    }
}
