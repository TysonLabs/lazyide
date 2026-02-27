use super::App;
use std::io;

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use tui_textarea::Input;

use crate::keybinds::{
    KeyAction, KeyBind, KeyBindings, KeyScope, save_keybindings, selected_action,
};
use crate::types::{Focus, PendingAction, PromptMode};
use crate::util::{
    context_actions, editor_context_actions, inside, pending_hint, primary_mod_label,
    text_to_lines, to_u16_saturating,
};

impl App {
    pub(crate) fn open_tree_context_menu_at(&mut self, column: u16, row: u16) {
        if let Some(idx) = self.tree_index_from_mouse(row) {
            self.selected = idx;
            self.context_menu.target = Some(self.tree[idx].path.clone());
            self.context_menu.index = 0;
        } else {
            // Right-click on empty tree space: open context menu at root for create actions.
            self.context_menu.target = Some(self.root.clone());
            self.context_menu.index = 1; // New File
        }
        self.context_menu.pos = (column, row);
        self.context_menu.open = true;
    }

    fn left_click_outside(mouse: MouseEvent, rect: Rect) -> bool {
        matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && !inside(mouse.column, mouse.row, rect)
    }

    pub(crate) fn handle_prompt_key(&mut self, key: KeyEvent) -> io::Result<()> {
        let Some(prompt) = self.prompt.as_mut() else {
            return Ok(());
        };
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) => {
                self.prompt = None;
                self.set_status("Canceled");
            }
            (_, KeyCode::Enter) => {
                let value = prompt.value.trim().to_string();
                if value.is_empty()
                    && !matches!(prompt.mode, PromptMode::FindInFile | PromptMode::GoToLine)
                {
                    self.set_status("Name cannot be empty");
                    return Ok(());
                }
                let mode = prompt.mode.clone();
                self.prompt = None;
                self.apply_prompt(mode, value)?;
            }
            (_, KeyCode::Backspace) => {
                prompt.value.pop();
            }
            (_, KeyCode::Char(c)) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL) {
                    prompt.value.push(c);
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn handle_file_picker_key(&mut self, key: KeyEvent) -> io::Result<()> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) => {
                self.file_picker_open = false;
                self.file_picker_query.clear();
                self.set_status("Canceled quick open");
            }
            (_, KeyCode::Enter) => {
                self.open_file_picker_selection()?;
            }
            (_, KeyCode::Down) | (_, KeyCode::Char('j')) => {
                if self.file_picker_index + 1 < self.file_picker_results.len() {
                    self.file_picker_index += 1;
                }
            }
            (_, KeyCode::Up) | (_, KeyCode::Char('k')) => {
                if self.file_picker_index > 0 {
                    self.file_picker_index -= 1;
                }
            }
            (_, KeyCode::Backspace) => {
                self.file_picker_query.pop();
                self.file_picker_index = 0;
                self.refresh_file_picker_results();
            }
            (_, KeyCode::Char(c)) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    self.file_picker_query.push(c);
                    self.file_picker_index = 0;
                    self.refresh_file_picker_results();
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn handle_search_results_key(&mut self, key: KeyEvent) -> io::Result<()> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) => {
                self.search_results.open = false;
                self.set_status("Closed search results");
            }
            (_, KeyCode::Down) | (_, KeyCode::Char('j')) => {
                if self.search_results.index + 1 < self.search_results.results.len() {
                    self.search_results.index += 1;
                }
            }
            (_, KeyCode::Up) | (_, KeyCode::Char('k')) => {
                if self.search_results.index > 0 {
                    self.search_results.index -= 1;
                }
            }
            (_, KeyCode::Enter) => {
                self.open_selected_search_result()?;
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn handle_completion_key(&mut self, key: KeyEvent) -> io::Result<()> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) => {
                self.completion.reset();
                self.set_status("Completion closed");
            }
            (_, KeyCode::Down) | (_, KeyCode::Char('j')) => {
                if self.completion.index + 1 < self.completion.items.len() {
                    self.completion.index += 1;
                }
                self.update_completion_ghost_from_selection();
            }
            (_, KeyCode::Up) | (_, KeyCode::Char('k')) => {
                if self.completion.index > 0 {
                    self.completion.index -= 1;
                }
                self.update_completion_ghost_from_selection();
            }
            (_, KeyCode::Enter) | (_, KeyCode::Tab) => {
                self.apply_completion();
            }
            _ => {
                self.completion.reset();
            }
        }
        Ok(())
    }

    pub(crate) fn handle_context_menu_key(&mut self, key: KeyEvent) -> io::Result<()> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) => {
                self.context_menu.open = false;
            }
            (_, KeyCode::Down) | (_, KeyCode::Char('j')) => {
                if self.context_menu.index < context_actions().len().saturating_sub(1) {
                    self.context_menu.index += 1;
                }
            }
            (_, KeyCode::Up) | (_, KeyCode::Char('k')) => {
                if self.context_menu.index > 0 {
                    self.context_menu.index -= 1;
                }
            }
            (_, KeyCode::Enter) => {
                let action = context_actions()[self.context_menu.index];
                self.apply_context_action(action)?;
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn handle_pending_key(&mut self, key: KeyEvent) -> io::Result<bool> {
        match (&self.pending, key.modifiers, key.code) {
            (PendingAction::None, _, _) => Ok(false),
            (PendingAction::Quit, KeyModifiers::CONTROL, KeyCode::Char('q' | 'Q')) => {
                self.quit = true;
                Ok(true)
            }
            (_, mods, KeyCode::Char('q' | 'Q'))
                if mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT) =>
            {
                self.pending = PendingAction::None;
                self.run_key_action(KeyAction::Quit)?;
                Ok(true)
            }
            (PendingAction::ClosePrompt, mods, KeyCode::Char('s'))
            | (PendingAction::ClosePrompt, mods, KeyCode::Char('S'))
                if mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT) =>
            {
                self.pending = PendingAction::None;
                self.save_file()?;
                self.close_file();
                Ok(true)
            }
            (PendingAction::ClosePrompt, KeyModifiers::NONE, KeyCode::Enter) => {
                self.pending = PendingAction::None;
                self.save_file()?;
                self.close_file();
                Ok(true)
            }
            (PendingAction::ClosePrompt, KeyModifiers::NONE, KeyCode::Esc) => {
                self.pending = PendingAction::None;
                self.close_file();
                Ok(true)
            }
            (PendingAction::ClosePrompt, KeyModifiers::NONE, KeyCode::Char('c'))
            | (PendingAction::ClosePrompt, KeyModifiers::NONE, KeyCode::Char('C')) => {
                self.pending = PendingAction::None;
                self.set_status("Close canceled");
                Ok(true)
            }
            (PendingAction::Delete(path), mods, KeyCode::Char('d' | 'D'))
                if mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT) =>
            {
                let target = path.clone();
                self.pending = PendingAction::None;
                self.delete_path(target)?;
                Ok(true)
            }
            (PendingAction::Delete(path), KeyModifiers::NONE, KeyCode::Enter)
            | (PendingAction::Delete(path), KeyModifiers::NONE, KeyCode::Char('y'))
            | (PendingAction::Delete(path), KeyModifiers::NONE, KeyCode::Char('Y')) => {
                let target = path.clone();
                self.pending = PendingAction::None;
                self.delete_path(target)?;
                Ok(true)
            }
            (PendingAction::Delete(_), KeyModifiers::NONE, KeyCode::Char('n'))
            | (PendingAction::Delete(_), KeyModifiers::NONE, KeyCode::Char('N'))
            | (PendingAction::Delete(_), KeyModifiers::NONE, KeyCode::Esc) => {
                self.pending = PendingAction::None;
                self.set_status("Delete canceled");
                Ok(true)
            }
            (_, KeyModifiers::NONE, KeyCode::Esc) => {
                self.pending = PendingAction::None;
                self.set_status("Canceled");
                Ok(true)
            }
            _ => {
                self.set_status(pending_hint(&self.pending));
                Ok(true)
            }
        }
    }

    pub(crate) fn handle_tree_key(&mut self, key: KeyEvent) -> io::Result<()> {
        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Down) | (KeyModifiers::NONE, KeyCode::Char('j')) => {
                if self.selected + 1 < self.tree.len() {
                    self.selected += 1;
                }
            }
            (KeyModifiers::NONE, KeyCode::Up) | (KeyModifiers::NONE, KeyCode::Char('k')) => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
            }
            (KeyModifiers::NONE, KeyCode::Right)
            | (KeyModifiers::NONE, KeyCode::Char('l'))
            | (KeyModifiers::NONE, KeyCode::Enter) => {
                self.tree_activate_selected()?;
            }
            (KeyModifiers::NONE, KeyCode::Left) | (KeyModifiers::NONE, KeyCode::Char('h')) => {
                self.tree_collapse_or_parent();
            }
            _ => {}
        }
        Ok(())
    }
    pub(crate) fn handle_editor_key(&mut self, key: KeyEvent) -> io::Result<()> {
        if self.open_path().is_none() {
            self.focus = Focus::Tree;
            self.set_status("No file open. Focus returned to files.");
            return Ok(());
        }

        // Non-remappable: Tab (completion/ghost/indent), auto-pair insertion
        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Tab) if self.completion.open => {
                self.apply_completion();
                return Ok(());
            }
            (KeyModifiers::NONE, KeyCode::Tab) => {
                if let Some(ghost) = self.completion.ghost.clone() {
                    let now_prefix = self.current_identifier_prefix();
                    if !ghost.is_empty()
                        && !self.completion.prefix.is_empty()
                        && now_prefix == self.completion.prefix
                    {
                        let inserted = self
                            .active_tab_mut()
                            .is_some_and(|t| t.editor.insert_str(ghost));
                        if inserted {
                            self.on_editor_content_changed();
                        }
                        self.completion.ghost = None;
                        self.completion.prefix.clear();
                        self.set_status("Accepted inline completion");
                        return Ok(());
                    } else if now_prefix != self.completion.prefix {
                        self.completion.ghost = None;
                    }
                }
                if !self.current_identifier_prefix().is_empty() {
                    self.request_lsp_completion();
                    return Ok(());
                }
            }
            (KeyModifiers::CONTROL, KeyCode::Null) => {
                self.request_lsp_completion();
                return Ok(());
            }
            (KeyModifiers::NONE, KeyCode::Char(c))
                if matches!(c, '(' | '[' | '{' | '"' | '\'')
                    && self
                        .active_tab()
                        .is_some_and(|t| t.editor.selection_range().is_none()) =>
            {
                let pair = match c {
                    '(' => "()",
                    '[' => "[]",
                    '{' => "{}",
                    '"' => "\"\"",
                    '\'' => "''",
                    _ => "",
                };
                if !pair.is_empty() {
                    let inserted = self
                        .active_tab_mut()
                        .is_some_and(|t| t.editor.insert_str(pair));
                    if inserted {
                        if let Some(tab) = self.active_tab_mut() {
                            tab.editor.move_cursor(tui_textarea::CursorMove::Back);
                        }
                        self.on_editor_content_changed();
                        self.set_status("Auto-pair inserted");
                        return Ok(());
                    }
                }
            }
            _ => {}
        }

        // Editor keybind lookup
        if self.word_wrap && key.modifiers == KeyModifiers::NONE {
            match key.code {
                KeyCode::Down => {
                    self.move_cursor_visual(true);
                    self.refresh_inline_ghost();
                    return Ok(());
                }
                KeyCode::Up => {
                    self.move_cursor_visual(false);
                    self.refresh_inline_ghost();
                    return Ok(());
                }
                _ => {}
            }
        }

        if let Some(action) = self.keybinds.lookup(&key, KeyScope::Editor) {
            return self.run_key_action(action);
        }

        let modified = self
            .active_tab_mut()
            .is_some_and(|t| t.editor.input(Input::from(key)));
        if modified {
            self.on_editor_content_changed();
        }
        self.sync_editor_scroll_guess();
        self.refresh_inline_ghost();
        Ok(())
    }

    pub(crate) fn run_key_action(&mut self, action: KeyAction) -> io::Result<()> {
        match action {
            // Global
            KeyAction::Save => self.save_file()?,
            KeyAction::CloseTab => {
                if !self.tabs.is_empty() {
                    if self.is_dirty() {
                        self.pending = PendingAction::ClosePrompt;
                        self.set_status(
                            "Unsaved changes: Enter save+close | Esc discard | C cancel",
                        );
                    } else {
                        self.close_file();
                    }
                }
            }
            KeyAction::Quit => {
                if self.any_tab_dirty() {
                    if matches!(self.pending, PendingAction::Quit) {
                        self.quit = true;
                    } else {
                        self.pending = PendingAction::Quit;
                        self.set_status(format!(
                            "Unsaved changes. Press {}+Q again to quit.",
                            primary_mod_label()
                        ));
                    }
                } else {
                    self.quit = true;
                }
            }
            KeyAction::ToggleFiles => {
                self.files_view_open = !self.files_view_open;
                if !self.files_view_open {
                    self.focus = Focus::Editor;
                    self.set_status("Files view hidden");
                } else {
                    self.set_status("Files view shown");
                }
            }
            KeyAction::CommandPalette => self.open_command_palette(),
            KeyAction::QuickOpen => {
                self.file_picker_open = true;
                self.file_picker_query.clear();
                self.file_picker_index = 0;
                self.refresh_file_picker_results();
            }
            KeyAction::Find => {
                self.open_find_prompt();
            }
            KeyAction::FindReplace => {
                self.open_replace_prompt();
            }
            KeyAction::SearchFiles => {
                self.open_project_search_prompt();
            }
            KeyAction::GoToLine => {
                self.open_go_to_line_prompt();
            }
            KeyAction::Help => self.help_open = true,
            KeyAction::NewFile => self.create_new_file()?,
            KeyAction::RefreshTree => {
                self.rebuild_tree()?;
                self.set_status("Tree refreshed");
            }
            KeyAction::PrevTab => {
                if !self.tabs.is_empty() {
                    let prev = if self.active_tab == 0 {
                        self.tabs.len() - 1
                    } else {
                        self.active_tab - 1
                    };
                    self.switch_to_tab(prev);
                }
            }
            KeyAction::NextTab => {
                if !self.tabs.is_empty() {
                    let next = (self.active_tab + 1) % self.tabs.len();
                    self.switch_to_tab(next);
                }
            }
            KeyAction::ToggleWordWrap => self.toggle_word_wrap(),
            // Editor
            KeyAction::GoToDefinition => {
                if self.focus == Focus::Editor {
                    self.request_lsp_definition();
                }
            }
            KeyAction::FoldToggle => self.toggle_fold_at_cursor(),
            KeyAction::FoldAllToggle => self.toggle_fold_all(),
            KeyAction::Fold => self.fold_current_block(),
            KeyAction::Unfold => self.unfold_current_block(),
            KeyAction::FoldAll => self.fold_all(),
            KeyAction::UnfoldAll => self.unfold_all(),
            KeyAction::FindNext => {
                if self
                    .active_tab_mut()
                    .is_some_and(|t| t.editor.search_forward(false))
                {
                    self.set_status("Find next");
                    self.sync_editor_scroll_guess();
                } else {
                    self.set_status("No next match");
                }
            }
            KeyAction::FindPrev => {
                if self
                    .active_tab_mut()
                    .is_some_and(|t| t.editor.search_back(false))
                {
                    self.set_status("Find previous");
                    self.sync_editor_scroll_guess();
                } else {
                    self.set_status("No previous match");
                }
            }
            KeyAction::DupLineDown => self.duplicate_current_line(false),
            KeyAction::DupLineUp => self.duplicate_current_line(true),
            KeyAction::Dedent => self.dedent_lines(),
            KeyAction::Completion => self.request_lsp_completion(),
            KeyAction::Undo => {
                if self.active_tab_mut().is_some_and(|t| t.editor.undo()) {
                    self.on_editor_content_changed();
                    self.set_status("Undo");
                } else {
                    self.set_status("Nothing to undo");
                }
                self.sync_editor_scroll_guess();
            }
            KeyAction::Redo => {
                if self.active_tab_mut().is_some_and(|t| t.editor.redo()) {
                    self.on_editor_content_changed();
                    self.set_status("Redo");
                } else {
                    self.set_status("Nothing to redo");
                }
                self.sync_editor_scroll_guess();
            }
            KeyAction::SelectAll => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.editor.select_all();
                }
                self.set_status("Selected all");
            }
            KeyAction::Copy => self.copy_selection_to_clipboard(),
            KeyAction::Cut => self.cut_selection_to_clipboard(),
            KeyAction::CutLine => self.cut_line(),
            KeyAction::Paste => self.paste_from_clipboard(),
            KeyAction::ToggleComment => self.toggle_comment(),
            KeyAction::PageDown => self.page_down(),
            KeyAction::PageUp => self.page_up(),
            KeyAction::GoToStart => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.editor.move_cursor(tui_textarea::CursorMove::Jump(0, 0));
                }
                self.sync_editor_scroll_guess();
                self.set_status("Top of file");
            }
            KeyAction::GoToEnd => {
                if let Some(tab) = self.active_tab() {
                    let last_row = tab.editor.lines().len().saturating_sub(1);
                    let last_col = tab.editor.lines().last().map_or(0, |l| l.len());
                    if let Some(tab) = self.active_tab_mut() {
                        tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
                            to_u16_saturating(last_row),
                            to_u16_saturating(last_col),
                        ));
                    }
                }
                self.sync_editor_scroll_guess();
                self.set_status("End of file");
            }
        }
        Ok(())
    }

    pub(crate) fn handle_keybind_editor_key(&mut self, key: KeyEvent) -> io::Result<()> {
        // Handle conflict confirmation state
        if let Some((bind, for_action)) = self.keybind_editor.conflict.take() {
            match key.code {
                KeyCode::Enter => {
                    // Overwrite: remove conflicting bind from other action
                    if let Some(conflict_action) = self.keybinds.find_conflict(&bind, for_action) {
                        self.keybinds.remove_bind_from(conflict_action, &bind);
                    }
                    // Replace target action bind (same behavior as normal rebind flow)
                    self.keybinds.map.insert(for_action, vec![bind]);
                    let _ = save_keybindings(&self.keybinds);
                    self.set_status(format!("Bound to {}", for_action.label()));
                    self.keybind_editor.recording = false;
                    return Ok(());
                }
                KeyCode::Esc => {
                    self.keybind_editor.recording = false;
                    self.set_status("Canceled");
                    return Ok(());
                }
                _ => {
                    // Let user immediately try a different key instead of getting stuck.
                    self.keybind_editor.conflict = None;
                    self.keybind_editor.recording = true;
                    return Ok(());
                }
            }
        }

        // Recording mode: next keypress becomes the new bind
        if self.keybind_editor.recording {
            if key.code == KeyCode::Esc {
                self.keybind_editor.recording = false;
                self.set_status("Canceled recording");
                return Ok(());
            }
            let Some(action) = self.selected_keybind_action() else {
                self.keybind_editor.recording = false;
                self.set_status("No matching actions to bind");
                return Ok(());
            };
            let bind = KeyBind {
                modifiers: key.modifiers,
                code: KeyBind::normalize_char_with_modifiers(key.code, key.modifiers),
            };
            // Check for conflicts
            if let Some(conflict_action) = self.keybinds.find_conflict(&bind, action) {
                self.set_status(format!(
                    "{} already bound to {}. Enter to overwrite, Esc to cancel",
                    bind.display(),
                    conflict_action.label()
                ));
                self.keybind_editor.conflict = Some((bind, action));
                return Ok(());
            }
            // No conflict, set the bind
            self.keybinds.map.insert(action, vec![bind]);
            let _ = save_keybindings(&self.keybinds);
            self.keybind_editor.recording = false;
            self.set_status(format!(
                "Bound {} to {}",
                self.keybinds.display_for(action),
                action.label()
            ));
            return Ok(());
        }

        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) => {
                self.keybind_editor.open = false;
                self.keybind_editor.query.clear();
            }
            (_, KeyCode::Down) => {
                if self.keybind_editor.index + 1 < self.keybind_editor.actions.len() {
                    self.keybind_editor.index += 1;
                }
            }
            (_, KeyCode::Up) => {
                if self.keybind_editor.index > 0 {
                    self.keybind_editor.index -= 1;
                }
            }
            (_, KeyCode::Enter) => {
                let Some(action) = self.selected_keybind_action() else {
                    self.set_status("No matching actions to bind");
                    return Ok(());
                };
                self.keybind_editor.recording = true;
                self.set_status(format!(
                    "Press new key for '{}' (Esc to cancel)",
                    action.label()
                ));
            }
            (_, KeyCode::Delete) | (_, KeyCode::Backspace)
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                // Remove all bindings for this action
                let Some(action) = self.selected_keybind_action() else {
                    self.set_status("No matching actions to clear");
                    return Ok(());
                };
                self.keybinds.map.insert(action, Vec::new());
                let _ = save_keybindings(&self.keybinds);
                self.set_status(format!("Cleared bindings for {}", action.label()));
            }
            (_, KeyCode::Char('r')) | (_, KeyCode::Char('R'))
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                // Reset to default
                let Some(action) = self.selected_keybind_action() else {
                    self.set_status("No matching actions to reset");
                    return Ok(());
                };
                let defaults = KeyBindings::defaults();
                let default_binds = defaults.map.get(&action).cloned().unwrap_or_default();
                self.keybinds.map.insert(action, default_binds);
                let _ = save_keybindings(&self.keybinds);
                self.set_status(format!("Reset {} to default", action.label()));
            }
            (_, KeyCode::Backspace) => {
                self.keybind_editor.query.pop();
                self.refresh_keybind_editor_actions();
            }
            (_, KeyCode::Char(c)) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    self.keybind_editor.query.push(c);
                    self.refresh_keybind_editor_actions();
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn selected_keybind_action(&self) -> Option<KeyAction> {
        selected_action(&self.keybind_editor.actions, self.keybind_editor.index)
    }

    pub(crate) fn refresh_keybind_editor_actions(&mut self) {
        let q = self.keybind_editor.query.to_ascii_lowercase();
        self.keybind_editor.actions = KeyAction::all()
            .iter()
            .copied()
            .filter(|a| q.is_empty() || a.label().to_ascii_lowercase().contains(&q))
            .collect();
        self.keybind_editor.index = self
            .keybind_editor
            .index
            .min(self.keybind_editor.actions.len().saturating_sub(1));
    }
    pub(crate) fn handle_menu_key(&mut self, key: KeyEvent) -> io::Result<()> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) => {
                self.menu_open = false;
                self.menu_query.clear();
            }
            (_, KeyCode::Down) => {
                if self.menu_index + 1 < self.menu_results.len() {
                    self.menu_index += 1;
                }
            }
            (_, KeyCode::Up) => {
                if self.menu_index > 0 {
                    self.menu_index -= 1;
                }
            }
            (_, KeyCode::Enter) => {
                if let Some(action) = self.menu_results.get(self.menu_index).copied() {
                    self.menu_open = false;
                    self.menu_query.clear();
                    self.run_command_action(action)?;
                }
            }
            (_, KeyCode::Backspace) => {
                self.menu_query.pop();
                self.refresh_menu_results();
            }
            (_, KeyCode::Char(c)) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    self.menu_query.push(c);
                    self.refresh_menu_results();
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn handle_recovery_prompt_key(&mut self, key: KeyEvent) -> io::Result<()> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Enter) | (_, KeyCode::Char('r')) | (_, KeyCode::Char('R')) => {
                let text = self.active_tab().and_then(|t| t.recovery_text.clone());
                if let Some(text) = text {
                    let lines = text_to_lines(&text);
                    let cursor = self.tabs[self.active_tab].editor.cursor();
                    self.replace_editor_text(lines, cursor);
                    self.mark_dirty();
                    self.notify_lsp_did_change();
                    self.set_status("Recovered autosave content");
                }
                if let Some(tab) = self.active_tab_mut() {
                    tab.recovery_prompt_open = false;
                    tab.recovery_text = None;
                }
            }
            (_, KeyCode::Char('d')) | (_, KeyCode::Char('D')) => {
                self.clear_autosave_for_open_file();
                if let Some(tab) = self.active_tab_mut() {
                    tab.recovery_prompt_open = false;
                    tab.recovery_text = None;
                }
                self.set_status("Discarded autosave");
            }
            (_, KeyCode::Esc) | (_, KeyCode::Char('c')) | (_, KeyCode::Char('C')) => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.recovery_prompt_open = false;
                }
                self.set_status("Recovery canceled");
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn handle_conflict_prompt_key(&mut self, key: KeyEvent) -> io::Result<()> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Char('r')) | (_, KeyCode::Char('R')) => {
                let disk = self.active_tab().and_then(|t| t.conflict_disk_text.clone());
                if let Some(disk) = disk {
                    let lines = text_to_lines(&disk);
                    let cursor = self.tabs[self.active_tab].editor.cursor();
                    self.replace_editor_text(lines, cursor);
                    if let Some(tab) = self.active_tab_mut() {
                        tab.dirty = false;
                        tab.open_disk_snapshot = Some(disk);
                    }
                    self.clear_autosave_for_open_file();
                    self.notify_lsp_did_change();
                    self.set_status("Reloaded file from disk");
                }
                if let Some(tab) = self.active_tab_mut() {
                    tab.conflict_prompt_open = false;
                    tab.conflict_disk_text = None;
                }
            }
            (_, KeyCode::Char('k')) | (_, KeyCode::Char('K')) => {
                if let Some(tab) = self.active_tab_mut() {
                    if let Some(disk) = tab.conflict_disk_text.clone() {
                        tab.open_disk_snapshot = Some(disk);
                    }
                    tab.conflict_prompt_open = false;
                    tab.conflict_disk_text = None;
                }
                self.set_status("Keeping local edits");
            }
            (_, KeyCode::Char('d')) | (_, KeyCode::Char('D')) | (_, KeyCode::Esc) => {
                if let Some(tab) = self.active_tab_mut() {
                    if let Some(disk) = tab.conflict_disk_text.clone() {
                        tab.open_disk_snapshot = Some(disk);
                    }
                    tab.conflict_prompt_open = false;
                    tab.conflict_disk_text = None;
                }
                self.set_status("Conflict deferred");
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn handle_theme_browser_key(&mut self, key: KeyEvent) -> io::Result<()> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) => {
                self.active_theme_index = self.preview_revert_index;
                self.theme_index = self.preview_revert_index;
                self.theme_browser_open = false;
                self.menu_open = false;
                self.set_status(format!("Theme reverted: {}", self.active_theme().name));
            }
            (_, KeyCode::Down) | (_, KeyCode::Char('j')) => {
                if self.theme_index + 1 < self.themes.len() {
                    self.theme_index += 1;
                    self.active_theme_index = self.theme_index;
                    self.set_status(format!("Preview: {}", self.active_theme().name));
                }
            }
            (_, KeyCode::Up) | (_, KeyCode::Char('k')) => {
                if self.theme_index > 0 {
                    self.theme_index -= 1;
                    self.active_theme_index = self.theme_index;
                    self.set_status(format!("Preview: {}", self.active_theme().name));
                }
            }
            (_, KeyCode::Enter) => {
                self.persist_theme_selection();
                self.theme_browser_open = false;
                self.menu_open = false;
                self.set_status(format!("Theme: {}", self.active_theme().name));
            }
            _ => {}
        }
        Ok(())
    }
    pub(crate) fn handle_completion_mouse(&mut self, mouse: MouseEvent) -> io::Result<()> {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return Ok(());
        }
        if Self::left_click_outside(mouse, self.completion.rect) {
            self.completion.reset();
            return Ok(());
        }
        let row = mouse.row.saturating_sub(self.completion.rect.y + 1) as usize;
        if row < self.completion.items.len() {
            self.completion.index = row;
            self.apply_completion();
        }
        Ok(())
    }

    pub(crate) fn tree_index_from_mouse(&self, y: u16) -> Option<usize> {
        let start = self.tree_rect.y.saturating_add(1);
        let end = self
            .tree_rect
            .y
            .saturating_add(self.tree_rect.height.saturating_sub(1));
        if y < start || y >= end {
            return None;
        }
        let idx = (y - start) as usize;
        if idx < self.tree.len() {
            Some(idx)
        } else {
            None
        }
    }

    pub(crate) fn handle_menu_mouse(&mut self, mouse: MouseEvent) -> io::Result<()> {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return Ok(());
        }
        if Self::left_click_outside(mouse, self.menu_rect) {
            self.menu_open = false;
            self.menu_query.clear();
            return Ok(());
        }
        let row = mouse.row.saturating_sub(self.menu_rect.y + 2) as usize;
        if row < self.menu_results.len() {
            self.menu_index = row;
            let action = self.menu_results[self.menu_index];
            self.menu_open = false;
            self.menu_query.clear();
            self.run_command_action(action)?;
        }
        Ok(())
    }

    pub(crate) fn handle_theme_browser_mouse(&mut self, mouse: MouseEvent) -> io::Result<()> {
        if Self::left_click_outside(mouse, self.theme_browser_rect) {
            self.active_theme_index = self.preview_revert_index;
            self.theme_index = self.preview_revert_index;
            self.theme_browser_open = false;
            self.menu_open = false;
            self.set_status(format!("Theme reverted: {}", self.active_theme().name));
            return Ok(());
        }
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                if self.theme_index + 1 < self.themes.len() {
                    self.theme_index += 1;
                    self.active_theme_index = self.theme_index;
                }
            }
            MouseEventKind::ScrollUp => {
                if self.theme_index > 0 {
                    self.theme_index -= 1;
                    self.active_theme_index = self.theme_index;
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let row = mouse.row.saturating_sub(self.theme_browser_rect.y + 1) as usize;
                if row < self.themes.len() {
                    self.theme_index = row;
                    self.active_theme_index = row;
                    self.persist_theme_selection();
                    self.theme_browser_open = false;
                    self.menu_open = false;
                    self.set_status(format!("Theme: {}", self.active_theme().name));
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn handle_context_menu_mouse(&mut self, mouse: MouseEvent) -> io::Result<()> {
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Right)) {
            // Reopen context menu on right-click so consecutive right-clicks retarget/reposition.
            self.context_menu.open = false;
            if inside(mouse.column, mouse.row, self.tree_rect) {
                self.open_tree_context_menu_at(mouse.column, mouse.row);
            }
            return Ok(());
        }
        if matches!(
            mouse.kind,
            MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left)
        ) {
            if inside(mouse.column, mouse.row, self.context_menu.rect) {
                let row = mouse.row.saturating_sub(self.context_menu.rect.y + 1) as usize;
                if row < context_actions().len() {
                    self.context_menu.index = row;
                }
            }
            return Ok(());
        }
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return Ok(());
        }
        if Self::left_click_outside(mouse, self.context_menu.rect) {
            self.context_menu.open = false;
            return Ok(());
        }
        let row = mouse.row.saturating_sub(self.context_menu.rect.y + 1) as usize;
        if row < context_actions().len() {
            self.context_menu.index = row;
            let action = context_actions()[row];
            self.apply_context_action(action)?;
        }
        Ok(())
    }

    pub(crate) fn handle_editor_context_menu_mouse(&mut self, mouse: MouseEvent) -> io::Result<()> {
        if matches!(
            mouse.kind,
            MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left)
        ) {
            if inside(mouse.column, mouse.row, self.editor_context_menu_rect) {
                let row = mouse
                    .row
                    .saturating_sub(self.editor_context_menu_rect.y + 1)
                    as usize;
                if row < editor_context_actions().len() {
                    self.editor_context_menu_index = row;
                }
            }
            return Ok(());
        }
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return Ok(());
        }
        if Self::left_click_outside(mouse, self.editor_context_menu_rect) {
            self.editor_context_menu_open = false;
            return Ok(());
        }
        let row = mouse
            .row
            .saturating_sub(self.editor_context_menu_rect.y + 1) as usize;
        if row < editor_context_actions().len() {
            self.editor_context_menu_index = row;
            let action = editor_context_actions()[row];
            self.apply_editor_context_action(action);
        }
        Ok(())
    }

    pub(crate) fn handle_search_results_mouse(&mut self, mouse: MouseEvent) -> io::Result<()> {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return Ok(());
        }
        if !inside(mouse.column, mouse.row, self.search_results_rect) {
            self.search_results.open = false;
            return Ok(());
        }
        let row = mouse.row.saturating_sub(self.search_results_rect.y + 1) as usize;
        if row < self.search_results.results.len() {
            self.search_results.index = row;
            self.open_selected_search_result()?;
        }
        Ok(())
    }
}
