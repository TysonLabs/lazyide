use std::io;

use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui_textarea::CursorMove;

use super::App;
use crate::diff::{DiffOutcome, git_diff_for_file, next_change_row};
use crate::util::{inside, relative_path, to_u16_saturating};

impl App {
    /// Open the side-by-side diff of the active file against HEAD.
    pub(crate) fn open_diff_view(&mut self) {
        let Some(path) = self.open_path().cloned() else {
            self.set_status("No file open to diff");
            return;
        };
        let diff = match git_diff_for_file(&self.root, &path) {
            DiffOutcome::Diff(d) => d,
            DiffOutcome::Binary => {
                self.set_status("Binary file changed; no text diff to show");
                return;
            }
            DiffOutcome::Clean => {
                self.set_status("No changes against HEAD");
                return;
            }
        };
        let rel = relative_path(&self.root, &path).display().to_string();
        let note = if self.is_dirty() {
            "  (unsaved edits not included)"
        } else {
            ""
        };
        self.set_status(format!(
            "Diff {rel}: +{} -{} in {} hunk(s){note}",
            diff.added,
            diff.removed,
            diff.hunks.len()
        ));
        self.diff_view.path = Some(path);
        self.diff_view.diff = diff;
        self.diff_view.scroll = 0;
        self.diff_view.hunk_index = 0;
        self.diff_view.open = true;
    }

    fn diff_view_page(&self) -> usize {
        (self.diff_view.rect.height.saturating_sub(2) as usize).max(1)
    }

    fn diff_view_max_scroll(&self) -> usize {
        self.diff_view
            .diff
            .rows
            .len()
            .saturating_sub(self.diff_view_page())
    }

    fn diff_view_go_to_hunk(&mut self, index: usize) {
        if let Some(h) = self.diff_view.diff.hunks.get(index) {
            self.diff_view.hunk_index = index;
            self.diff_view.scroll = h.start_row.min(self.diff_view_max_scroll());
        }
    }

    /// Clamp the scroll offset and point `hunk_index` at the hunk whose
    /// header is at or above the top of the viewport.
    fn diff_view_sync(&mut self) {
        let max = self.diff_view_max_scroll();
        if self.diff_view.scroll > max {
            self.diff_view.scroll = max;
        }
        let scroll = self.diff_view.scroll;
        if let Some(i) = self
            .diff_view
            .diff
            .hunks
            .iter()
            .rposition(|h| h.start_row <= scroll)
        {
            self.diff_view.hunk_index = i;
        }
    }

    pub(crate) fn handle_diff_view_key(&mut self, key: KeyEvent) -> io::Result<()> {
        let max = self.diff_view_max_scroll();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.diff_view.open = false;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.diff_view.scroll = (self.diff_view.scroll + 1).min(max);
                self.diff_view_sync();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.diff_view.scroll = self.diff_view.scroll.saturating_sub(1);
                self.diff_view_sync();
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                let page = self.diff_view_page();
                self.diff_view.scroll = (self.diff_view.scroll + page).min(max);
                self.diff_view_sync();
            }
            KeyCode::PageUp => {
                let page = self.diff_view_page();
                self.diff_view.scroll = self.diff_view.scroll.saturating_sub(page);
                self.diff_view_sync();
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.diff_view.scroll = 0;
                self.diff_view_sync();
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.diff_view.scroll = max;
                self.diff_view_sync();
            }
            KeyCode::Char('n') | KeyCode::Char(']') => {
                let next = self.diff_view.hunk_index + 1;
                if next < self.diff_view.diff.hunks.len() {
                    self.diff_view_go_to_hunk(next);
                }
            }
            KeyCode::Char('p') | KeyCode::Char('[') => {
                if self.diff_view.hunk_index > 0 {
                    let prev = self.diff_view.hunk_index - 1;
                    self.diff_view_go_to_hunk(prev);
                }
            }
            KeyCode::Enter => {
                let target = self
                    .diff_view
                    .diff
                    .hunks
                    .get(self.diff_view.hunk_index)
                    .map(|h| h.new_start.saturating_sub(1));
                self.diff_view.open = false;
                if let Some(row) = target {
                    self.jump_editor_to_row(row);
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn handle_diff_view_mouse(&mut self, mouse: MouseEvent) -> io::Result<()> {
        let max = self.diff_view_max_scroll();
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                self.diff_view.scroll = (self.diff_view.scroll + 3).min(max);
                self.diff_view_sync();
            }
            MouseEventKind::ScrollUp => {
                self.diff_view.scroll = self.diff_view.scroll.saturating_sub(3);
                self.diff_view_sync();
            }
            MouseEventKind::Down(MouseButton::Left)
                if !inside(mouse.column, mouse.row, self.diff_view.rect) =>
            {
                self.diff_view.open = false;
            }
            _ => {}
        }
        Ok(())
    }

    /// Move the editor cursor to the start of the next or previous changed
    /// region, based on the gutter's git line status.
    pub(crate) fn jump_to_change(&mut self, forward: bool) {
        let Some(tab) = self.active_tab() else {
            self.set_status("No file open");
            return;
        };
        let from = tab.editor.cursor().0;
        match next_change_row(&tab.git_line_status, from, forward) {
            Some(row) => {
                self.jump_editor_to_row(row);
                self.set_status(format!("Change at line {}", row + 1));
            }
            None => self.set_status(if forward {
                "No more changes below"
            } else {
                "No more changes above"
            }),
        }
    }

    fn jump_editor_to_row(&mut self, row: usize) {
        if let Some(tab) = self.active_tab_mut() {
            tab.editor
                .move_cursor(CursorMove::Jump(to_u16_saturating(row), 0));
        }
        self.focus = crate::types::Focus::Editor;
        self.sync_editor_scroll_guess();
    }
}
