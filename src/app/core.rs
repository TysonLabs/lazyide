use super::{App, CompletionState, ContextMenuState, KeybindEditorState, SearchResultsState};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use arboard::Clipboard;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use ratatui::layout::Rect;

use crate::keybinds::{KeyAction, load_keybindings};
use crate::lsp_client::resolve_rust_analyzer_bin;
use crate::persistence::{
    PersistedState, autosave_path_for, load_persisted_state, save_persisted_state,
};
use crate::syntax::syntax_lang_for_path;
use crate::tab::{FoldRange, Tab};
use crate::theme::{Theme, load_themes};
use crate::types::{CommandAction, Focus, PendingAction, PromptMode, PromptState};
use crate::util::{
    command_action_label, compute_fold_ranges, compute_git_change_summary,
    compute_git_file_statuses, compute_git_line_status, detect_git_branch, relative_path,
    text_to_lines,
};

impl App {
    pub(crate) const INLINE_GHOST_MIN_PREFIX: usize = 3;
    pub(crate) const EDITOR_GUTTER_WIDTH: u16 = 11;
    pub(crate) const MIN_FILES_PANE_WIDTH: u16 = 18;
    pub(crate) const MIN_EDITOR_PANE_WIDTH: u16 = 28;
    pub(crate) const FS_REFRESH_DEBOUNCE_MS: u64 = 120;
    pub(crate) const AUTOSAVE_INTERVAL_MS: u64 = 2000;

    pub(crate) fn new(root: PathBuf) -> io::Result<Self> {
        let themes = load_themes();
        let mut expanded = HashSet::new();
        expanded.insert(root.clone());
        let mut app = Self {
            root,
            tree: Vec::new(),
            selected: 0,
            expanded,
            focus: Focus::Tree,
            tabs: Vec::new(),
            active_tab: 0,
            last_tree_click: None,
            status: String::new(),
            pending: PendingAction::None,
            quit: false,
            files_view_open: true,
            files_pane_width: 32,
            divider_dragging: false,
            menu_open: false,
            menu_index: 0,
            menu_query: String::new(),
            menu_results: Vec::new(),
            theme_browser_open: false,
            theme_index: 0,
            preview_revert_index: 0,
            themes,
            active_theme_index: 0,
            help_open: false,
            tree_rect: Rect::default(),
            editor_rect: Rect::default(),
            divider_rect: Rect::default(),
            tab_rects: Vec::new(),
            context_menu: ContextMenuState {
                open: false,
                index: 0,
                target: None,
                pos: (0, 0),
                rect: Rect::default(),
            },
            prompt: None,
            clipboard: Clipboard::new().ok(),
            editor_context_menu_open: false,
            editor_context_menu_index: 0,
            editor_context_menu_pos: (0, 0),
            editor_context_menu_rect: Rect::default(),
            editor_dragging: false,
            editor_drag_anchor: None,
            search_results: SearchResultsState {
                open: false,
                query: String::new(),
                results: Vec::new(),
                index: 0,
            },
            file_picker_open: false,
            file_picker_query: String::new(),
            file_picker_results: Vec::new(),
            file_picker_index: 0,
            lsp: None,
            completion: CompletionState {
                open: false,
                items: Vec::new(),
                index: 0,
                rect: Rect::default(),
                ghost: None,
                prefix: String::new(),
            },
            pending_completion_request: None,
            pending_definition_request: None,
            fs_watcher: None,
            fs_rx: None,
            fs_refresh_pending: false,
            fs_full_refresh_pending: false,
            fs_changed_paths: HashSet::new(),
            last_fs_refresh: Instant::now(),
            autosave_last_write: Instant::now(),
            replace_after_find: false,
            git_branch: None,
            enhanced_keys: false,
            keybinds: load_keybindings(),
            keybind_editor: KeybindEditorState {
                open: false,
                index: 0,
                recording: false,
                query: String::new(),
                conflict: None,
                actions: KeyAction::all().to_vec(),
            },
            git_file_statuses: HashMap::new(),
            git_change_summary: Default::default(),
        };
        app.git_branch = detect_git_branch(&app.root);
        app.git_file_statuses = compute_git_file_statuses(&app.root);
        app.git_change_summary = compute_git_change_summary(&app.root);
        app.restore_persisted_state();
        app.rebuild_tree()?;
        app.start_fs_watcher();
        let has_ra = resolve_rust_analyzer_bin().is_some();
        let has_rg = Command::new("rg").arg("--version").output().is_ok();
        if !has_ra || !has_rg {
            let mut missing = Vec::new();
            if !has_ra {
                missing.push("rust-analyzer");
            }
            if !has_rg {
                missing.push("rg");
            }
            app.status = format!(
                "Missing tools: {}. Run `lazyide --setup` to install.",
                missing.join(", ")
            );
        } else {
            app.status = format!("Root: {}", app.root.display());
        }
        Ok(app)
    }

    pub(crate) fn start_fs_watcher(&mut self) {
        let (tx, rx) = mpsc::channel::<super::FsChangeEvent>();
        let mut watcher = match RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    let full_refresh = matches!(event.kind, EventKind::Any | EventKind::Other);
                    let _ = tx.send(super::FsChangeEvent {
                        paths: event.paths,
                        full_refresh,
                    });
                }
            },
            Config::default().with_poll_interval(Duration::from_millis(250)),
        ) {
            Ok(w) => w,
            Err(err) => {
                self.set_status(format!("Filesystem watch unavailable: {err}"));
                return;
            }
        };
        if let Err(err) = watcher.watch(&self.root, RecursiveMode::Recursive) {
            self.set_status(format!("Filesystem watch unavailable: {err}"));
            return;
        }
        self.fs_rx = Some(rx);
        self.fs_watcher = Some(watcher);
        self.fs_refresh_pending = false;
        self.fs_full_refresh_pending = false;
        self.fs_changed_paths.clear();
        self.last_fs_refresh = Instant::now();
    }

    pub(crate) fn poll_fs_changes(&mut self) -> io::Result<()> {
        let mut saw_event = false;
        if let Some(rx) = self.fs_rx.as_ref() {
            while let Ok(change) = rx.try_recv() {
                saw_event = true;
                if change.full_refresh {
                    self.fs_full_refresh_pending = true;
                }
                for path in change.paths {
                    let abs = if path.is_absolute() {
                        path
                    } else {
                        self.root.join(path)
                    };
                    if abs.starts_with(self.root.join(".git")) {
                        self.fs_full_refresh_pending = true;
                    }
                    self.fs_changed_paths.insert(abs);
                }
            }
        }
        if saw_event {
            self.fs_refresh_pending = true;
        }
        if self.fs_refresh_pending
            && self.last_fs_refresh.elapsed() >= Duration::from_millis(Self::FS_REFRESH_DEBOUNCE_MS)
        {
            self.rebuild_tree()?;
            self.git_branch = detect_git_branch(&self.root);
            self.git_file_statuses = compute_git_file_statuses(&self.root);
            self.git_change_summary = compute_git_change_summary(&self.root);
            if self.file_picker_open {
                self.refresh_file_picker_results();
            }
            if let Some(path) = self.open_path().cloned() {
                if !path.exists() {
                    if self.is_dirty() {
                        self.set_status(
                            "Open file was removed externally (unsaved buffer preserved)",
                        );
                    } else {
                        self.close_file();
                        self.set_status("Open file was removed externally");
                    }
                } else if !self.is_dirty() {
                    self.reload_open_file_from_disk_if_pristine()?;
                } else {
                    self.maybe_flag_external_conflict()?;
                }
            }
            // Refresh git line status only for affected tabs; full refresh for ambiguous events.
            let full_refresh = self.fs_full_refresh_pending || self.fs_changed_paths.is_empty();
            let root = self.root.clone();
            let changed = self.fs_changed_paths.clone();
            for tab in &mut self.tabs {
                let should_refresh = if full_refresh {
                    true
                } else {
                    changed.iter().any(|p| {
                        tab.path == *p
                            || tab
                                .path
                                .parent()
                                .is_some_and(|parent| parent.starts_with(p))
                    })
                };
                if should_refresh {
                    let line_count = tab.editor.lines().len();
                    tab.git_line_status = compute_git_line_status(&root, &tab.path, line_count);
                }
            }
            self.fs_refresh_pending = false;
            self.fs_full_refresh_pending = false;
            self.fs_changed_paths.clear();
            self.last_fs_refresh = Instant::now();
        }
        Ok(())
    }

    pub(crate) fn reload_open_file_from_disk_if_pristine(&mut self) -> io::Result<()> {
        let Some(path) = self.open_path().cloned() else {
            return Ok(());
        };
        if self.is_dirty() || !path.exists() {
            return Ok(());
        }
        let bytes = fs::read(&path)?;
        let disk_text = String::from_utf8_lossy(&bytes).to_string();
        let current_text = self.tabs[self.active_tab].editor.lines().join("\n");
        if disk_text == current_text {
            return Ok(());
        }
        let lines = text_to_lines(&disk_text);
        let (row, col) = self.tabs[self.active_tab].editor.cursor();
        let clamped_row = row.min(lines.len().saturating_sub(1));
        let line_len = lines[clamped_row].chars().count();
        let clamped_col = col.min(line_len);
        self.replace_editor_text(lines, (clamped_row, clamped_col));
        if let Some(tab) = self.active_tab_mut() {
            tab.dirty = false;
            tab.open_disk_snapshot = Some(disk_text);
        }
        self.notify_lsp_did_change();
        self.set_status(format!(
            "Reloaded {} from disk",
            relative_path(&self.root, &path).display()
        ));
        Ok(())
    }

    pub(crate) fn active_theme(&self) -> &Theme {
        &self.themes[self.active_theme_index]
    }

    pub(crate) fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active_tab)
    }

    pub(crate) fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active_tab)
    }

    pub(crate) fn open_path(&self) -> Option<&PathBuf> {
        self.active_tab().map(|t| &t.path)
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.active_tab().is_some_and(|t| t.dirty)
    }

    pub(crate) fn any_tab_dirty(&self) -> bool {
        self.tabs.iter().any(|t| t.dirty)
    }

    pub(crate) fn mark_dirty(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.dirty = true;
            tab.is_preview = false;
        }
    }

    pub(crate) fn switch_to_tab(&mut self, idx: usize) {
        if idx < self.tabs.len() {
            self.active_tab = idx;
            self.completion.reset();
            self.focus = Focus::Editor;
        }
    }

    pub(crate) fn restore_persisted_state(&mut self) {
        let Some(saved) = load_persisted_state() else {
            return;
        };
        if let Some(width) = saved.files_pane_width {
            self.files_pane_width = width.max(Self::MIN_FILES_PANE_WIDTH);
        }
        if let Some(idx) = self
            .themes
            .iter()
            .position(|t| t.name.eq_ignore_ascii_case(&saved.theme_name))
        {
            self.active_theme_index = idx;
            self.theme_index = idx;
            self.preview_revert_index = idx;
        }
    }

    pub(crate) fn persist_state(&mut self) {
        let state = PersistedState {
            theme_name: self.active_theme().name.clone(),
            files_pane_width: Some(self.files_pane_width),
        };
        if save_persisted_state(&state).is_err() {
            self.set_status("Failed to persist app state");
        }
    }

    pub(crate) fn persist_theme_selection(&mut self) {
        self.persist_state();
    }

    pub(crate) fn on_editor_content_changed(&mut self) {
        self.mark_dirty();
        self.notify_lsp_did_change();
        self.recompute_folds();
    }

    pub(crate) fn open_find_prompt(&mut self) {
        self.prompt = Some(PromptState {
            title: "Find in file (regex)".to_string(),
            value: String::new(),
            mode: PromptMode::FindInFile,
        });
    }

    pub(crate) fn open_project_search_prompt(&mut self) {
        self.prompt = Some(PromptState {
            title: "Search in files (ripgrep)".to_string(),
            value: String::new(),
            mode: PromptMode::FindInProject,
        });
    }

    pub(crate) fn open_go_to_line_prompt(&mut self) {
        self.prompt = Some(PromptState {
            title: "Go to line".to_string(),
            value: String::new(),
            mode: PromptMode::GoToLine,
        });
    }

    pub(crate) fn open_replace_prompt(&mut self) {
        self.open_find_prompt();
        self.replace_after_find = true;
    }

    pub(crate) fn open_command_palette(&mut self) {
        self.menu_open = true;
        self.menu_query.clear();
        self.menu_index = 0;
        self.refresh_menu_results();
    }

    pub(crate) fn refresh_menu_results(&mut self) {
        let all = [
            CommandAction::Theme,
            CommandAction::Help,
            CommandAction::QuickOpen,
            CommandAction::FindInFile,
            CommandAction::FindInProject,
            CommandAction::SaveFile,
            CommandAction::RefreshTree,
            CommandAction::ToggleFiles,
            CommandAction::GotoDefinition,
            CommandAction::ReplaceInFile,
            CommandAction::GoToLine,
            CommandAction::Keybinds,
        ];
        let q = self.menu_query.to_ascii_lowercase();
        self.menu_results = all
            .into_iter()
            .filter(|a| {
                q.is_empty()
                    || command_action_label(*a)
                        .to_ascii_lowercase()
                        .contains(q.as_str())
            })
            .collect();
        self.menu_index = self
            .menu_index
            .min(self.menu_results.len().saturating_sub(1));
    }

    pub(crate) fn run_command_action(&mut self, action: CommandAction) -> io::Result<()> {
        match action {
            CommandAction::Theme => {
                self.theme_browser_open = true;
                self.theme_index = self.active_theme_index;
                self.preview_revert_index = self.active_theme_index;
                self.set_status("Theme browser: arrows preview, Enter keep, Esc revert");
            }
            CommandAction::Help => self.help_open = true,
            CommandAction::QuickOpen => {
                self.file_picker_open = true;
                self.file_picker_query.clear();
                self.file_picker_index = 0;
                self.refresh_file_picker_results();
            }
            CommandAction::FindInFile => {
                self.open_find_prompt();
            }
            CommandAction::FindInProject => {
                self.open_project_search_prompt();
            }
            CommandAction::SaveFile => {
                self.save_file()?;
            }
            CommandAction::RefreshTree => {
                self.rebuild_tree()?;
                self.set_status("Tree refreshed");
            }
            CommandAction::ToggleFiles => {
                self.files_view_open = !self.files_view_open;
                if !self.files_view_open {
                    self.focus = Focus::Editor;
                    self.set_status("Files view hidden");
                } else {
                    self.set_status("Files view shown");
                }
            }
            CommandAction::GotoDefinition => self.request_lsp_definition(),
            CommandAction::ReplaceInFile => {
                self.open_replace_prompt();
            }
            CommandAction::GoToLine => {
                self.open_go_to_line_prompt();
            }
            CommandAction::Keybinds => {
                self.keybind_editor.open = true;
                self.keybind_editor.index = 0;
                self.keybind_editor.recording = false;
                self.keybind_editor.query.clear();
                self.keybind_editor.conflict = None;
                self.refresh_keybind_editor_actions();
            }
        }
        Ok(())
    }

    pub(crate) fn update_status_for_cursor(&mut self) {
        if self.focus == Focus::Editor {
            if let Some(tab) = self.active_tab() {
                let cursor_row = tab.editor.cursor().0;
                if let Some(diag) = tab.diagnostics.iter().find(|d| d.line == cursor_row + 1) {
                    self.status = format!("[{}] {}", diag.severity, diag.message);
                }
            }
        }
    }

    pub(crate) fn poll_autosave(&mut self) -> io::Result<()> {
        if self.autosave_last_write.elapsed() < Duration::from_millis(Self::AUTOSAVE_INTERVAL_MS) {
            return Ok(());
        }
        for tab in &self.tabs {
            if !tab.dirty {
                continue;
            }
            let autosave = autosave_path_for(&tab.path);
            if let Some(parent) = autosave.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&autosave, tab.editor.lines().join("\n"))?;
        }
        self.autosave_last_write = Instant::now();
        Ok(())
    }

    pub(crate) fn check_recovery_for_open_file(&mut self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let path = tab.path.clone();
        let autosave = autosave_path_for(&path);
        let Ok(recovered) = fs::read_to_string(autosave) else {
            return;
        };
        let current = self.tabs[self.active_tab].editor.lines().join("\n");
        if recovered != current {
            if let Some(tab) = self.active_tab_mut() {
                tab.recovery_prompt_open = true;
                tab.recovery_text = Some(recovered);
            }
        }
    }

    pub(crate) fn clear_autosave_for_open_file(&mut self) {
        if let Some(tab) = self.active_tab() {
            let _ = fs::remove_file(autosave_path_for(&tab.path));
        }
    }

    pub(crate) fn maybe_flag_external_conflict(&mut self) -> io::Result<()> {
        let Some(tab) = self.active_tab() else {
            return Ok(());
        };
        if !tab.dirty || !tab.path.exists() || tab.conflict_prompt_open {
            return Ok(());
        }
        let path = tab.path.clone();
        let disk = fs::read_to_string(&path)?;
        let current = self.tabs[self.active_tab].editor.lines().join("\n");
        let snapshot = self.tabs[self.active_tab]
            .open_disk_snapshot
            .clone()
            .unwrap_or_default();
        if disk != snapshot && disk != current {
            if let Some(tab) = self.active_tab_mut() {
                tab.conflict_prompt_open = true;
                tab.conflict_disk_text = Some(disk);
            }
        }
        Ok(())
    }
    pub(crate) fn clamp_files_pane_width(&mut self, total_width: u16) {
        let min_files = Self::MIN_FILES_PANE_WIDTH.min(total_width.saturating_sub(1));
        let max_files = total_width
            .saturating_sub(Self::MIN_EDITOR_PANE_WIDTH)
            .max(min_files);
        self.files_pane_width = self.files_pane_width.clamp(min_files, max_files);
    }

    pub(crate) fn recompute_folds(&mut self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let lang = syntax_lang_for_path(Some(tab.path.as_path()));
        let (fold_ranges, bracket_depths) =
            compute_fold_ranges(self.tabs[self.active_tab].editor.lines(), lang);
        let tab = &mut self.tabs[self.active_tab];
        tab.fold_ranges = fold_ranges;
        tab.bracket_depths = bracket_depths;
        tab.folded_starts
            .retain(|start| tab.fold_ranges.iter().any(|r| r.start_line == *start));
        self.rebuild_visible_rows();
    }

    pub(crate) fn rebuild_visible_rows(&mut self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let lines = tab.editor.lines();
        let num_lines = lines.len();
        let tab = &mut self.tabs[self.active_tab];
        tab.visible_rows_map.clear();
        tab.visible_rows_map.reserve(num_lines);
        for row in 0..num_lines {
            let hidden = tab.fold_ranges.iter().any(|fr| {
                tab.folded_starts.contains(&fr.start_line)
                    && row > fr.start_line
                    && row <= fr.end_line
            });
            if !hidden {
                tab.visible_rows_map.push(row);
            }
        }
        if tab.visible_rows_map.is_empty() {
            tab.visible_rows_map.push(0);
        }
        let max_scroll = tab.visible_rows_map.len().saturating_sub(1);
        tab.editor_scroll_row = tab.editor_scroll_row.min(max_scroll);
    }

    pub(crate) fn visible_index_of_source_row(&self, row: usize) -> usize {
        let Some(tab) = self.active_tab() else {
            return 0;
        };
        tab.visible_rows_map
            .iter()
            .position(|r| *r == row)
            .unwrap_or_else(|| {
                tab.visible_rows_map
                    .iter()
                    .position(|r| *r > row)
                    .unwrap_or(tab.visible_rows_map.len().saturating_sub(1))
            })
    }

    pub(crate) fn fold_range_starting_at(&self, row: usize) -> Option<&FoldRange> {
        let tab = self.active_tab()?;
        tab.fold_ranges.iter().find(|fr| fr.start_line == row)
    }

    pub(crate) fn toggle_fold_at_row(&mut self, row: usize) {
        if let Some(fr) = self.fold_range_starting_at(row) {
            let start_line = fr.start_line;
            let end_line = fr.end_line;
            let tab = &mut self.tabs[self.active_tab];
            if tab.folded_starts.contains(&start_line) {
                tab.folded_starts.remove(&start_line);
                self.set_status(format!(
                    "Unfolded lines {}-{}",
                    start_line + 1,
                    end_line + 1
                ));
            } else {
                tab.folded_starts.insert(start_line);
                self.set_status(format!("Folded lines {}-{}", start_line + 1, end_line + 1));
            }
            self.rebuild_visible_rows();
            self.sync_editor_scroll_guess();
        }
    }

    pub(crate) fn fold_current_block(&mut self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let (cursor_row, _) = tab.editor.cursor();
        let mut candidate: Option<(usize, usize)> = None;
        for fr in &tab.fold_ranges {
            if fr.start_line == cursor_row {
                candidate = Some((fr.start_line, fr.end_line));
                break;
            }
            if fr.start_line <= cursor_row && cursor_row <= fr.end_line {
                candidate = Some((fr.start_line, fr.end_line));
            }
        }
        if let Some((start_line, end_line)) = candidate {
            self.tabs[self.active_tab].folded_starts.insert(start_line);
            self.rebuild_visible_rows();
            self.sync_editor_scroll_guess();
            self.set_status(format!("Folded lines {}-{}", start_line + 1, end_line + 1));
        } else {
            self.set_status("No foldable block at cursor");
        }
    }

    pub(crate) fn unfold_current_block(&mut self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let (cursor_row, _) = tab.editor.cursor();
        let mut unfolded = false;
        let starts: Vec<usize> = tab.folded_starts.iter().copied().collect();
        for start in starts {
            if let Some(fr) = tab.fold_ranges.iter().find(|fr| fr.start_line == start) {
                if fr.start_line == cursor_row
                    || (fr.start_line <= cursor_row && cursor_row <= fr.end_line)
                {
                    self.tabs[self.active_tab].folded_starts.remove(&start);
                    unfolded = true;
                    break;
                }
            }
        }
        if unfolded {
            self.rebuild_visible_rows();
            self.sync_editor_scroll_guess();
            self.set_status("Unfolded block");
        } else {
            self.set_status("No folded block at cursor");
        }
    }

    pub(crate) fn fold_all(&mut self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let starts: Vec<usize> = tab.fold_ranges.iter().map(|fr| fr.start_line).collect();
        let tab = &mut self.tabs[self.active_tab];
        for start in starts {
            tab.folded_starts.insert(start);
        }
        let count = tab.folded_starts.len();
        self.rebuild_visible_rows();
        self.sync_editor_scroll_guess();
        self.set_status(format!("Folded {} blocks", count));
    }

    pub(crate) fn unfold_all(&mut self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        if tab.folded_starts.is_empty() {
            self.set_status("No folded blocks");
            return;
        }
        self.tabs[self.active_tab].folded_starts.clear();
        self.rebuild_visible_rows();
        self.sync_editor_scroll_guess();
        self.set_status("Unfolded all blocks");
    }

    pub(crate) fn toggle_fold_at_cursor(&mut self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let (cursor_row, _) = tab.editor.cursor();
        // Check if cursor is on/in a folded block
        let mut is_folded = false;
        for &start in &tab.folded_starts {
            if let Some(fr) = tab.fold_ranges.iter().find(|fr| fr.start_line == start) {
                if fr.start_line == cursor_row
                    || (fr.start_line <= cursor_row && cursor_row <= fr.end_line)
                {
                    is_folded = true;
                    break;
                }
            }
        }
        if is_folded {
            self.unfold_current_block();
        } else {
            self.fold_current_block();
        }
    }

    pub(crate) fn toggle_fold_all(&mut self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        if tab.folded_starts.is_empty() {
            self.fold_all();
        } else {
            self.unfold_all();
        }
    }
}
