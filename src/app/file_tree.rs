use super::App;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::tree_item::TreeItem;
use crate::types::{ContextAction, PendingAction, PromptMode, PromptState};
use crate::util::{
    collect_all_files, fuzzy_score, normalize_lexically, relative_path, to_u16_saturating,
};

impl App {
    fn sanitize_entry_name<'a>(&self, value: &'a str) -> Result<&'a str, &'static str> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err("Name cannot be empty");
        }
        let mut components = Path::new(trimmed).components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(_)), None) => Ok(trimmed),
            _ => Err("Name must be a single path component"),
        }
    }

    /// Resolve the user-entered destination folder for a move. Relative paths
    /// are taken from the project root; absolute paths are allowed so items can
    /// leave the project. The returned path is lexically normalized but NOT
    /// canonicalized, so it stays comparable with `self.root` and tree paths
    /// (the root itself may sit behind a symlink, e.g. macOS `/var`).
    fn resolve_move_destination(&self, target: &Path, value: &str) -> Result<PathBuf, String> {
        let trimmed = value.trim();
        let raw = if trimmed.is_empty() || trimmed == "." {
            self.root.clone()
        } else {
            self.root.join(trimmed)
        };
        let dest = normalize_lexically(&raw);
        if !dest.is_dir() {
            return Err(if dest.exists() {
                "Destination is not a folder".to_string()
            } else {
                "Destination folder does not exist".to_string()
            });
        }
        // Canonical forms are used only for identity comparisons.
        let dest_canon = dest.canonicalize().unwrap_or_else(|_| dest.clone());
        let current_parent = target
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .unwrap_or_else(|| self.root.clone());
        if dest_canon == current_parent {
            return Err("Already in that folder".to_string());
        }
        if target.is_dir()
            && let Ok(target_canon) = target.canonicalize()
            && dest_canon.starts_with(&target_canon)
        {
            return Err("Cannot move a folder into itself".to_string());
        }
        Ok(dest)
    }

    fn close_tabs_for_path_prefix(&mut self, path: &Path) {
        let mut indices: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(idx, tab)| {
                if tab.path == path || tab.path.starts_with(path) {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect();
        indices.sort_unstable();
        indices.reverse();
        for idx in indices {
            self.close_tab_at(idx);
        }
    }

    fn retarget_tabs_for_rename(&mut self, from: &Path, to: &Path) {
        for tab in &mut self.tabs {
            if tab.path == from {
                tab.path = to.to_path_buf();
                continue;
            }
            if let Ok(suffix) = tab.path.strip_prefix(from) {
                tab.path = to.join(suffix);
            }
        }
    }

    fn retarget_expanded_for_rename(&mut self, from: &Path, to: &Path) {
        let mut moved = Vec::new();
        for p in &self.expanded {
            if p == from {
                moved.push((p.clone(), to.to_path_buf()));
                continue;
            }
            if let Ok(suffix) = p.strip_prefix(from) {
                moved.push((p.clone(), to.join(suffix)));
            }
        }
        for (old, new) in moved {
            self.expanded.remove(&old);
            self.expanded.insert(new);
        }
    }

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
        // Invalidate the cached file list; it will be rebuilt lazily when needed.
        self.cached_file_list.clear();
        Ok(())
    }

    pub(crate) fn walk_dir(
        &self,
        dir: &Path,
        depth: usize,
        out: &mut Vec<TreeItem>,
    ) -> io::Result<()> {
        let is_root = dir == self.root;

        // For non-root directories, push the directory node itself.
        // The root is implicit — its children appear at the top level.
        if !is_root {
            let name = dir
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| dir.display().to_string());
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
        }

        let child_depth = if is_root { depth } else { depth + 1 };

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
                self.walk_dir(&path, child_depth, out)?;
            } else {
                out.push(TreeItem {
                    path,
                    name,
                    depth: child_depth,
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
        self.status_set_at = Some(std::time::Instant::now());
    }

    pub(crate) fn refresh_file_picker_results(&mut self) {
        // Lazily rebuild the file list if it was invalidated
        if self.cached_file_list.is_empty() {
            let mut files = Vec::new();
            collect_all_files(&self.root, &mut files);
            self.cached_file_list = files;
        }
        let query = self.file_picker_query.to_ascii_lowercase();
        let mut scored: Vec<(usize, PathBuf)> = self
            .cached_file_list
            .iter()
            .filter_map(|path| {
                let rel = relative_path(&self.root, path).display().to_string();
                fuzzy_score(&query, &rel).map(|score| (score, path.clone()))
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
        if let Some(parent) = item.path.parent()
            && let Some(idx) = self.tree.iter().position(|i| i.path == parent)
        {
            self.selected = idx;
        }
    }

    pub(crate) fn tree_expand_recursive(&mut self) -> io::Result<()> {
        let Some(item) = self.selected_item().cloned() else {
            return Ok(());
        };
        if !item.is_dir {
            return Ok(());
        }
        fn collect_dirs(path: &Path, set: &mut std::collections::HashSet<PathBuf>) {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        set.insert(p.clone());
                        collect_dirs(&p, set);
                    }
                }
            }
        }
        self.expanded.insert(item.path.clone());
        collect_dirs(&item.path, &mut self.expanded);
        self.rebuild_tree()
    }

    pub(crate) fn tree_collapse_recursive(&mut self) -> io::Result<()> {
        let Some(item) = self.selected_item().cloned() else {
            return Ok(());
        };
        // If selection is a file or a collapsed dir, walk up to parent first.
        let target = if item.is_dir && self.expanded.contains(&item.path) {
            item.path.clone()
        } else if let Some(parent) = item.path.parent() {
            if let Some(idx) = self.tree.iter().position(|i| i.path == parent) {
                self.selected = idx;
            }
            parent.to_path_buf()
        } else {
            return Ok(());
        };
        // Remove target and all descendants from expanded set.
        self.expanded.retain(|p| !p.starts_with(&target));
        self.rebuild_tree()
    }

    pub(crate) fn tree_expand_all(&mut self) -> io::Result<()> {
        fn collect_dirs(path: &Path, set: &mut std::collections::HashSet<PathBuf>) {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        set.insert(p.clone());
                        collect_dirs(&p, set);
                    }
                }
            }
        }
        self.expanded.insert(self.root.clone());
        collect_dirs(&self.root, &mut self.expanded);
        self.rebuild_tree()
    }

    pub(crate) fn tree_collapse_all(&mut self) -> io::Result<()> {
        self.expanded.clear();
        self.selected = 0;
        self.rebuild_tree()
    }

    pub(crate) fn delete_path(&mut self, path: PathBuf) -> io::Result<()> {
        if path == self.root {
            self.set_status("Cannot delete project root");
            return Ok(());
        }
        if !path.exists() {
            self.set_status("Path no longer exists");
            self.rebuild_tree()?;
            return Ok(());
        }
        if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
        // Close any tab at this path or under this directory.
        self.close_tabs_for_path_prefix(&path);
        self.expanded.retain(|p| !p.starts_with(&path));
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
                let name = match self.sanitize_entry_name(&value) {
                    Ok(name) => name,
                    Err(msg) => {
                        self.set_status(msg);
                        return Ok(());
                    }
                };
                let target = parent.join(name);
                if target.exists() {
                    self.set_status("File already exists");
                    return Ok(());
                }
                fs::write(&target, b"")?;
                // Ensure parent is visible after creating from a collapsed directory.
                self.expanded.insert(parent.clone());
                self.rebuild_tree()?;
                self.set_status(format!(
                    "Created {}",
                    relative_path(&self.root, &target).display()
                ));
            }
            PromptMode::NewFolder { parent } => {
                let name = match self.sanitize_entry_name(&value) {
                    Ok(name) => name,
                    Err(msg) => {
                        self.set_status(msg);
                        return Ok(());
                    }
                };
                let target = parent.join(name);
                if target.exists() {
                    self.set_status("Folder already exists");
                    return Ok(());
                }
                fs::create_dir_all(&target)?;
                // Ensure parent and new folder are both visible.
                self.expanded.insert(parent.clone());
                self.expanded.insert(target.clone());
                self.rebuild_tree()?;
                self.set_status(format!(
                    "Created {}",
                    relative_path(&self.root, &target).display()
                ));
            }
            PromptMode::Rename { target } => {
                if target == self.root {
                    self.set_status("Cannot rename project root");
                    return Ok(());
                }
                let Some(parent) = target.parent() else {
                    self.set_status("Cannot rename root");
                    return Ok(());
                };
                let name = match self.sanitize_entry_name(&value) {
                    Ok(name) => name,
                    Err(msg) => {
                        self.set_status(msg);
                        return Ok(());
                    }
                };
                let renamed = parent.join(name);
                if renamed == target {
                    self.set_status("Name unchanged");
                    return Ok(());
                }
                if renamed.exists() {
                    self.set_status("Name already exists");
                    return Ok(());
                }
                fs::rename(&target, &renamed)?;
                self.retarget_tabs_for_rename(&target, &renamed);
                self.retarget_expanded_for_rename(&target, &renamed);
                self.rebuild_tree()?;
                self.set_status(format!(
                    "Renamed to {}",
                    relative_path(&self.root, &renamed).display()
                ));
            }
            PromptMode::Move { target } => {
                let dest_dir = match self.resolve_move_destination(&target, &value) {
                    Ok(dir) => dir,
                    Err(msg) => {
                        self.set_status(msg);
                        return Ok(());
                    }
                };
                let Some(name) = target.file_name() else {
                    self.set_status("Cannot move root");
                    return Ok(());
                };
                let moved = dest_dir.join(name);
                // symlink_metadata also catches dangling symlinks, which exists() misses.
                match fs::symlink_metadata(&moved) {
                    Ok(_) => {
                        self.set_status("Destination already has an item with that name");
                        return Ok(());
                    }
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                    Err(err) => {
                        self.set_status(format!("Cannot check destination: {err}"));
                        return Ok(());
                    }
                }
                if let Err(err) = fs::rename(&target, &moved) {
                    self.set_status(format!("Move failed: {err}"));
                    return Ok(());
                }
                self.retarget_tabs_for_rename(&target, &moved);
                self.retarget_expanded_for_rename(&target, &moved);
                // Reveal the destination: expand every ancestor inside the project.
                for dir in dest_dir.ancestors() {
                    if dir == self.root || !dir.starts_with(&self.root) {
                        break;
                    }
                    self.expanded.insert(dir.to_path_buf());
                }
                let prev_selected = self.selected;
                self.rebuild_tree()?;
                // Select the moved item; if it left the project, stay near where we were.
                self.selected = self
                    .tree
                    .iter()
                    .position(|i| i.path == moved)
                    .unwrap_or_else(|| prev_selected.min(self.tree.len().saturating_sub(1)));
                self.set_status(format!(
                    "Moved {} to {}",
                    name.to_string_lossy(),
                    relative_path(&self.root, &dest_dir).display()
                ));
            }
            PromptMode::FindInFile => {
                self.search_in_open_file(&value);
                if self.replace_after_find && !value.is_empty() {
                    self.replace_after_find = false;
                    self.prompt = Some(PromptState {
                        title: format!("Replace '{}' with", value),
                        value: String::new(),
                        cursor: 0,
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
                        tab.editor.move_cursor(ratatui_textarea::CursorMove::Jump(
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
                    cursor: 0,
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
                    cursor: 0,
                    mode: PromptMode::NewFolder { parent },
                });
            }
            ContextAction::Rename => {
                if target == self.root {
                    self.set_status("Cannot rename project root");
                    return Ok(());
                }
                let default_name = target
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let cursor = default_name.len();
                self.prompt = Some(PromptState {
                    title: "Rename to".to_string(),
                    value: default_name,
                    cursor,
                    mode: PromptMode::Rename { target },
                });
            }
            ContextAction::Move => {
                if target == self.root {
                    self.set_status("Cannot move project root");
                    return Ok(());
                }
                let parent = target.parent().unwrap_or(&self.root);
                let default_dir = relative_path(&self.root, parent).display().to_string();
                let cursor = default_dir.len();
                self.prompt = Some(PromptState {
                    title: "Move to folder (relative to project root)".to_string(),
                    value: default_dir,
                    cursor,
                    mode: PromptMode::Move { target },
                });
            }
            ContextAction::Delete => {
                if target == self.root {
                    self.set_status("Cannot delete project root");
                    return Ok(());
                }
                self.pending = PendingAction::Delete(target.clone());
                self.set_status(format!(
                    "Delete {} ? Press Enter to confirm, Esc to cancel.",
                    target
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| target.display().to_string()),
                ));
            }
            ContextAction::Cancel => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn new_app(root: &Path) -> App {
        App::new(root.to_path_buf()).expect("app should initialize")
    }

    #[test]
    fn delete_path_rejects_project_root() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let mut app = new_app(root);

        app.delete_path(root.to_path_buf())
            .expect("delete root should be non-fatal");

        assert!(root.exists());
        assert_eq!(app.status, "Cannot delete project root");
    }

    #[test]
    fn apply_context_action_rejects_rename_root() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let mut app = new_app(root);
        app.context_menu.target = Some(root.to_path_buf());

        app.apply_context_action(ContextAction::Rename)
            .expect("rename root should be non-fatal");

        assert!(app.prompt.is_none());
        assert_eq!(app.status, "Cannot rename project root");
    }

    #[test]
    fn apply_context_action_delete_opens_confirmation_state() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let path = root.join("delete_me.txt");
        fs::write(&path, "hello\n").expect("write file");
        let mut app = new_app(root);
        app.context_menu.target = Some(path.clone());

        app.apply_context_action(ContextAction::Delete)
            .expect("context delete should succeed");

        assert!(path.exists());
        match &app.pending {
            PendingAction::Delete(p) => assert_eq!(p, &path),
            _ => panic!("expected pending delete"),
        }
    }

    #[test]
    fn rename_directory_retargets_descendant_open_tabs() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let old_dir = root.join("old");
        fs::create_dir_all(&old_dir).expect("create old dir");
        let old_a = old_dir.join("a.rs");
        let old_b = old_dir.join("b.rs");
        fs::write(&old_a, "fn a() {}\n").expect("write a");
        fs::write(&old_b, "fn b() {}\n").expect("write b");

        let mut app = new_app(root);
        app.open_file(old_a.clone()).expect("open a");
        app.open_file(old_b.clone()).expect("open b");
        assert_eq!(app.tabs.len(), 2);

        app.apply_prompt(
            PromptMode::Rename {
                target: old_dir.clone(),
            },
            "new".to_string(),
        )
        .expect("rename directory");

        let new_dir = root.join("new");
        assert!(new_dir.is_dir());
        let new_a = new_dir.join("a.rs");
        let new_b = new_dir.join("b.rs");
        assert!(new_a.exists());
        assert!(new_b.exists());
        assert!(app.tabs.iter().any(|t| t.path == new_a));
        assert!(app.tabs.iter().any(|t| t.path == new_b));
        assert!(!app.tabs.iter().any(|t| t.path == old_a));
        assert!(!app.tabs.iter().any(|t| t.path == old_b));
    }

    #[test]
    fn move_file_into_subdir_retargets_tab_and_selects_it() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let file = root.join("a.rs");
        fs::write(&file, "fn a() {}\n").expect("write a");
        fs::create_dir_all(root.join("src/app")).expect("create dirs");

        let mut app = new_app(&root);
        app.open_file(file.clone()).expect("open a");

        app.apply_prompt(
            PromptMode::Move {
                target: file.clone(),
            },
            "src/app".to_string(),
        )
        .expect("move file");

        let moved = root.join("src/app/a.rs");
        assert!(moved.exists());
        assert!(!file.exists());
        assert!(app.tabs.iter().any(|t| t.path == moved));
        assert!(app.expanded.contains(&root.join("src/app")));
        assert_eq!(app.tree[app.selected].path, moved);
        assert_eq!(app.status, "Moved a.rs to src/app");
    }

    #[test]
    fn move_rejects_folder_into_itself_and_missing_destination() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let dir = root.join("pkg");
        fs::create_dir_all(dir.join("inner")).expect("create dirs");
        let mut app = new_app(&root);

        app.apply_prompt(
            PromptMode::Move {
                target: dir.clone(),
            },
            "pkg/inner".to_string(),
        )
        .expect("non-fatal");
        assert_eq!(app.status, "Cannot move a folder into itself");
        assert!(dir.is_dir());

        app.apply_prompt(
            PromptMode::Move {
                target: dir.clone(),
            },
            "nowhere".to_string(),
        )
        .expect("non-fatal");
        assert_eq!(app.status, "Destination folder does not exist");

        app.apply_prompt(
            PromptMode::Move {
                target: dir.clone(),
            },
            ".".to_string(),
        )
        .expect("non-fatal");
        assert_eq!(app.status, "Already in that folder");
    }

    #[test]
    fn move_rejects_name_collision_and_allows_leaving_project() {
        let tmp = tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("proj");
        let outside = base.join("outside");
        fs::create_dir_all(root.join("dst")).expect("create");
        fs::create_dir_all(&outside).expect("create");
        fs::write(root.join("x.txt"), "1").expect("write");
        fs::write(root.join("dst/x.txt"), "2").expect("write");
        let mut app = new_app(&root);

        app.apply_prompt(
            PromptMode::Move {
                target: root.join("x.txt"),
            },
            "dst".to_string(),
        )
        .expect("non-fatal");
        assert_eq!(app.status, "Destination already has an item with that name");
        assert!(root.join("x.txt").exists());

        app.apply_prompt(
            PromptMode::Move {
                target: root.join("x.txt"),
            },
            outside.display().to_string(),
        )
        .expect("move outside");
        assert!(outside.join("x.txt").exists());
        assert!(!root.join("x.txt").exists());
        assert!(app.selected < app.tree.len());
    }

    #[test]
    fn apply_prompt_new_file_rejects_traversal_name() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let mut app = new_app(root);

        app.apply_prompt(
            PromptMode::NewFile {
                parent: root.to_path_buf(),
            },
            "../escape.rs".to_string(),
        )
        .expect("new file with traversal should be non-fatal");

        assert_eq!(app.status, "Name must be a single path component");
        assert!(!root.join("../escape.rs").exists());
    }

    #[test]
    fn apply_prompt_rename_rejects_nested_name() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let file = root.join("file.txt");
        fs::write(&file, "x\n").expect("write file");
        let mut app = new_app(root);

        app.apply_prompt(
            PromptMode::Rename {
                target: file.clone(),
            },
            "a/b.txt".to_string(),
        )
        .expect("rename with nested path should be non-fatal");

        assert_eq!(app.status, "Name must be a single path component");
        assert!(file.exists());
        assert!(!root.join("a").join("b.txt").exists());
    }

    #[test]
    fn cached_file_list_populated_on_init() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("a.rs"), "fn a() {}\n").expect("write a");
        fs::write(root.join("b.rs"), "fn b() {}\n").expect("write b");
        fs::create_dir_all(root.join("src")).expect("create src");
        fs::write(root.join("src/c.rs"), "fn c() {}\n").expect("write c");
        let mut app = new_app(root);
        // Cache starts empty (lazy); trigger lazy rebuild via file picker
        assert!(app.cached_file_list.is_empty(), "cache should start empty");
        app.refresh_file_picker_results();
        assert!(
            app.cached_file_list.len() >= 3,
            "cached_file_list should contain at least the 3 created files, got {}",
            app.cached_file_list.len()
        );
        assert!(app.cached_file_list.iter().any(|p| p.ends_with("a.rs")));
        assert!(app.cached_file_list.iter().any(|p| p.ends_with("b.rs")));
        assert!(
            app.cached_file_list
                .iter()
                .any(|p| p.ends_with("src/c.rs") || p.ends_with("src\\c.rs"))
        );
    }

    #[test]
    fn file_picker_uses_cached_list() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("main.rs"), "fn main() {}\n").expect("write main");
        fs::write(root.join("lib.rs"), "pub fn lib() {}\n").expect("write lib");
        let mut app = new_app(root);
        app.file_picker_open = true;
        app.file_picker_query = "main".to_string();
        app.refresh_file_picker_results();
        assert!(
            !app.file_picker_results.is_empty(),
            "should find main.rs via cached list"
        );
        assert!(app.file_picker_results[0].ends_with("main.rs"));
    }

    #[test]
    fn file_picker_empty_query_returns_all() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("a.txt"), "a\n").expect("write a");
        fs::write(root.join("b.txt"), "b\n").expect("write b");
        let mut app = new_app(root);
        app.file_picker_query.clear();
        app.refresh_file_picker_results();
        assert!(
            app.file_picker_results.len() >= 2,
            "empty query should return all files"
        );
    }
}
