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
    compute_git_file_statuses, detect_git_branch, relative_path, spawn_git_refresh,
    text_to_lines, wrap_segments_for_line,
};

impl App {
    pub(crate) const INLINE_GHOST_MIN_PREFIX: usize = 3;
    pub(crate) const EDITOR_GUTTER_WIDTH: u16 = 11;
    pub(crate) const MIN_FILES_PANE_WIDTH: u16 = 18;
    pub(crate) const MIN_EDITOR_PANE_WIDTH: u16 = 28;
    pub(crate) const FS_REFRESH_DEBOUNCE_MS: u64 = 120;
    pub(crate) const AUTOSAVE_INTERVAL_MS: u64 = 2000;
    pub(crate) const SCROLL_LINES: usize = 3;

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
            menu_rect: Rect::default(),
            theme_browser_open: false,
            theme_browser_rect: Rect::default(),
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
            prompt_rect: Rect::default(),
            clipboard: Clipboard::new().ok(),
            editor_context_menu_open: false,
            editor_context_menu_index: 0,
            editor_context_menu_pos: (0, 0),
            editor_context_menu_rect: Rect::default(),
            editor_dragging: false,
            editor_drag_anchor: None,
            gutter_drag_anchor: None,
            search_results: SearchResultsState {
                open: false,
                query: String::new(),
                results: Vec::new(),
                index: 0,
            },
            search_results_rect: Rect::default(),
            file_picker_open: false,
            file_picker_query: String::new(),
            file_picker_results: Vec::new(),
            file_picker_index: 0,
            file_picker_rect: Rect::default(),
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
            word_wrap: false,
            wrap_width_cache: usize::MAX,
            wrap_rebuild_deadline: None,
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
            git_result_rx: None,
            git_refresh_in_flight: false,
            git_thread_handle: None,
            cached_file_list: Vec::new(),
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
            // Dispatch async git refresh if not already in flight
            if !self.git_refresh_in_flight {
                // Join the previous thread (prevents handle accumulation)
                if let Some(handle) = self.git_thread_handle.take() {
                    if handle.join().is_err() {
                        self.set_status("Git refresh thread panicked");
                    }
                }
                let root = self.root.clone();
                let tab_paths: Vec<(PathBuf, usize)> = self
                    .tabs
                    .iter()
                    .map(|tab| (tab.path.clone(), tab.editor.lines().len()))
                    .collect();
                let (tx, rx) = mpsc::channel();
                self.git_result_rx = Some(rx);
                self.git_refresh_in_flight = true;
                self.git_thread_handle = Some(spawn_git_refresh(root, tab_paths, tx));
            }
            self.fs_refresh_pending = false;
            self.fs_full_refresh_pending = false;
            self.fs_changed_paths.clear();
            self.last_fs_refresh = Instant::now();
        }
        Ok(())
    }

    pub(crate) fn poll_git_results(&mut self) {
        let result = self
            .git_result_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());
        let Some(result) = result else {
            return;
        };
        self.git_refresh_in_flight = false;
        self.git_branch = result.branch;
        self.git_file_statuses = result.file_statuses;
        self.git_change_summary = result.change_summary;
        for (path, line_status) in result.line_statuses {
            if let Some(tab) = self.tabs.iter_mut().find(|t| t.path == path) {
                tab.git_line_status = line_status;
            }
        }
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
        if let Some(word_wrap) = saved.word_wrap {
            self.word_wrap = word_wrap;
        }
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
            word_wrap: Some(self.word_wrap),
        };
        if save_persisted_state(&state).is_err() {
            self.set_status("Failed to persist app state");
        }
    }

    pub(crate) fn persist_theme_selection(&mut self) {
        self.persist_state();
    }

    pub(crate) fn toggle_word_wrap(&mut self) {
        self.word_wrap = !self.word_wrap;
        if self.word_wrap {
            // Reset horizontal scroll for all tabs when wrapping takes over
            for tab in &mut self.tabs {
                tab.editor_scroll_col = 0;
            }
        }
        self.wrap_width_cache = self.editor_wrap_width_chars();
        self.rebuild_all_visible_rows();
        self.sync_editor_scroll_guess();
        self.persist_state();
        if self.word_wrap {
            self.set_status("Word wrap enabled");
        } else {
            self.set_status("Word wrap disabled");
        }
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
            cursor: 0,
            mode: PromptMode::FindInFile,
        });
    }

    pub(crate) fn open_project_search_prompt(&mut self) {
        self.prompt = Some(PromptState {
            title: "Search in files (ripgrep)".to_string(),
            value: String::new(),
            cursor: 0,
            mode: PromptMode::FindInProject,
        });
    }

    pub(crate) fn open_go_to_line_prompt(&mut self) {
        self.prompt = Some(PromptState {
            title: "Go to line".to_string(),
            value: String::new(),
            cursor: 0,
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
            CommandAction::ToggleWordWrap,
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
            CommandAction::ToggleWordWrap => self.toggle_word_wrap(),
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
        let lines = tab.editor.lines().to_vec();
        let num_lines = lines.len();
        let wrap_width = self.editor_wrap_width_chars();
        let word_wrap = self.word_wrap;
        // Precompute hidden rows via HashSet for O(1) lookup per row
        let mut hidden: HashSet<usize> = HashSet::new();
        let tab = &self.tabs[self.active_tab];
        for fr in &tab.fold_ranges {
            if tab.folded_starts.contains(&fr.start_line) {
                for row in (fr.start_line + 1)..=fr.end_line {
                    hidden.insert(row);
                }
            }
        }
        let tab = &mut self.tabs[self.active_tab];
        tab.visible_rows_map.clear();
        tab.visible_row_starts.clear();
        tab.visible_row_ends.clear();
        tab.visible_rows_map
            .reserve(num_lines.saturating_sub(hidden.len()));
        tab.visible_row_starts
            .reserve(num_lines.saturating_sub(hidden.len()));
        tab.visible_row_ends
            .reserve(num_lines.saturating_sub(hidden.len()));
        for row in 0..num_lines {
            if !hidden.contains(&row) {
                let segments = if word_wrap {
                    wrap_segments_for_line(&lines[row], wrap_width)
                } else {
                    vec![(0, lines[row].chars().count())]
                };
                for (start, end) in segments {
                    tab.visible_rows_map.push(row);
                    tab.visible_row_starts.push(start);
                    tab.visible_row_ends.push(end);
                }
            }
        }
        if tab.visible_rows_map.is_empty() {
            tab.visible_rows_map.push(0);
            tab.visible_row_starts.push(0);
            tab.visible_row_ends.push(0);
        }
        let max_scroll = tab.visible_rows_map.len().saturating_sub(1);
        tab.editor_scroll_row = tab.editor_scroll_row.min(max_scroll);
    }

    pub(crate) fn rebuild_all_visible_rows(&mut self) {
        let prev = self.active_tab;
        for i in 0..self.tabs.len() {
            self.active_tab = i;
            self.rebuild_visible_rows();
        }
        self.active_tab = prev.min(self.tabs.len().saturating_sub(1));
    }

    /// Called from the main loop to flush any pending wrap rebuild after a
    /// resize has settled.
    pub(crate) fn poll_wrap_rebuild(&mut self) {
        if let Some(deadline) = self.wrap_rebuild_deadline {
            if Instant::now() >= deadline {
                self.wrap_rebuild_deadline = None;
                self.rebuild_all_visible_rows();
            }
        }
    }

    fn editor_wrap_width_chars(&self) -> usize {
        let inner_width = self.editor_rect.width.saturating_sub(2);
        let content_width = inner_width.saturating_sub(Self::EDITOR_GUTTER_WIDTH);
        if content_width == 0 {
            usize::MAX
        } else {
            content_width as usize
        }
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

    pub(crate) fn visible_index_of_source_position(&self, row: usize, col: usize) -> usize {
        let Some(tab) = self.active_tab() else {
            return 0;
        };
        let mut fallback = None;
        for idx in 0..tab.visible_rows_map.len() {
            if tab.visible_rows_map[idx] != row {
                continue;
            }
            fallback.get_or_insert(idx);
            let start = tab.visible_row_starts.get(idx).copied().unwrap_or(0);
            let end = tab.visible_row_ends.get(idx).copied().unwrap_or(start);
            if col >= start && col < end {
                return idx;
            }
            if col >= end {
                fallback = Some(idx);
            }
        }
        fallback.unwrap_or_else(|| self.visible_index_of_source_row(row))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn new_app(root: &std::path::Path) -> App {
        App::new(root.to_path_buf()).expect("app should initialize")
    }

    #[test]
    fn rebuild_visible_rows_no_folds_shows_all() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let file = root.join("test.rs");
        fs::write(
            &file,
            "line 0\nline 1\nline 2\nline 3\nline 4\n",
        )
        .expect("write");
        let mut app = new_app(root);
        app.open_file(file).expect("open");
        app.rebuild_visible_rows();
        let tab = app.active_tab().expect("should have tab");
        assert_eq!(tab.visible_rows_map, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn rebuild_visible_rows_with_fold_hides_interior() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let file = root.join("test.rs");
        fs::write(
            &file,
            "fn main() {\n    line 1\n    line 2\n}\nline 4\n",
        )
        .expect("write");
        let mut app = new_app(root);
        app.open_file(file).expect("open");
        // Fold the block starting at line 0 (fn main)
        app.tabs[app.active_tab].folded_starts.insert(0);
        app.rebuild_visible_rows();
        let tab = app.active_tab().expect("should have tab");
        // Lines 1, 2, 3 should be hidden (inside fold 0..3)
        assert!(!tab.visible_rows_map.contains(&1));
        assert!(!tab.visible_rows_map.contains(&2));
        assert!(!tab.visible_rows_map.contains(&3));
        // Line 0 (fold start) and line 4+ should be visible
        assert!(tab.visible_rows_map.contains(&0));
        assert!(tab.visible_rows_map.contains(&4));
    }

    #[test]
    fn rebuild_visible_rows_multiple_folds() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let file = root.join("test.rs");
        fs::write(
            &file,
            "fn a() {\n    body a\n}\nfn b() {\n    body b\n}\n",
        )
        .expect("write");
        let mut app = new_app(root);
        app.open_file(file).expect("open");
        // Fold both functions
        app.tabs[app.active_tab].folded_starts.insert(0);
        app.tabs[app.active_tab].folded_starts.insert(3);
        app.rebuild_visible_rows();
        let tab = app.active_tab().expect("should have tab");
        // Lines 1, 2 (inside first fold) and 4, 5 (inside second fold) should be hidden
        assert!(!tab.visible_rows_map.contains(&1));
        assert!(!tab.visible_rows_map.contains(&2));
        assert!(!tab.visible_rows_map.contains(&4));
        assert!(!tab.visible_rows_map.contains(&5));
        // Lines 0, 3, 6 should be visible
        assert!(tab.visible_rows_map.contains(&0));
        assert!(tab.visible_rows_map.contains(&3));
    }

    #[test]
    fn git_result_fields_initialized() {
        let tmp = tempdir().expect("tempdir");
        let app = new_app(tmp.path());
        assert!(!app.git_refresh_in_flight);
        assert!(app.git_result_rx.is_none());
    }

    #[test]
    fn poll_git_results_noop_when_no_receiver() {
        let tmp = tempdir().expect("tempdir");
        let mut app = new_app(tmp.path());
        // Should not panic when there's no receiver
        app.poll_git_results();
        assert!(!app.git_refresh_in_flight);
    }

    #[test]
    fn poll_git_results_applies_received_result() {
        let tmp = tempdir().expect("tempdir");
        let mut app = new_app(tmp.path());
        let (tx, rx) = std::sync::mpsc::channel();
        app.git_result_rx = Some(rx);
        app.git_refresh_in_flight = true;
        // Send a mock result
        let result = crate::app::GitResult {
            branch: Some("test-branch".to_string()),
            file_statuses: HashMap::new(),
            change_summary: crate::tab::GitChangeSummary {
                files_changed: 3,
                insertions: 10,
                deletions: 5,
            },
            line_statuses: vec![],
        };
        tx.send(result).expect("send");
        app.poll_git_results();
        assert_eq!(app.git_branch.as_deref(), Some("test-branch"));
        assert_eq!(app.git_change_summary.files_changed, 3);
        assert_eq!(app.git_change_summary.insertions, 10);
        assert_eq!(app.git_change_summary.deletions, 5);
        assert!(!app.git_refresh_in_flight);
    }

    #[test]
    fn poll_git_results_applies_line_statuses_to_matching_tabs() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let file = root.join("test.rs");
        fs::write(&file, "line 0\nline 1\n").expect("write");
        let mut app = new_app(root);
        app.open_file(file.clone()).expect("open");
        let (tx, rx) = std::sync::mpsc::channel();
        app.git_result_rx = Some(rx);
        app.git_refresh_in_flight = true;
        let result = crate::app::GitResult {
            branch: None,
            file_statuses: HashMap::new(),
            change_summary: Default::default(),
            line_statuses: vec![(
                file.clone(),
                vec![
                    crate::tab::GitLineStatus::Added,
                    crate::tab::GitLineStatus::Modified,
                ],
            )],
        };
        tx.send(result).expect("send");
        app.poll_git_results();
        let tab = app.active_tab().expect("tab");
        assert_eq!(tab.git_line_status[0], crate::tab::GitLineStatus::Added);
        assert_eq!(tab.git_line_status[1], crate::tab::GitLineStatus::Modified);
    }

    // ── Word wrap rebuild tests ──

    #[test]
    fn rebuild_visible_rows_with_word_wrap_creates_segments() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let file = root.join("test.txt");
        // A line that's wider than a small wrap width
        fs::write(&file, "hello world this is a long line\nshort\n").expect("write");
        let mut app = new_app(root);
        app.open_file(file).expect("open");
        app.word_wrap = true;
        // Simulate a narrow editor (wrap_width ~ 10 chars)
        app.editor_rect = Rect::new(0, 0, 22, 20); // 22 - 2 border - 10 gutter = 10
        app.rebuild_visible_rows();
        let tab = app.active_tab().expect("tab");
        // The long line should produce multiple segments (source row 0 appears more than once)
        let row0_count = tab.visible_rows_map.iter().filter(|&&r| r == 0).count();
        assert!(row0_count > 1, "long line should wrap into multiple segments");
        // The short line should produce a single segment
        let row1_count = tab.visible_rows_map.iter().filter(|&&r| r == 1).count();
        assert_eq!(row1_count, 1, "short line should be a single segment");
        // Segment arrays should all be the same length
        assert_eq!(tab.visible_rows_map.len(), tab.visible_row_starts.len());
        assert_eq!(tab.visible_rows_map.len(), tab.visible_row_ends.len());
    }

    #[test]
    fn rebuild_visible_rows_wrap_disabled_no_segments() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let file = root.join("test.txt");
        fs::write(&file, "hello world this is a long line\nshort\n").expect("write");
        let mut app = new_app(root);
        app.open_file(file).expect("open");
        app.word_wrap = false;
        app.rebuild_visible_rows();
        let tab = app.active_tab().expect("tab");
        // Without wrap, each source line maps to exactly one visual row
        let row0_count = tab.visible_rows_map.iter().filter(|&&r| r == 0).count();
        assert_eq!(row0_count, 1);
    }

    #[test]
    fn rebuild_all_visible_rows_updates_inactive_tabs() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let file1 = root.join("a.txt");
        let file2 = root.join("b.txt");
        fs::write(&file1, "hello world this is long\n").expect("write");
        fs::write(&file2, "another long line here too\n").expect("write");
        let mut app = new_app(root);
        app.open_file(file1).expect("open");
        app.open_file(file2).expect("open");
        app.word_wrap = true;
        app.editor_rect = Rect::new(0, 0, 22, 20); // wrap_width ~ 10
        // Only active tab (tab 1) should have been rebuilt by previous opens
        // Now rebuild all:
        app.rebuild_all_visible_rows();
        // Both tabs should have valid visible_rows_map
        assert!(!app.tabs[0].visible_rows_map.is_empty());
        assert!(!app.tabs[1].visible_rows_map.is_empty());
        // Segment arrays should be consistent for both tabs
        for tab in &app.tabs {
            assert_eq!(tab.visible_rows_map.len(), tab.visible_row_starts.len());
            assert_eq!(tab.visible_rows_map.len(), tab.visible_row_ends.len());
        }
    }

    #[test]
    fn rebuild_visible_rows_fold_plus_wrap_interaction() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let file = root.join("test.rs");
        // 6 lines (0-5): fold range covers lines 0-3, so folding hides 1,2,3
        fs::write(
            &file,
            "fn main() {\n    a very long body line that should wrap\n    short\n}\nfn other() {}\nextra\n",
        )
        .expect("write");
        let mut app = new_app(root);
        app.open_file(file).expect("open");
        app.word_wrap = true;
        app.editor_rect = Rect::new(0, 0, 22, 20); // wrap_width ~ 10
        // Fold the function body (lines 1-3 hidden, fold range 0..3)
        app.tabs[app.active_tab].folded_starts.insert(0);
        app.rebuild_visible_rows();
        let tab = app.active_tab().expect("tab");
        // Folded interior lines should NOT appear
        assert!(!tab.visible_rows_map.contains(&1));
        assert!(!tab.visible_rows_map.contains(&2));
        assert!(!tab.visible_rows_map.contains(&3));
        // Fold header (line 0) and lines after fold (4, 5) should appear
        assert!(tab.visible_rows_map.contains(&0));
        assert!(tab.visible_rows_map.contains(&4));
        assert!(tab.visible_rows_map.contains(&5));
    }

    #[test]
    fn wrap_rebuild_deadline_initialized_none() {
        let tmp = tempdir().expect("tempdir");
        let app = new_app(tmp.path());
        assert!(app.wrap_rebuild_deadline.is_none());
    }

    #[test]
    fn poll_wrap_rebuild_fires_after_deadline() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let file = root.join("test.txt");
        fs::write(&file, "content\n").expect("write");
        let mut app = new_app(root);
        app.open_file(file).expect("open");
        app.word_wrap = true;
        // Set deadline in the past so it fires immediately
        app.wrap_rebuild_deadline =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(1));
        app.poll_wrap_rebuild();
        assert!(app.wrap_rebuild_deadline.is_none(), "deadline should be cleared");
    }

    #[test]
    fn poll_wrap_rebuild_skips_before_deadline() {
        let tmp = tempdir().expect("tempdir");
        let mut app = new_app(tmp.path());
        // Set deadline far in the future
        app.wrap_rebuild_deadline =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(60));
        app.poll_wrap_rebuild();
        assert!(
            app.wrap_rebuild_deadline.is_some(),
            "deadline should NOT be cleared yet"
        );
    }
}
