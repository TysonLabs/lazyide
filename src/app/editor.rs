use super::App;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Style;
use serde_json::json;
use tui_textarea::TextArea;

use crate::keybinds::{KeyAction, KeyScope};
use crate::persistence::autosave_path_for;
use crate::syntax::syntax_lang_for_path;
use crate::tab::Tab;
use crate::types::{EditorContextAction, Focus};
use crate::util::{
    comment_prefix_for_path, compute_fold_ranges, compute_git_line_status, editor_context_actions,
    inside, leading_indent_bytes, relative_path, text_to_lines, to_u16_saturating,
};

impl App {
    pub(crate) fn duplicate_current_line(&mut self, above: bool) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let (row, col) = tab.editor.cursor();
        let mut lines = tab.editor.lines().to_vec();
        if lines.is_empty() || row >= lines.len() {
            self.set_status("No line to duplicate");
            return;
        }
        let line = lines[row].clone();
        let insert_at = if above { row } else { row + 1 };
        lines.insert(insert_at, line);
        let new_row = if above { row + 1 } else { row };
        self.replace_editor_text(lines, (new_row, col));
        self.on_editor_content_changed();
        if above {
            self.set_status("Duplicated line above");
        } else {
            self.set_status("Duplicated line below");
        }
    }

    pub(crate) fn toggle_comment(&mut self) {
        let Some(tab) = self.active_tab() else {
            self.set_status("No file open");
            return;
        };
        let Some(prefix) = comment_prefix_for_path(&tab.path) else {
            self.set_status("No comment style for file type");
            return;
        };
        let mut lines = tab.editor.lines().to_vec();
        let (start_row, end_row) = match tab.editor.selection_range() {
            Some(((s, _), (e, _))) => (s.min(e), s.max(e)),
            None => {
                let (row, _) = tab.editor.cursor();
                (row, row)
            }
        };
        if lines.is_empty() || start_row >= lines.len() {
            return;
        }
        let end_row = end_row.min(lines.len().saturating_sub(1));
        let mut all_commented = true;
        for line in lines.iter().take(end_row + 1).skip(start_row) {
            if line.trim().is_empty() {
                continue;
            }
            let indent = leading_indent_bytes(line);
            let rest = &line[indent..];
            if !rest.starts_with(prefix) {
                all_commented = false;
                break;
            }
        }
        for line in lines.iter_mut().take(end_row + 1).skip(start_row) {
            if line.trim().is_empty() {
                continue;
            }
            let indent = leading_indent_bytes(line);
            if all_commented {
                let rest = &line[indent..];
                let new_rest = if let Some(stripped) = rest.strip_prefix(&format!("{prefix} ")) {
                    stripped.to_string()
                } else if let Some(stripped) = rest.strip_prefix(prefix) {
                    stripped.to_string()
                } else {
                    rest.to_string()
                };
                *line = format!("{}{}", &line[..indent], new_rest);
            } else {
                *line = format!("{}{} {}", &line[..indent], prefix, &line[indent..]);
            }
        }
        let cursor = self.tabs[self.active_tab].editor.cursor();
        self.replace_editor_text(lines, cursor);
        self.on_editor_content_changed();
        self.set_status("Toggled comment");
    }

    pub(crate) fn dedent_lines(&mut self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let mut lines = tab.editor.lines().to_vec();
        let (start_row, end_row) = match tab.editor.selection_range() {
            Some(((s, _), (e, _))) => (s.min(e), s.max(e)),
            None => {
                let (row, _) = tab.editor.cursor();
                (row, row)
            }
        };
        if lines.is_empty() || start_row >= lines.len() {
            return;
        }
        let end_row = end_row.min(lines.len().saturating_sub(1));
        let mut changed = false;
        for line in lines.iter_mut().take(end_row + 1).skip(start_row) {
            if line.starts_with("    ") {
                *line = line[4..].to_string();
                changed = true;
            } else if line.starts_with('\t') {
                *line = line[1..].to_string();
                changed = true;
            } else {
                // Remove any leading spaces (less than 4)
                let spaces = line.len() - line.trim_start_matches(' ').len();
                if spaces > 0 {
                    *line = line[spaces..].to_string();
                    changed = true;
                }
            }
        }
        if changed {
            let (row, col) = self.tabs[self.active_tab].editor.cursor();
            let new_col = col.saturating_sub(4);
            self.replace_editor_text(lines, (row, new_col));
            self.on_editor_content_changed();
            self.set_status("Dedented");
        }
    }

    pub(crate) fn replace_editor_text(&mut self, lines: Vec<String>, cursor: (usize, usize)) {
        let mut ta = TextArea::from(lines);
        ta.set_cursor_line_style(Style::default().bg(self.active_theme().bg_alt));
        ta.set_selection_style(Style::default().bg(self.active_theme().selection));
        ta.move_cursor(tui_textarea::CursorMove::Jump(
            to_u16_saturating(cursor.0),
            to_u16_saturating(cursor.1),
        ));
        if let Some(tab) = self.active_tab_mut() {
            tab.editor = ta;
        }
        self.recompute_folds();
        self.sync_editor_scroll_guess();
    }

    pub(crate) fn copy_selection_to_clipboard(&mut self) {
        let Some(tab) = self.active_tab_mut() else {
            return;
        };
        if tab.editor.selection_range().is_none() {
            self.set_status("No selection to copy");
            return;
        }
        self.tabs[self.active_tab].editor.copy();
        let copied = self.tabs[self.active_tab].editor.yank_text();
        if copied.is_empty() {
            self.set_status("No selection to copy");
        } else if let Some(clipboard) = self.clipboard.as_mut() {
            match clipboard.set_text(copied) {
                Ok(()) => self.set_status("Copied"),
                Err(_) => self.set_status("Copied (internal clipboard only)"),
            }
        } else {
            self.set_status("Copied (internal clipboard only)");
        }
    }

    pub(crate) fn cut_line(&mut self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let (row, _col) = tab.editor.cursor();
        let lines = tab.editor.lines();
        if lines.is_empty() || row >= lines.len() {
            self.set_status("No line to cut");
            return;
        }
        let line_text = lines[row].to_string();
        let total_lines = lines.len();
        let is_last_line = row == total_lines - 1;

        // Select the entire line including its trailing newline, then cut via
        // TextArea so the deletion is recorded in the undo history.
        let tab = &mut self.tabs[self.active_tab];
        if is_last_line && row > 0 {
            // Last line: select from end of previous line through end of this line
            tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
                to_u16_saturating(row - 1),
                u16::MAX,
            ));
            tab.editor.start_selection();
            tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
                to_u16_saturating(row),
                u16::MAX,
            ));
        } else if total_lines == 1 {
            // Only one line: select all text on it
            tab.editor
                .move_cursor(tui_textarea::CursorMove::Jump(to_u16_saturating(row), 0));
            tab.editor.start_selection();
            tab.editor.move_cursor(tui_textarea::CursorMove::End);
        } else {
            // Select from start of this line to start of next line
            tab.editor
                .move_cursor(tui_textarea::CursorMove::Jump(to_u16_saturating(row), 0));
            tab.editor.start_selection();
            tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
                to_u16_saturating(row + 1),
                0,
            ));
        }
        tab.editor.cut();

        // Overwrite yank buffer and system clipboard with the clean line text
        if let Some(clipboard) = self.clipboard.as_mut() {
            let _ = clipboard.set_text(line_text.clone());
        }
        self.tabs[self.active_tab]
            .editor
            .set_yank_text(line_text);
        self.on_editor_content_changed();
        self.set_status("Cut line");
    }

    pub(crate) fn cut_selection_to_clipboard(&mut self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        if tab.editor.selection_range().is_none() {
            self.set_status("No selection to cut");
            return;
        }
        let modified = self.tabs[self.active_tab].editor.cut();
        if modified {
            self.on_editor_content_changed();
        }
        let cut = self.tabs[self.active_tab].editor.yank_text();
        if cut.is_empty() {
            self.set_status("No selection to cut");
        } else if let Some(clipboard) = self.clipboard.as_mut() {
            match clipboard.set_text(cut) {
                Ok(()) => self.set_status("Cut"),
                Err(_) => self.set_status("Cut (internal clipboard only)"),
            }
        } else {
            self.set_status("Cut (internal clipboard only)");
        }
    }

    /// Handle a bracketed paste event from the terminal. Inserts text
    /// directly into the editor, bypassing auto-pair logic.
    pub(crate) fn handle_paste(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        if self.active_tab_mut().is_none() {
            return;
        }
        let inserted = self
            .active_tab_mut()
            .is_some_and(|t| t.editor.insert_str(&text));
        if inserted {
            self.on_editor_content_changed();
            self.set_status("Pasted");
        }
    }

    pub(crate) fn paste_from_clipboard(&mut self) {
        let mut from_system = false;
        if let Some(clipboard) = self.clipboard.as_mut() {
            if let Ok(text) = clipboard.get_text() {
                if !text.is_empty() {
                    if let Some(tab) = self.active_tab_mut() {
                        tab.editor.set_yank_text(text);
                    }
                    from_system = true;
                }
            }
        }
        if self.active_tab_mut().is_some_and(|t| t.editor.paste()) {
            self.on_editor_content_changed();
            if from_system {
                self.set_status("Pasted");
            } else {
                self.set_status("Pasted (internal clipboard)");
            }
        } else {
            self.set_status("Clipboard empty");
        }
    }

    pub(crate) fn open_file(&mut self, path: PathBuf) -> io::Result<()> {
        self.open_file_as(path, false)
    }

    pub(crate) fn open_file_as(&mut self, path: PathBuf, as_preview: bool) -> io::Result<()> {
        // If file is already open in a tab, just switch to it
        if let Some(idx) = self.tabs.iter().position(|t| t.path == path) {
            self.switch_to_tab(idx);
            if !as_preview {
                self.tabs[idx].is_preview = false;
            }
            self.set_status(format!(
                "Switched to {}",
                relative_path(&self.root, &path).display()
            ));
            return Ok(());
        }

        let bytes = fs::read(&path)?;
        if bytes.iter().take(8192).any(|&b| b == 0) {
            self.set_status(format!(
                "Cannot open binary file: {}",
                relative_path(&self.root, &path).display()
            ));
            return Ok(());
        }
        let text = String::from_utf8_lossy(&bytes).to_string();
        let mut ta = TextArea::from(text_to_lines(&text));
        ta.set_cursor_line_style(Style::default().bg(self.active_theme().bg_alt));
        ta.set_selection_style(Style::default().bg(self.active_theme().selection));

        let lang = syntax_lang_for_path(Some(path.as_path()));
        let (fold_ranges, bracket_depths) = compute_fold_ranges(ta.lines(), lang);
        let mut visible_rows_map = Vec::new();
        let mut visible_row_starts = Vec::new();
        let mut visible_row_ends = Vec::new();
        for row in 0..ta.lines().len() {
            visible_rows_map.push(row);
            visible_row_starts.push(0);
            visible_row_ends.push(ta.lines()[row].chars().count());
        }
        if visible_rows_map.is_empty() {
            visible_rows_map.push(0);
            visible_row_starts.push(0);
            visible_row_ends.push(0);
        }

        let git_line_status = compute_git_line_status(&self.root, &path, ta.lines().len());

        let tab = Tab {
            path: path.clone(),
            is_preview: as_preview,
            editor: ta,
            dirty: false,
            open_disk_snapshot: Some(text),
            editor_scroll_row: 0,
            editor_scroll_col: 0,
            fold_ranges,
            bracket_depths,
            folded_starts: HashSet::new(),
            visible_rows_map,
            visible_row_starts,
            visible_row_ends,
            open_doc_uri: None,
            open_doc_version: 0,
            diagnostics: Vec::new(),
            conflict_prompt_open: false,
            conflict_disk_text: None,
            recovery_prompt_open: false,
            recovery_text: None,
            git_line_status,
        };

        // If opening as preview, replace existing preview tab
        if as_preview {
            if let Some(idx) = self.tabs.iter().position(|t| t.is_preview) {
                self.close_tab_at(idx);
                // Insert new tab at the same position
                self.tabs.insert(idx, tab);
                self.active_tab = idx;
            } else {
                self.tabs.push(tab);
                self.active_tab = self.tabs.len() - 1;
            }
        } else {
            self.tabs.push(tab);
            self.active_tab = self.tabs.len() - 1;
        }

        self.focus = Focus::Editor;
        self.completion.reset();
        self.ensure_lsp_for_path(&path);
        self.check_recovery_for_open_file();
        self.set_status(format!(
            "Opened {}",
            relative_path(&self.root, &path).display()
        ));
        Ok(())
    }

    pub(crate) fn save_file(&mut self) -> io::Result<()> {
        let Some(tab) = self.active_tab_mut() else {
            self.set_status("No file open");
            return Ok(());
        };
        let path = tab.path.clone();
        let mut content = tab.editor.lines().join("\n");
        // Ensure file ends with a trailing newline (POSIX convention)
        if !content.ends_with('\n') {
            content.push('\n');
        }
        fs::write(&path, &content)?;
        tab.dirty = false;
        tab.open_disk_snapshot = Some(content);
        tab.conflict_prompt_open = false;
        tab.conflict_disk_text = None;
        self.clear_autosave_for_open_file();
        // Trigger an immediate async git refresh so the gutter updates promptly
        self.fs_refresh_pending = true;
        self.fs_full_refresh_pending = true;
        self.last_fs_refresh = Instant::now()
            .checked_sub(Duration::from_millis(Self::FS_REFRESH_DEBOUNCE_MS + 1))
            .unwrap_or_else(Instant::now);
        self.set_status(format!(
            "Saved {}",
            relative_path(&self.root, &path).display()
        ));
        Ok(())
    }

    pub(crate) fn close_file(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        self.close_tab_at(self.active_tab);
    }

    pub(crate) fn close_tab_at(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        // Close LSP document for this tab
        let tab = &self.tabs[idx];
        if let (Some(uri), Some(lsp)) = (tab.open_doc_uri.clone(), self.lsp.as_ref()) {
            let _ = lsp.send_notification(
                "textDocument/didClose",
                json!({
                    "textDocument": { "uri": uri }
                }),
            );
        }
        // Clear autosave
        let _ = fs::remove_file(autosave_path_for(&self.tabs[idx].path));
        self.tabs.remove(idx);
        if self.tabs.is_empty() {
            self.active_tab = 0;
            self.focus = Focus::Tree;
            self.completion.reset();
            self.set_status("Closed file");
        } else if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        } else if self.active_tab > idx {
            self.active_tab -= 1;
        }
    }
    pub(crate) fn handle_help_key(&mut self, key: KeyEvent) -> io::Result<()> {
        let is_help_key = self.keybinds.lookup(&key, KeyScope::Global) == Some(KeyAction::Help);
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) => {
                self.help_open = false;
            }
            _ if is_help_key => {
                self.help_open = false;
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn handle_editor_context_menu_key(&mut self, key: KeyEvent) -> io::Result<()> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) => {
                self.editor_context_menu_open = false;
            }
            (_, KeyCode::Down) | (_, KeyCode::Char('j')) => {
                if self.editor_context_menu_index < editor_context_actions().len().saturating_sub(1)
                {
                    self.editor_context_menu_index += 1;
                }
            }
            (_, KeyCode::Up) | (_, KeyCode::Char('k')) => {
                if self.editor_context_menu_index > 0 {
                    self.editor_context_menu_index -= 1;
                }
            }
            (_, KeyCode::Enter) => {
                let action = editor_context_actions()[self.editor_context_menu_index];
                self.apply_editor_context_action(action);
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn apply_editor_context_action(&mut self, action: EditorContextAction) {
        self.editor_context_menu_open = false;
        self.focus = Focus::Editor;
        match action {
            EditorContextAction::Copy => self.copy_selection_to_clipboard(),
            EditorContextAction::Cut => self.cut_selection_to_clipboard(),
            EditorContextAction::Paste => self.paste_from_clipboard(),
            EditorContextAction::SelectAll => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.editor.select_all();
                }
                self.set_status("Selected all");
            }
            EditorContextAction::Cancel => {}
        }
    }

    pub(crate) fn sync_editor_scroll_guess(&mut self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let (cursor_row, cursor_col) = tab.editor.cursor();
        let inner_height = self.editor_rect.height.saturating_sub(2) as usize;
        if inner_height == 0 {
            if let Some(tab) = self.active_tab_mut() {
                tab.editor_scroll_row = 0;
            }
            return;
        }
        if self
            .active_tab()
            .is_some_and(|t| t.visible_rows_map.is_empty())
        {
            self.rebuild_visible_rows();
        }
        let cursor_visible = self.visible_index_of_source_position(cursor_row, cursor_col);
        let Some(tab) = self.active_tab_mut() else {
            return;
        };
        if cursor_visible < tab.editor_scroll_row {
            tab.editor_scroll_row = cursor_visible;
        } else if cursor_visible >= tab.editor_scroll_row + inner_height {
            tab.editor_scroll_row = cursor_visible.saturating_sub(inner_height.saturating_sub(1));
        }
        self.sync_editor_scroll_col();
    }

    pub(crate) fn sync_editor_scroll_col(&mut self) {
        if self.word_wrap {
            return;
        }
        let Some(tab) = self.active_tab() else {
            return;
        };
        let (cursor_row, cursor_col) = tab.editor.cursor();
        let content_width = self
            .editor_rect
            .width
            .saturating_sub(2)
            .saturating_sub(Self::EDITOR_GUTTER_WIDTH) as usize;
        if content_width == 0 {
            return;
        }
        // Compute cursor's display-width offset from start of line
        let line = tab
            .editor
            .lines()
            .get(cursor_row)
            .map(|l| l.replace('\t', "    "))
            .unwrap_or_default();
        let chars: Vec<char> = line.chars().collect();
        let mut cursor_display_col = 0usize;
        for i in 0..cursor_col.min(chars.len()) {
            cursor_display_col +=
                unicode_width::UnicodeWidthChar::width(chars[i]).unwrap_or(0);
        }
        let scroll_col = tab.editor_scroll_col;
        if cursor_display_col < scroll_col {
            if let Some(tab) = self.active_tab_mut() {
                tab.editor_scroll_col = cursor_display_col;
            }
        } else if cursor_display_col >= scroll_col + content_width {
            if let Some(tab) = self.active_tab_mut() {
                tab.editor_scroll_col = cursor_display_col.saturating_sub(content_width.saturating_sub(1));
            }
        }
    }

    /// After a scroll event, ensure the cursor stays within the visible
    /// viewport. This prevents `sync_editor_scroll_guess` from snapping
    /// the viewport back to the old cursor position on the next action.
    pub(crate) fn clamp_cursor_to_viewport(&mut self) {
        let inner_height = self.editor_rect.height.saturating_sub(2) as usize;
        if inner_height == 0 {
            return;
        }
        let Some(tab) = self.active_tab() else {
            return;
        };
        let (cursor_row, cursor_col) = tab.editor.cursor();
        let cursor_vis = self.visible_index_of_source_row(cursor_row);
        let Some(tab) = self.active_tab() else {
            return;
        };
        let scroll = tab.editor_scroll_row;
        let viewport_end = scroll + inner_height;

        // Cursor is already in the viewport — nothing to do.
        if cursor_vis >= scroll && cursor_vis < viewport_end {
            return;
        }

        // Pick the closest viewport edge.
        let target_vis = if cursor_vis < scroll {
            scroll
        } else {
            viewport_end.saturating_sub(1)
        };

        let target_row = tab
            .visible_rows_map
            .get(target_vis)
            .copied()
            .unwrap_or(cursor_row);

        if let Some(tab) = self.active_tab_mut() {
            tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
                to_u16_saturating(target_row),
                to_u16_saturating(cursor_col),
            ));
        }
    }

    fn page_move(&mut self, down: bool) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let inner_height = self.editor_rect.height.saturating_sub(2) as usize;
        if inner_height == 0 {
            return;
        }
        let (cursor_row, cursor_col) = tab.editor.cursor();
        let visible_rows = &tab.visible_rows_map;
        if visible_rows.is_empty() {
            return;
        }
        let cursor_vis = self.visible_index_of_source_position(cursor_row, cursor_col);
        let target_vis = if down {
            (cursor_vis + inner_height).min(visible_rows.len().saturating_sub(1))
        } else {
            cursor_vis.saturating_sub(inner_height)
        };
        let target_row = visible_rows[target_vis];
        let target_start_col = tab.visible_row_starts.get(target_vis).copied().unwrap_or(0);
        let target_end_col = tab
            .visible_row_ends
            .get(target_vis)
            .copied()
            .unwrap_or(target_start_col);
        let target_lines = self.active_tab().map_or(0, |t| {
            t.editor
                .lines()
                .get(target_row)
                .map_or(0, |l| l.chars().count())
        });
        let col = cursor_col
            .min(target_end_col)
            .max(target_start_col)
            .min(target_lines);
        if let Some(tab) = self.active_tab_mut() {
            tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
                to_u16_saturating(target_row),
                to_u16_saturating(col),
            ));
        }
        self.sync_editor_scroll_guess();
    }

    pub(crate) fn move_cursor_visual(&mut self, down: bool) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        if tab.visible_rows_map.is_empty() {
            return;
        }
        let (cursor_row, cursor_col) = tab.editor.cursor();
        let cursor_vis = self.visible_index_of_source_position(cursor_row, cursor_col);
        let target_vis = if down {
            (cursor_vis + 1).min(tab.visible_rows_map.len().saturating_sub(1))
        } else {
            cursor_vis.saturating_sub(1)
        };
        let target_row = tab
            .visible_rows_map
            .get(target_vis)
            .copied()
            .unwrap_or(cursor_row);
        let target_start = tab.visible_row_starts.get(target_vis).copied().unwrap_or(0);
        let target_end = tab
            .visible_row_ends
            .get(target_vis)
            .copied()
            .unwrap_or(target_start);
        let line_len = tab
            .editor
            .lines()
            .get(target_row)
            .map_or(0, |l| l.chars().count());
        let target_col = cursor_col.max(target_start).min(target_end).min(line_len);
        if let Some(tab) = self.active_tab_mut() {
            tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
                to_u16_saturating(target_row),
                to_u16_saturating(target_col),
            ));
        }
        self.sync_editor_scroll_guess();
    }

    pub(crate) fn page_down(&mut self) {
        self.page_move(true);
    }

    pub(crate) fn page_up(&mut self) {
        self.page_move(false);
    }

    pub(crate) fn editor_pos_from_mouse(&self, x: u16, y: u16) -> Option<(usize, usize)> {
        if !inside(x, y, self.editor_rect) {
            return None;
        }
        let tab = self.active_tab()?;
        let inner_x = x.saturating_sub(self.editor_rect.x.saturating_add(1)) as usize;
        let inner_y = y.saturating_sub(self.editor_rect.y.saturating_add(1)) as usize;
        let lines = tab.editor.lines();
        if lines.is_empty() {
            return Some((0, 0));
        }
        let visible_idx = tab.editor_scroll_row + inner_y;
        let row = tab
            .visible_rows_map
            .get(visible_idx)
            .copied()
            .unwrap_or_else(|| *tab.visible_rows_map.last().unwrap_or(&0));
        let seg_start = tab
            .visible_row_starts
            .get(visible_idx)
            .copied()
            .unwrap_or(0);
        let seg_end = tab
            .visible_row_ends
            .get(visible_idx)
            .copied()
            .unwrap_or(seg_start);
        let text_x = inner_x.saturating_sub(Self::EDITOR_GUTTER_WIDTH as usize);
        let max_col = lines[row].chars().count();
        // text_x is in screen columns; map to char index within the segment
        // by walking chars and accumulating display width.
        let display_line = lines[row].replace('\t', "    ");
        let chars: Vec<char> = display_line.chars().collect();
        // When not wrapping, offset text_x by editor_scroll_col so clicks
        // land on the correct character in the horizontally-scrolled view.
        let effective_text_x = if !self.word_wrap {
            text_x + tab.editor_scroll_col
        } else {
            text_x
        };
        let mut col = seg_start;
        let mut width_acc = 0usize;
        for i in seg_start..seg_end.min(chars.len()) {
            let cw = unicode_width::UnicodeWidthChar::width(chars[i]).unwrap_or(0);
            if width_acc + cw > effective_text_x {
                break;
            }
            width_acc += cw;
            col = i + 1;
        }
        let col = col.min(seg_end).min(max_col);
        Some((row, col))
    }
    pub(crate) fn select_line(&mut self, row: usize) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let lines = tab.editor.lines();
        let total = lines.len();
        if total == 0 || row >= total {
            return;
        }
        let line_len = lines[row].chars().count();
        // Start selection from end and move cursor to start, so the cursor
        // ends up at the beginning of the line rather than the next line.
        if let Some(tab) = self.active_tab_mut() {
            if row + 1 < total {
                tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
                    to_u16_saturating(row + 1),
                    0,
                ));
            } else {
                tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
                    to_u16_saturating(row),
                    to_u16_saturating(line_len),
                ));
            }
            tab.editor.start_selection();
            tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
                to_u16_saturating(row),
                0,
            ));
        }
        self.sync_editor_scroll_guess();
    }

    pub(crate) fn select_line_range(&mut self, from: usize, to: usize) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let lines = tab.editor.lines();
        let total = lines.len();
        if total == 0 {
            return;
        }
        let start = from.min(to).min(total.saturating_sub(1));
        let end = from.max(to).min(total.saturating_sub(1));
        // Select from start of first line to start of line after last (or end of last line)
        if let Some(tab) = self.active_tab_mut() {
            if end + 1 < total {
                tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
                    to_u16_saturating(end + 1),
                    0,
                ));
            } else {
                let line_len = tab
                    .editor
                    .lines()
                    .get(end)
                    .map_or(0, |l| l.chars().count());
                tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
                    to_u16_saturating(end),
                    to_u16_saturating(line_len),
                ));
            }
            tab.editor.start_selection();
            tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
                to_u16_saturating(start),
                0,
            ));
        }
        self.sync_editor_scroll_guess();
    }

    pub(crate) fn gutter_row_from_mouse(&self, y: u16) -> Option<usize> {
        let tab = self.active_tab()?;
        let inner_y = y.saturating_sub(self.editor_rect.y.saturating_add(1)) as usize;
        let visible_idx = tab.editor_scroll_row + inner_y;
        tab.visible_rows_map.get(visible_idx).copied()
    }

    pub(crate) fn extend_mouse_selection(&mut self, x: u16, y: u16) {
        if let (Some((anchor_row, anchor_col)), Some((row, col))) =
            (self.editor_drag_anchor, self.editor_pos_from_mouse(x, y))
        {
            if let Some(tab) = self.active_tab_mut() {
                tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
                    to_u16_saturating(anchor_row),
                    to_u16_saturating(anchor_col),
                ));
                tab.editor.start_selection();
                tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
                    to_u16_saturating(row),
                    to_u16_saturating(col),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn new_app(root: &std::path::Path) -> App {
        App::new(root.to_path_buf()).expect("app should initialize")
    }

    #[test]
    fn click_line_number_selects_line() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let file = root.join("test.txt");
        fs::write(&file, "line 0\nline 1\nline 2\n").expect("write");
        let mut app = new_app(root);
        app.open_file(file).expect("open");

        app.select_line(1);

        let tab = app.active_tab().expect("tab");
        let sel = tab.editor.selection_range().expect("should have selection");
        assert_eq!(sel, ((1, 0), (2, 0)));
        // Cursor should be at the start of the selected line
        assert_eq!(tab.editor.cursor(), (1, 0));
    }

    #[test]
    fn click_line_number_selects_last_line() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let file = root.join("test.txt");
        fs::write(&file, "line 0\nline 1\nlast line").expect("write");
        let mut app = new_app(root);
        app.open_file(file).expect("open");

        app.select_line(2);

        let tab = app.active_tab().expect("tab");
        let sel = tab.editor.selection_range().expect("should have selection");
        assert_eq!(sel, ((2, 0), (2, 9)));
    }

    #[test]
    fn click_line_number_selects_first_line() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let file = root.join("test.txt");
        fs::write(&file, "first\nsecond\n").expect("write");
        let mut app = new_app(root);
        app.open_file(file).expect("open");

        app.select_line(0);

        let tab = app.active_tab().expect("tab");
        let sel = tab.editor.selection_range().expect("should have selection");
        assert_eq!(sel, ((0, 0), (1, 0)));
    }

    #[test]
    fn cut_line_removes_middle_line() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let file = root.join("test.txt");
        fs::write(&file, "aaa\nbbb\nccc\n").expect("write");
        let mut app = new_app(root);
        app.open_file(file).expect("open");
        // Move cursor to line 1 (bbb)
        app.tabs[app.active_tab]
            .editor
            .move_cursor(tui_textarea::CursorMove::Jump(1, 0));
        app.cut_line();
        let lines = app.tabs[app.active_tab].editor.lines().to_vec();
        assert_eq!(lines, vec!["aaa", "ccc", ""]);
    }

    #[test]
    fn cut_line_removes_last_line() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let file = root.join("test.txt");
        fs::write(&file, "aaa\nbbb\n").expect("write");
        let mut app = new_app(root);
        app.open_file(file).expect("open");
        // Move cursor to last content line (index 2 is the empty trailing line)
        app.tabs[app.active_tab]
            .editor
            .move_cursor(tui_textarea::CursorMove::Jump(2, 0));
        app.cut_line();
        let lines = app.tabs[app.active_tab].editor.lines().to_vec();
        assert_eq!(lines, vec!["aaa", "bbb"]);
        // Cursor should be clamped, not out of bounds
        let (row, _) = app.tabs[app.active_tab].editor.cursor();
        assert!(row < lines.len());
    }
}
