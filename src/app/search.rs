use super::App;
use std::io;
use std::process::Command;

use crate::util::{parse_rg_line, relative_path, to_u16_saturating};

impl App {
    pub(crate) fn search_in_open_file(&mut self, query: &str) {
        if self.open_path().is_none() {
            self.set_status("Open a file first");
            return;
        }
        if query.trim().is_empty() {
            if let Some(tab) = self.active_tab_mut() {
                let _ = tab.editor.set_search_pattern("");
            }
            self.set_status("Find cleared");
            return;
        }
        let tab = &mut self.tabs[self.active_tab];
        match tab.editor.set_search_pattern(query) {
            Ok(()) => {
                if tab.editor.search_forward(true) {
                    self.set_status(format!("Find: {}", query));
                } else {
                    self.set_status(format!("No match: {}", query));
                }
            }
            Err(err) => {
                self.set_status(format!("Invalid regex: {}", err));
            }
        }
    }

    pub(crate) fn replace_in_open_file(&mut self, search: &str, replacement: &str) {
        if self.open_path().is_none() {
            self.set_status("Open a file first");
            return;
        }
        if search.is_empty() {
            self.set_status("Search pattern cannot be empty");
            return;
        }
        let mut lines = self.tabs[self.active_tab].editor.lines().to_vec();
        let mut count = 0usize;
        for line in &mut lines {
            let occurrences = line.matches(search).count();
            if occurrences > 0 {
                *line = line.replace(search, replacement);
                count += occurrences;
            }
        }
        if count > 0 {
            let cursor = self.tabs[self.active_tab].editor.cursor();
            self.replace_editor_text(lines, cursor);
            self.on_editor_content_changed();
            self.set_status(format!("Replaced {} occurrence(s)", count));
        } else {
            self.set_status(format!("No occurrences of '{}' found", search));
        }
    }

    pub(crate) fn search_in_project(&mut self, query: &str) {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            self.set_status("Search query is empty");
            return;
        }
        let output = Command::new("rg")
            .arg("--line-number")
            .arg("--no-heading")
            .arg("--color")
            .arg("never")
            .arg("--smart-case")
            .arg(trimmed)
            .arg(&self.root)
            .output();
        let Ok(output) = output else {
            self.set_status(
                "rg (ripgrep) not found -- install: https://github.com/BurntSushi/ripgrep#installation",
            );
            return;
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut hits = Vec::new();
        for line in stdout.lines() {
            if let Some(hit) = parse_rg_line(line) {
                hits.push(hit);
            }
        }
        self.search_results.query = trimmed.to_string();
        self.search_results.results = hits;
        self.search_results.index = 0;
        self.search_results.open = true;
        if self.search_results.results.is_empty() {
            self.set_status(format!("No results for '{}'", trimmed));
        } else {
            self.set_status(format!(
                "{} results for '{}'",
                self.search_results.results.len(),
                trimmed
            ));
        }
    }

    pub(crate) fn open_selected_search_result(&mut self) -> io::Result<()> {
        let Some(hit) = self
            .search_results
            .results
            .get(self.search_results.index)
            .cloned()
        else {
            return Ok(());
        };
        self.open_file(hit.path.clone())?;
        let target_row = hit.line.saturating_sub(1);
        if let Some(tab) = self.active_tab_mut() {
            tab.editor.move_cursor(ratatui_textarea::CursorMove::Jump(
                to_u16_saturating(target_row),
                0,
            ));
        }
        self.sync_editor_scroll_guess();
        self.search_results.open = false;
        self.set_status(format!(
            "Opened {}:{}",
            relative_path(&self.root, &hit.path).display(),
            hit.line
        ));
        Ok(())
    }
}
