use super::App;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::tree_item::TreeItem;
use crate::types::{ContextAction, PendingAction, PromptMode, PromptState};
use crate::util::{
    collect_all_files, fuzzy_score, primary_mod_label, relative_path, to_u16_saturating,
};

impl App {
    pub(crate) fn rebuild_tree(&mut self) -> io::Result<()> {
        let selected_path = self.tree.get(self.selected).map(|i| i.path.clone());
        let mut out = Vec::new();
        self.walk_dir(&self.root, 0, &mut out)?;
        if out.is_empty() {
            out.push(TreeItem {
                path: self.root.clone(),
                name: self.root.display().to_string(),
                depth: 0,
                is_dir: true,
                expanded: true,
            });
        }
        self.tree = out;
        self.selected = selected_path
            .and_then(|p| self.tree.iter().position(|i| i.path == p))
            .unwrap_or(0);
        Ok(())
    }

    pub(crate) fn walk_dir(
        &self,
        dir: &Path,
        depth: usize,
        out: &mut Vec<TreeItem>,
    ) -> io::Result<()> {
        let is_root = dir == self.root;
        let name = if is_root {
            dir.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| dir.display().to_string())
        } else {
            dir.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| dir.display().to_string())
        };
        let expanded = self.expanded.contains(dir);
        out.push(TreeItem {
            path: dir.to_path_buf(),
            name,
            depth,
            is_dir: true,
            expanded,
        });
        if !expanded {
            return Ok(());
        }

        let mut entries: Vec<_> = fs::read_dir(dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        entries.sort_by_key(|p| {
            (
                !p.is_dir(),
                p.file_name()
                    .map(|s| s.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default(),
            )
        });

        for path in entries {
            let Ok(ft) = fs::symlink_metadata(&path).map(|m| m.file_type()) else {
                continue;
            };
            // Avoid following directory symlink cycles.
            if ft.is_symlink() {
                continue;
            }
            let is_dir = ft.is_dir();
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());
            if is_dir {
                self.walk_dir(&path, depth + 1, out)?;
            } else {
                out.push(TreeItem {
                    path,
                    name,
                    depth: depth + 1,
                    is_dir: false,
                    expanded: false,
                });
            }
        }
        Ok(())
    }

    pub(crate) fn selected_item(&self) -> Option<&TreeItem> {
        self.tree.get(self.selected)
    }

    pub(crate) fn set_status<S: Into<String>>(&mut self, status: S) {
        self.status = status.into();
    }

    pub(crate) fn refresh_file_picker_results(&mut self) {
        let query = self.file_picker_query.to_ascii_lowercase();
        let mut all_files = Vec::new();
        collect_all_files(&self.root, &mut all_files);
        let mut scored: Vec<(usize, PathBuf)> = all_files
            .into_iter()
            .filter_map(|path| {
                let rel = relative_path(&self.root, &path).display().to_string();
                fuzzy_score(&query, &rel).map(|score| (score, path))
            })
            .collect();
        scored.sort_by(|(sa, pa), (sb, pb)| {
            sa.cmp(sb)
                .then_with(|| pa.as_os_str().len().cmp(&pb.as_os_str().len()))
        });
        self.file_picker_results = scored.into_iter().map(|(_, p)| p).take(200).collect();
        self.file_picker_index = self
            .file_picker_index
            .min(self.file_picker_results.len().saturating_sub(1));
    }

    pub(crate) fn open_file_picker_selection(&mut self) -> io::Result<()> {
        let Some(path) = self
            .file_picker_results
            .get(self.file_picker_index)
            .cloned()
        else {
            return Ok(());
        };
        self.file_picker_open = false;
        self.file_picker_query.clear();
        self.open_file(path)?;
        Ok(())
    }
    pub(crate) fn tree_activate_selected(&mut self) -> io::Result<()> {
        self.tree_activate_selected_as(false)
    }

    pub(crate) fn tree_activate_selected_as(&mut self, as_preview: bool) -> io::Result<()> {
        let Some(item) = self.selected_item().cloned() else {
            return Ok(());
        };
        if item.is_dir {
            if self.expanded.contains(&item.path) {
                self.expanded.remove(&item.path);
            } else {
                self.expanded.insert(item.path.clone());
            }
            self.rebuild_tree()?;
            self.set_status(format!("Directory: {}", item.path.display()));
        } else {
            self.open_file_as(item.path.clone(), as_preview)?;
        }
        Ok(())
    }

    pub(crate) fn tree_collapse_or_parent(&mut self) {
        let Some(item) = self.selected_item().cloned() else {
            return;
        };
        if item.is_dir && self.expanded.contains(&item.path) {
            self.expanded.remove(&item.path);
            let _ = self.rebuild_tree();
            return;
        }
        if let Some(parent) = item.path.parent() {
            if let Some(idx) = self.tree.iter().position(|i| i.path == parent) {
                self.selected = idx;
            }
        }
    }

    pub(crate) fn delete_path(&mut self, path: PathBuf) -> io::Result<()> {
        if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
        // Close any tab that has this path open
        if let Some(idx) = self.tabs.iter().position(|t| t.path == path) {
            self.close_tab_at(idx);
        }
        self.rebuild_tree()?;
        self.set_status(format!("Deleted {}", path.display()));
        Ok(())
    }

    pub(crate) fn create_new_file(&mut self) -> io::Result<()> {
        let base = self
            .selected_item()
            .map(|i| i.path.clone())
            .unwrap_or_else(|| self.root.clone());
        let parent = if base.is_dir() {
            base
        } else {
            base.parent().unwrap_or(&self.root).to_path_buf()
        };
        let mut n = 1usize;
        loop {
            let candidate = parent.join(format!("new_file_{n}.txt"));
            if !candidate.exists() {
                fs::write(&candidate, b"")?;
                self.rebuild_tree()?;
                self.set_status(format!(
                    "Created {}",
                    relative_path(&self.root, &candidate).display()
                ));
                return Ok(());
            }
            n += 1;
        }
    }

    pub(crate) fn apply_prompt(&mut self, mode: PromptMode, value: String) -> io::Result<()> {
        match mode {
            PromptMode::NewFile { parent } => {
                let target = parent.join(value);
                if target.exists() {
                    self.set_status("File already exists");
                    return Ok(());
                }
                fs::write(&target, b"")?;
                self.rebuild_tree()?;
                self.set_status(format!(
                    "Created {}",
                    relative_path(&self.root, &target).display()
                ));
            }
            PromptMode::NewFolder { parent } => {
                let target = parent.join(value);
                if target.exists() {
                    self.set_status("Folder already exists");
                    return Ok(());
                }
                fs::create_dir_all(&target)?;
                self.expanded.insert(target.clone());
                self.rebuild_tree()?;
                self.set_status(format!(
                    "Created {}",
                    relative_path(&self.root, &target).display()
                ));
            }
            PromptMode::Rename { target } => {
                let Some(parent) = target.parent() else {
                    self.set_status("Cannot rename root");
                    return Ok(());
                };
                let renamed = parent.join(value);
                if renamed.exists() {
                    self.set_status("Name already exists");
                    return Ok(());
                }
                fs::rename(&target, &renamed)?;
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.path == target) {
                    tab.path = renamed.clone();
                }
                self.rebuild_tree()?;
                self.set_status(format!(
                    "Renamed to {}",
                    relative_path(&self.root, &renamed).display()
                ));
            }
            PromptMode::FindInFile => {
                self.search_in_open_file(&value);
                if self.replace_after_find && !value.is_empty() {
                    self.replace_after_find = false;
                    self.prompt = Some(PromptState {
                        title: format!("Replace '{}' with", value),
                        value: String::new(),
                        mode: PromptMode::ReplaceInFile { search: value },
                    });
                }
            }
            PromptMode::FindInProject => {
                self.search_in_project(&value);
            }
            PromptMode::ReplaceInFile { search } => {
                self.replace_in_open_file(&search, &value);
            }
            PromptMode::GoToLine => {
                if let Ok(line_num) = value.parse::<usize>() {
                    if line_num == 0 {
                        self.set_status("Line number must be >= 1");
                        return Ok(());
                    }
                    let target = line_num.saturating_sub(1);
                    if let Some(tab) = self.active_tab_mut() {
                        let max_line = tab.editor.lines().len().saturating_sub(1);
                        let clamped = target.min(max_line);
                        tab.editor.cancel_selection();
                        tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
                            to_u16_saturating(clamped),
                            0,
                        ));
                    }
                    self.sync_editor_scroll_guess();
                    self.set_status(format!("Jumped to line {}", target + 1));
                } else {
                    self.set_status("Invalid line number");
                }
            }
        }
        Ok(())
    }

    pub(crate) fn apply_context_action(&mut self, action: ContextAction) -> io::Result<()> {
        let target = self.context_menu.target.clone();
        self.context_menu.open = false;
        let Some(target) = target else {
            return Ok(());
        };
        match action {
            ContextAction::Open => {
                if let Some(idx) = self.tree.iter().position(|i| i.path == target) {
                    self.selected = idx;
                }
                self.tree_activate_selected()?;
            }
            ContextAction::NewFile => {
                let parent = if target.is_dir() {
                    target
                } else {
                    target.parent().unwrap_or(&self.root).to_path_buf()
                };
                self.prompt = Some(PromptState {
                    title: format!(
                        "New file in {}",
                        relative_path(&self.root, &parent).display()
                    ),
                    value: String::new(),
                    mode: PromptMode::NewFile { parent },
                });
            }
            ContextAction::NewFolder => {
                let parent = if target.is_dir() {
                    target
                } else {
                    target.parent().unwrap_or(&self.root).to_path_buf()
                };
                self.prompt = Some(PromptState {
                    title: format!(
                        "New folder in {}",
                        relative_path(&self.root, &parent).display()
                    ),
                    value: String::new(),
                    mode: PromptMode::NewFolder { parent },
                });
            }
            ContextAction::Rename => {
                let default_name = target
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.prompt = Some(PromptState {
                    title: "Rename to".to_string(),
                    value: default_name,
                    mode: PromptMode::Rename { target },
                });
            }
            ContextAction::Delete => {
                self.pending = PendingAction::Delete(target.clone());
                self.set_status(format!(
                    "Delete {} ? Press {}+D to confirm.",
                    target
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| target.display().to_string()),
                    primary_mod_label()
                ));
            }
            ContextAction::Cancel => {}
        }
        Ok(())
    }
}
