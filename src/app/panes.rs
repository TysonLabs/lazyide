//! Two-pane editor support. The focused pane's tab group lives directly on
//! `App` (`tabs`, `active_tab`, `editor_rect`, ...) so every editor operation
//! keeps working unchanged; the other pane's group and geometry sit in
//! `App::other_pane`, and focusing it swaps the two. A file is open in one
//! pane at a time.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

use super::{App, Pane, SplitDirection};
use crate::tab::Tab;
use crate::types::Focus;
use crate::util::relative_path;

impl App {
    /// Percent of the editor column given to the first (left/top) pane.
    pub(crate) const DEFAULT_SPLIT_RATIO: u16 = 50;
    pub(crate) const MIN_SPLIT_RATIO: u16 = 15;

    /// Swap the focused pane's state with `other_pane` without changing which
    /// slot is focused. Used by rendering to draw the other pane through the
    /// same code path, and by `focus_other_pane`.
    pub(crate) fn swap_pane_state(&mut self) {
        let Some(other) = self.other_pane.as_mut() else {
            return;
        };
        std::mem::swap(&mut self.tabs, &mut other.tabs);
        std::mem::swap(&mut self.active_tab, &mut other.active_tab);
        std::mem::swap(&mut self.editor_rect, &mut other.editor_rect);
        std::mem::swap(&mut self.tab_bar_rect, &mut other.tab_bar_rect);
        std::mem::swap(&mut self.tab_rects, &mut other.tab_rects);
        std::mem::swap(&mut self.wrap_width_cache, &mut other.wrap_width_cache);
    }

    pub(crate) fn is_split(&self) -> bool {
        self.other_pane.is_some()
    }

    pub(crate) fn focus_other_pane(&mut self) {
        if !self.is_split() {
            self.set_status("Only one pane open");
            return;
        }
        self.swap_pane_state();
        self.focused_pane_first = !self.focused_pane_first;
        self.completion.reset();
        self.editor_dragging = false;
        self.editor_drag_anchor = None;
        self.gutter_drag_anchor = None;
        self.focus = Focus::Editor;
        let name = self
            .open_path()
            .map(|p| relative_path(&self.root, p).display().to_string())
            .unwrap_or_else(|| "empty pane".to_string());
        self.set_status(format!("Focus: {name}"));
    }

    pub(crate) fn split_pane(&mut self, direction: SplitDirection) {
        if self.is_split() {
            self.split = Some(direction);
            self.set_status(match direction {
                SplitDirection::Vertical => "Panes arranged side by side",
                SplitDirection::Horizontal => "Panes arranged top and bottom",
            });
            return;
        }
        self.other_pane = Some(Pane::default());
        self.split = Some(direction);
        self.focused_pane_first = true;
        self.split_ratio = Self::DEFAULT_SPLIT_RATIO;
        self.set_status(match direction {
            SplitDirection::Vertical => "Split vertically; new pane on the right",
            SplitDirection::Horizontal => "Split horizontally; new pane below",
        });
    }

    /// Close the focused pane, moving its tabs into the remaining pane.
    pub(crate) fn close_pane(&mut self) {
        let Some(mut other) = self.other_pane.take() else {
            self.set_status("Only one pane open");
            return;
        };
        // The remaining pane keeps its own tabs first, then adopts ours.
        let mut mine = std::mem::take(&mut self.tabs);
        for tab in &mut mine {
            tab.visible_rows_map.clear();
        }
        let adopted_active = other.tabs.len() + self.active_tab;
        let had_mine = !mine.is_empty();
        other.tabs.append(&mut mine);
        self.tabs = other.tabs;
        self.active_tab = if had_mine {
            adopted_active.min(self.tabs.len().saturating_sub(1))
        } else {
            other.active_tab.min(self.tabs.len().saturating_sub(1))
        };
        self.wrap_width_cache = 0;
        self.split = None;
        self.focused_pane_first = true;
        self.split_dragging = false;
        self.completion.reset();
        for tab in &mut self.tabs {
            tab.visible_rows_map.clear();
        }
        self.set_status("Pane closed");
    }

    /// Move the active tab into the other pane and focus it there.
    pub(crate) fn move_tab_to_other_pane(&mut self) {
        if !self.is_split() {
            self.set_status("Split the editor first");
            return;
        }
        if self.tabs.is_empty() {
            self.set_status("No tab to move");
            return;
        }
        let idx = self.active_tab.min(self.tabs.len() - 1);
        let mut tab = self.tabs.remove(idx);
        tab.visible_rows_map.clear();
        if self.tabs.is_empty() {
            self.active_tab = 0;
        } else if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        let name = relative_path(&self.root, &tab.path).display().to_string();
        if let Some(other) = self.other_pane.as_mut() {
            other.tabs.push(tab);
            other.active_tab = other.tabs.len() - 1;
        }
        self.focus_other_pane();
        self.set_status(format!("Moved {name} to the other pane"));
    }

    /// Index of a tab open in the other pane, if any.
    pub(crate) fn other_pane_tab_index(&self, path: &std::path::Path) -> Option<usize> {
        self.other_pane
            .as_ref()
            .and_then(|p| p.tabs.iter().position(|t| t.path == path))
    }

    pub(crate) fn all_tabs(&self) -> impl Iterator<Item = &Tab> {
        self.tabs
            .iter()
            .chain(self.other_pane.iter().flat_map(|p| p.tabs.iter()))
    }

    pub(crate) fn all_tabs_mut(&mut self) -> impl Iterator<Item = &mut Tab> {
        self.tabs
            .iter_mut()
            .chain(self.other_pane.iter_mut().flat_map(|p| p.tabs.iter_mut()))
    }

    /// Rebuild wrapped/folded row maps for every tab in both panes.
    pub(crate) fn rebuild_all_visible_rows_all_panes(&mut self) {
        self.rebuild_all_visible_rows();
        if self.is_split() {
            self.swap_pane_state();
            self.rebuild_all_visible_rows();
            self.swap_pane_state();
        }
    }

    /// Split the editor column into (first, second, divider) rects.
    pub(crate) fn pane_layout(&self, column: Rect) -> (Rect, Rect, Rect) {
        let ratio = self
            .split_ratio
            .clamp(Self::MIN_SPLIT_RATIO, 100 - Self::MIN_SPLIT_RATIO);
        match self.split.unwrap_or(SplitDirection::Vertical) {
            SplitDirection::Vertical => {
                let parts = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(ratio), Constraint::Min(1)])
                    .split(column);
                let divider = Rect::new(parts[1].x, parts[1].y, 1, parts[1].height);
                (parts[0], parts[1], divider)
            }
            SplitDirection::Horizontal => {
                let parts = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Percentage(ratio), Constraint::Min(3)])
                    .split(column);
                // The second pane's tab row doubles as the drag handle.
                let divider = Rect::new(parts[1].x, parts[1].y, parts[1].width, 1);
                (parts[0], parts[1], divider)
            }
        }
    }

    /// Update the split ratio from a mouse position inside the editor column.
    pub(crate) fn drag_split_to(&mut self, column: u16, row: u16) {
        let col_rect = self.editor_column_rect;
        let (pos, total) = match self.split {
            Some(SplitDirection::Horizontal) => (row.saturating_sub(col_rect.y), col_rect.height),
            _ => (column.saturating_sub(col_rect.x), col_rect.width),
        };
        if total == 0 {
            return;
        }
        let pct = (u32::from(pos) * 100 / u32::from(total)) as u16;
        self.split_ratio = pct.clamp(Self::MIN_SPLIT_RATIO, 100 - Self::MIN_SPLIT_RATIO);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn app_with_files(root: &Path, names: &[&str]) -> App {
        for n in names {
            fs::write(root.join(n), format!("// {n}\n")).expect("write");
        }
        App::new(root.to_path_buf()).expect("app")
    }

    #[test]
    fn split_focus_and_close_round_trip_tabs() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let mut app = app_with_files(root, &["a.rs", "b.rs"]);
        app.open_file(root.join("a.rs")).unwrap();
        app.open_file(root.join("b.rs")).unwrap();
        assert_eq!(app.tabs.len(), 2);

        app.split_pane(SplitDirection::Vertical);
        assert!(app.is_split());
        assert!(app.focused_pane_first);

        // New pane is empty; focusing it swaps the groups.
        app.focus_other_pane();
        assert!(app.tabs.is_empty());
        assert!(!app.focused_pane_first);
        assert_eq!(app.other_pane.as_ref().unwrap().tabs.len(), 2);

        // Opening a file in the empty pane lands there.
        fs::write(root.join("c.rs"), "// c\n").unwrap();
        app.open_file(root.join("c.rs")).unwrap();
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.all_tabs().count(), 3);

        // Closing the focused pane merges everything into one group.
        app.close_pane();
        assert!(!app.is_split());
        assert_eq!(app.tabs.len(), 3);
        assert_eq!(app.tabs[app.active_tab].path, root.join("c.rs"));
    }

    #[test]
    fn move_tab_to_other_pane_focuses_it_there() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let mut app = app_with_files(root, &["a.rs", "b.rs"]);
        app.open_file(root.join("a.rs")).unwrap();
        app.open_file(root.join("b.rs")).unwrap();
        app.split_pane(SplitDirection::Horizontal);

        app.move_tab_to_other_pane();
        // Now focused on the other pane, which holds b.rs.
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].path, root.join("b.rs"));
        assert_eq!(app.other_pane.as_ref().unwrap().tabs.len(), 1);
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn open_file_already_in_other_pane_switches_focus_instead_of_duplicating() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let mut app = app_with_files(root, &["a.rs"]);
        app.open_file(root.join("a.rs")).unwrap();
        app.split_pane(SplitDirection::Vertical);
        app.focus_other_pane();
        assert!(app.tabs.is_empty());

        app.open_file(root.join("a.rs")).unwrap();
        assert_eq!(app.all_tabs().count(), 1, "file must not be opened twice");
        assert_eq!(app.tabs.len(), 1);
        assert!(app.focused_pane_first);
    }

    #[test]
    fn pane_layout_respects_ratio_bounds() {
        let tmp = tempdir().unwrap();
        let mut app = App::new(tmp.path().to_path_buf()).unwrap();
        app.split = Some(SplitDirection::Vertical);
        app.split_ratio = 1; // clamped up to MIN_SPLIT_RATIO
        let (first, second, divider) = app.pane_layout(Rect::new(0, 0, 100, 40));
        assert_eq!(first.width, App::MIN_SPLIT_RATIO);
        assert_eq!(second.x, first.width);
        assert_eq!(divider, Rect::new(second.x, 0, 1, 40));

        app.split = Some(SplitDirection::Horizontal);
        app.split_ratio = 50;
        let (first, second, divider) = app.pane_layout(Rect::new(0, 0, 100, 40));
        assert_eq!(first.height, 20);
        assert_eq!(second.y, 20);
        assert_eq!(divider, Rect::new(0, 20, 100, 1));
    }

    #[test]
    fn close_pane_without_split_is_a_noop() {
        let tmp = tempdir().unwrap();
        let mut app = App::new(tmp.path().to_path_buf()).unwrap();
        app.close_pane();
        app.focus_other_pane();
        app.move_tab_to_other_pane();
        assert!(!app.is_split());
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::fs;
    use tempfile::tempdir;

    fn draw(app: &mut App) {
        let backend = TestBackend::new(140, 45);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| crate::ui::draw(app, f)).unwrap();
    }

    #[test]
    fn move_only_tab_then_render_does_not_panic() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
        let mut app = App::new(root.to_path_buf()).unwrap();
        app.open_file(root.join("a.rs")).unwrap();
        draw(&mut app);
        app.split_pane(SplitDirection::Horizontal);
        draw(&mut app);
        app.move_tab_to_other_pane();
        draw(&mut app);
        draw(&mut app);
        app.focus_other_pane();
        draw(&mut app);
        app.close_pane();
        draw(&mut app);
    }
}
