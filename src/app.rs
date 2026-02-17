use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use arboard::Clipboard;
use notify::RecommendedWatcher;
use ratatui::layout::Rect;

use crate::keybinds::{KeyAction, KeyBind, KeyBindings};
use crate::lsp_client::{LspClient, LspCompletionItem};
use crate::tab::{GitChangeSummary, GitFileStatus, ProjectSearchHit, Tab};
use crate::theme::Theme;
use crate::tree_item::TreeItem;
use crate::types::{CommandAction, Focus, PendingAction, PromptState};

mod core;
mod editor;
mod file_tree;
mod input;
mod input_handlers;
mod lsp;
mod search;

pub(crate) struct ContextMenuState {
    pub(crate) open: bool,
    pub(crate) index: usize,
    pub(crate) target: Option<PathBuf>,
    pub(crate) pos: (u16, u16),
    pub(crate) rect: Rect,
}

pub(crate) struct SearchResultsState {
    pub(crate) open: bool,
    pub(crate) query: String,
    pub(crate) results: Vec<ProjectSearchHit>,
    pub(crate) index: usize,
}

pub(crate) struct CompletionState {
    pub(crate) open: bool,
    pub(crate) items: Vec<LspCompletionItem>,
    pub(crate) index: usize,
    pub(crate) rect: Rect,
    pub(crate) ghost: Option<String>,
    pub(crate) prefix: String,
}

impl CompletionState {
    pub(crate) fn reset(&mut self) {
        self.open = false;
        self.ghost = None;
        self.prefix.clear();
    }
}

pub(crate) struct KeybindEditorState {
    pub(crate) open: bool,
    pub(crate) index: usize,
    pub(crate) recording: bool,
    pub(crate) query: String,
    pub(crate) conflict: Option<(KeyBind, KeyAction)>,
    pub(crate) actions: Vec<KeyAction>,
}

pub(crate) struct FsChangeEvent {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) full_refresh: bool,
}

pub(crate) struct App {
    pub(crate) root: PathBuf,
    pub(crate) tree: Vec<TreeItem>,
    pub(crate) selected: usize,
    pub(crate) expanded: HashSet<PathBuf>,
    pub(crate) focus: Focus,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active_tab: usize,
    pub(crate) last_tree_click: Option<(Instant, usize)>,
    pub(crate) status: String,
    pub(crate) pending: PendingAction,
    pub(crate) quit: bool,
    pub(crate) files_view_open: bool,
    pub(crate) files_pane_width: u16,
    pub(crate) divider_dragging: bool,
    pub(crate) menu_open: bool,
    pub(crate) menu_index: usize,
    pub(crate) menu_query: String,
    pub(crate) menu_results: Vec<CommandAction>,
    pub(crate) theme_browser_open: bool,
    pub(crate) theme_index: usize,
    pub(crate) preview_revert_index: usize,
    pub(crate) themes: Vec<Theme>,
    pub(crate) active_theme_index: usize,
    pub(crate) help_open: bool,
    pub(crate) tree_rect: Rect,
    pub(crate) editor_rect: Rect,
    pub(crate) divider_rect: Rect,
    pub(crate) tab_rects: Vec<(Rect, Rect)>,
    pub(crate) context_menu: ContextMenuState,
    pub(crate) prompt: Option<PromptState>,
    pub(crate) clipboard: Option<Clipboard>,
    pub(crate) editor_context_menu_open: bool,
    pub(crate) editor_context_menu_index: usize,
    pub(crate) editor_context_menu_pos: (u16, u16),
    pub(crate) editor_context_menu_rect: Rect,
    pub(crate) editor_dragging: bool,
    pub(crate) editor_drag_anchor: Option<(usize, usize)>,
    pub(crate) search_results: SearchResultsState,
    pub(crate) file_picker_open: bool,
    pub(crate) file_picker_query: String,
    pub(crate) file_picker_results: Vec<PathBuf>,
    pub(crate) file_picker_index: usize,
    pub(crate) lsp: Option<LspClient>,
    pub(crate) completion: CompletionState,
    pub(crate) pending_completion_request: Option<i64>,
    pub(crate) pending_definition_request: Option<i64>,
    pub(crate) fs_watcher: Option<RecommendedWatcher>,
    pub(crate) fs_rx: Option<Receiver<FsChangeEvent>>,
    pub(crate) fs_refresh_pending: bool,
    pub(crate) fs_full_refresh_pending: bool,
    pub(crate) fs_changed_paths: HashSet<PathBuf>,
    pub(crate) last_fs_refresh: Instant,
    pub(crate) autosave_last_write: Instant,
    pub(crate) replace_after_find: bool,
    pub(crate) git_branch: Option<String>,
    pub(crate) enhanced_keys: bool,
    pub(crate) keybinds: KeyBindings,
    pub(crate) keybind_editor: KeybindEditorState,
    pub(crate) git_file_statuses: HashMap<PathBuf, GitFileStatus>,
    pub(crate) git_change_summary: GitChangeSummary,
}
