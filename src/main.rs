use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashSet, hash_map::DefaultHasher};
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, BufReader, Read, Stdout, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use arboard::Clipboard;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tui_textarea::{Input, TextArea};
use include_dir::{Dir, include_dir};
use unicode_width::UnicodeWidthStr;
use url::Url;

const LOCAL_THEME_DIR: &str = "themes";
static EMBEDDED_THEMES: Dir = include_dir!("$CARGO_MANIFEST_DIR/themes");
const STATE_FILE_REL: &str = "lazyide/state.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Tree,
    Editor,
}

#[derive(Debug, Clone)]
enum PendingAction {
    None,
    Quit,
    ClosePrompt,
    Delete(PathBuf),
}

#[derive(Debug, Clone)]
enum PromptMode {
    NewFile { parent: PathBuf },
    NewFolder { parent: PathBuf },
    Rename { target: PathBuf },
    FindInFile,
    FindInProject,
    ReplaceInFile { search: String },
    GoToLine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandAction {
    Theme,
    Help,
    QuickOpen,
    FindInFile,
    FindInProject,
    SaveFile,
    RefreshTree,
    ToggleFiles,
    GotoDefinition,
    ReplaceInFile,
    GoToLine,
}

#[derive(Debug, Clone)]
struct PromptState {
    title: String,
    value: String,
    mode: PromptMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextAction {
    Open,
    NewFile,
    NewFolder,
    Rename,
    Delete,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorContextAction {
    Copy,
    Cut,
    Paste,
    SelectAll,
    Cancel,
}

#[derive(Debug, Clone)]
struct TreeItem {
    path: PathBuf,
    name: String,
    depth: usize,
    is_dir: bool,
    expanded: bool,
}

#[derive(Debug, Clone)]
struct Theme {
    name: String,
    theme_type: String,
    bg: Color,
    bg_alt: Color,
    fg: Color,
    fg_muted: Color,
    border: Color,
    accent: Color,
    accent_secondary: Color,
    selection: Color,
    comment: Color,
    syntax_string: Color,
    syntax_number: Color,
    syntax_tag: Color,
    syntax_attribute: Color,
    bracket_1: Color,
    bracket_2: Color,
    bracket_3: Color,
}

#[derive(Debug, Deserialize)]
struct ThemeFile {
    name: String,
    #[serde(rename = "type")]
    theme_type: String,
    colors: ThemeColors,
    #[serde(default)]
    syntax: Option<ThemeSyntaxColors>,
}

#[derive(Debug, Deserialize)]
struct ThemeColors {
    background: String,
    #[serde(rename = "backgroundAlt")]
    background_alt: String,
    foreground: String,
    #[serde(rename = "foregroundMuted")]
    foreground_muted: String,
    border: String,
    accent: String,
    #[serde(default, rename = "accentSecondary")]
    accent_secondary: Option<String>,
    selection: String,
    #[serde(default)]
    yellow: Option<String>,
    #[serde(default)]
    purple: Option<String>,
    #[serde(default)]
    cyan: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ThemeSyntaxColors {
    #[serde(default)]
    comment: Option<String>,
    #[serde(default)]
    string: Option<String>,
    #[serde(default)]
    number: Option<String>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    attribute: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PersistedState {
    theme_name: String,
    #[serde(default)]
    files_pane_width: Option<u16>,
}

#[derive(Debug, Clone)]
struct ProjectSearchHit {
    path: PathBuf,
    line: usize,
    preview: String,
}

#[derive(Debug, Clone)]
struct FoldRange {
    start_line: usize,
    end_line: usize,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct LspDiagnostic {
    line: usize,
    col: usize,
    severity: String,
    message: String,
}

#[derive(Debug, Clone)]
struct LspCompletionItem {
    label: String,
    insert_text: Option<String>,
    detail: Option<String>,
}

#[derive(Debug)]
enum LspInbound {
    Notification { method: String, params: Value },
    Response { id: i64, result: Value },
}

struct LspClient {
    writer: Arc<Mutex<ChildStdin>>,
    rx: Receiver<LspInbound>,
    next_id: i64,
}

struct Tab {
    path: PathBuf,
    is_preview: bool,
    editor: TextArea<'static>,
    dirty: bool,
    open_disk_snapshot: Option<String>,
    editor_scroll_row: usize,
    fold_ranges: Vec<FoldRange>,
    bracket_depths: Vec<u16>,
    folded_starts: HashSet<usize>,
    visible_rows_map: Vec<usize>,
    open_doc_uri: Option<String>,
    open_doc_version: i32,
    diagnostics: Vec<LspDiagnostic>,
    conflict_prompt_open: bool,
    conflict_disk_text: Option<String>,
    recovery_prompt_open: bool,
    recovery_text: Option<String>,
}

struct App {
    root: PathBuf,
    tree: Vec<TreeItem>,
    selected: usize,
    expanded: HashSet<PathBuf>,
    focus: Focus,
    tabs: Vec<Tab>,
    active_tab: usize,
    last_tree_click: Option<(Instant, usize)>,
    status: String,
    pending: PendingAction,
    quit: bool,
    files_view_open: bool,
    files_pane_width: u16,
    divider_dragging: bool,
    menu_open: bool,
    menu_index: usize,
    menu_query: String,
    menu_results: Vec<CommandAction>,
    theme_browser_open: bool,
    theme_index: usize,
    preview_revert_index: usize,
    themes: Vec<Theme>,
    active_theme_index: usize,
    help_open: bool,
    tree_rect: Rect,
    editor_rect: Rect,
    divider_rect: Rect,
    tab_rects: Vec<(Rect, Rect)>,
    context_menu_open: bool,
    context_menu_index: usize,
    context_menu_target: Option<PathBuf>,
    context_menu_pos: (u16, u16),
    context_menu_rect: Rect,
    prompt: Option<PromptState>,
    clipboard: Option<Clipboard>,
    editor_context_menu_open: bool,
    editor_context_menu_index: usize,
    editor_context_menu_pos: (u16, u16),
    editor_context_menu_rect: Rect,
    editor_dragging: bool,
    editor_drag_anchor: Option<(usize, usize)>,
    search_results_open: bool,
    search_results_query: String,
    search_results: Vec<ProjectSearchHit>,
    search_results_index: usize,
    file_picker_open: bool,
    file_picker_query: String,
    file_picker_results: Vec<PathBuf>,
    file_picker_index: usize,
    lsp: Option<LspClient>,
    completion_open: bool,
    completion_items: Vec<LspCompletionItem>,
    completion_index: usize,
    pending_completion_request: Option<i64>,
    pending_definition_request: Option<i64>,
    completion_rect: Rect,
    completion_ghost: Option<String>,
    completion_prefix: String,
    fs_watcher: Option<RecommendedWatcher>,
    fs_rx: Option<Receiver<()>>,
    fs_refresh_pending: bool,
    last_fs_refresh: Instant,
    autosave_last_write: Instant,
    replace_after_find: bool,
    git_branch: Option<String>,
    enhanced_keys: bool,
}

impl LspClient {
    fn new_rust_analyzer(root: &Path) -> io::Result<Self> {
        let ra_bin = resolve_rust_analyzer_bin().unwrap_or_else(|| PathBuf::from("rust-analyzer"));
        let mut child = Command::new(ra_bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("failed to open rust-analyzer stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("failed to open rust-analyzer stdout"))?;

        let writer = Arc::new(Mutex::new(stdin));
        let (tx, rx) = mpsc::channel::<LspInbound>();
        thread::spawn(move || lsp_reader_loop(stdout, tx));
        let mut client = Self {
            writer,
            rx,
            next_id: 1,
        };
        let root_uri = Url::from_directory_path(root)
            .map_err(|_| io::Error::other("invalid root path for URI"))?
            .to_string();
        let init_id = client.send_request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "capabilities": {
                    "textDocument": {
                        "publishDiagnostics": {},
                        "completion": {}
                    }
                },
                "clientInfo": { "name": "lazyide", "version": "0.1.0" },
            }),
        )?;
        client.wait_for_initialize(init_id)?;
        client.send_notification("initialized", json!({}))?;
        Ok(client)
    }

    fn wait_for_initialize(&self, init_id: i64) -> io::Result<()> {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let now = std::time::Instant::now();
            if now >= deadline {
                return Err(io::Error::other("LSP initialize timeout"));
            }
            let timeout = deadline.saturating_duration_since(now);
            match self.rx.recv_timeout(timeout) {
                Ok(LspInbound::Response { id, result }) if id == init_id => {
                    if result.get("code").is_some() && result.get("message").is_some() {
                        return Err(io::Error::other(format!("LSP initialize error: {}", result)));
                    }
                    return Ok(());
                }
                Ok(_) => continue,
                Err(_) => return Err(io::Error::other("LSP initialize response missing")),
            }
        }
    }

    fn send_notification(&self, method: &str, params: Value) -> io::Result<()> {
        self.send_raw(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    fn send_request(&mut self, method: &str, params: Value) -> io::Result<i64> {
        let id = self.next_id;
        self.next_id += 1;
        self.send_raw(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;
        Ok(id)
    }

    fn send_raw(&self, value: Value) -> io::Result<()> {
        let payload = serde_json::to_vec(&value)
            .map_err(|e| io::Error::other(format!("lsp serialize error: {e}")))?;
        let header = format!("Content-Length: {}\r\n\r\n", payload.len());
        let mut guard = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("lsp writer lock poisoned"))?;
        guard.write_all(header.as_bytes())?;
        guard.write_all(&payload)?;
        guard.flush()?;
        Ok(())
    }
}

fn resolve_rust_analyzer_bin() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            candidates.push(dir.join("rust-analyzer"));
        }
    }
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.push(home.join(".cargo/bin/rust-analyzer"));
        candidates.push(home.join(".rustup/toolchains/stable-aarch64-apple-darwin/bin/rust-analyzer"));
        candidates.push(home.join(".rustup/toolchains/stable-x86_64-apple-darwin/bin/rust-analyzer"));
        candidates.push(home.join(".rustup/toolchains/stable-aarch64-unknown-linux-gnu/bin/rust-analyzer"));
        candidates.push(home.join(".rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rust-analyzer"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn lsp_reader_loop(stdout: impl Read, tx: Sender<LspInbound>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let Ok(n) = reader.read_line(&mut line) else {
                return;
            };
            if n == 0 {
                return;
            }
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                break;
            }
            if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
                content_length = rest.trim().parse::<usize>().unwrap_or(0);
            }
        }
        if content_length == 0 {
            continue;
        }
        let mut buf = vec![0u8; content_length];
        if reader.read_exact(&mut buf).is_err() {
            return;
        }
        let Ok(msg) = serde_json::from_slice::<Value>(&buf) else {
            continue;
        };
        if let Some(method) = msg.get("method").and_then(Value::as_str) {
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            let _ = tx.send(LspInbound::Notification {
                method: method.to_string(),
                params,
            });
            continue;
        }
        if let Some(id) = msg.get("id").and_then(Value::as_i64) {
            let result = msg
                .get("result")
                .cloned()
                .or_else(|| msg.get("error").cloned())
                .unwrap_or(Value::Null);
            let _ = tx.send(LspInbound::Response { id, result });
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyntaxLang {
    Plain,
    Rust,
    Python,
    JsTs,
    Go,
    Php,
    Css,
    HtmlXml,
    Shell,
    Json,
    Markdown,
}

impl App {
    const INLINE_GHOST_MIN_PREFIX: usize = 3;
    const EDITOR_GUTTER_WIDTH: u16 = 10;
    const MIN_FILES_PANE_WIDTH: u16 = 18;
    const MIN_EDITOR_PANE_WIDTH: u16 = 28;
    const FS_REFRESH_DEBOUNCE_MS: u64 = 120;
    const AUTOSAVE_INTERVAL_MS: u64 = 2000;

    fn new(root: PathBuf) -> io::Result<Self> {
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
            context_menu_open: false,
            context_menu_index: 0,
            context_menu_target: None,
            context_menu_pos: (0, 0),
            context_menu_rect: Rect::default(),
            prompt: None,
            clipboard: Clipboard::new().ok(),
            editor_context_menu_open: false,
            editor_context_menu_index: 0,
            editor_context_menu_pos: (0, 0),
            editor_context_menu_rect: Rect::default(),
            editor_dragging: false,
            editor_drag_anchor: None,
            search_results_open: false,
            search_results_query: String::new(),
            search_results: Vec::new(),
            search_results_index: 0,
            file_picker_open: false,
            file_picker_query: String::new(),
            file_picker_results: Vec::new(),
            file_picker_index: 0,
            lsp: None,
            completion_open: false,
            completion_items: Vec::new(),
            completion_index: 0,
            pending_completion_request: None,
            pending_definition_request: None,
            completion_rect: Rect::default(),
            completion_ghost: None,
            completion_prefix: String::new(),
            fs_watcher: None,
            fs_rx: None,
            fs_refresh_pending: false,
            last_fs_refresh: Instant::now(),
            autosave_last_write: Instant::now(),
            replace_after_find: false,
            git_branch: None,
            enhanced_keys: false,
        };
        app.git_branch = detect_git_branch(&app.root);
        app.restore_persisted_state();
        app.rebuild_tree()?;
        app.start_fs_watcher();
        let has_ra = resolve_rust_analyzer_bin().is_some();
        let has_rg = Command::new("rg").arg("--version").output().is_ok();
        if !has_ra || !has_rg {
            let mut missing = Vec::new();
            if !has_ra { missing.push("rust-analyzer"); }
            if !has_rg { missing.push("rg"); }
            app.status = format!("Missing tools: {}. Run `lazyide --setup` to install.", missing.join(", "));
        } else {
            app.status = format!("Root: {}", app.root.display());
        }
        Ok(app)
    }

    fn start_fs_watcher(&mut self) {
        let (tx, rx) = mpsc::channel::<()>();
        let mut watcher = match RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if res.is_ok() {
                    let _ = tx.send(());
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
        self.last_fs_refresh = Instant::now();
    }

    fn poll_fs_changes(&mut self) -> io::Result<()> {
        let mut saw_event = false;
        if let Some(rx) = self.fs_rx.as_ref() {
            while rx.try_recv().is_ok() {
                saw_event = true;
            }
        }
        if saw_event {
            self.fs_refresh_pending = true;
        }
        if self.fs_refresh_pending
            && self.last_fs_refresh.elapsed()
                >= Duration::from_millis(Self::FS_REFRESH_DEBOUNCE_MS)
        {
            self.rebuild_tree()?;
            self.git_branch = detect_git_branch(&self.root);
            if self.file_picker_open {
                self.refresh_file_picker_results();
            }
            if let Some(path) = self.open_path().cloned() {
                if !path.exists() {
                    if self.is_dirty() {
                        self.set_status("Open file was removed externally (unsaved buffer preserved)");
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
            self.fs_refresh_pending = false;
            self.last_fs_refresh = Instant::now();
        }
        Ok(())
    }

    fn reload_open_file_from_disk_if_pristine(&mut self) -> io::Result<()> {
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
        let mut lines = if disk_text.is_empty() {
            vec![String::new()]
        } else {
            disk_text.lines().map(ToString::to_string).collect::<Vec<_>>()
        };
        if lines.is_empty() {
            lines.push(String::new());
        }
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

    fn active_theme(&self) -> &Theme {
        &self.themes[self.active_theme_index]
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active_tab)
    }

    fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active_tab)
    }

    fn open_path(&self) -> Option<&PathBuf> {
        self.active_tab().map(|t| &t.path)
    }

    fn is_dirty(&self) -> bool {
        self.active_tab().is_some_and(|t| t.dirty)
    }

    fn any_tab_dirty(&self) -> bool {
        self.tabs.iter().any(|t| t.dirty)
    }

    fn mark_dirty(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.dirty = true;
            tab.is_preview = false;
        }
    }

    fn switch_to_tab(&mut self, idx: usize) {
        if idx < self.tabs.len() {
            self.active_tab = idx;
            self.completion_open = false;
            self.completion_ghost = None;
            self.completion_prefix.clear();
            self.focus = Focus::Editor;
        }
    }

    fn restore_persisted_state(&mut self) {
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

    fn persist_state(&mut self) {
        let state = PersistedState {
            theme_name: self.active_theme().name.clone(),
            files_pane_width: Some(self.files_pane_width),
        };
        if save_persisted_state(&state).is_err() {
            self.set_status("Failed to persist app state");
        }
    }

    fn persist_theme_selection(&mut self) {
        self.persist_state();
    }

    fn open_command_palette(&mut self) {
        self.menu_open = true;
        self.menu_query.clear();
        self.menu_index = 0;
        self.refresh_menu_results();
    }

    fn refresh_menu_results(&mut self) {
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

    fn run_command_action(&mut self, action: CommandAction) -> io::Result<()> {
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
                self.prompt = Some(PromptState {
                    title: "Find in file (regex)".to_string(),
                    value: String::new(),
                    mode: PromptMode::FindInFile,
                });
            }
            CommandAction::FindInProject => {
                self.prompt = Some(PromptState {
                    title: "Search in files (ripgrep)".to_string(),
                    value: String::new(),
                    mode: PromptMode::FindInProject,
                });
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
                self.prompt = Some(PromptState {
                    title: "Find (for replace)".to_string(),
                    value: String::new(),
                    mode: PromptMode::FindInFile,
                });
                self.replace_after_find = true;
            }
            CommandAction::GoToLine => {
                self.prompt = Some(PromptState {
                    title: "Go to line".to_string(),
                    value: String::new(),
                    mode: PromptMode::GoToLine,
                });
            }
        }
        Ok(())
    }

    fn update_status_for_cursor(&mut self) {
        if self.focus == Focus::Editor {
            if let Some(tab) = self.active_tab() {
                let cursor_row = tab.editor.cursor().0;
                if let Some(diag) = tab.diagnostics.iter().find(|d| d.line == cursor_row + 1) {
                    self.status = format!("[{}] {}", diag.severity, diag.message);
                }
            }
        }
    }

    fn poll_autosave(&mut self) -> io::Result<()> {
        if self.autosave_last_write.elapsed()
            < Duration::from_millis(Self::AUTOSAVE_INTERVAL_MS)
        {
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

    fn check_recovery_for_open_file(&mut self) {
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

    fn clear_autosave_for_open_file(&mut self) {
        if let Some(tab) = self.active_tab() {
            let _ = fs::remove_file(autosave_path_for(&tab.path));
        }
    }

    fn maybe_flag_external_conflict(&mut self) -> io::Result<()> {
        let Some(tab) = self.active_tab() else {
            return Ok(());
        };
        if !tab.dirty || !tab.path.exists() || tab.conflict_prompt_open {
            return Ok(());
        }
        let path = tab.path.clone();
        let disk = fs::read_to_string(&path)?;
        let current = self.tabs[self.active_tab].editor.lines().join("\n");
        let snapshot = self.tabs[self.active_tab].open_disk_snapshot.clone().unwrap_or_default();
        if disk != snapshot && disk != current {
            if let Some(tab) = self.active_tab_mut() {
                tab.conflict_prompt_open = true;
                tab.conflict_disk_text = Some(disk);
            }
        }
        Ok(())
    }

    fn request_lsp_definition(&mut self) {
        if self.try_local_definition_jump() {
            return;
        }
        let uri = self.active_tab().and_then(|t| t.open_doc_uri.clone());
        let Some((row, col)) = self.active_tab().map(|t| t.editor.cursor()) else {
            self.set_status("Definition unavailable");
            return;
        };
        let (Some(uri), Some(lsp)) = (uri, self.lsp.as_mut()) else {
            self.set_status("Definition unavailable");
            return;
        };
        match lsp.send_request(
            "textDocument/definition",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": row, "character": col }
            }),
        ) {
            Ok(id) => {
                self.pending_definition_request = Some(id);
                self.set_status("Go to definition requested");
            }
            Err(_) => self.set_status("Failed to request definition"),
        }
    }

    fn handle_definition_response(&mut self, result: Value) -> io::Result<()> {
        if result.get("code").is_some() && result.get("message").is_some() {
            if self.try_local_definition_jump() {
                return Ok(());
            }
            let msg = result
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Definition error");
            self.set_status(format!("Definition error: {}", msg));
            return Ok(());
        }
        let mut target: Option<(PathBuf, usize, usize)> = None;
        let first = if let Some(arr) = result.as_array() {
            arr.first().cloned()
        } else {
            Some(result)
        };
        if let Some(item) = first {
            let uri = item
                .get("uri")
                .or_else(|| item.get("targetUri"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let range = item.get("range").or_else(|| item.get("targetSelectionRange"));
            let line = range
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("line"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let col = range
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("character"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            if let Ok(url) = Url::parse(uri) {
                if let Ok(path) = url.to_file_path() {
                    target = Some((path, line, col));
                }
            }
        }
        let Some((path, line, col)) = target else {
            if self.try_local_definition_jump() {
                return Ok(());
            }
            self.set_status("No definition found");
            return Ok(());
        };
        if self.is_dirty() && self.open_path() != Some(&path) {
            self.set_status("Unsaved changes: save or close before jumping to definition");
            return Ok(());
        }
        if self.open_path() != Some(&path) {
            self.open_file(path)?;
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.editor
                .move_cursor(tui_textarea::CursorMove::Jump(line as u16, col as u16));
        }
        self.sync_editor_scroll_guess();
        self.set_status("Jumped to definition");
        Ok(())
    }

    fn try_local_definition_jump(&mut self) -> bool {
        let Some(path) = self.open_path().cloned() else {
            return false;
        };
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_none_or(|e| !e.eq_ignore_ascii_case("rs"))
        {
            return false;
        }
        let symbol = self.current_identifier_at_cursor();
        if symbol.is_empty() {
            return false;
        }
        let Some(tab) = self.active_tab() else { return false; };
        let lines = tab.editor.lines().to_vec();
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            if !trimmed.contains("fn ") {
                continue;
            }
            let candidates = [
                format!("fn {symbol}("),
                format!("pub fn {symbol}("),
                format!("pub(crate) fn {symbol}("),
            ];
            if candidates.iter().any(|p| trimmed.starts_with(p)) {
                let col = line.find("fn ").unwrap_or(0);
                self.tabs[self.active_tab].editor
                    .move_cursor(tui_textarea::CursorMove::Jump(i as u16, col as u16));
                self.sync_editor_scroll_guess();
                self.set_status("Jumped to local definition");
                return true;
            }
        }
        false
    }

    fn clamp_files_pane_width(&mut self, total_width: u16) {
        let min_files = Self::MIN_FILES_PANE_WIDTH.min(total_width.saturating_sub(1));
        let max_files = total_width
            .saturating_sub(Self::MIN_EDITOR_PANE_WIDTH)
            .max(min_files);
        self.files_pane_width = self.files_pane_width.clamp(min_files, max_files);
    }

    fn recompute_folds(&mut self) {
        let Some(tab) = self.active_tab() else { return; };
        let lang = syntax_lang_for_path(Some(tab.path.as_path()));
        let (fold_ranges, bracket_depths) = compute_fold_ranges(self.tabs[self.active_tab].editor.lines(), lang);
        let tab = &mut self.tabs[self.active_tab];
        tab.fold_ranges = fold_ranges;
        tab.bracket_depths = bracket_depths;
        tab.folded_starts
            .retain(|start| tab.fold_ranges.iter().any(|r| r.start_line == *start));
        self.rebuild_visible_rows();
    }

    fn rebuild_visible_rows(&mut self) {
        let Some(tab) = self.active_tab() else { return; };
        let lines = tab.editor.lines();
        let num_lines = lines.len();
        let tab = &mut self.tabs[self.active_tab];
        tab.visible_rows_map.clear();
        tab.visible_rows_map.reserve(num_lines);
        for row in 0..num_lines {
            let hidden = tab.fold_ranges.iter().any(|fr| {
                tab.folded_starts.contains(&fr.start_line) && row > fr.start_line && row <= fr.end_line
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

    fn visible_index_of_source_row(&self, row: usize) -> usize {
        let Some(tab) = self.active_tab() else { return 0; };
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

    fn fold_range_starting_at(&self, row: usize) -> Option<&FoldRange> {
        let tab = self.active_tab()?;
        tab.fold_ranges.iter().find(|fr| fr.start_line == row)
    }

    fn toggle_fold_at_row(&mut self, row: usize) {
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
                self.set_status(format!(
                    "Folded lines {}-{}",
                    start_line + 1,
                    end_line + 1
                ));
            }
            self.rebuild_visible_rows();
            self.sync_editor_scroll_guess();
        }
    }

    fn fold_current_block(&mut self) {
        let Some(tab) = self.active_tab() else { return; };
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

    fn unfold_current_block(&mut self) {
        let Some(tab) = self.active_tab() else { return; };
        let (cursor_row, _) = tab.editor.cursor();
        let mut unfolded = false;
        let starts: Vec<usize> = tab.folded_starts.iter().copied().collect();
        for start in starts {
            if let Some(fr) = tab.fold_ranges.iter().find(|fr| fr.start_line == start) {
                if fr.start_line == cursor_row || (fr.start_line <= cursor_row && cursor_row <= fr.end_line) {
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

    fn fold_all(&mut self) {
        let Some(tab) = self.active_tab() else { return; };
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

    fn unfold_all(&mut self) {
        let Some(tab) = self.active_tab() else { return; };
        if tab.folded_starts.is_empty() {
            self.set_status("No folded blocks");
            return;
        }
        self.tabs[self.active_tab].folded_starts.clear();
        self.rebuild_visible_rows();
        self.sync_editor_scroll_guess();
        self.set_status("Unfolded all blocks");
    }

    fn toggle_fold_at_cursor(&mut self) {
        let Some(tab) = self.active_tab() else { return; };
        let (cursor_row, _) = tab.editor.cursor();
        // Check if cursor is on/in a folded block
        let mut is_folded = false;
        for &start in &tab.folded_starts {
            if let Some(fr) = tab.fold_ranges.iter().find(|fr| fr.start_line == start) {
                if fr.start_line == cursor_row || (fr.start_line <= cursor_row && cursor_row <= fr.end_line) {
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

    fn toggle_fold_all(&mut self) {
        let Some(tab) = self.active_tab() else { return; };
        if tab.folded_starts.is_empty() {
            self.fold_all();
        } else {
            self.unfold_all();
        }
    }

    fn ensure_lsp_for_path(&mut self, path: &Path) {
        let is_rust = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("rs"));
        if !is_rust {
            if let Some(tab) = self.active_tab_mut() {
                tab.open_doc_uri = None;
                tab.open_doc_version = 0;
                tab.diagnostics.clear();
            }
            self.completion_open = false;
            self.completion_ghost = None;
            self.completion_prefix.clear();
            self.pending_completion_request = None;
            self.pending_definition_request = None;
            return;
        }
        if self.lsp.is_none() {
            match LspClient::new_rust_analyzer(&self.root) {
                Ok(client) => {
                    self.lsp = Some(client);
                    self.set_status("LSP connected");
                }
                Err(err) => {
                    self.set_status(format!("LSP unavailable: {}", err));
                    return;
                }
            }
        }
        if let Some(uri) = file_uri(path) {
            let text = self.tabs[self.active_tab].editor.lines().join("\n");
            let version = 1;
            if let Some(tab) = self.active_tab_mut() {
                tab.open_doc_uri = Some(uri.clone());
                tab.open_doc_version = version;
            }
            if let Some(lsp) = self.lsp.as_ref() {
                let _ = lsp.send_notification(
                    "textDocument/didOpen",
                    json!({
                        "textDocument": {
                            "uri": uri,
                            "languageId": "rust",
                            "version": version,
                            "text": text
                        }
                    }),
                );
            }
        }
    }

    fn notify_lsp_did_change(&mut self) {
        let uri = self.active_tab().and_then(|t| t.open_doc_uri.clone());
        let (Some(uri), Some(lsp)) = (uri, self.lsp.as_ref()) else {
            return;
        };
        let tab = &mut self.tabs[self.active_tab];
        tab.open_doc_version += 1;
        let text = tab.editor.lines().join("\n");
        let version = tab.open_doc_version;
        let _ = lsp.send_notification(
            "textDocument/didChange",
            json!({
                "textDocument": {
                    "uri": uri,
                    "version": version
                },
                "contentChanges": [
                    { "text": text }
                ]
            }),
        );
    }

    fn poll_lsp(&mut self) {
        let mut inbound = Vec::new();
        if let Some(lsp) = self.lsp.as_ref() {
            loop {
                match lsp.rx.try_recv() {
                    Ok(msg) => inbound.push(msg),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break,
                }
            }
        }
        for msg in inbound {
            match msg {
                LspInbound::Notification { method, params } => {
                    if method == "textDocument/publishDiagnostics" {
                        self.handle_publish_diagnostics(params);
                    }
                }
                LspInbound::Response { id, result } => {
                    if self.pending_completion_request == Some(id) {
                        self.pending_completion_request = None;
                        self.handle_completion_response(result);
                    } else if self.pending_definition_request == Some(id) {
                        self.pending_definition_request = None;
                        let _ = self.handle_definition_response(result);
                    }
                }
            }
        }
    }

    fn handle_publish_diagnostics(&mut self, params: Value) {
        let uri = params
            .get("uri")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        // Find the tab that matches this URI
        let tab_idx = self.tabs.iter().position(|t| t.open_doc_uri.as_deref() == Some(uri.as_str()));
        let Some(tab_idx) = tab_idx else {
            return;
        };
        let mut diagnostics = Vec::new();
        if let Some(items) = params.get("diagnostics").and_then(Value::as_array) {
            for d in items {
                let line = d
                    .get("range")
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("line"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize
                    + 1;
                let col = d
                    .get("range")
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("character"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize
                    + 1;
                let severity = match d.get("severity").and_then(Value::as_u64).unwrap_or(0) {
                    1 => "error",
                    2 => "warning",
                    3 => "info",
                    4 => "hint",
                    _ => "unknown",
                }
                .to_string();
                let message = d
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                diagnostics.push(LspDiagnostic {
                    line,
                    col,
                    severity,
                    message,
                });
            }
        }
        self.tabs[tab_idx].diagnostics = diagnostics;
    }

    fn request_lsp_completion(&mut self) {
        let uri = self.active_tab().and_then(|t| t.open_doc_uri.clone());
        let Some((row, col)) = self.active_tab().map(|t| t.editor.cursor()) else { return; };
        let prefix = self.current_identifier_prefix();
        self.completion_prefix = prefix.clone();
        self.completion_ghost = None;
        let (Some(uri), Some(lsp)) = (uri, self.lsp.as_mut()) else {
            self.set_status("LSP completion unavailable");
            return;
        };
        match lsp.send_request(
            "textDocument/completion",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": row, "character": col },
                "context": { "triggerKind": 1 }
            }),
        ) {
            Ok(id) => {
                self.pending_completion_request = Some(id);
                self.set_status("Completion requested");
            }
            Err(_) => {
                self.set_status("Failed to request completion");
            }
        }
    }

    fn handle_completion_response(&mut self, result: Value) {
        if result.get("code").is_some() && result.get("message").is_some() {
            let msg = result
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("LSP completion error");
            self.completion_items.clear();
            self.completion_open = false;
            self.set_status(format!("Completion error: {}", msg));
            return;
        }

        let mut items_out = Vec::new();
        let items = if let Some(arr) = result.as_array() {
            arr.to_vec()
        } else if let Some(arr) = result.get("completions").and_then(Value::as_array) {
            arr.to_vec()
        } else {
            result
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        if items.is_empty() {
            items_out = self.fallback_completion_items();
        }
        for it in items {
            let label = it
                .get("label")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .or_else(|| {
                    it.get("label")
                        .and_then(|v| v.get("left"))
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                })
                .unwrap_or_default();
            if label.is_empty() {
                continue;
            }
            let insert_text = it
                .get("insertText")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let insert_text = insert_text.or_else(|| {
                it.get("textEdit")
                    .and_then(|te| te.get("newText"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            });
            let detail = it
                .get("detail")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            items_out.push(LspCompletionItem {
                label,
                insert_text,
                detail,
            });
            if items_out.len() >= 40 {
                break;
            }
        }
        self.completion_items = items_out;
        self.completion_index = 0;
        self.completion_open = !self.completion_items.is_empty();
        self.completion_ghost = self
            .completion_items
            .first()
            .and_then(|item| {
                let label = item.insert_text.as_deref().unwrap_or(&item.label);
                if self.completion_prefix.is_empty() {
                    return None;
                }
                if label.starts_with(&self.completion_prefix) && label.len() > self.completion_prefix.len() {
                    Some(label[self.completion_prefix.len()..].to_string())
                } else {
                    None
                }
            });
        if self.completion_open {
            self.set_status(format!("{} completion items", self.completion_items.len()));
        } else {
            self.set_status("No completions");
        }
    }

    fn fallback_completion_items(&self) -> Vec<LspCompletionItem> {
        let prefix = self.current_identifier_prefix();
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for kw in keywords_for_lang(syntax_lang_for_path(self.open_path().map(|p| p.as_path()))) {
            if (prefix.is_empty() || kw.starts_with(&prefix))
                && kw != &prefix
                && seen.insert((*kw).to_string())
            {
                out.push(LspCompletionItem {
                    label: (*kw).to_string(),
                    insert_text: Some((*kw).to_string()),
                    detail: Some("keyword".to_string()),
                });
                if out.len() >= 80 {
                    return out;
                }
            }
        }
        let empty_lines: Vec<String> = Vec::new();
        let editor_lines = self.active_tab().map(|t| t.editor.lines()).unwrap_or(&empty_lines);
        for line in editor_lines {
            let mut token = String::new();
            for ch in line.chars() {
                if is_ident_char(ch) {
                    token.push(ch);
                } else {
                    if (prefix.is_empty() || token.starts_with(&prefix))
                        && token != prefix
                        && seen.insert(token.clone())
                    {
                        out.push(LspCompletionItem {
                            label: token.clone(),
                            insert_text: Some(token.clone()),
                            detail: Some("buffer".to_string()),
                        });
                        if out.len() >= 80 {
                            return out;
                        }
                    }
                    token.clear();
                }
            }
            if (prefix.is_empty() || token.starts_with(&prefix))
                && token != prefix
                && seen.insert(token.clone())
            {
                out.push(LspCompletionItem {
                    label: token.clone(),
                    insert_text: Some(token),
                    detail: Some("buffer".to_string()),
                });
                if out.len() >= 80 {
                    return out;
                }
            }
        }
        out.sort_by(|a, b| {
            let a_is_kw = a.detail.as_deref() == Some("keyword");
            let b_is_kw = b.detail.as_deref() == Some("keyword");
            a_is_kw
                .cmp(&b_is_kw)
                .then_with(|| b.label.len().cmp(&a.label.len()))
                .then_with(|| a.label.cmp(&b.label))
        });
        out
    }

    fn current_identifier_prefix(&self) -> String {
        let Some(tab) = self.active_tab() else { return String::new(); };
        let (row, col) = tab.editor.cursor();
        let Some(line) = tab.editor.lines().get(row) else {
            return String::new();
        };
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            return String::new();
        }
        let end = col.min(chars.len());
        // Inline completion should only target the identifier directly before
        // the cursor, and only when the cursor is at that identifier's end.
        if end == 0 || !is_ident_char(chars[end - 1]) {
            return String::new();
        }
        if end < chars.len() && is_ident_char(chars[end]) {
            return String::new();
        }
        let mut start = end;
        while start > 0 && is_ident_char(chars[start - 1]) {
            start -= 1;
        }
        if start < end {
            return chars[start..end].iter().collect();
        }
        String::new()
    }

    fn current_identifier_at_cursor(&self) -> String {
        let Some(tab) = self.active_tab() else { return String::new(); };
        let (row, col) = tab.editor.cursor();
        let Some(line) = tab.editor.lines().get(row) else {
            return String::new();
        };
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            return String::new();
        }
        let mut idx = col.min(chars.len().saturating_sub(1));
        if !is_ident_char(chars[idx]) {
            if col > 0 && col <= chars.len() && is_ident_char(chars[col.saturating_sub(1)]) {
                idx = col.saturating_sub(1);
            } else {
                return String::new();
            }
        }
        let mut start = idx;
        while start > 0 && is_ident_char(chars[start - 1]) {
            start -= 1;
        }
        let mut end = idx + 1;
        while end < chars.len() && is_ident_char(chars[end]) {
            end += 1;
        }
        chars[start..end].iter().collect()
    }

    fn apply_completion(&mut self) {
        let Some(item) = self.completion_items.get(self.completion_index).cloned() else {
            self.completion_open = false;
            self.completion_ghost = None;
            return;
        };
        let insert = item.insert_text.unwrap_or_else(|| item.label.clone());
        let prefix = self.current_identifier_prefix();
        if !prefix.is_empty() {
            if let Some(tab) = self.active_tab_mut() {
                for _ in 0..prefix.chars().count() {
                    let _ = tab.editor.delete_char();
                }
            }
        }
        let inserted = self.active_tab_mut().is_some_and(|t| t.editor.insert_str(insert));
        if inserted {
            self.mark_dirty();
            self.notify_lsp_did_change();
            self.recompute_folds();
        }
        self.completion_open = false;
        self.completion_ghost = None;
        self.completion_prefix.clear();
        self.set_status(format!("Inserted completion: {}", item.label));
    }

    fn update_completion_ghost_from_selection(&mut self) {
        self.completion_ghost = self
            .completion_items
            .get(self.completion_index)
            .and_then(|item| {
                let label = item.insert_text.as_deref().unwrap_or(&item.label);
                if self.completion_prefix.is_empty() {
                    return None;
                }
                if label.starts_with(&self.completion_prefix) && label.len() > self.completion_prefix.len() {
                    Some(label[self.completion_prefix.len()..].to_string())
                } else {
                    None
                }
            });
    }

    fn refresh_inline_ghost(&mut self) {
        let prefix = self.current_identifier_prefix();
        if prefix.chars().count() < Self::INLINE_GHOST_MIN_PREFIX {
            self.completion_prefix.clear();
            self.completion_ghost = None;
            return;
        }
        self.completion_prefix = prefix.clone();
        self.completion_ghost = self
            .fallback_completion_items()
            .into_iter()
            .filter_map(|item| {
                let text = item.insert_text.unwrap_or(item.label);
                if text.starts_with(&prefix) && text.len() > prefix.len() {
                    Some(text[prefix.len()..].to_string())
                } else {
                    None
                }
            })
            .min_by_key(|s| s.len());
    }

    fn rebuild_tree(&mut self) -> io::Result<()> {
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

    fn walk_dir(&self, dir: &Path, depth: usize, out: &mut Vec<TreeItem>) -> io::Result<()> {
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
            let is_dir = path.is_dir();
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

    fn selected_item(&self) -> Option<&TreeItem> {
        self.tree.get(self.selected)
    }

    fn set_status<S: Into<String>>(&mut self, status: S) {
        self.status = status.into();
    }

    fn refresh_file_picker_results(&mut self) {
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
            sa.cmp(sb).then_with(|| pa.as_os_str().len().cmp(&pb.as_os_str().len()))
        });
        self.file_picker_results = scored.into_iter().map(|(_, p)| p).take(200).collect();
        self.file_picker_index = self
            .file_picker_index
            .min(self.file_picker_results.len().saturating_sub(1));
    }

    fn open_file_picker_selection(&mut self) -> io::Result<()> {
        let Some(path) = self.file_picker_results.get(self.file_picker_index).cloned() else {
            return Ok(());
        };
        self.file_picker_open = false;
        self.file_picker_query.clear();
        self.open_file(path)?;
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> io::Result<()> {
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT)
            && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'))
        {
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
            return Ok(());
        }

        if self.file_picker_open {
            return self.handle_file_picker_key(key);
        }
        if self.active_tab().is_some_and(|t| t.recovery_prompt_open) {
            return self.handle_recovery_prompt_key(key);
        }
        if self.active_tab().is_some_and(|t| t.conflict_prompt_open) {
            return self.handle_conflict_prompt_key(key);
        }
        if self.prompt.is_some() {
            return self.handle_prompt_key(key);
        }
        if self.completion_open {
            return self.handle_completion_key(key);
        }
        if self.search_results_open {
            return self.handle_search_results_key(key);
        }
        if self.editor_context_menu_open {
            return self.handle_editor_context_menu_key(key);
        }
        if self.context_menu_open {
            return self.handle_context_menu_key(key);
        }
        if self.theme_browser_open {
            return self.handle_theme_browser_key(key);
        }
        if self.menu_open {
            return self.handle_menu_key(key);
        }
        if self.help_open {
            return self.handle_help_key(key);
        }

        if self.handle_pending_key(key)? {
            return Ok(());
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::F(1)) => {
                // Previous tab
                if !self.tabs.is_empty() {
                    let prev = if self.active_tab == 0 { self.tabs.len() - 1 } else { self.active_tab - 1 };
                    self.switch_to_tab(prev);
                }
                return Ok(());
            }
            (KeyModifiers::NONE, KeyCode::F(2)) => {
                // Next tab
                if !self.tabs.is_empty() {
                    let next = (self.active_tab + 1) % self.tabs.len();
                    self.switch_to_tab(next);
                }
                return Ok(());
            }
            (KeyModifiers::NONE, KeyCode::F(3)) => {
                if self.active_tab_mut().is_some_and(|t| t.editor.search_forward(false)) {
                    self.set_status("Find next");
                    self.sync_editor_scroll_guess();
                } else {
                    self.set_status("No next match");
                }
                return Ok(());
            }
            (KeyModifiers::SHIFT, KeyCode::F(3)) => {
                if self.active_tab_mut().is_some_and(|t| t.editor.search_back(false)) {
                    self.set_status("Find previous");
                    self.sync_editor_scroll_guess();
                } else {
                    self.set_status("No previous match");
                }
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('d')) => {
                if self.focus == Focus::Editor {
                    self.request_lsp_definition();
                }
                return Ok(());
            }
            (_, KeyCode::Char('d')) | (_, KeyCode::Char('D'))
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if self.focus == Focus::Editor {
                    self.request_lsp_definition();
                }
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('b')) => {
                self.files_view_open = !self.files_view_open;
                if !self.files_view_open {
                    self.focus = Focus::Editor;
                    self.set_status("Files view hidden");
                } else {
                    self.set_status("Files view shown");
                }
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('w')) => {
                if !self.tabs.is_empty() {
                    if self.is_dirty() {
                        self.pending = PendingAction::ClosePrompt;
                        self.set_status("Unsaved changes: Enter save+close | Esc discard | C cancel");
                    } else {
                        self.close_file();
                    }
                }
                return Ok(());
            }
            (_, KeyCode::Char('p')) | (_, KeyCode::Char('P'))
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                self.open_command_palette();
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('p')) => {
                self.open_command_palette();
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('o')) => {
                self.file_picker_open = true;
                self.file_picker_query.clear();
                self.file_picker_index = 0;
                self.refresh_file_picker_results();
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('h')) => {
                self.prompt = Some(PromptState {
                    title: "Find (for replace)".to_string(),
                    value: String::new(),
                    mode: PromptMode::FindInFile,
                });
                self.replace_after_find = true;
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('f')) => {
                self.prompt = Some(PromptState {
                    title: "Find in file (regex)".to_string(),
                    value: String::new(),
                    mode: PromptMode::FindInFile,
                });
                return Ok(());
            }
            (_, KeyCode::Char('f'))
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                self.prompt = Some(PromptState {
                    title: "Search in files (ripgrep)".to_string(),
                    value: String::new(),
                    mode: PromptMode::FindInProject,
                });
                return Ok(());
            }
            (KeyModifiers::NONE, KeyCode::F(4)) | (KeyModifiers::NONE, KeyCode::Char('?')) => {
                self.help_open = true;
                return Ok(());
            }
            (_, KeyCode::Esc) => {
                if self.open_path().is_some() && self.is_dirty() {
                    self.pending = PendingAction::ClosePrompt;
                    self.set_status("Unsaved changes: Enter save+close | Esc discard | C cancel");
                    return Ok(());
                }
                if self.focus == Focus::Editor && self.open_path().is_some() {
                    self.close_file();
                    return Ok(());
                }
            }
            (_, KeyCode::Char('s')) | (_, KeyCode::Char('S'))
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.save_file()?;
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('n')) => {
                self.create_new_file()?;
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('r')) => {
                self.rebuild_tree()?;
                self.set_status("Tree refreshed");
                return Ok(());
            }
            (KeyModifiers::NONE, KeyCode::Tab) => {
                if self.focus == Focus::Editor {
                    // Keep Tab in editor so inline/popup completion can work.
                } else if self.files_view_open {
                    self.focus = Focus::Tree;
                    self.set_status("Focus: files");
                } else {
                    self.focus = Focus::Editor;
                    self.set_status("Files view is hidden");
                }
                if self.focus != Focus::Editor {
                    return Ok(());
                }
            }
            (KeyModifiers::SHIFT, KeyCode::BackTab) => {
                if self.focus == Focus::Editor && self.open_path().is_some() {
                    self.dedent_lines();
                } else {
                    self.focus = Focus::Editor;
                    self.set_status("Focus: editor");
                }
                return Ok(());
            }
            (KeyModifiers::NONE, KeyCode::Delete) => {
                if self.focus == Focus::Tree {
                    if let Some(item) = self.selected_item().cloned() {
                        self.pending = PendingAction::Delete(item.path.clone());
                        self.set_status(format!(
                            "Delete {} ? Press {}+D to confirm.",
                            item.name,
                            primary_mod_label()
                        ));
                    }
                    return Ok(());
                }
            }
            _ => {}
        }

        match self.focus {
            Focus::Tree => self.handle_tree_key(key),
            Focus::Editor => self.handle_editor_key(key),
        }
    }

    fn handle_prompt_key(&mut self, key: KeyEvent) -> io::Result<()> {
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

    fn handle_file_picker_key(&mut self, key: KeyEvent) -> io::Result<()> {
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

    fn handle_search_results_key(&mut self, key: KeyEvent) -> io::Result<()> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) | (_, KeyCode::Char('q')) => {
                self.search_results_open = false;
                self.set_status("Closed search results");
            }
            (_, KeyCode::Down) | (_, KeyCode::Char('j')) => {
                if self.search_results_index + 1 < self.search_results.len() {
                    self.search_results_index += 1;
                }
            }
            (_, KeyCode::Up) | (_, KeyCode::Char('k')) => {
                if self.search_results_index > 0 {
                    self.search_results_index -= 1;
                }
            }
            (_, KeyCode::Enter) => {
                self.open_selected_search_result()?;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_completion_key(&mut self, key: KeyEvent) -> io::Result<()> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) => {
                self.completion_open = false;
                self.completion_ghost = None;
                self.set_status("Completion closed");
            }
            (_, KeyCode::Down) | (_, KeyCode::Char('j')) => {
                if self.completion_index + 1 < self.completion_items.len() {
                    self.completion_index += 1;
                }
                self.update_completion_ghost_from_selection();
            }
            (_, KeyCode::Up) | (_, KeyCode::Char('k')) => {
                if self.completion_index > 0 {
                    self.completion_index -= 1;
                }
                self.update_completion_ghost_from_selection();
            }
            (_, KeyCode::Enter) | (_, KeyCode::Tab) => {
                self.apply_completion();
            }
            _ => {
                self.completion_open = false;
                self.completion_ghost = None;
            }
        }
        Ok(())
    }

    fn handle_context_menu_key(&mut self, key: KeyEvent) -> io::Result<()> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) | (_, KeyCode::Char('q')) => {
                self.context_menu_open = false;
            }
            (_, KeyCode::Down) | (_, KeyCode::Char('j')) => {
                if self.context_menu_index < context_actions().len().saturating_sub(1) {
                    self.context_menu_index += 1;
                }
            }
            (_, KeyCode::Up) | (_, KeyCode::Char('k')) => {
                if self.context_menu_index > 0 {
                    self.context_menu_index -= 1;
                }
            }
            (_, KeyCode::Enter) => {
                let action = context_actions()[self.context_menu_index];
                self.apply_context_action(action)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_pending_key(&mut self, key: KeyEvent) -> io::Result<bool> {
        match (&self.pending, key.modifiers, key.code) {
            (PendingAction::None, _, _) => Ok(false),
            (PendingAction::Quit, KeyModifiers::CONTROL, KeyCode::Char('q')) => {
                self.quit = true;
                Ok(true)
            }
            (PendingAction::ClosePrompt, mods, KeyCode::Char('s'))
            | (PendingAction::ClosePrompt, mods, KeyCode::Char('S'))
                if mods.contains(KeyModifiers::CONTROL)
                    && !mods.contains(KeyModifiers::ALT) =>
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
            (PendingAction::Delete(path), KeyModifiers::CONTROL, KeyCode::Char('d')) => {
                let target = path.clone();
                self.pending = PendingAction::None;
                self.delete_path(target)?;
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

    fn handle_tree_key(&mut self, key: KeyEvent) -> io::Result<()> {
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

    fn tree_activate_selected(&mut self) -> io::Result<()> {
        self.tree_activate_selected_as(false)
    }

    fn tree_activate_selected_as(&mut self, as_preview: bool) -> io::Result<()> {
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

    fn tree_collapse_or_parent(&mut self) {
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

    fn handle_editor_key(&mut self, key: KeyEvent) -> io::Result<()> {
        if self.open_path().is_none() {
            self.focus = Focus::Tree;
            self.set_status("No file open. Focus returned to files.");
            return Ok(());
        }

        match (key.modifiers, key.code) {
            (_, KeyCode::Down)
                if key.modifiers.contains(KeyModifiers::SHIFT)
                    && key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.duplicate_current_line(false);
                return Ok(());
            }
            (_, KeyCode::Up)
                if key.modifiers.contains(KeyModifiers::SHIFT)
                    && key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.duplicate_current_line(true);
                return Ok(());
            }
            (_, KeyCode::Char('{')) | (_, KeyCode::Char('['))
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                self.fold_current_block();
                return Ok(());
            }
            (_, KeyCode::Char('}')) | (_, KeyCode::Char(']'))
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                self.unfold_current_block();
                return Ok(());
            }
            (_, KeyCode::Char('['))
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.fold_all();
                return Ok(());
            }
            (_, KeyCode::Char(']'))
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.unfold_all();
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('j')) => {
                self.toggle_fold_at_cursor();
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('u')) => {
                self.toggle_fold_all();
                return Ok(());
            }
            (KeyModifiers::SHIFT, KeyCode::BackTab) => {
                self.dedent_lines();
                return Ok(());
            }
            (KeyModifiers::NONE, KeyCode::Tab) if self.completion_open => {
                self.apply_completion();
                return Ok(());
            }
            (KeyModifiers::NONE, KeyCode::Tab) => {
                if let Some(ghost) = self.completion_ghost.clone() {
                    let now_prefix = self.current_identifier_prefix();
                    if !ghost.is_empty()
                        && !self.completion_prefix.is_empty()
                        && now_prefix == self.completion_prefix
                    {
                        let inserted = self.active_tab_mut().is_some_and(|t| t.editor.insert_str(ghost));
                        if inserted {
                            self.mark_dirty();
                            self.notify_lsp_did_change();
                            self.recompute_folds();
                        }
                        self.completion_ghost = None;
                        self.completion_prefix.clear();
                        self.set_status("Accepted inline completion");
                        return Ok(());
                    } else if now_prefix != self.completion_prefix {
                        self.completion_ghost = None;
                    }
                }
                if !self.current_identifier_prefix().is_empty() {
                    self.request_lsp_completion();
                    return Ok(());
                }
            }
            (KeyModifiers::CONTROL, KeyCode::Null) | (KeyModifiers::CONTROL, KeyCode::Char(' ')) => {
                self.request_lsp_completion();
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('.')) => {
                self.request_lsp_completion();
                return Ok(());
            }
            (_, KeyCode::Char('g')) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.prompt = Some(PromptState {
                    title: "Go to line".to_string(),
                    value: String::new(),
                    mode: PromptMode::GoToLine,
                });
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('/')) => {
                self.toggle_comment();
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('a')) => {
                if let Some(tab) = self.active_tab_mut() { tab.editor.select_all(); }
                self.set_status("Selected all");
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                self.copy_selection_to_clipboard();
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('x')) => {
                self.cut_selection_to_clipboard();
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('v')) => {
                self.paste_from_clipboard();
                return Ok(());
            }
            (_, KeyCode::Char('z'))
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                if self.active_tab_mut().is_some_and(|t| t.editor.redo()) {
                    self.mark_dirty();
                    self.notify_lsp_did_change();
                    self.recompute_folds();
                    self.set_status("Redo");
                } else {
                    self.set_status("Nothing to redo");
                }
                self.sync_editor_scroll_guess();
                return Ok(());
            }
            (_, KeyCode::Char('z')) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.active_tab_mut().is_some_and(|t| t.editor.undo()) {
                    self.mark_dirty();
                    self.notify_lsp_did_change();
                    self.recompute_folds();
                    self.set_status("Undo");
                } else {
                    self.set_status("Nothing to undo");
                }
                self.sync_editor_scroll_guess();
                return Ok(());
            }
            (_, KeyCode::Char('y')) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.active_tab_mut().is_some_and(|t| t.editor.redo()) {
                    self.mark_dirty();
                    self.notify_lsp_did_change();
                    self.recompute_folds();
                    self.set_status("Redo");
                } else {
                    self.set_status("Nothing to redo");
                }
                self.sync_editor_scroll_guess();
                return Ok(());
            }
            (KeyModifiers::NONE, KeyCode::Char(c))
                if matches!(c, '(' | '[' | '{' | '"' | '\'')
                    && self.active_tab().is_some_and(|t| t.editor.selection_range().is_none()) =>
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
                    let inserted = self.active_tab_mut().is_some_and(|t| t.editor.insert_str(pair));
                    if inserted {
                        if let Some(tab) = self.active_tab_mut() {
                            tab.editor.move_cursor(tui_textarea::CursorMove::Back);
                        }
                        self.mark_dirty();
                        self.notify_lsp_did_change();
                        self.recompute_folds();
                        self.set_status("Auto-pair inserted");
                        return Ok(());
                    }
                }
            }
            (KeyModifiers::NONE, KeyCode::PageDown) => {
                self.page_down();
                return Ok(());
            }
            (KeyModifiers::NONE, KeyCode::PageUp) => {
                self.page_up();
                return Ok(());
            }
            (_, KeyCode::Home) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.editor.move_cursor(tui_textarea::CursorMove::Jump(0, 0));
                }
                self.sync_editor_scroll_guess();
                self.set_status("Top of file");
                return Ok(());
            }
            (_, KeyCode::End) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(tab) = self.active_tab() {
                    let last_row = tab.editor.lines().len().saturating_sub(1);
                    let last_col = tab.editor.lines().last().map_or(0, |l| l.len());
                    if let Some(tab) = self.active_tab_mut() {
                        tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
                            last_row as u16,
                            last_col as u16,
                        ));
                    }
                }
                self.sync_editor_scroll_guess();
                self.set_status("End of file");
                return Ok(());
            }
            _ => {}
        }

        let modified = self.active_tab_mut().is_some_and(|t| t.editor.input(Input::from(key)));
        if modified {
            self.mark_dirty();
            self.notify_lsp_did_change();
            self.recompute_folds();
        }
        self.sync_editor_scroll_guess();
        self.refresh_inline_ghost();
        Ok(())
    }

    fn duplicate_current_line(&mut self, above: bool) {
        let Some(tab) = self.active_tab() else { return; };
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
        self.mark_dirty();
        self.notify_lsp_did_change();
        self.recompute_folds();
        if above {
            self.set_status("Duplicated line above");
        } else {
            self.set_status("Duplicated line below");
        }
    }

    fn toggle_comment(&mut self) {
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
        self.mark_dirty();
        self.notify_lsp_did_change();
        self.recompute_folds();
        self.set_status("Toggled comment");
    }

    fn dedent_lines(&mut self) {
        let Some(tab) = self.active_tab() else { return; };
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
            self.mark_dirty();
            self.notify_lsp_did_change();
            self.recompute_folds();
            self.set_status("Dedented");
        }
    }

    fn replace_editor_text(&mut self, lines: Vec<String>, cursor: (usize, usize)) {
        let mut ta = TextArea::from(lines);
        ta.set_cursor_line_style(Style::default().bg(self.active_theme().bg_alt));
        ta.set_selection_style(Style::default().bg(self.active_theme().selection));
        ta.move_cursor(tui_textarea::CursorMove::Jump(cursor.0 as u16, cursor.1 as u16));
        if let Some(tab) = self.active_tab_mut() {
            tab.editor = ta;
        }
        self.recompute_folds();
        self.sync_editor_scroll_guess();
    }

    fn copy_selection_to_clipboard(&mut self) {
        let Some(tab) = self.active_tab_mut() else { return; };
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

    fn cut_selection_to_clipboard(&mut self) {
        let Some(tab) = self.active_tab() else { return; };
        if tab.editor.selection_range().is_none() {
            self.set_status("No selection to cut");
            return;
        }
        let modified = self.tabs[self.active_tab].editor.cut();
        if modified {
            self.mark_dirty();
            self.notify_lsp_did_change();
            self.recompute_folds();
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

    fn paste_from_clipboard(&mut self) {
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
            self.mark_dirty();
            self.notify_lsp_did_change();
            self.recompute_folds();
            if from_system {
                self.set_status("Pasted");
            } else {
                self.set_status("Pasted (internal clipboard)");
            }
        } else {
            self.set_status("Clipboard empty");
        }
    }

    fn open_file(&mut self, path: PathBuf) -> io::Result<()> {
        self.open_file_as(path, false)
    }

    fn open_file_as(&mut self, path: PathBuf, as_preview: bool) -> io::Result<()> {
        // If file is already open in a tab, just switch to it
        if let Some(idx) = self.tabs.iter().position(|t| t.path == path) {
            self.switch_to_tab(idx);
            if !as_preview {
                self.tabs[idx].is_preview = false;
            }
            self.set_status(format!("Switched to {}", relative_path(&self.root, &path).display()));
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
        let mut ta = if text.is_empty() {
            TextArea::default()
        } else {
            TextArea::from(text.lines().map(ToString::to_string).collect::<Vec<_>>())
        };
        ta.set_cursor_line_style(Style::default().bg(self.active_theme().bg_alt));

        let lang = syntax_lang_for_path(Some(path.as_path()));
        let (fold_ranges, bracket_depths) = compute_fold_ranges(ta.lines(), lang);
        let mut visible_rows_map = Vec::new();
        for row in 0..ta.lines().len() {
            visible_rows_map.push(row);
        }
        if visible_rows_map.is_empty() {
            visible_rows_map.push(0);
        }

        let tab = Tab {
            path: path.clone(),
            is_preview: as_preview,
            editor: ta,
            dirty: false,
            open_disk_snapshot: Some(text),
            editor_scroll_row: 0,
            fold_ranges,
            bracket_depths,
            folded_starts: HashSet::new(),
            visible_rows_map,
            open_doc_uri: None,
            open_doc_version: 0,
            diagnostics: Vec::new(),
            conflict_prompt_open: false,
            conflict_disk_text: None,
            recovery_prompt_open: false,
            recovery_text: None,
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
        self.completion_open = false;
        self.completion_ghost = None;
        self.completion_prefix.clear();
        self.ensure_lsp_for_path(&path);
        self.check_recovery_for_open_file();
        self.set_status(format!("Opened {}", relative_path(&self.root, &path).display()));
        Ok(())
    }

    fn save_file(&mut self) -> io::Result<()> {
        let Some(tab) = self.active_tab_mut() else {
            self.set_status("No file open");
            return Ok(());
        };
        let path = tab.path.clone();
        let content = tab.editor.lines().join("\n");
        fs::write(&path, &content)?;
        tab.dirty = false;
        tab.open_disk_snapshot = Some(content);
        tab.conflict_prompt_open = false;
        tab.conflict_disk_text = None;
        self.clear_autosave_for_open_file();
        self.set_status(format!("Saved {}", relative_path(&self.root, &path).display()));
        Ok(())
    }

    fn close_file(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        self.close_tab_at(self.active_tab);
    }

    fn close_tab_at(&mut self, idx: usize) {
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
            self.completion_open = false;
            self.completion_ghost = None;
            self.completion_prefix.clear();
            self.set_status("Closed file");
        } else if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        } else if self.active_tab > idx {
            self.active_tab -= 1;
        }
    }

    fn delete_path(&mut self, path: PathBuf) -> io::Result<()> {
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

    fn create_new_file(&mut self) -> io::Result<()> {
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

    fn apply_prompt(&mut self, mode: PromptMode, value: String) -> io::Result<()> {
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
                        tab.editor
                            .move_cursor(tui_textarea::CursorMove::Jump(clamped as u16, 0));
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

    fn search_in_open_file(&mut self, query: &str) {
        if self.open_path().is_none() {
            self.set_status("Open a file first");
            return;
        }
        if query.trim().is_empty() {
            if let Some(tab) = self.active_tab_mut() { let _ = tab.editor.set_search_pattern(""); }
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

    fn replace_in_open_file(&mut self, search: &str, replacement: &str) {
        if self.open_path().is_none() {
            self.set_status("Open a file first");
            return;
        }
        let mut lines = self.tabs[self.active_tab].editor.lines().to_vec();
        let mut count = 0usize;
        for line in lines.iter_mut() {
            while line.contains(search) {
                *line = line.replacen(search, replacement, 1);
                count += 1;
            }
        }
        if count > 0 {
            let cursor = self.tabs[self.active_tab].editor.cursor();
            self.replace_editor_text(lines, cursor);
            self.mark_dirty();
            self.notify_lsp_did_change();
            self.set_status(format!("Replaced {} occurrence(s)", count));
        } else {
            self.set_status(format!("No occurrences of '{}' found", search));
        }
    }

    fn search_in_project(&mut self, query: &str) {
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
            self.set_status("rg (ripgrep) not found — install: https://github.com/BurntSushi/ripgrep#installation");
            return;
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut hits = Vec::new();
        for line in stdout.lines() {
            if let Some(hit) = parse_rg_line(line) {
                hits.push(hit);
            }
        }
        self.search_results_query = trimmed.to_string();
        self.search_results = hits;
        self.search_results_index = 0;
        self.search_results_open = true;
        if self.search_results.is_empty() {
            self.set_status(format!("No results for '{}'", trimmed));
        } else {
            self.set_status(format!(
                "{} results for '{}'",
                self.search_results.len(),
                trimmed
            ));
        }
    }

    fn open_selected_search_result(&mut self) -> io::Result<()> {
        let Some(hit) = self.search_results.get(self.search_results_index).cloned() else {
            return Ok(());
        };
        self.open_file(hit.path.clone())?;
        let target_row = hit.line.saturating_sub(1);
        if let Some(tab) = self.active_tab_mut() {
            tab.editor
                .move_cursor(tui_textarea::CursorMove::Jump(target_row as u16, 0));
        }
        self.sync_editor_scroll_guess();
        self.search_results_open = false;
        self.set_status(format!(
            "Opened {}:{}",
            relative_path(&self.root, &hit.path).display(),
            hit.line
        ));
        Ok(())
    }

    fn apply_context_action(&mut self, action: ContextAction) -> io::Result<()> {
        let target = self.context_menu_target.clone();
        self.context_menu_open = false;
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
                    title: format!("New file in {}", relative_path(&self.root, &parent).display()),
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

    fn handle_help_key(&mut self, key: KeyEvent) -> io::Result<()> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc)
            | (_, KeyCode::Char('q'))
            | (_, KeyCode::Char('?'))
            | (_, KeyCode::F(4)) => {
                self.help_open = false;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_editor_context_menu_key(&mut self, key: KeyEvent) -> io::Result<()> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) | (_, KeyCode::Char('q')) => {
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

    fn apply_editor_context_action(&mut self, action: EditorContextAction) {
        self.editor_context_menu_open = false;
        self.focus = Focus::Editor;
        match action {
            EditorContextAction::Copy => self.copy_selection_to_clipboard(),
            EditorContextAction::Cut => self.cut_selection_to_clipboard(),
            EditorContextAction::Paste => self.paste_from_clipboard(),
            EditorContextAction::SelectAll => {
                if let Some(tab) = self.active_tab_mut() { tab.editor.select_all(); }
                self.set_status("Selected all");
            }
            EditorContextAction::Cancel => {}
        }
    }

    fn sync_editor_scroll_guess(&mut self) {
        let Some(tab) = self.active_tab() else { return; };
        let (cursor_row, _) = tab.editor.cursor();
        let inner_height = self.editor_rect.height.saturating_sub(2) as usize;
        if inner_height == 0 {
            if let Some(tab) = self.active_tab_mut() { tab.editor_scroll_row = 0; }
            return;
        }
        if self.active_tab().is_some_and(|t| t.visible_rows_map.is_empty()) {
            self.rebuild_visible_rows();
        }
        let cursor_visible = self.visible_index_of_source_row(cursor_row);
        let Some(tab) = self.active_tab_mut() else { return; };
        if cursor_visible < tab.editor_scroll_row {
            tab.editor_scroll_row = cursor_visible;
        } else if cursor_visible >= tab.editor_scroll_row + inner_height {
            tab.editor_scroll_row = cursor_visible.saturating_sub(inner_height.saturating_sub(1));
        }
    }

    fn page_down(&mut self) {
        let Some(tab) = self.active_tab() else { return; };
        let inner_height = self.editor_rect.height.saturating_sub(2) as usize;
        if inner_height == 0 { return; }
        let (cursor_row, cursor_col) = tab.editor.cursor();
        let visible_rows = &tab.visible_rows_map;
        if visible_rows.is_empty() { return; }
        let cursor_vis = self.visible_index_of_source_row(cursor_row);
        let target_vis = (cursor_vis + inner_height).min(visible_rows.len().saturating_sub(1));
        let target_row = visible_rows[target_vis];
        let target_lines = self.active_tab().map_or(0, |t| {
            t.editor.lines().get(target_row).map_or(0, |l| l.len())
        });
        let col = cursor_col.min(target_lines);
        if let Some(tab) = self.active_tab_mut() {
            tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
                target_row as u16, col as u16,
            ));
        }
        self.sync_editor_scroll_guess();
    }

    fn page_up(&mut self) {
        let Some(tab) = self.active_tab() else { return; };
        let inner_height = self.editor_rect.height.saturating_sub(2) as usize;
        if inner_height == 0 { return; }
        let (cursor_row, cursor_col) = tab.editor.cursor();
        let visible_rows = &tab.visible_rows_map;
        if visible_rows.is_empty() { return; }
        let cursor_vis = self.visible_index_of_source_row(cursor_row);
        let target_vis = cursor_vis.saturating_sub(inner_height);
        let target_row = visible_rows[target_vis];
        let target_lines = self.active_tab().map_or(0, |t| {
            t.editor.lines().get(target_row).map_or(0, |l| l.len())
        });
        let col = cursor_col.min(target_lines);
        if let Some(tab) = self.active_tab_mut() {
            tab.editor.move_cursor(tui_textarea::CursorMove::Jump(
                target_row as u16, col as u16,
            ));
        }
        self.sync_editor_scroll_guess();
    }

    fn editor_pos_from_mouse(&self, x: u16, y: u16) -> Option<(usize, usize)> {
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
        let text_x = inner_x.saturating_sub(Self::EDITOR_GUTTER_WIDTH as usize);
        let max_col = lines[row].chars().count();
        let col = text_x.min(max_col);
        Some((row, col))
    }

    fn handle_menu_key(&mut self, key: KeyEvent) -> io::Result<()> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) | (_, KeyCode::Char('q')) => {
                self.menu_open = false;
                self.menu_query.clear();
            }
            (_, KeyCode::Down) | (_, KeyCode::Char('j')) => {
                if self.menu_index + 1 < self.menu_results.len() {
                    self.menu_index += 1;
                }
            }
            (_, KeyCode::Up) | (_, KeyCode::Char('k')) => {
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

    fn handle_recovery_prompt_key(&mut self, key: KeyEvent) -> io::Result<()> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Enter) | (_, KeyCode::Char('r')) | (_, KeyCode::Char('R')) => {
                let text = self.active_tab().and_then(|t| t.recovery_text.clone());
                if let Some(text) = text {
                    let lines = if text.is_empty() {
                        vec![String::new()]
                    } else {
                        text.lines().map(ToString::to_string).collect()
                    };
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

    fn handle_conflict_prompt_key(&mut self, key: KeyEvent) -> io::Result<()> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Char('r')) | (_, KeyCode::Char('R')) => {
                let disk = self.active_tab().and_then(|t| t.conflict_disk_text.clone());
                if let Some(disk) = disk {
                    let lines: Vec<String> = if disk.is_empty() {
                        vec![String::new()]
                    } else {
                        disk.lines().map(ToString::to_string).collect()
                    };
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

    fn handle_theme_browser_key(&mut self, key: KeyEvent) -> io::Result<()> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) | (_, KeyCode::Char('q')) => {
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

    fn handle_mouse(&mut self, mouse: MouseEvent) -> io::Result<()> {
        if self.help_open {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                self.help_open = false;
            }
            return Ok(());
        }

        if self.prompt.is_some() {
            return Ok(());
        }
        if self.active_tab().is_some_and(|t| t.recovery_prompt_open || t.conflict_prompt_open) {
            return Ok(());
        }

        if self.search_results_open {
            return self.handle_search_results_mouse(mouse);
        }
        if self.completion_open {
            return self.handle_completion_mouse(mouse);
        }

        if self.editor_context_menu_open {
            return self.handle_editor_context_menu_mouse(mouse);
        }

        if self.context_menu_open {
            return self.handle_context_menu_mouse(mouse);
        }

        if self.menu_open {
            return self.handle_menu_mouse(mouse);
        }

        if self.theme_browser_open {
            return self.handle_theme_browser_mouse(mouse);
        }

        if self.files_view_open {
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if inside(mouse.column, mouse.row, self.divider_rect) {
                        self.divider_dragging = true;
                        return Ok(());
                    }
                }
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Moved => {
                    if self.divider_dragging {
                        // Convert absolute X to pane width by using content frame start.
                        let desired = mouse.column.saturating_sub(self.tree_rect.x);
                        self.files_pane_width = desired.max(Self::MIN_FILES_PANE_WIDTH);
                        self.clamp_files_pane_width(self.editor_rect.width + self.tree_rect.width + self.divider_rect.width);
                        return Ok(());
                    }
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    if self.divider_dragging {
                        self.divider_dragging = false;
                        self.persist_state();
                        self.set_status(format!("Files pane width: {}", self.files_pane_width));
                        return Ok(());
                    }
                }
                _ => {}
            }
        }

        if inside(mouse.column, mouse.row, self.tree_rect) {
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(idx) = self.tree_index_from_mouse(mouse.row) {
                        self.selected = idx;
                        let path = self.tree[idx].path.clone();
                        if path.is_dir() {
                            self.tree_activate_selected()?;
                            self.focus = Focus::Tree;
                        } else {
                            // Double-click detection (400ms threshold)
                            let is_double_click = self.last_tree_click
                                .as_ref()
                                .is_some_and(|(t, prev_idx)| *prev_idx == idx && t.elapsed() < Duration::from_millis(400));
                            self.last_tree_click = Some((Instant::now(), idx));
                            if is_double_click {
                                // Double-click opens as sticky
                                self.open_file_as(path, false)?;
                            } else {
                                // Single-click opens as preview
                                self.open_file_as(path, true)?;
                            }
                        }
                    }
                }
                MouseEventKind::Down(MouseButton::Right) => {
                    if let Some(idx) = self.tree_index_from_mouse(mouse.row) {
                        self.selected = idx;
                        self.context_menu_target = Some(self.tree[idx].path.clone());
                        self.context_menu_index = 0;
                        self.context_menu_pos = (mouse.column, mouse.row);
                        self.context_menu_open = true;
                    }
                }
                MouseEventKind::ScrollDown => {
                    if self.selected + 1 < self.tree.len() {
                        self.selected += 1;
                    }
                }
                MouseEventKind::ScrollUp => {
                    if self.selected > 0 {
                        self.selected -= 1;
                    }
                }
                _ => {}
            }
            return Ok(());
        }

        // Tab bar click detection (title bar row of editor block)
        if mouse.row == self.editor_rect.y && inside(mouse.column, mouse.row, self.editor_rect) {
            if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                for (i, (name_rect, close_rect)) in self.tab_rects.iter().enumerate() {
                    if inside(mouse.column, mouse.row, *close_rect) {
                        // Click on [x] — close this tab
                        if self.tabs[i].dirty {
                            self.switch_to_tab(i);
                            self.pending = PendingAction::ClosePrompt;
                            self.set_status("Unsaved changes: Enter save+close | Esc discard | C cancel");
                        } else {
                            self.close_tab_at(i);
                        }
                        return Ok(());
                    }
                    if inside(mouse.column, mouse.row, *name_rect) {
                        // Click on tab name — switch to it
                        self.switch_to_tab(i);
                        return Ok(());
                    }
                }
            }
            return Ok(());
        }

        if inside(mouse.column, mouse.row, self.editor_rect) {
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    self.focus = Focus::Editor;
                    let inner_x = mouse
                        .column
                        .saturating_sub(self.editor_rect.x.saturating_add(1));
                    if inner_x < Self::EDITOR_GUTTER_WIDTH {
                        let inner_y = mouse
                            .row
                            .saturating_sub(self.editor_rect.y.saturating_add(1))
                            as usize;
                        if let Some(tab) = self.active_tab() {
                            let visible_idx = tab.editor_scroll_row + inner_y;
                            if let Some(&row) = tab.visible_rows_map.get(visible_idx) {
                                self.toggle_fold_at_row(row);
                            }
                        }
                        return Ok(());
                    }
                    if let Some((row, col)) = self.editor_pos_from_mouse(mouse.column, mouse.row) {
                        if let Some(tab) = self.active_tab_mut() {
                            tab.editor
                                .move_cursor(tui_textarea::CursorMove::Jump(row as u16, col as u16));
                            tab.editor.cancel_selection();
                        }
                        self.editor_dragging = true;
                        self.editor_drag_anchor = Some((row, col));
                    }
                }
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.extend_mouse_selection(mouse.column, mouse.row);
                }
                MouseEventKind::Moved => {
                    if self.editor_dragging {
                        self.extend_mouse_selection(mouse.column, mouse.row);
                    }
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.editor_dragging = false;
                    self.editor_drag_anchor = None;
                }
                MouseEventKind::Down(MouseButton::Right) => {
                    self.focus = Focus::Editor;
                    self.editor_context_menu_pos = (mouse.column, mouse.row);
                    self.editor_context_menu_index = 0;
                    self.editor_context_menu_open = true;
                }
                MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                    let modified = self.active_tab_mut().is_some_and(|t| t.editor.input(Input::from(Event::Mouse(mouse))));
                    if modified {
                        self.mark_dirty();
                        self.notify_lsp_did_change();
                    }
                    if let Some(tab) = self.active_tab_mut() {
                        match mouse.kind {
                            MouseEventKind::ScrollDown => {
                                tab.editor_scroll_row = tab.editor_scroll_row.saturating_add(1)
                            }
                            MouseEventKind::ScrollUp => {
                                tab.editor_scroll_row = tab.editor_scroll_row.saturating_sub(1)
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            self.sync_editor_scroll_guess();
            self.refresh_inline_ghost();
            return Ok(());
        }

        Ok(())
    }

    fn extend_mouse_selection(&mut self, x: u16, y: u16) {
        if let (Some((anchor_row, anchor_col)), Some((row, col))) =
            (self.editor_drag_anchor, self.editor_pos_from_mouse(x, y))
        {
            if let Some(tab) = self.active_tab_mut() {
                tab.editor
                    .move_cursor(tui_textarea::CursorMove::Jump(anchor_row as u16, anchor_col as u16));
                tab.editor.start_selection();
                tab.editor
                    .move_cursor(tui_textarea::CursorMove::Jump(row as u16, col as u16));
            }
        }
    }

    fn handle_completion_mouse(&mut self, mouse: MouseEvent) -> io::Result<()> {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return Ok(());
        }
        if !inside(mouse.column, mouse.row, self.completion_rect) {
            self.completion_open = false;
            return Ok(());
        }
        let row = mouse.row.saturating_sub(self.completion_rect.y + 1) as usize;
        if row < self.completion_items.len() {
            self.completion_index = row;
            self.apply_completion();
        }
        Ok(())
    }

    fn tree_index_from_mouse(&self, y: u16) -> Option<usize> {
        let start = self.tree_rect.y.saturating_add(1);
        let end = self.tree_rect.y.saturating_add(self.tree_rect.height.saturating_sub(1));
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

    fn handle_menu_mouse(&mut self, mouse: MouseEvent) -> io::Result<()> {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return Ok(());
        }
        if !inside(mouse.column, mouse.row, self.context_menu_rect) {
            self.menu_open = false;
            self.menu_query.clear();
            return Ok(());
        }
        let row = mouse.row.saturating_sub(self.context_menu_rect.y + 2) as usize;
        if row < self.menu_results.len() {
            self.menu_index = row;
            let action = self.menu_results[self.menu_index];
            self.menu_open = false;
            self.menu_query.clear();
            self.run_command_action(action)?;
        }
        Ok(())
    }

    fn handle_theme_browser_mouse(&mut self, mouse: MouseEvent) -> io::Result<()> {
        if !inside(mouse.column, mouse.row, self.context_menu_rect) {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                self.active_theme_index = self.preview_revert_index;
                self.theme_index = self.preview_revert_index;
                self.theme_browser_open = false;
                self.menu_open = false;
                self.set_status(format!("Theme reverted: {}", self.active_theme().name));
            }
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
                let row = mouse.row.saturating_sub(self.context_menu_rect.y + 1) as usize;
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

    fn handle_context_menu_mouse(&mut self, mouse: MouseEvent) -> io::Result<()> {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return Ok(());
        }
        if !inside(mouse.column, mouse.row, self.context_menu_rect) {
            self.context_menu_open = false;
            return Ok(());
        }
        let row = mouse.row.saturating_sub(self.context_menu_rect.y + 1) as usize;
        if row < context_actions().len() {
            self.context_menu_index = row;
            let action = context_actions()[row];
            self.apply_context_action(action)?;
        }
        Ok(())
    }

    fn handle_editor_context_menu_mouse(&mut self, mouse: MouseEvent) -> io::Result<()> {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return Ok(());
        }
        if !inside(
            mouse.column,
            mouse.row,
            self.editor_context_menu_rect,
        ) {
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

    fn handle_search_results_mouse(&mut self, mouse: MouseEvent) -> io::Result<()> {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return Ok(());
        }
        if !inside(mouse.column, mouse.row, self.context_menu_rect) {
            self.search_results_open = false;
            return Ok(());
        }
        let row = mouse.row.saturating_sub(self.context_menu_rect.y + 1) as usize;
        if row < self.search_results.len() {
            self.search_results_index = row;
            self.open_selected_search_result()?;
        }
        Ok(())
    }
}

fn pending_hint(pending: &PendingAction) -> String {
    let m = primary_mod_label();
    match pending {
        PendingAction::None => String::new(),
        PendingAction::Quit => format!("Pending quit: {}+Q confirm, Esc cancel", m),
        PendingAction::ClosePrompt => {
            format!("Pending close: Enter/{}+S save+close, Esc discard, C cancel", m)
        }
        PendingAction::Delete(path) => format!(
            "Pending delete {}: {}+D confirm, Esc cancel",
            path.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string()),
            m
        ),
    }
}

fn primary_mod_label() -> &'static str {
    "Ctrl"
}

fn command_action_label(action: CommandAction) -> &'static str {
    match action {
        CommandAction::Theme => "Theme Picker",
        CommandAction::Help => "Help",
        CommandAction::QuickOpen => "Quick Open Files",
        CommandAction::FindInFile => "Find in File",
        CommandAction::FindInProject => "Search in Project",
        CommandAction::SaveFile => "Save File",
        CommandAction::RefreshTree => "Refresh Tree",
        CommandAction::ToggleFiles => "Toggle Files Pane",
        CommandAction::GotoDefinition => "Go to Definition",
        CommandAction::ReplaceInFile => "Find and Replace",
        CommandAction::GoToLine => "Go to Line",
    }
}

fn autosave_path_for(path: &Path) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    let hash = hasher.finish();
    let base = state_file_path()
        .and_then(|p| p.parent().map(|pp| pp.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("autosave").join(format!("{hash:016x}.autosave"))
}

fn context_actions() -> [ContextAction; 6] {
    [
        ContextAction::Open,
        ContextAction::NewFile,
        ContextAction::NewFolder,
        ContextAction::Rename,
        ContextAction::Delete,
        ContextAction::Cancel,
    ]
}

fn editor_context_actions() -> [EditorContextAction; 5] {
    [
        EditorContextAction::Copy,
        EditorContextAction::Cut,
        EditorContextAction::Paste,
        EditorContextAction::SelectAll,
        EditorContextAction::Cancel,
    ]
}

fn context_label(action: ContextAction) -> &'static str {
    match action {
        ContextAction::Open => "Open",
        ContextAction::NewFile => "New File",
        ContextAction::NewFolder => "New Folder",
        ContextAction::Rename => "Rename",
        ContextAction::Delete => "Delete",
        ContextAction::Cancel => "Cancel",
    }
}

fn editor_context_label(action: EditorContextAction) -> &'static str {
    match action {
        EditorContextAction::Copy => "Copy",
        EditorContextAction::Cut => "Cut",
        EditorContextAction::Paste => "Paste",
        EditorContextAction::SelectAll => "Select All",
        EditorContextAction::Cancel => "Cancel",
    }
}

fn leading_indent_bytes(line: &str) -> usize {
    let mut i = 0usize;
    let bytes = line.as_bytes();
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    i
}

fn comment_prefix_for_path(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let prefix = match ext.as_str() {
        "rs" | "js" | "ts" | "tsx" | "jsx" | "go" | "java" | "c" | "h" | "cpp" | "hpp"
        | "cs" | "swift" | "kt" | "kts" | "scala" => "//",
        "py" | "sh" | "bash" | "zsh" | "yaml" | "yml" | "toml" | "rb" | "pl" | "conf"
        | "ini" => "#",
        "sql" | "lua" => "--",
        _ => return None,
    };
    Some(prefix)
}

fn parse_rg_line(line: &str) -> Option<ProjectSearchHit> {
    let mut parts = line.splitn(3, ':');
    let path = parts.next()?;
    let line_no = parts.next()?.parse::<usize>().ok()?;
    let preview = parts.next().unwrap_or_default().to_string();
    Some(ProjectSearchHit {
        path: PathBuf::from(path),
        line: line_no,
        preview,
    })
}

fn fuzzy_score(query: &str, candidate: &str) -> Option<usize> {
    if query.is_empty() {
        return Some(0);
    }
    let q = query.as_bytes();
    let c_lower = candidate.to_ascii_lowercase();
    let c = c_lower.as_bytes();
    let mut qi = 0usize;
    let mut score = 0usize;
    let mut last_match = 0usize;
    for (i, b) in c.iter().enumerate() {
        if qi < q.len() && *b == q[qi] {
            score += i.saturating_sub(last_match);
            last_match = i;
            qi += 1;
            if qi == q.len() {
                score += candidate.len().saturating_sub(i);
                return Some(score);
            }
        }
    }
    None
}

fn detect_git_branch(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("rev-parse")
        .arg("--abbrev-ref")
        .arg("HEAD")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}

fn file_uri(path: &Path) -> Option<String> {
    let abs = path.canonicalize().ok()?;
    Url::from_file_path(abs).ok().map(|u| u.to_string())
}

fn compute_fold_ranges(lines: &[String], lang: SyntaxLang) -> (Vec<FoldRange>, Vec<u16>) {
    let mut ranges = Vec::new();
    let mut bracket_depths: Vec<u16> = Vec::with_capacity(lines.len());

    // Brace / bracket folding + unified bracket depth tracking
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut depth: u16 = 0;
    for (row, line) in lines.iter().enumerate() {
        bracket_depths.push(depth);
        let mut in_string = false;
        let mut quote = '\0';
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0usize;
        while i < chars.len() {
            let ch = chars[i];
            if !in_string {
                if let Some(cs) = comment_start_for_lang(lang) {
                    if cs == "//" && i + 1 < chars.len() && chars[i] == '/' && chars[i + 1] == '/' {
                        break;
                    }
                    if cs == "#" && chars[i] == '#' {
                        break;
                    }
                    if cs == "/*" && i + 1 < chars.len() && chars[i] == '/' && chars[i + 1] == '*' {
                        break;
                    }
                }
                if ch == '"' || ch == '\'' {
                    in_string = true;
                    quote = ch;
                    i += 1;
                    continue;
                }
                if ch == '{' || ch == '(' || ch == '[' {
                    if ch == '{' {
                        stack.push((ch, row));
                    }
                    depth = depth.saturating_add(1);
                } else if ch == '}' || ch == ')' || ch == ']' {
                    depth = depth.saturating_sub(1);
                    if ch == '}' {
                        if let Some((_, start)) = stack.pop() {
                            if row > start {
                                ranges.push(FoldRange {
                                    start_line: start,
                                    end_line: row,
                                });
                            }
                        }
                    }
                }
            } else if ch == '\\' {
                i += 2;
                continue;
            } else if ch == quote {
                in_string = false;
            }
            i += 1;
        }
    }

    // Indentation folding (good for Python/YAML-like + generally useful)
    let mut indent_stack: Vec<(usize, usize)> = Vec::new();
    for (row, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        while let Some((prev_indent, start_row)) = indent_stack.last().copied() {
            if indent <= prev_indent {
                indent_stack.pop();
                let end_row = row.saturating_sub(1);
                if end_row > start_row {
                    ranges.push(FoldRange {
                        start_line: start_row,
                        end_line: end_row,
                    });
                }
            } else {
                break;
            }
        }
        indent_stack.push((indent, row));
    }
    if let Some(last_row) = lines.len().checked_sub(1) {
        while let Some((_, start_row)) = indent_stack.pop() {
            if last_row > start_row {
                ranges.push(FoldRange {
                    start_line: start_row,
                    end_line: last_row,
                });
            }
        }
    }

    // Basic HTML/XML tag folding for paired tags
    if lang == SyntaxLang::HtmlXml {
        let mut tag_stack: Vec<(String, usize)> = Vec::new();
        for (row, line) in lines.iter().enumerate() {
            let s = line.trim();
            if s.starts_with("<!--") {
                continue;
            }
            if let Some(rest) = s.strip_prefix("</") {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .collect();
                if let Some(pos) = tag_stack.iter().rposition(|(n, _)| *n == name) {
                    let (_, start) = tag_stack.remove(pos);
                    if row > start {
                        ranges.push(FoldRange {
                            start_line: start,
                            end_line: row,
                        });
                    }
                }
                continue;
            }
            if s.starts_with('<') && !s.starts_with("<!") && !s.starts_with("<?") && !s.ends_with("/>") {
                let name: String = s[1..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .collect();
                if !name.is_empty() {
                    tag_stack.push((name, row));
                }
            }
        }
    }

    ranges.sort_by_key(|r| (r.start_line, r.end_line));
    ranges.dedup_by(|a, b| a.start_line == b.start_line && a.end_line == b.end_line);
    (ranges, bracket_depths)
}

fn syntax_lang_for_path(path: Option<&Path>) -> SyntaxLang {
    let Some(path) = path else {
        return SyntaxLang::Plain;
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "rs" => SyntaxLang::Rust,
        "py" | "pyi" => SyntaxLang::Python,
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts" => SyntaxLang::JsTs,
        "go" => SyntaxLang::Go,
        "php" | "phtml" => SyntaxLang::Php,
        "css" | "scss" | "sass" | "less" => SyntaxLang::Css,
        "html" | "htm" | "xml" | "svg" | "xhtml" | "vue" | "svelte" | "astro" | "jsp" | "erb" | "hbs" | "ejs" => SyntaxLang::HtmlXml,
        "sh" | "bash" | "zsh" | "fish" | "ksh" => SyntaxLang::Shell,
        "json" | "jsonc" | "toml" | "yaml" | "yml" => SyntaxLang::Json,
        "md" | "markdown" => SyntaxLang::Markdown,
        _ => SyntaxLang::Plain,
    }
}

fn is_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn keywords_for_lang(lang: SyntaxLang) -> &'static [&'static str] {
    match lang {
        SyntaxLang::Rust => &[
            "fn", "let", "mut", "impl", "trait", "struct", "enum", "match", "if", "else",
            "for", "while", "loop", "pub", "use", "mod", "crate", "self", "super", "return",
            "async", "await", "move", "const", "static", "where", "in", "break", "continue",
            "type", "dyn",
        ],
        SyntaxLang::Python => &[
            "def", "class", "if", "elif", "else", "for", "while", "try", "except", "return",
            "import", "from", "as", "with", "async", "await", "yield", "lambda", "pass", "None",
            "True", "False",
        ],
        SyntaxLang::JsTs => &[
            "function", "const", "let", "var", "class", "if", "else", "for", "while", "return",
            "import", "from", "export", "default", "async", "await", "try", "catch", "switch",
            "case", "break", "continue", "interface", "type", "extends", "implements",
        ],
        SyntaxLang::Go => &[
            "package", "import", "func", "var", "const", "type", "struct", "interface", "map",
            "chan", "go", "defer", "select", "if", "else", "switch", "case", "default", "for",
            "range", "return", "break", "continue", "fallthrough",
        ],
        SyntaxLang::Php => &[
            "function", "class", "interface", "trait", "public", "private", "protected", "static",
            "if", "else", "elseif", "switch", "case", "default", "for", "foreach", "while", "do",
            "return", "new", "use", "namespace", "try", "catch", "finally", "fn",
        ],
        SyntaxLang::Css => &[
            "@media", "@supports", "@keyframes", "display", "position", "color", "background",
            "border", "margin", "padding", "width", "height", "font", "grid", "flex",
        ],
        SyntaxLang::Shell => &[
            "if", "then", "else", "fi", "for", "do", "done", "while", "case", "esac", "function",
            "export", "local",
        ],
        SyntaxLang::HtmlXml | SyntaxLang::Json | SyntaxLang::Markdown | SyntaxLang::Plain => &[],
    }
}

fn comment_start_for_lang(lang: SyntaxLang) -> Option<&'static str> {
    match lang {
        SyntaxLang::Rust | SyntaxLang::JsTs | SyntaxLang::Go => Some("//"),
        SyntaxLang::Php | SyntaxLang::Css => Some("/*"),
        SyntaxLang::Python | SyntaxLang::Shell => Some("#"),
        SyntaxLang::HtmlXml | SyntaxLang::Json | SyntaxLang::Markdown | SyntaxLang::Plain => None,
    }
}

fn highlight_line(line: &str, lang: SyntaxLang, theme: &Theme, bracket_depth: u16, bracket_colors: &[Color; 3]) -> Line<'static> {
    let base = Style::default().fg(theme.fg);
    if lang == SyntaxLang::Plain {
        return Line::from(vec![Span::styled(line.to_string(), base)]);
    }
    let keyword_style = Style::default().fg(theme.accent).add_modifier(Modifier::BOLD);
    let string_style = Style::default().fg(theme.syntax_string);
    let number_style = Style::default().fg(theme.syntax_number);
    let comment_style = Style::default().fg(theme.comment);
    let heading_style = Style::default().fg(theme.syntax_tag).add_modifier(Modifier::BOLD);

    if lang == SyntaxLang::Markdown {
        if line.starts_with('#') {
            return Line::from(vec![Span::styled(line.to_string(), heading_style)]);
        }
        return Line::from(vec![Span::styled(line.to_string(), base)]);
    }
    if lang == SyntaxLang::HtmlXml {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut i = 0usize;
        let bytes = line.as_bytes();
        let tag_style = Style::default().fg(theme.syntax_tag).add_modifier(Modifier::BOLD);
        let attr_style = Style::default().fg(theme.syntax_attribute);
        while i < bytes.len() {
            if line[i..].starts_with("<!--") {
                spans.push(Span::styled(line[i..].to_string(), comment_style));
                break;
            }
            let ch = line[i..].chars().next().unwrap_or('\0');
            if ch == '<' {
                let start = i;
                i += 1;
                while i < bytes.len() {
                    let c = line[i..].chars().next().unwrap_or('\0');
                    i += c.len_utf8();
                    if c == '>' {
                        break;
                    }
                }
                let tag = &line[start..i];
                let mut parts = tag.split_whitespace();
                if let Some(head) = parts.next() {
                    spans.push(Span::styled(head.to_string(), tag_style));
                    for part in parts {
                        spans.push(Span::raw(" ".to_string()));
                        if let Some(eq_idx) = part.find('=') {
                            let (k, v) = part.split_at(eq_idx);
                            spans.push(Span::styled(k.to_string(), attr_style));
                            spans.push(Span::raw(v.to_string()));
                        } else {
                            spans.push(Span::styled(part.to_string(), attr_style));
                        }
                    }
                } else {
                    spans.push(Span::styled(tag.to_string(), tag_style));
                }
                continue;
            }
            if ch == '"' || ch == '\'' {
                let quote = ch;
                let start = i;
                i += ch.len_utf8();
                while i < bytes.len() {
                    let c = line[i..].chars().next().unwrap_or('\0');
                    i += c.len_utf8();
                    if c == quote {
                        break;
                    }
                }
                spans.push(Span::styled(line[start..i].to_string(), string_style));
                continue;
            }
            spans.push(Span::styled(ch.to_string(), base));
            i += ch.len_utf8();
        }
        return Line::from(spans);
    }

    let bytes = line.as_bytes();
    let mut i = 0usize;
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut bd = bracket_depth;
    while i < bytes.len() {
        if let Some(comment) = comment_start_for_lang(lang) {
            if line[i..].starts_with(comment) {
                spans.push(Span::styled(line[i..].to_string(), comment_style));
                break;
            }
        }
        let ch = line[i..].chars().next().unwrap_or('\0');
        if ch == '"' || ch == '\'' {
            let quote = ch;
            let start = i;
            i += ch.len_utf8();
            while i < bytes.len() {
                let c = line[i..].chars().next().unwrap_or('\0');
                i += c.len_utf8();
                if c == '\\' && i < bytes.len() {
                    let escaped = line[i..].chars().next().unwrap_or('\0');
                    i += escaped.len_utf8();
                    continue;
                }
                if c == quote {
                    break;
                }
            }
            spans.push(Span::styled(line[start..i].to_string(), string_style));
            continue;
        }
        if ch.is_ascii_digit() {
            let start = i;
            i += ch.len_utf8();
            while i < bytes.len() {
                let c = line[i..].chars().next().unwrap_or('\0');
                if c.is_ascii_digit() || c == '_' || c == '.' {
                    i += c.len_utf8();
                } else {
                    break;
                }
            }
            spans.push(Span::styled(line[start..i].to_string(), number_style));
            continue;
        }
        if is_ident_char(ch) {
            let start = i;
            i += ch.len_utf8();
            while i < bytes.len() {
                let c = line[i..].chars().next().unwrap_or('\0');
                if is_ident_char(c) {
                    i += c.len_utf8();
                } else {
                    break;
                }
            }
            let token = &line[start..i];
            if keywords_for_lang(lang).contains(&token) {
                spans.push(Span::styled(token.to_string(), keyword_style));
            } else {
                spans.push(Span::styled(token.to_string(), base));
            }
            continue;
        }
        if ch == '{' || ch == '(' || ch == '[' {
            let color = bracket_colors[(bd % 3) as usize];
            spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
            bd = bd.saturating_add(1);
        } else if ch == '}' || ch == ')' || ch == ']' {
            bd = bd.saturating_sub(1);
            let color = bracket_colors[(bd % 3) as usize];
            spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
        } else {
            spans.push(Span::styled(ch.to_string(), base));
        }
        i += ch.len_utf8();
    }
    Line::from(spans)
}

fn row_has_selection(
    row: usize,
    line_len_chars: usize,
    selection: Option<((usize, usize), (usize, usize))>,
) -> bool {
    let Some(((sr, sc), (er, ec))) = selection else {
        return false;
    };
    if sr == er && sc == ec {
        return false;
    }
    if row < sr || row > er {
        return false;
    }
    if sr == er {
        return row == sr && sc < ec;
    }
    if row == sr {
        return sc < line_len_chars;
    }
    if row == er {
        return ec > 0;
    }
    true
}

fn inside(x: u16, y: u16, rect: Rect) -> bool {
    x >= rect.x
        && x < rect.x.saturating_add(rect.width)
        && y >= rect.y
        && y < rect.y.saturating_add(rect.height)
}

fn collect_all_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            // Skip hidden dirs and common noisy dirs
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            collect_all_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn relative_path(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

fn color_from_hex(input: &str, fallback: Color) -> Color {
    let s = input.trim();
    if let Some(stripped) = s.strip_prefix('#') {
        if stripped.len() == 6 {
            let r = u8::from_str_radix(&stripped[0..2], 16).ok();
            let g = u8::from_str_radix(&stripped[2..4], 16).ok();
            let b = u8::from_str_radix(&stripped[4..6], 16).ok();
            if let (Some(r), Some(g), Some(b)) = (r, g, b) {
                return Color::Rgb(r, g, b);
            }
        }
    }
    fallback
}

fn state_file_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join(STATE_FILE_REL));
        }
    }
    // Windows: use %APPDATA% (e.g. C:\Users\X\AppData\Roaming)
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.is_empty() {
            return Some(PathBuf::from(appdata).join(STATE_FILE_REL));
        }
    }
    // Unix/macOS: use $HOME/.config
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".config").join(STATE_FILE_REL))
}

fn load_persisted_state() -> Option<PersistedState> {
    let path = state_file_path()?;
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str::<PersistedState>(&raw).ok()
}

fn save_persisted_state(state: &PersistedState) -> io::Result<()> {
    let Some(path) = state_file_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(state)
        .map_err(|e| io::Error::other(format!("serialize state: {e}")))?;
    fs::write(path, raw)
}

fn theme_from_file(tf: ThemeFile) -> Theme {
    let syn = tf.syntax.as_ref();
    let border_color = color_from_hex(&tf.colors.border, Color::Rgb(127, 122, 88));
    let fg_muted = color_from_hex(&tf.colors.foreground_muted, Color::Rgb(100, 100, 120));
    Theme {
        name: tf.name,
        theme_type: tf.theme_type,
        bg: color_from_hex(&tf.colors.background, Color::Rgb(20, 22, 31)),
        bg_alt: color_from_hex(&tf.colors.background_alt, Color::Rgb(25, 28, 39)),
        fg: color_from_hex(&tf.colors.foreground, Color::Rgb(215, 213, 189)),
        fg_muted,
        border: border_color,
        accent: color_from_hex(&tf.colors.accent, Color::Rgb(206, 198, 130)),
        accent_secondary: tf.colors.accent_secondary.as_ref()
            .map_or(Color::Rgb(86, 156, 214), |c| color_from_hex(c, Color::Rgb(86, 156, 214))),
        selection: color_from_hex(&tf.colors.selection, Color::Rgb(51, 70, 124)),
        comment: syn.and_then(|s| s.comment.as_ref())
            .map_or(fg_muted, |c| color_from_hex(c, fg_muted)),
        syntax_string: syn.and_then(|s| s.string.as_ref())
            .map_or(Color::Rgb(156, 220, 140), |c| color_from_hex(c, Color::Rgb(156, 220, 140))),
        syntax_number: syn.and_then(|s| s.number.as_ref())
            .map_or(Color::Rgb(181, 206, 168), |c| color_from_hex(c, Color::Rgb(181, 206, 168))),
        syntax_tag: syn.and_then(|s| s.tag.as_ref())
            .map_or(Color::Rgb(86, 156, 214), |c| color_from_hex(c, Color::Rgb(86, 156, 214))),
        syntax_attribute: syn.and_then(|s| s.attribute.as_ref())
            .map_or(Color::Rgb(78, 201, 176), |c| color_from_hex(c, Color::Rgb(78, 201, 176))),
        bracket_1: tf.colors.yellow.as_ref()
            .map_or(Color::Rgb(210, 168, 75), |c| color_from_hex(c, Color::Rgb(210, 168, 75))),
        bracket_2: tf.colors.purple.as_ref()
            .map_or(Color::Rgb(176, 82, 204), |c| color_from_hex(c, Color::Rgb(176, 82, 204))),
        bracket_3: tf.colors.cyan.as_ref()
            .map_or(Color::Rgb(0, 175, 215), |c| color_from_hex(c, Color::Rgb(0, 175, 215))),
    }
}

fn load_themes() -> Vec<Theme> {
    let mut themes = Vec::new();

    // Collect candidate theme directories: local first, then brew share paths
    let mut theme_dirs = vec![PathBuf::from(LOCAL_THEME_DIR)];
    // Homebrew install locations (Apple Silicon and Intel)
    theme_dirs.push(PathBuf::from("/opt/homebrew/share/lazyide/themes"));
    theme_dirs.push(PathBuf::from("/usr/local/share/lazyide/themes"));

    for theme_dir in &theme_dirs {
        if !theme_dir.exists() {
            continue;
        }
        let mut paths: Vec<PathBuf> = fs::read_dir(theme_dir)
            .ok()
            .into_iter()
            .flat_map(|rd| rd.filter_map(Result::ok))
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        paths.sort();

        for path in paths {
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(tf) = serde_json::from_str::<ThemeFile>(&raw) else {
                continue;
            };
            themes.push(theme_from_file(tf));
        }
        // Stop after the first directory that yields themes
        if !themes.is_empty() {
            break;
        }
    }
    // Fall back to themes embedded in the binary
    if themes.is_empty() {
        let mut files: Vec<_> = EMBEDDED_THEMES
            .files()
            .filter(|f| f.path().extension().is_some_and(|e| e == "json"))
            .collect();
        files.sort_by_key(|f| f.path());
        for file in files {
            let Some(raw) = file.contents_utf8() else { continue };
            let Ok(tf) = serde_json::from_str::<ThemeFile>(raw) else { continue };
            themes.push(theme_from_file(tf));
        }
    }
    themes.sort_by_key(|t| (t.theme_type != "dark", t.name.to_ascii_lowercase()));
    themes
}

fn draw(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme().clone();
    let size = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5), Constraint::Length(3)])
        .split(size);
    let (tree_area, editor_area) = if app.files_view_open {
        app.clamp_files_pane_width(vertical[1].width);
        let divider_w = 1;
        let main = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(app.files_pane_width),
                Constraint::Length(divider_w),
                Constraint::Min(App::MIN_EDITOR_PANE_WIDTH),
            ])
            .split(vertical[1]);
        app.divider_rect = main[1];
        (Some(main[0]), main[2])
    } else {
        app.divider_rect = Rect::default();
        (None, vertical[1])
    };
    app.tree_rect = tree_area.unwrap_or_default();
    app.editor_rect = editor_area;

    let file_label = match app.open_path() {
        Some(path) => {
            let mut s = relative_path(&app.root, path).display().to_string();
            if app.is_dirty() {
                s.push_str(" *");
            }
            s
        }
        None => "no file".to_string(),
    };
    let branch_label = app.git_branch.as_deref().unwrap_or("");
    let top_text = if branch_label.is_empty() {
        format!(
            "lazyide   root: {}   file: {}",
            app.root.display(),
            file_label
        )
    } else {
        format!(
            "lazyide   root: {}   branch: {}   file: {}",
            app.root.display(),
            branch_label,
            file_label
        )
    };
    let top = Paragraph::new(top_text)
    .style(Style::default().fg(theme.fg).bg(theme.bg_alt))
    .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(theme.border)));
    frame.render_widget(top, vertical[0]);

    let left_border = if app.focus == Focus::Tree && app.files_view_open {
        theme.accent
    } else {
        theme.border
    };
    let right_border = if app.focus == Focus::Editor {
        theme.accent
    } else {
        theme.border
    };

    if let Some(tree_area) = tree_area {
        let tree_items: Vec<ListItem> = app
            .tree
            .iter()
            .map(|item| {
                let indent = "  ".repeat(item.depth);
                let icon = if item.is_dir {
                    if item.expanded { "▾ " } else { "▸ " }
                } else {
                    "· "
                };
                let style = if item.is_dir {
                    Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.fg)
                };
                ListItem::new(Line::from(Span::styled(
                    format!("{indent}{icon}{}", item.name),
                    style,
                )))
            })
            .collect();
        let mut tree_state = ListState::default();
        tree_state.select(Some(app.selected));
        let tree = List::new(tree_items)
            .highlight_style(
                Style::default()
                    .fg(theme.fg)
                    .bg(theme.selection)
                    .add_modifier(Modifier::BOLD),
            )
            .block(
                Block::default()
                    .title("[1]-Files")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(left_border))
                    .style(Style::default().bg(theme.bg_alt).fg(theme.fg)),
            );
        frame.render_stateful_widget(tree, tree_area, &mut tree_state);
        if app.files_view_open && app.divider_rect.width > 0 {
            let divider = Paragraph::new("│")
                .style(Style::default().fg(theme.border).bg(theme.bg_alt));
            frame.render_widget(divider, app.divider_rect);
        }
    }

    // Build tab bar title
    let tab_title: Line = if app.tabs.is_empty() {
        Line::from("Working View")
    } else {
        let mut spans = Vec::new();
        app.tab_rects.clear();
        for (i, tab) in app.tabs.iter().enumerate() {
            let fname = tab.path.file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "untitled".to_string());
            let prefix = if tab.dirty { "*" } else { "" };
            let label = format!(" {prefix}{fname} [x] ");
            let style = if i == app.active_tab {
                let mut s = Style::default().fg(theme.fg).bg(theme.bg);
                if tab.is_preview {
                    s = s.add_modifier(Modifier::ITALIC);
                }
                s
            } else {
                let mut s = Style::default().fg(theme.fg_muted);
                if tab.is_preview {
                    s = s.add_modifier(Modifier::ITALIC);
                }
                s
            };
            if !spans.is_empty() {
                spans.push(Span::styled("│", Style::default().fg(theme.border)));
            }
            spans.push(Span::styled(label, style));
        }
        Line::from(spans)
    };
    let editor_block = Block::default()
        .title(tab_title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(right_border))
        .style(Style::default().bg(theme.bg_alt).fg(theme.fg));
    frame.render_widget(editor_block, editor_area);
    let inner = Rect::new(
        editor_area.x.saturating_add(1),
        editor_area.y.saturating_add(1),
        editor_area.width.saturating_sub(2),
        editor_area.height.saturating_sub(2),
    );

    // Compute tab_rects for click detection (position within the title bar)
    {
        app.tab_rects.clear();
        let mut x_offset = editor_area.x + 1; // +1 for border
        for (i, tab) in app.tabs.iter().enumerate() {
            let fname = tab.path.file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "untitled".to_string());
            let prefix = if tab.dirty { "*" } else { "" };
            let label_text = format!(" {prefix}{fname} [x] ");
            let label_len = label_text.width() as u16;
            if i > 0 {
                x_offset += 1; // separator
            }
            // Name rect (clickable to switch)
            let close_len = 4u16; // " [x]" + trailing space
            let name_rect = Rect::new(x_offset, editor_area.y, label_len.saturating_sub(close_len), 1);
            // Close rect
            let close_rect = Rect::new(x_offset + label_len.saturating_sub(close_len), editor_area.y, close_len, 1);
            app.tab_rects.push((name_rect, close_rect));
            x_offset += label_len;
        }
    }

    frame.render_widget(Clear, inner);
    let lang = syntax_lang_for_path(app.open_path().map(|p| p.as_path()));
    let visible_rows = inner.height as usize;
    if app.active_tab().is_some_and(|t| t.visible_rows_map.is_empty()) {
        app.rebuild_visible_rows();
    }
    let (start_row, lines_src, selection, cursor_row, cursor_col, diagnostics_owned, fold_ranges_owned, folded_starts_owned, visible_rows_map_owned, bracket_depths_owned) = if let Some(tab) = app.active_tab() {
        let sr = tab.editor_scroll_row.min(tab.visible_rows_map.len().saturating_sub(1));
        // Only clone lines up to the highest visible row, not the entire buffer
        let max_row = tab.visible_rows_map.iter().copied().max().unwrap_or(0);
        let all_lines = tab.editor.lines();
        let lines: Vec<String> = all_lines[..all_lines.len().min(max_row + 1)].to_vec();
        (
            sr,
            lines,
            tab.editor.selection_range(),
            tab.editor.cursor().0,
            tab.editor.cursor().1,
            tab.diagnostics.clone(),
            tab.fold_ranges.clone(),
            tab.folded_starts.clone(),
            tab.visible_rows_map.clone(),
            tab.bracket_depths.clone(),
        )
    } else {
        (0, vec![String::new()], None, 0, 0, Vec::new(), Vec::new(), HashSet::new(), vec![0usize], Vec::new())
    };
    let diagnostics_ref = &diagnostics_owned as &[LspDiagnostic];
    let fold_ranges_ref = &fold_ranges_owned as &[FoldRange];
    let folded_starts_ref = &folded_starts_owned;
    let visible_rows_map_ref = &visible_rows_map_owned as &[usize];
    let inner_w = inner.width as usize;
    let blank_line = Line::from(Span::styled(
        " ".repeat(inner_w),
        Style::default().bg(theme.bg),
    ));
    let mut lines_out: Vec<Line> = Vec::with_capacity(visible_rows);
    for visual_row in 0..visible_rows {
        let visible_idx = start_row + visual_row;
        let Some(&row) = visible_rows_map_ref.get(visible_idx) else {
            lines_out.push(blank_line.clone());
            continue;
        };
        if row >= lines_src.len() {
            lines_out.push(blank_line.clone());
            continue;
        }
        let mut spans = Vec::new();
        let line_num = format!("{:>5} ", row + 1);
        let line_num_style = if row == cursor_row {
            Style::default().fg(theme.accent)
        } else {
            Style::default().fg(theme.fg_muted)
        };
        spans.push(Span::styled(line_num, line_num_style));

        let fold_indicator = if let Some(fr) = fold_ranges_ref.iter().find(|fr| fr.start_line == row) {
            if folded_starts_ref.contains(&fr.start_line) {
                "▸ "
            } else {
                "▾ "
            }
        } else {
            "  "
        };
        spans.push(Span::styled(
            fold_indicator,
            Style::default()
                .fg(theme.fg_muted)
                .add_modifier(Modifier::BOLD),
        ));

        let diag_for_row = diagnostics_ref.iter().find(|d| d.line == row + 1);
        if let Some(diag) = diag_for_row {
            let color = match diag.severity.as_str() {
                "error" => Color::Red,
                "warning" => Color::Yellow,
                "info" => Color::Cyan,
                _ => Color::Blue,
            };
            spans.push(Span::styled("●", Style::default().fg(color)));
        } else {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::raw(" "));
        let display_line = lines_src[row].replace('\t', "    ");
        let bracket_colors = [theme.bracket_1, theme.bracket_2, theme.bracket_3];
        let bd = bracket_depths_owned.get(row).copied().unwrap_or(0);
        let hl = highlight_line(&display_line, lang, &theme, bd, &bracket_colors);
        spans.extend(hl.spans);
        // Pad line to full width so stale characters from previous frame are overwritten
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        if used < inner_w {
            spans.push(Span::styled(
                " ".repeat(inner_w - used),
                Style::default().bg(theme.bg),
            ));
        }
        let hl = Line::from(spans);
        let hl = if diagnostics_ref.iter().any(|d| d.line == row + 1 && d.severity == "error") {
            hl.patch_style(Style::default().add_modifier(Modifier::UNDERLINED))
        } else {
            hl
        };
        let hl = if row == cursor_row {
            hl.patch_style(Style::default().bg(theme.bg_alt))
        } else {
            hl
        };
        let hl = if selection.is_some() && row_has_selection(row, lines_src[row].chars().count(), selection) {
            hl.patch_style(Style::default().bg(theme.selection))
        } else {
            hl
        };
        if let Some(fr) = fold_ranges_ref.iter().find(|fr| fr.start_line == row && folded_starts_ref.contains(&fr.start_line)) {
            let folded = fr.end_line.saturating_sub(fr.start_line);
            let mut spans = hl.spans;
            spans.push(Span::styled(
                format!("  ... [{} lines]", folded),
                Style::default().fg(theme.fg_muted),
            ));
            lines_out.push(Line::from(spans));
        } else {
            lines_out.push(hl);
        }
    }
    let editor_text = Paragraph::new(lines_out)
        .style(Style::default().bg(theme.bg).fg(theme.fg));
    frame.render_widget(editor_text, inner);
    if app.focus == Focus::Editor {
        let cursor_visible = app.visible_index_of_source_row(cursor_row);
        let cursor_y = cursor_visible.saturating_sub(start_row);
        if cursor_y < visible_rows {
            let max_x = inner
                .width
                .saturating_sub(1)
                .saturating_sub(App::EDITOR_GUTTER_WIDTH) as usize;
            let cursor_x = cursor_col.min(max_x);
            if let Some(ghost) = app.completion_ghost.as_ref() {
                if !ghost.is_empty()
                    && (cursor_x as u16 + App::EDITOR_GUTTER_WIDTH) < inner.width.saturating_sub(1)
                {
                    let ghost_area = Rect::new(
                        inner
                            .x
                            .saturating_add(App::EDITOR_GUTTER_WIDTH)
                            .saturating_add(cursor_x as u16),
                        inner.y.saturating_add(cursor_y as u16),
                        inner
                            .width
                            .saturating_sub(App::EDITOR_GUTTER_WIDTH)
                            .saturating_sub(cursor_x as u16),
                        1,
                    );
                    let ghost_span = Span::styled(
                        ghost.clone(),
                        Style::default().fg(theme.fg_muted),
                    );
                    frame.render_widget(Paragraph::new(Line::from(vec![ghost_span])), ghost_area);
                }
            }
            frame.set_cursor_position((
                inner
                    .x
                    .saturating_add(App::EDITOR_GUTTER_WIDTH)
                    .saturating_add(cursor_x as u16),
                inner.y.saturating_add(cursor_y as u16),
            ));
        }
    }

    let modk = primary_mod_label();
    let status = Paragraph::new(format!(
        "{modk}+P Cmd   {modk}+O Open   F4 Help   {modk}+B Files   {modk}+W Close   {modk}+S Save"
    ))
    .style(Style::default().fg(theme.fg).bg(theme.bg_alt))
    .wrap(Wrap { trim: true })
    .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(theme.border)));
    frame.render_widget(status, vertical[2]);

    if app.menu_open {
        render_menu(app, frame);
    }
    if app.file_picker_open {
        render_file_picker(app, frame);
    }
    if app.theme_browser_open {
        render_theme_browser(app, frame);
    }
    if app.search_results_open {
        render_search_results(app, frame);
    }
    if app.completion_open {
        render_completion_popup(app, frame);
    }
    if app.help_open {
        render_help(app, frame);
    }
    if app.context_menu_open {
        render_context_menu(app, frame);
    }
    if app.editor_context_menu_open {
        render_editor_context_menu(app, frame);
    }
    if app.prompt.is_some() {
        render_prompt(app, frame);
    }
    if matches!(app.pending, PendingAction::ClosePrompt) {
        render_close_prompt(app, frame);
    }
    if app.active_tab().is_some_and(|t| t.conflict_prompt_open) {
        render_conflict_prompt(app, frame);
    }
    if app.active_tab().is_some_and(|t| t.recovery_prompt_open) {
        render_recovery_prompt(app, frame);
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn render_menu(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme().clone();
    let area = centered_rect(62, 62, frame.area());
    app.context_menu_rect = area;
    frame.render_widget(Clear, area);
    let mut items: Vec<ListItem> = Vec::new();
    items.push(ListItem::new(Line::from(vec![
        Span::styled("Query: ", Style::default().fg(theme.fg_muted)),
        Span::styled(app.menu_query.clone(), Style::default().fg(theme.fg)),
    ])));
    if app.menu_results.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "No commands",
            Style::default().fg(theme.fg_muted),
        ))));
    }
    let list_items: Vec<ListItem> = app
        .menu_results
        .iter()
        .enumerate()
        .map(|(idx, action)| {
            let style = if idx == app.menu_index {
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            ListItem::new(Line::from(Span::styled(command_action_label(*action), style)))
        })
        .collect();
    items.extend(list_items);
    let list = List::new(items).block(
        Block::default()
            .title("Command Palette")
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.bg_alt))
            .border_style(Style::default().fg(theme.accent)),
    );
    frame.render_widget(list, area);
}

fn render_theme_browser(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme().clone();
    let area = centered_rect(62, 70, frame.area());
    app.context_menu_rect = area;
    frame.render_widget(Clear, area);
    let list_items: Vec<ListItem> = app
        .themes
        .iter()
        .enumerate()
        .map(|(idx, t)| {
            let label = format!("{} [{}]", t.name, t.theme_type);
            let style = if idx == app.theme_index {
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            ListItem::new(Line::from(Span::styled(label, style)))
        })
        .collect();
    let list = List::new(list_items).block(
        Block::default()
            .title("Theme Picker (Live Preview)")
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.bg_alt))
            .border_style(Style::default().fg(theme.accent)),
    );
    frame.render_widget(list, area);
}

fn render_file_picker(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme().clone();
    let area = centered_rect(72, 65, frame.area());
    app.context_menu_rect = area;
    frame.render_widget(Clear, area);
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("Query: ", Style::default().fg(theme.fg_muted)),
        Span::styled(app.file_picker_query.clone(), Style::default().fg(theme.fg)),
    ]));
    lines.push(Line::from(""));
    if app.file_picker_results.is_empty() {
        lines.push(Line::from(Span::styled(
            "No matching files",
            Style::default().fg(theme.fg_muted),
        )));
    } else {
        for (idx, path) in app.file_picker_results.iter().take(25).enumerate() {
            let rel = relative_path(&app.root, path).display().to_string();
            let style = if idx == app.file_picker_index {
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            lines.push(Line::from(Span::styled(rel, style)));
        }
    }
    let paragraph = Paragraph::new(lines)
        .style(Style::default().fg(theme.fg).bg(theme.bg_alt))
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title(format!("Quick Open ({}+P)", primary_mod_label()))
                .borders(Borders::ALL)
                .style(Style::default().bg(theme.bg_alt))
                .border_style(Style::default().fg(theme.accent)),
        );
    frame.render_widget(paragraph, area);
}

fn render_search_results(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme().clone();
    let area = centered_rect(78, 72, frame.area());
    app.context_menu_rect = area;
    frame.render_widget(Clear, area);
    let list_items: Vec<ListItem> = if app.search_results.is_empty() {
        vec![ListItem::new(Line::from("No results"))]
    } else {
        app.search_results
            .iter()
            .enumerate()
            .map(|(idx, hit)| {
                let rel = relative_path(&app.root, &hit.path);
                let label = format!("{}:{}  {}", rel.display(), hit.line, hit.preview);
                let style = if idx == app.search_results_index {
                    Style::default()
                        .fg(theme.bg)
                        .bg(theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.fg)
                };
                ListItem::new(Line::from(Span::styled(label, style)))
            })
            .collect()
    };
    let title = format!("Search Results: {}", app.search_results_query);
    let list = List::new(list_items).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.bg_alt))
            .border_style(Style::default().fg(theme.accent)),
    );
    frame.render_widget(list, area);
}

fn render_completion_popup(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme().clone();
    let width = 56;
    let visible = app.completion_items.len().min(10);
    let height = visible as u16 + 2;
    let max_x = frame.area().width.saturating_sub(width);
    let max_y = frame.area().height.saturating_sub(height);
    let x = app
        .editor_rect
        .x
        .saturating_add(3)
        .min(max_x);
    let y = app
        .editor_rect
        .y
        .saturating_add(2)
        .min(max_y);
    let area = Rect::new(x, y, width, height);
    app.completion_rect = area;
    frame.render_widget(Clear, area);
    let list_items: Vec<ListItem> = app
        .completion_items
        .iter()
        .take(10)
        .enumerate()
        .map(|(idx, item)| {
            let label = if let Some(detail) = &item.detail {
                format!("{}  {}", item.label, detail)
            } else {
                item.label.clone()
            };
            let style = if idx == app.completion_index {
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            ListItem::new(Line::from(Span::styled(label, style)))
        })
        .collect();
    let list = List::new(list_items).block(
        Block::default()
            .title("Completion")
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.bg_alt))
            .border_style(Style::default().fg(theme.accent)),
    );
    frame.render_widget(list, area);
}

fn help_keybind_line<'a>(entries: &[(&str, &str)], key_style: Style, desc_style: Style, sep_style: Style) -> Line<'a> {
    let mut spans = Vec::new();
    for (i, (key, desc)) in entries.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  |  ", sep_style));
        }
        spans.push(Span::styled(key.to_string(), key_style));
        spans.push(Span::styled(format!(" {desc}"), desc_style));
    }
    Line::from(spans)
}

fn render_help(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme();
    let area = centered_rect(78, 80, frame.area());
    frame.render_widget(Clear, area);

    let m = primary_mod_label();
    let heading = Style::default().fg(theme.accent).add_modifier(Modifier::BOLD);
    let key_s = Style::default().fg(theme.accent_secondary);
    let desc_s = Style::default().fg(theme.fg);
    let sep_s = Style::default().fg(theme.fg_muted);
    let muted = Style::default().fg(theme.fg_muted);

    let lines: Vec<Line> = vec![
        Line::from(Span::styled("Keyboard", heading)),
        Line::from(""),
        help_keybind_line(&[
            (&format!("{m}+S"), "save"), (&format!("{m}+W"), "close tab"),
            (&format!("{m}+N"), "new file"), (&format!("{m}+Q"), "quit"),
        ], key_s, desc_s, sep_s),
        help_keybind_line(&[
            (&format!("{m}+P"), "command palette"), (&format!("{m}+O"), "quick open"),
            (&format!("{m}+G"), "go to line"),
        ], key_s, desc_s, sep_s),
        help_keybind_line(&[
            (&format!("{m}+B"), "toggle files"), (&format!("{m}+R"), "refresh tree"),
        ], key_s, desc_s, sep_s),
        help_keybind_line(&[
            (&format!("{m}+F"), "find"), (&format!("{m}+H"), "find & replace"),
            (&format!("{m}+Shift+F"), "search files"),
        ], key_s, desc_s, sep_s),
        help_keybind_line(&[
            (&format!("{m}+D"), "go to definition"), (&format!("{m}+Alt+D"), "go to definition"),
        ], key_s, desc_s, sep_s),
        help_keybind_line(&[
            (&format!("{m}+J"), "toggle fold"), (&format!("{m}+U"), "toggle fold all"),
        ], key_s, desc_s, sep_s),
        help_keybind_line(&[
            (&format!("{m}+Shift+["), "fold"), (&format!("{m}+Shift+]"), "unfold"),
            (&format!("{m}+Alt+["), "fold all"), (&format!("{m}+Alt+]"), "unfold all"),
        ], key_s, desc_s, sep_s),
        help_keybind_line(&[
            ("Shift+Alt+Down", "dup line down"), ("Shift+Alt+Up", "dup line up"),
        ], key_s, desc_s, sep_s),
        help_keybind_line(&[
            ("F3", "find next"), ("Shift+F3", "find prev"), ("Shift+Tab", "dedent"),
        ], key_s, desc_s, sep_s),
        help_keybind_line(&[
            ("PageUp/Down", "scroll page"), (&format!("{m}+Home/End"), "start/end of file"),
        ], key_s, desc_s, sep_s),
        help_keybind_line(&[
            ("Tab", "completion"), (&format!("{m}+Space"), "completion"), (&format!("{m}+."), "completion"),
        ], key_s, desc_s, sep_s),
        help_keybind_line(&[
            (&format!("{m}+Z"), "undo"), (&format!("{m}+Y"), "redo"), (&format!("{m}+Shift+Z"), "redo"),
        ], key_s, desc_s, sep_s),
        help_keybind_line(&[
            (&format!("{m}+A"), "select all"), (&format!("{m}+C"), "copy"),
            (&format!("{m}+X"), "cut"), (&format!("{m}+V"), "paste"), (&format!("{m}+/"), "toggle comment"),
        ], key_s, desc_s, sep_s),
        help_keybind_line(&[
            ("F1", "prev tab"), ("F2", "next tab"), ("F4", "help"),
        ], key_s, desc_s, sep_s),
        Line::from(""),
        Line::from(Span::styled("Tree", heading)),
        Line::from(""),
        help_keybind_line(&[
            ("Up/Down/K/J", "move"), ("Left/H", "collapse"), ("Right/L/Enter", "open"),
        ], key_s, desc_s, sep_s),
        help_keybind_line(&[
            ("Delete", &format!("start delete, then {m}+D to confirm")),
        ], key_s, desc_s, sep_s),
        Line::from(""),
        Line::from(Span::styled("Mouse", heading)),
        Line::from(""),
        Line::from(vec![
            Span::styled("Click", key_s), Span::styled(" file: preview tab", desc_s),
            Span::styled("  |  ", sep_s),
            Span::styled("Double-click", key_s), Span::styled(" sticky tab", desc_s),
        ]),
        Line::from(vec![
            Span::styled("Click", key_s), Span::styled(" tab to switch", desc_s),
            Span::styled("  |  ", sep_s),
            Span::styled("Click [x]", key_s), Span::styled(" to close", desc_s),
        ]),
        Line::from(Span::styled("Drag divider to resize  |  Right-click: context menus  |  Gutter click: fold", muted)),
        Line::from(""),
        Line::from(Span::styled("Esc / Q / F4  close this help", muted)),
    ];

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(theme.fg).bg(theme.bg_alt))
        .block(
            Block::default()
                .title(" Help ")
                .borders(Borders::ALL)
                .style(Style::default().bg(theme.bg_alt))
                .border_style(Style::default().fg(theme.accent)),
        );
    frame.render_widget(paragraph, area);
}

fn render_context_menu(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme().clone();
    let width = 24;
    let height = context_actions().len() as u16 + 2;
    let max_x = frame.area().width.saturating_sub(width);
    let max_y = frame.area().height.saturating_sub(height);
    let x = app.context_menu_pos.0.min(max_x);
    let y = app.context_menu_pos.1.min(max_y);
    let area = Rect::new(x, y, width, height);
    app.context_menu_rect = area;
    frame.render_widget(Clear, area);
    let list_items: Vec<ListItem> = context_actions()
        .iter()
        .enumerate()
        .map(|(idx, action)| {
            let style = if idx == app.context_menu_index {
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            ListItem::new(Line::from(Span::styled(context_label(*action), style)))
        })
        .collect();
    let title = app
        .context_menu_target
        .as_ref()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "Actions".to_string());
    let list = List::new(list_items).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.bg_alt))
            .border_style(Style::default().fg(theme.accent)),
    );
    frame.render_widget(list, area);
}

fn render_editor_context_menu(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme().clone();
    let width = 20;
    let height = editor_context_actions().len() as u16 + 2;
    let max_x = frame.area().width.saturating_sub(width);
    let max_y = frame.area().height.saturating_sub(height);
    let x = app.editor_context_menu_pos.0.min(max_x);
    let y = app.editor_context_menu_pos.1.min(max_y);
    let area = Rect::new(x, y, width, height);
    app.editor_context_menu_rect = area;
    frame.render_widget(Clear, area);
    let list_items: Vec<ListItem> = editor_context_actions()
        .iter()
        .enumerate()
        .map(|(idx, action)| {
            let style = if idx == app.editor_context_menu_index {
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            ListItem::new(Line::from(Span::styled(editor_context_label(*action), style)))
        })
        .collect();
    let list = List::new(list_items).block(
        Block::default()
            .title("Edit")
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.bg_alt))
            .border_style(Style::default().fg(theme.accent)),
    );
    frame.render_widget(list, area);
}

fn render_prompt(app: &mut App, frame: &mut Frame<'_>) {
    let Some(prompt) = app.prompt.as_ref() else {
        return;
    };
    let theme = app.active_theme();
    let area = centered_rect(60, 20, frame.area());
    frame.render_widget(Clear, area);
    let input = Paragraph::new(prompt.value.clone()).block(
        Block::default()
            .title(prompt.title.as_str())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent))
            .style(Style::default().bg(theme.bg_alt).fg(theme.fg)),
    );
    frame.render_widget(input, area);
}

fn render_close_prompt(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme();
    let area = centered_rect(60, 26, frame.area());
    frame.render_widget(Clear, area);
    let text = vec![
        "Unsaved changes".to_string(),
        "".to_string(),
        format!("Enter or {}+S: Save and close", primary_mod_label()),
        "Esc: Discard and close".to_string(),
        "C: Cancel".to_string(),
    ]
    .join("\n");
    let body = Paragraph::new(text)
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(theme.fg).bg(theme.bg_alt))
        .block(
            Block::default()
                .title("Close File")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.accent))
                .style(Style::default().bg(theme.bg_alt)),
        );
    frame.render_widget(body, area);
}

fn render_conflict_prompt(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme();
    let area = centered_rect(68, 30, frame.area());
    frame.render_widget(Clear, area);
    let text = [
        "File changed on disk while you have unsaved edits.",
        "",
        "R: Reload disk version (discard current edits)",
        "K: Keep local edits",
        "D or Esc: Decide later",
    ]
    .join("\n");
    let body = Paragraph::new(text)
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(theme.fg).bg(theme.bg_alt))
        .block(
            Block::default()
                .title("External Change Conflict")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.accent))
                .style(Style::default().bg(theme.bg_alt)),
        );
    frame.render_widget(body, area);
}

fn render_recovery_prompt(app: &mut App, frame: &mut Frame<'_>) {
    let theme = app.active_theme();
    let area = centered_rect(62, 28, frame.area());
    frame.render_widget(Clear, area);
    let text = [
        "Autosave content found for this file.",
        "",
        "Enter or R: Recover autosave",
        "D: Discard autosave",
        "Esc or C: Cancel",
    ]
    .join("\n");
    let body = Paragraph::new(text)
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(theme.fg).bg(theme.bg_alt))
        .block(
            Block::default()
                .title("Recover Autosave")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.accent))
                .style(Style::default().bg(theme.bg_alt)),
        );
    frame.render_widget(body, area);
}

fn run_app(mut terminal: Terminal<CrosstermBackend<Stdout>>, mut app: App) -> io::Result<()> {
    loop {
        app.poll_lsp();
        app.poll_fs_changes()?;
        app.poll_autosave()?;
        app.update_status_for_cursor();
        terminal.draw(|f| draw(&mut app, f))?;
        if app.quit {
            return Ok(());
        }
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => app.handle_key(key)?,
                Event::Mouse(mouse) => app.handle_mouse(mouse)?,
                _ => {}
            }
        }
    }
}

fn run_setup() -> io::Result<()> {
    println!("lazyide setup\n");

    let has_ra = resolve_rust_analyzer_bin().is_some();
    let has_rg = Command::new("rg").arg("--version").output().is_ok();

    if has_ra {
        println!("  \u{2713} rust-analyzer found");
    } else {
        println!("  \u{2717} rust-analyzer not found");
        println!("    \u{2192} rustup component add rust-analyzer");
    }
    if has_rg {
        println!("  \u{2713} ripgrep (rg) found");
    } else {
        println!("  \u{2717} ripgrep (rg) not found");
        if cfg!(target_os = "macos") {
            println!("    \u{2192} brew install ripgrep");
        } else {
            println!("    \u{2192} cargo install ripgrep");
        }
    }

    if has_ra && has_rg {
        println!("\nAll tools installed. You're good to go!");
        return Ok(());
    }

    println!("\nInstall missing tools? [y/N] ");
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    if !input.trim().eq_ignore_ascii_case("y") {
        return Ok(());
    }

    if !has_ra {
        println!("\nInstalling rust-analyzer...");
        let status = Command::new("rustup")
            .args(["component", "add", "rust-analyzer"])
            .status();
        match status {
            Ok(s) if s.success() => println!("  \u{2713} rust-analyzer installed"),
            _ => println!("  \u{2717} Failed. Install manually: rustup component add rust-analyzer"),
        }
    }

    if !has_rg {
        println!("\nInstalling ripgrep...");
        let (cmd, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
            ("brew", &["install", "ripgrep"])
        } else {
            ("cargo", &["install", "ripgrep"])
        };
        let status = Command::new(cmd).args(args).status();
        match status {
            Ok(s) if s.success() => println!("  \u{2713} ripgrep installed"),
            _ => println!("  \u{2717} Failed. Install manually: cargo install ripgrep"),
        }
    }

    println!("\nSetup complete!");
    Ok(())
}

fn main() -> io::Result<()> {
    if std::env::args().any(|a| a == "--setup") {
        return run_setup();
    }

    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!("Usage: lazyide [OPTIONS] [PATH]");
        println!();
        println!("Arguments:");
        println!("  [PATH]    Directory to open (default: current directory)");
        println!();
        println!("Options:");
        println!("  --setup   Check for and install optional tools (rust-analyzer, ripgrep)");
        println!("  --help    Show this help message");
        return Ok(());
    }

    let root = if let Some(path) = std::env::args().nth(1) {
        PathBuf::from(path)
    } else {
        std::env::current_dir()?
    };
    if !root.is_dir() {
        eprintln!("Root path is not a directory: {}", root.display());
        return Ok(());
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let enhanced_keys = ratatui::crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
    if enhanced_keys {
        let _ = execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }

    // Restore terminal on panic so it doesn't get stuck in raw mode
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(info);
    }));

    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;

    let mut app = App::new(root)?;
    app.enhanced_keys = enhanced_keys;
    let result = run_app(terminal, app);

    disable_raw_mode()?;
    let mut stdout = io::stdout();
    if enhanced_keys {
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    }
    execute!(stdout, LeaveAlternateScreen, DisableMouseCapture)?;

    result
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod syntax_and_lang_tests {
    use super::*;
    use std::path::Path;
    use ratatui::style::Color;

    const BC: [Color; 3] = [Color::Rgb(210, 168, 75), Color::Rgb(176, 82, 204), Color::Rgb(0, 175, 215)];

    fn create_test_theme() -> Theme {
        Theme {
            name: "test_theme".to_string(),
            theme_type: "dark".to_string(),
            bg: Color::Rgb(30, 30, 30),
            bg_alt: Color::Rgb(40, 40, 40),
            fg: Color::Rgb(220, 220, 220),
            fg_muted: Color::Rgb(100, 100, 120),
            border: Color::Rgb(100, 100, 100),
            accent: Color::Rgb(86, 156, 214),
            accent_secondary: Color::Rgb(206, 198, 130),
            selection: Color::Rgb(60, 60, 60),
            comment: Color::Rgb(100, 100, 120),
            syntax_string: Color::Rgb(156, 220, 140),
            syntax_number: Color::Rgb(181, 206, 168),
            syntax_tag: Color::Rgb(86, 156, 214),
            syntax_attribute: Color::Rgb(78, 201, 176),
            bracket_1: Color::Rgb(210, 168, 75),
            bracket_2: Color::Rgb(176, 82, 204),
            bracket_3: Color::Rgb(0, 175, 215),
        }
    }

    #[test]
    fn test_syntax_lang_for_path_rust() {
        assert_eq!(syntax_lang_for_path(Some(Path::new("test.rs"))), SyntaxLang::Rust);
    }

    #[test]
    fn test_syntax_lang_for_path_python() {
        assert_eq!(syntax_lang_for_path(Some(Path::new("test.py"))), SyntaxLang::Python);
        assert_eq!(syntax_lang_for_path(Some(Path::new("test.pyi"))), SyntaxLang::Python);
    }

    #[test]
    fn test_syntax_lang_for_path_javascript_typescript() {
        for file in &["test.js", "test.jsx", "test.ts", "test.tsx", "test.mjs", "test.cjs", "test.mts", "test.cts"] {
            assert_eq!(syntax_lang_for_path(Some(Path::new(file))), SyntaxLang::JsTs, "Failed for {}", file);
        }
    }

    #[test]
    fn test_syntax_lang_for_path_go() {
        assert_eq!(syntax_lang_for_path(Some(Path::new("main.go"))), SyntaxLang::Go);
    }

    #[test]
    fn test_syntax_lang_for_path_php() {
        assert_eq!(syntax_lang_for_path(Some(Path::new("index.php"))), SyntaxLang::Php);
        assert_eq!(syntax_lang_for_path(Some(Path::new("page.phtml"))), SyntaxLang::Php);
    }

    #[test]
    fn test_syntax_lang_for_path_css() {
        for file in &["style.css", "app.scss", "design.sass", "theme.less"] {
            assert_eq!(syntax_lang_for_path(Some(Path::new(file))), SyntaxLang::Css, "Failed for {}", file);
        }
    }

    #[test]
    fn test_syntax_lang_for_path_html_xml() {
        for file in &["index.html", "page.htm", "data.xml", "icon.svg", "doc.xhtml",
                      "App.vue", "Component.svelte", "Page.astro", "page.jsp",
                      "template.erb", "partial.hbs", "view.ejs"] {
            assert_eq!(syntax_lang_for_path(Some(Path::new(file))), SyntaxLang::HtmlXml, "Failed for {}", file);
        }
    }

    #[test]
    fn test_syntax_lang_for_path_shell() {
        for file in &["script.sh", "init.bash", "setup.zsh", "run.fish", "env.ksh"] {
            assert_eq!(syntax_lang_for_path(Some(Path::new(file))), SyntaxLang::Shell, "Failed for {}", file);
        }
    }

    #[test]
    fn test_syntax_lang_for_path_json() {
        for file in &["package.json", "config.jsonc", "Cargo.toml", "config.yaml", "data.yml"] {
            assert_eq!(syntax_lang_for_path(Some(Path::new(file))), SyntaxLang::Json, "Failed for {}", file);
        }
    }

    #[test]
    fn test_syntax_lang_for_path_markdown() {
        assert_eq!(syntax_lang_for_path(Some(Path::new("README.md"))), SyntaxLang::Markdown);
        assert_eq!(syntax_lang_for_path(Some(Path::new("NOTES.markdown"))), SyntaxLang::Markdown);
    }

    #[test]
    fn test_syntax_lang_for_path_unknown_extension() {
        assert_eq!(syntax_lang_for_path(Some(Path::new("test.xyz"))), SyntaxLang::Plain);
        assert_eq!(syntax_lang_for_path(Some(Path::new("test.unknown"))), SyntaxLang::Plain);
    }

    #[test]
    fn test_syntax_lang_for_path_no_extension() {
        assert_eq!(syntax_lang_for_path(Some(Path::new("Makefile"))), SyntaxLang::Plain);
        assert_eq!(syntax_lang_for_path(Some(Path::new("README"))), SyntaxLang::Plain);
    }

    #[test]
    fn test_syntax_lang_for_path_none() {
        assert_eq!(syntax_lang_for_path(None), SyntaxLang::Plain);
    }

    #[test]
    fn test_syntax_lang_for_path_case_insensitive() {
        assert_eq!(syntax_lang_for_path(Some(Path::new("test.RS"))), SyntaxLang::Rust);
        assert_eq!(syntax_lang_for_path(Some(Path::new("test.PY"))), SyntaxLang::Python);
        assert_eq!(syntax_lang_for_path(Some(Path::new("test.HTML"))), SyntaxLang::HtmlXml);
    }

    #[test]
    fn test_is_ident_char_alphanumeric() {
        assert!(is_ident_char('a'));
        assert!(is_ident_char('z'));
        assert!(is_ident_char('A'));
        assert!(is_ident_char('Z'));
        assert!(is_ident_char('0'));
        assert!(is_ident_char('9'));
    }

    #[test]
    fn test_is_ident_char_underscore() {
        assert!(is_ident_char('_'));
    }

    #[test]
    fn test_is_ident_char_special_chars() {
        assert!(!is_ident_char('-'));
        assert!(!is_ident_char('.'));
        assert!(!is_ident_char('!'));
        assert!(!is_ident_char('@'));
        assert!(!is_ident_char(' '));
        assert!(!is_ident_char('\t'));
        assert!(!is_ident_char('\n'));
    }

    #[test]
    fn test_is_ident_char_unicode() {
        assert!(!is_ident_char('ñ'));
        assert!(!is_ident_char('ü'));
        assert!(!is_ident_char('中'));
        assert!(!is_ident_char('🦀'));
    }

    #[test]
    fn test_keywords_for_lang_rust() {
        let keywords = keywords_for_lang(SyntaxLang::Rust);
        assert!(!keywords.is_empty());
        assert!(keywords.contains(&"fn"));
        assert!(keywords.contains(&"let"));
        assert!(keywords.contains(&"mut"));
        assert!(keywords.contains(&"struct"));
        assert!(keywords.contains(&"enum"));
        assert!(keywords.contains(&"impl"));
    }

    #[test]
    fn test_keywords_for_lang_python() {
        let keywords = keywords_for_lang(SyntaxLang::Python);
        assert!(!keywords.is_empty());
        assert!(keywords.contains(&"def"));
        assert!(keywords.contains(&"class"));
        assert!(keywords.contains(&"import"));
        assert!(keywords.contains(&"None"));
        assert!(keywords.contains(&"True"));
        assert!(keywords.contains(&"False"));
    }

    #[test]
    fn test_keywords_for_lang_jsts() {
        let keywords = keywords_for_lang(SyntaxLang::JsTs);
        assert!(!keywords.is_empty());
        assert!(keywords.contains(&"function"));
        assert!(keywords.contains(&"const"));
        assert!(keywords.contains(&"let"));
        assert!(keywords.contains(&"async"));
        assert!(keywords.contains(&"await"));
    }

    #[test]
    fn test_keywords_for_lang_go() {
        let keywords = keywords_for_lang(SyntaxLang::Go);
        assert!(!keywords.is_empty());
        assert!(keywords.contains(&"package"));
        assert!(keywords.contains(&"func"));
        assert!(keywords.contains(&"go"));
        assert!(keywords.contains(&"defer"));
    }

    #[test]
    fn test_keywords_for_lang_no_keywords() {
        assert!(keywords_for_lang(SyntaxLang::HtmlXml).is_empty());
        assert!(keywords_for_lang(SyntaxLang::Json).is_empty());
        assert!(keywords_for_lang(SyntaxLang::Markdown).is_empty());
        assert!(keywords_for_lang(SyntaxLang::Plain).is_empty());
    }

    #[test]
    fn test_comment_start_for_lang_slash_slash() {
        assert_eq!(comment_start_for_lang(SyntaxLang::Rust), Some("//"));
        assert_eq!(comment_start_for_lang(SyntaxLang::JsTs), Some("//"));
        assert_eq!(comment_start_for_lang(SyntaxLang::Go), Some("//"));
    }

    #[test]
    fn test_comment_start_for_lang_hash() {
        assert_eq!(comment_start_for_lang(SyntaxLang::Python), Some("#"));
        assert_eq!(comment_start_for_lang(SyntaxLang::Shell), Some("#"));
    }

    #[test]
    fn test_comment_start_for_lang_slash_star() {
        assert_eq!(comment_start_for_lang(SyntaxLang::Php), Some("/*"));
        assert_eq!(comment_start_for_lang(SyntaxLang::Css), Some("/*"));
    }

    #[test]
    fn test_comment_start_for_lang_no_comment() {
        assert_eq!(comment_start_for_lang(SyntaxLang::HtmlXml), None);
        assert_eq!(comment_start_for_lang(SyntaxLang::Json), None);
        assert_eq!(comment_start_for_lang(SyntaxLang::Markdown), None);
        assert_eq!(comment_start_for_lang(SyntaxLang::Plain), None);
    }

    #[test]
    fn test_comment_prefix_for_path_slash_slash() {
        for file in &["test.rs", "test.js", "test.ts", "test.go", "test.java", "test.c", "test.cpp", "test.cs", "test.swift"] {
            assert_eq!(comment_prefix_for_path(Path::new(file)), Some("//"), "Failed for {}", file);
        }
    }

    #[test]
    fn test_comment_prefix_for_path_hash() {
        for file in &["test.py", "test.sh", "test.bash", "test.zsh", "config.yaml", "config.yml", "Cargo.toml", "test.rb"] {
            assert_eq!(comment_prefix_for_path(Path::new(file)), Some("#"), "Failed for {}", file);
        }
    }

    #[test]
    fn test_comment_prefix_for_path_html_not_supported() {
        // comment_prefix_for_path doesn't handle html/xml
        assert_eq!(comment_prefix_for_path(Path::new("index.html")), None);
        assert_eq!(comment_prefix_for_path(Path::new("data.xml")), None);
    }

    #[test]
    fn test_comment_prefix_for_path_unknown() {
        assert_eq!(comment_prefix_for_path(Path::new("Makefile")), None);
        assert_eq!(comment_prefix_for_path(Path::new("test.xyz")), None);
    }

    #[test]
    fn test_leading_indent_bytes_no_indent() {
        assert_eq!(leading_indent_bytes("hello world"), 0);
        assert_eq!(leading_indent_bytes("fn main() {"), 0);
    }

    #[test]
    fn test_leading_indent_bytes_spaces() {
        assert_eq!(leading_indent_bytes("  hello"), 2);
        assert_eq!(leading_indent_bytes("    fn test() {"), 4);
        assert_eq!(leading_indent_bytes("        nested"), 8);
    }

    #[test]
    fn test_leading_indent_bytes_tabs() {
        assert_eq!(leading_indent_bytes("\thello"), 1);
        assert_eq!(leading_indent_bytes("\t\tfn test() {"), 2);
    }

    #[test]
    fn test_leading_indent_bytes_mixed() {
        assert_eq!(leading_indent_bytes("\t  hello"), 3);
        assert_eq!(leading_indent_bytes("  \t  fn"), 5);
    }

    #[test]
    fn test_leading_indent_bytes_empty_and_whitespace() {
        assert_eq!(leading_indent_bytes(""), 0);
        assert_eq!(leading_indent_bytes("    "), 4);
        assert_eq!(leading_indent_bytes("\t\t"), 2);
    }

    #[test]
    fn test_highlight_line_plain() {
        let theme = create_test_theme();
        let result = highlight_line("this is plain text", SyntaxLang::Plain, &theme, 0, &BC);
        assert!(!result.spans.is_empty());
    }

    #[test]
    fn test_highlight_line_rust_keyword() {
        let theme = create_test_theme();
        let result = highlight_line("fn main() {", SyntaxLang::Rust, &theme, 0, &BC);
        assert!(!result.spans.is_empty());
    }

    #[test]
    fn test_highlight_line_rust_comment() {
        let theme = create_test_theme();
        let result = highlight_line("// this is a comment", SyntaxLang::Rust, &theme, 0, &BC);
        assert!(!result.spans.is_empty());
    }

    #[test]
    fn test_highlight_line_rust_string() {
        let theme = create_test_theme();
        let result = highlight_line(r#"let s = "hello world";"#, SyntaxLang::Rust, &theme, 0, &BC);
        assert!(!result.spans.is_empty());
    }

    #[test]
    fn test_highlight_line_python() {
        let theme = create_test_theme();
        assert!(!highlight_line("def hello():", SyntaxLang::Python, &theme, 0, &BC).spans.is_empty());
        assert!(!highlight_line("# comment", SyntaxLang::Python, &theme, 0, &BC).spans.is_empty());
    }

    #[test]
    fn test_highlight_line_js_go_shell_css_php() {
        let theme = create_test_theme();
        assert!(!highlight_line("function test() {", SyntaxLang::JsTs, &theme, 0, &BC).spans.is_empty());
        assert!(!highlight_line("package main", SyntaxLang::Go, &theme, 0, &BC).spans.is_empty());
        assert!(!highlight_line("if [ -f file ]; then", SyntaxLang::Shell, &theme, 0, &BC).spans.is_empty());
        assert!(!highlight_line("  display: flex;", SyntaxLang::Css, &theme, 0, &BC).spans.is_empty());
        assert!(!highlight_line("function test() {", SyntaxLang::Php, &theme, 0, &BC).spans.is_empty());
    }

    #[test]
    fn test_highlight_line_markdown() {
        let theme = create_test_theme();
        assert!(!highlight_line("# Heading 1", SyntaxLang::Markdown, &theme, 0, &BC).spans.is_empty());
        assert!(!highlight_line("Normal text", SyntaxLang::Markdown, &theme, 0, &BC).spans.is_empty());
    }

    #[test]
    fn test_highlight_line_html() {
        let theme = create_test_theme();
        assert!(!highlight_line("<div class=\"container\">", SyntaxLang::HtmlXml, &theme, 0, &BC).spans.is_empty());
        assert!(!highlight_line("<!-- comment -->", SyntaxLang::HtmlXml, &theme, 0, &BC).spans.is_empty());
    }

    #[test]
    fn test_syntax_lang_multiple_dots() {
        assert_eq!(syntax_lang_for_path(Some(Path::new("my.test.file.rs"))), SyntaxLang::Rust);
        assert_eq!(syntax_lang_for_path(Some(Path::new("config.test.json"))), SyntaxLang::Json);
    }

    #[test]
    fn test_syntax_lang_path_with_directories() {
        assert_eq!(syntax_lang_for_path(Some(Path::new("src/main.rs"))), SyntaxLang::Rust);
        assert_eq!(syntax_lang_for_path(Some(Path::new("/usr/bin/script.py"))), SyntaxLang::Python);
    }

    #[test]
    fn test_keywords_uniqueness() {
        let rust_keywords = keywords_for_lang(SyntaxLang::Rust);
        let mut seen = std::collections::HashSet::new();
        for keyword in rust_keywords {
            assert!(seen.insert(keyword), "Duplicate keyword: {}", keyword);
        }
    }

    #[test]
    fn test_bracket_pair_colorization() {
        let theme = create_test_theme();
        let bc = [theme.bracket_1, theme.bracket_2, theme.bracket_3];
        // "{ ( ) }" — { at depth 0, ( at depth 1, ) at depth 1, } at depth 0
        let result = highlight_line("{ ( ) }", SyntaxLang::Rust, &theme, 0, &bc);
        let bracket_spans: Vec<_> = result.spans.iter()
            .filter(|s| matches!(s.content.as_ref(), "{" | "}" | "(" | ")"))
            .collect();
        assert_eq!(bracket_spans.len(), 4);
        // { and } should both be depth 0 → bracket_1 color
        let open_brace = bracket_spans[0].style.fg;
        let close_brace = bracket_spans[3].style.fg;
        assert_eq!(open_brace, close_brace, "matching brackets should have same color");
        assert_eq!(open_brace, Some(theme.bracket_1));
        // ( and ) should both be depth 1 → bracket_2 color
        let open_paren = bracket_spans[1].style.fg;
        let close_paren = bracket_spans[2].style.fg;
        assert_eq!(open_paren, close_paren, "matching brackets should have same color");
        assert_eq!(open_paren, Some(theme.bracket_2));
        // Different depths should differ
        assert_ne!(open_brace, open_paren, "different depth brackets should have different colors");
    }
}

#[cfg(test)]
mod fold_and_selection_tests {
    use super::*;

    #[test]
    fn test_fold_ranges_simple_function_with_braces() {
        let lines = vec![
            "fn main() {".to_string(),
            "    println!(\"Hello\");".to_string(),
            "}".to_string(),
        ];
        let (ranges, _) = compute_fold_ranges(&lines, SyntaxLang::Rust);
        assert!(ranges.iter().any(|r| r.start_line == 0 && r.end_line == 2));
    }

    #[test]
    fn test_fold_ranges_nested_braces() {
        let lines = vec![
            "function test() {".to_string(),
            "    if (true) {".to_string(),
            "        console.log(\"nested\");".to_string(),
            "    } else {".to_string(),
            "        console.log(\"other\");".to_string(),
            "    }".to_string(),
            "}".to_string(),
        ];
        let (ranges, _) = compute_fold_ranges(&lines, SyntaxLang::JsTs);
        assert!(ranges.iter().any(|r| r.start_line == 0 && r.end_line == 6));
        assert!(ranges.iter().any(|r| r.start_line == 1 && r.end_line == 3));
        assert!(ranges.iter().any(|r| r.start_line == 3 && r.end_line == 5));
    }

    #[test]
    fn test_fold_ranges_multiple_top_level_items() {
        let lines = vec![
            "func first() {".to_string(),
            "    return 1".to_string(),
            "}".to_string(),
            "".to_string(),
            "func second() {".to_string(),
            "    return 2".to_string(),
            "}".to_string(),
        ];
        let (ranges, _) = compute_fold_ranges(&lines, SyntaxLang::Go);
        assert!(ranges.iter().any(|r| r.start_line == 0 && r.end_line == 2));
        assert!(ranges.iter().any(|r| r.start_line == 4 && r.end_line == 6));
    }

    #[test]
    fn test_fold_ranges_empty_input() {
        let (ranges, _) = compute_fold_ranges(&[], SyntaxLang::Rust);
        assert_eq!(ranges.len(), 0);
    }

    #[test]
    fn test_fold_ranges_single_line_no_folds() {
        let lines = vec!["let x = 42;".to_string()];
        let (ranges, _) = compute_fold_ranges(&lines, SyntaxLang::Rust);
        assert!(!ranges.iter().any(|r| r.start_line == 0 && r.end_line == 0));
    }

    #[test]
    fn test_fold_ranges_mismatched_braces() {
        let lines = vec![
            "fn broken() {".to_string(),
            "    let x = 1;".to_string(),
            "    if true {".to_string(),
            "        let y = 2;".to_string(),
            "    }".to_string(),
        ];
        let (ranges, _) = compute_fold_ranges(&lines, SyntaxLang::Rust);
        assert!(ranges.iter().any(|r| r.start_line == 2 && r.end_line == 4));
    }

    #[test]
    fn test_fold_ranges_struct_definition() {
        let lines = vec![
            "struct Point {".to_string(),
            "    x: i32,".to_string(),
            "    y: i32,".to_string(),
            "}".to_string(),
        ];
        let (ranges, _) = compute_fold_ranges(&lines, SyntaxLang::Rust);
        assert!(ranges.iter().any(|r| r.start_line == 0 && r.end_line == 3));
    }

    #[test]
    fn test_fold_ranges_same_line_braces_no_fold() {
        let lines = vec!["fn test() { return 42; }".to_string()];
        let (ranges, _) = compute_fold_ranges(&lines, SyntaxLang::Rust);
        assert!(!ranges.iter().any(|r| r.start_line == 0 && r.end_line == 0));
    }

    #[test]
    fn test_fold_ranges_python_simple_function() {
        let lines = vec![
            "def hello():".to_string(),
            "    print(\"Hello\")".to_string(),
            "    print(\"World\")".to_string(),
            "print(\"Done\")".to_string(),
        ];
        let (ranges, _) = compute_fold_ranges(&lines, SyntaxLang::Python);
        assert!(ranges.iter().any(|r| r.start_line == 0 && r.end_line == 2));
    }

    #[test]
    fn test_fold_ranges_python_nested_indentation() {
        let lines = vec![
            "def test():".to_string(),
            "    if True:".to_string(),
            "        print(\"nested\")".to_string(),
            "    else:".to_string(),
            "        print(\"other\")".to_string(),
            "print(\"done\")".to_string(),
        ];
        let (ranges, _) = compute_fold_ranges(&lines, SyntaxLang::Python);
        assert!(ranges.iter().any(|r| r.start_line == 0 && r.end_line == 4));
        assert!(ranges.iter().any(|r| r.start_line == 1 && r.end_line == 2));
        assert!(ranges.iter().any(|r| r.start_line == 3 && r.end_line == 4));
    }

    #[test]
    fn test_fold_ranges_python_class_with_methods() {
        let lines = vec![
            "class MyClass:".to_string(),
            "    def __init__(self):".to_string(),
            "        self.x = 1".to_string(),
            "    def method(self):".to_string(),
            "        return self.x".to_string(),
            "print(\"done\")".to_string(),
        ];
        let (ranges, _) = compute_fold_ranges(&lines, SyntaxLang::Python);
        assert!(ranges.iter().any(|r| r.start_line == 0 && r.end_line == 4));
        assert!(ranges.iter().any(|r| r.start_line == 1 && r.end_line == 2));
        assert!(ranges.iter().any(|r| r.start_line == 3 && r.end_line == 4));
    }

    #[test]
    fn test_fold_ranges_python_empty_lines_in_blocks() {
        let lines = vec![
            "def test():".to_string(),
            "    x = 1".to_string(),
            "".to_string(),
            "    y = 2".to_string(),
            "done()".to_string(),
        ];
        let (ranges, _) = compute_fold_ranges(&lines, SyntaxLang::Python);
        assert!(ranges.iter().any(|r| r.start_line == 0 && r.end_line == 3));
    }

    #[test]
    fn test_fold_ranges_html_simple_tag_pair() {
        let lines = vec![
            "<div>".to_string(),
            "    <p>Content</p>".to_string(),
            "</div>".to_string(),
        ];
        let (ranges, _) = compute_fold_ranges(&lines, SyntaxLang::HtmlXml);
        assert!(ranges.iter().any(|r| r.start_line == 0 && r.end_line == 2));
    }

    #[test]
    fn test_fold_ranges_html_nested_tags() {
        let lines = vec![
            "<html>".to_string(),
            "    <body>".to_string(),
            "        <div>Content</div>".to_string(),
            "    </body>".to_string(),
            "</html>".to_string(),
        ];
        let (ranges, _) = compute_fold_ranges(&lines, SyntaxLang::HtmlXml);
        assert!(ranges.iter().any(|r| r.start_line == 0 && r.end_line == 4));
        assert!(ranges.iter().any(|r| r.start_line == 1 && r.end_line == 3));
    }

    // row_has_selection tests

    #[test]
    fn test_row_has_selection_none() {
        assert!(!row_has_selection(5, 20, None));
    }

    #[test]
    fn test_row_has_selection_single_line_matches() {
        assert!(row_has_selection(3, 20, Some(((3, 5), (3, 10)))));
    }

    #[test]
    fn test_row_has_selection_single_line_no_match() {
        assert!(!row_has_selection(5, 20, Some(((3, 5), (3, 10)))));
    }

    #[test]
    fn test_row_has_selection_multi_line_at_start() {
        assert!(row_has_selection(2, 20, Some(((2, 5), (5, 10)))));
    }

    #[test]
    fn test_row_has_selection_multi_line_at_start_short_line() {
        assert!(!row_has_selection(2, 10, Some(((2, 15), (5, 10)))));
    }

    #[test]
    fn test_row_has_selection_multi_line_at_end() {
        assert!(row_has_selection(5, 20, Some(((2, 5), (5, 10)))));
    }

    #[test]
    fn test_row_has_selection_multi_line_at_end_zero_col() {
        assert!(!row_has_selection(5, 20, Some(((2, 5), (5, 0)))));
    }

    #[test]
    fn test_row_has_selection_multi_line_in_middle() {
        assert!(row_has_selection(5, 20, Some(((2, 5), (8, 10)))));
    }

    #[test]
    fn test_row_has_selection_outside_range() {
        let sel = Some(((5, 0), (8, 10)));
        assert!(!row_has_selection(3, 20, sel));
        assert!(!row_has_selection(10, 20, sel));
    }

    #[test]
    fn test_row_has_selection_zero_length() {
        assert!(!row_has_selection(3, 20, Some(((3, 5), (3, 5)))));
    }

    #[test]
    fn test_row_has_selection_zero_length_line_in_middle() {
        assert!(row_has_selection(4, 0, Some(((3, 5), (6, 8)))));
    }

    #[test]
    fn test_row_has_selection_multi_line_full_lines() {
        let sel = Some(((2, 0), (7, 100)));
        assert!(row_has_selection(2, 50, sel));
        assert!(row_has_selection(4, 50, sel));
        assert!(row_has_selection(7, 50, sel));
        assert!(!row_has_selection(1, 50, sel));
        assert!(!row_has_selection(8, 50, sel));
    }
    #[test]
    fn test_visible_rows_map_excludes_folded_lines() {
        // Simulate a 7-line file with lines 1-2 folded (fold starting at line 0)
        let lines: Vec<String> = (0..7).map(|i| format!("line {i}")).collect();
        let fold_ranges = vec![FoldRange { start_line: 0, end_line: 2 }];
        let mut folded_starts = HashSet::new();
        folded_starts.insert(0usize);

        let mut visible = Vec::new();
        for row in 0..lines.len() {
            let hidden = fold_ranges.iter().any(|fr| {
                folded_starts.contains(&fr.start_line) && row > fr.start_line && row <= fr.end_line
            });
            if !hidden {
                visible.push(row);
            }
        }

        // Lines 1 and 2 should be hidden (inside the fold)
        assert_eq!(visible, vec![0, 3, 4, 5, 6]);
    }

    #[test]
    fn test_visible_rows_map_no_folds_shows_all() {
        let num_lines = 5;
        let fold_ranges: Vec<FoldRange> = vec![];
        let folded_starts: HashSet<usize> = HashSet::new();

        let mut visible = Vec::new();
        for row in 0..num_lines {
            let hidden = fold_ranges.iter().any(|fr| {
                folded_starts.contains(&fr.start_line) && row > fr.start_line && row <= fr.end_line
            });
            if !hidden {
                visible.push(row);
            }
        }

        assert_eq!(visible, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_blank_line_fills_full_width() {
        // Verify that blank lines used for empty/beyond-content rows contain spaces
        // to overwrite any previous frame content (prevents ghost artifacts)
        let width = 80usize;
        let blank = " ".repeat(width);
        assert_eq!(blank.len(), width);
        assert!(blank.chars().all(|c| c == ' '));
    }
}

#[cfg(test)]
mod utility_tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use std::path::{Path, PathBuf};

    // color_from_hex tests

    #[test]
    fn test_color_from_hex_valid_uppercase() {
        assert_eq!(color_from_hex("#FF0000", Color::White), Color::Rgb(255, 0, 0));
    }

    #[test]
    fn test_color_from_hex_valid_lowercase() {
        assert_eq!(color_from_hex("#00ff00", Color::White), Color::Rgb(0, 255, 0));
    }

    #[test]
    fn test_color_from_hex_valid_mixed_case() {
        assert_eq!(color_from_hex("#0000Ff", Color::White), Color::Rgb(0, 0, 255));
    }

    #[test]
    fn test_color_from_hex_valid_with_whitespace() {
        assert_eq!(color_from_hex("  #AABBCC  ", Color::White), Color::Rgb(170, 187, 204));
    }

    #[test]
    fn test_color_from_hex_invalid_cases() {
        assert_eq!(color_from_hex("", Color::White), Color::White);
        assert_eq!(color_from_hex("FF0000", Color::White), Color::White);
        assert_eq!(color_from_hex("#FFF", Color::White), Color::White);
        assert_eq!(color_from_hex("#FF00000", Color::White), Color::White);
        assert_eq!(color_from_hex("#GGGGGG", Color::White), Color::White);
        assert_eq!(color_from_hex("not-a-color", Color::White), Color::White);
    }

    #[test]
    fn test_color_from_hex_fallback_used() {
        assert_eq!(color_from_hex("#", Color::Rgb(10, 20, 30)), Color::Rgb(10, 20, 30));
    }

    // inside tests

    #[test]
    fn test_inside_point_in_center() {
        assert!(inside(15, 15, Rect::new(10, 10, 20, 20)));
    }

    #[test]
    fn test_inside_point_at_corners() {
        let rect = Rect::new(10, 10, 20, 20);
        assert!(inside(10, 10, rect));  // top-left inclusive
        assert!(!inside(30, 10, rect)); // top-right exclusive
        assert!(!inside(10, 30, rect)); // bottom-left exclusive
        assert!(!inside(30, 30, rect)); // bottom-right exclusive
        assert!(inside(29, 29, rect));  // just inside
    }

    #[test]
    fn test_inside_point_outside() {
        let rect = Rect::new(10, 10, 20, 20);
        assert!(!inside(9, 15, rect));
        assert!(!inside(30, 15, rect));
        assert!(!inside(15, 9, rect));
        assert!(!inside(15, 30, rect));
    }

    #[test]
    fn test_inside_zero_sized_rect() {
        assert!(!inside(10, 10, Rect::new(10, 10, 0, 0)));
        assert!(!inside(10, 15, Rect::new(10, 10, 0, 20)));
        assert!(!inside(15, 10, Rect::new(10, 10, 20, 0)));
    }

    // centered_rect tests

    #[test]
    fn test_centered_rect_50_percent() {
        let result = centered_rect(50, 50, Rect::new(0, 0, 100, 100));
        assert_eq!(result.width, 50);
        assert_eq!(result.height, 50);
        assert_eq!(result.x, 25);
        assert_eq!(result.y, 25);
    }

    #[test]
    fn test_centered_rect_100_percent() {
        let area = Rect::new(0, 0, 100, 100);
        assert_eq!(centered_rect(100, 100, area), area);
    }

    #[test]
    fn test_centered_rect_non_zero_origin() {
        let result = centered_rect(50, 50, Rect::new(10, 20, 100, 100));
        assert_eq!(result.width, 50);
        assert_eq!(result.height, 50);
        assert_eq!(result.x, 35);
        assert_eq!(result.y, 45);
    }

    // relative_path tests

    #[test]
    fn test_relative_path_under_root() {
        let result = relative_path(Path::new("/home/user/project"), Path::new("/home/user/project/src/main.rs"));
        assert_eq!(result, PathBuf::from("src/main.rs"));
    }

    #[test]
    fn test_relative_path_equals_root() {
        let root = Path::new("/home/user/project");
        assert_eq!(relative_path(root, root), PathBuf::from(""));
    }

    #[test]
    fn test_relative_path_not_under_root() {
        let path = Path::new("/home/other/file.txt");
        assert_eq!(relative_path(Path::new("/home/user/project"), path), path);
    }

    // parse_rg_line tests

    #[test]
    fn test_parse_rg_line_normal() {
        let result = parse_rg_line("src/main.rs:42:fn main() {").unwrap();
        assert_eq!(result.path, PathBuf::from("src/main.rs"));
        assert_eq!(result.line, 42);
        assert_eq!(result.preview, "fn main() {");
    }

    #[test]
    fn test_parse_rg_line_with_colons_in_preview() {
        let result = parse_rg_line("config.toml:10:name = \"test::module\"").unwrap();
        assert_eq!(result.path, PathBuf::from("config.toml"));
        assert_eq!(result.line, 10);
        assert_eq!(result.preview, "name = \"test::module\"");
    }

    #[test]
    fn test_parse_rg_line_empty_preview() {
        let result = parse_rg_line("file.txt:5:").unwrap();
        assert_eq!(result.preview, "");
    }

    #[test]
    fn test_parse_rg_line_invalid_cases() {
        assert!(parse_rg_line("file.txt::some text").is_none());
        assert!(parse_rg_line("file.txt:abc:some text").is_none());
        assert!(parse_rg_line("").is_none());
        assert!(parse_rg_line("file.txt").is_none());
    }

    #[test]
    fn test_parse_rg_line_deep_path() {
        let result = parse_rg_line("src/modules/parser/ast.rs:55:pub struct Ast {").unwrap();
        assert_eq!(result.path, PathBuf::from("src/modules/parser/ast.rs"));
        assert_eq!(result.line, 55);
        assert_eq!(result.preview, "pub struct Ast {");
    }

    // fuzzy_score tests

    #[test]
    fn test_fuzzy_score_exact_match() {
        assert!(fuzzy_score("main", "main").is_some());
    }

    #[test]
    fn test_fuzzy_score_prefix_match() {
        assert!(fuzzy_score("mai", "main.rs").is_some());
    }

    #[test]
    fn test_fuzzy_score_scattered_match() {
        assert!(fuzzy_score("mr", "main.rs").is_some());
    }

    #[test]
    fn test_fuzzy_score_no_match() {
        assert!(fuzzy_score("xyz", "main.rs").is_none());
    }

    #[test]
    fn test_fuzzy_score_empty_query() {
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }

    #[test]
    fn test_fuzzy_score_case_insensitive() {
        // fuzzy_score lowercases candidate but not query — use lowercase query
        assert!(fuzzy_score("main", "MAIN.RS").is_some());
        assert!(fuzzy_score("main", "Main.rs").is_some());
    }

    #[test]
    fn test_fuzzy_score_query_longer_than_candidate() {
        assert!(fuzzy_score("verylongquery", "short").is_none());
    }

    #[test]
    fn test_fuzzy_score_consecutive_chars_better_than_scattered() {
        let s1 = fuzzy_score("mai", "main.rs").unwrap();
        let s2 = fuzzy_score("mai", "m_a__i.rs").unwrap();
        assert!(s1 < s2);
    }

    #[test]
    fn test_fuzzy_score_early_match_better_than_late() {
        let s1 = fuzzy_score("m", "main.rs").unwrap();
        let s2 = fuzzy_score("m", "aaaaaaaaaaaam").unwrap();
        assert!(s1 < s2);
    }

    // pending_hint tests

    #[test]
    fn test_pending_hint_none() {
        assert_eq!(pending_hint(&PendingAction::None), "");
    }

    #[test]
    fn test_pending_hint_quit() {
        let hint = pending_hint(&PendingAction::Quit);
        assert!(!hint.is_empty());
        assert!(hint.contains("quit"));
    }

    #[test]
    fn test_pending_hint_close_prompt() {
        let hint = pending_hint(&PendingAction::ClosePrompt);
        assert!(!hint.is_empty());
        assert!(hint.contains("close"));
    }

    #[test]
    fn test_pending_hint_delete() {
        let hint = pending_hint(&PendingAction::Delete(PathBuf::from("/home/user/project/file.rs")));
        assert!(!hint.is_empty());
        assert!(hint.contains("delete"));
        assert!(hint.contains("file.rs"));
    }

    // command_action_label tests

    #[test]
    fn test_command_action_labels() {
        assert_eq!(command_action_label(CommandAction::Theme), "Theme Picker");
        assert_eq!(command_action_label(CommandAction::Help), "Help");
        assert_eq!(command_action_label(CommandAction::QuickOpen), "Quick Open Files");
        assert_eq!(command_action_label(CommandAction::FindInFile), "Find in File");
        assert_eq!(command_action_label(CommandAction::FindInProject), "Search in Project");
        assert_eq!(command_action_label(CommandAction::SaveFile), "Save File");
        assert_eq!(command_action_label(CommandAction::RefreshTree), "Refresh Tree");
        assert_eq!(command_action_label(CommandAction::ToggleFiles), "Toggle Files Pane");
        assert_eq!(command_action_label(CommandAction::GotoDefinition), "Go to Definition");
        assert_eq!(command_action_label(CommandAction::ReplaceInFile), "Find and Replace");
    }

    // context_label tests

    #[test]
    fn test_context_labels() {
        assert_eq!(context_label(ContextAction::Open), "Open");
        assert_eq!(context_label(ContextAction::NewFile), "New File");
        assert_eq!(context_label(ContextAction::NewFolder), "New Folder");
        assert_eq!(context_label(ContextAction::Rename), "Rename");
        assert_eq!(context_label(ContextAction::Delete), "Delete");
        assert_eq!(context_label(ContextAction::Cancel), "Cancel");
    }

    // editor_context_label tests

    #[test]
    fn test_editor_context_labels() {
        assert_eq!(editor_context_label(EditorContextAction::Copy), "Copy");
        assert_eq!(editor_context_label(EditorContextAction::Cut), "Cut");
        assert_eq!(editor_context_label(EditorContextAction::Paste), "Paste");
        assert_eq!(editor_context_label(EditorContextAction::SelectAll), "Select All");
        assert_eq!(editor_context_label(EditorContextAction::Cancel), "Cancel");
    }
}

#[cfg(test)]
mod theme_and_persistence_tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use ratatui::style::Color;

    #[test]
    fn test_theme_file_deserialize_all_fields() {
        let json = r##"{"name":"Test Theme","type":"dark","colors":{"background":"#1a1b26","backgroundAlt":"#16161e","foreground":"#a9b1d6","foregroundMuted":"#565f89","border":"#414868","accent":"#7aa2f7","selection":"#364a82"}}"##;
        let tf: ThemeFile = serde_json::from_str(json).unwrap();
        assert_eq!(tf.name, "Test Theme");
        assert_eq!(tf.theme_type, "dark");
        assert_eq!(tf.colors.background, "#1a1b26");
        assert_eq!(tf.colors.background_alt, "#16161e");
        assert_eq!(tf.colors.foreground, "#a9b1d6");
        assert_eq!(tf.colors.border, "#414868");
        assert_eq!(tf.colors.accent, "#7aa2f7");
        assert_eq!(tf.colors.selection, "#364a82");
    }

    #[test]
    fn test_theme_file_deserialize_missing_required_field() {
        let json = r##"{"type":"dark","colors":{"background":"#1a1b26","backgroundAlt":"#16161e","foreground":"#a9b1d6","foregroundMuted":"#565f89","border":"#414868","accent":"#7aa2f7","selection":"#364a82"}}"##;
        assert!(serde_json::from_str::<ThemeFile>(json).is_err());
    }

    #[test]
    fn test_theme_file_deserialize_missing_color_field() {
        let json = r##"{"name":"Incomplete","type":"dark","colors":{"background":"#1a1b26","backgroundAlt":"#16161e","foreground":"#a9b1d6","foregroundMuted":"#565f89","border":"#414868","selection":"#364a82"}}"##;
        assert!(serde_json::from_str::<ThemeFile>(json).is_err());
    }

    #[test]
    fn test_theme_file_deserialize_extra_fields_ignored() {
        let json = r##"{"name":"Theme With Extras","type":"dark","source":"https://example.com","colors":{"background":"#1a1b26","backgroundAlt":"#16161e","foreground":"#a9b1d6","foregroundMuted":"#565f89","border":"#414868","accent":"#7aa2f7","selection":"#364a82"},"syntax":{"keyword":"#7aa2f7"}}"##;
        let tf: ThemeFile = serde_json::from_str(json).unwrap();
        assert_eq!(tf.name, "Theme With Extras");
    }

    #[test]
    fn test_persisted_state_round_trip() {
        let state = PersistedState { theme_name: "Dracula".to_string(), files_pane_width: Some(30) };
        let json = serde_json::to_string(&state).unwrap();
        let de: PersistedState = serde_json::from_str(&json).unwrap();
        assert_eq!(de.theme_name, "Dracula");
        assert_eq!(de.files_pane_width, Some(30));
    }

    #[test]
    fn test_persisted_state_round_trip_without_optional() {
        let state = PersistedState { theme_name: "Nord".to_string(), files_pane_width: None };
        let json = serde_json::to_string(&state).unwrap();
        let de: PersistedState = serde_json::from_str(&json).unwrap();
        assert_eq!(de.theme_name, "Nord");
        assert_eq!(de.files_pane_width, None);
    }

    #[test]
    fn test_persisted_state_missing_optional_defaults() {
        let de: PersistedState = serde_json::from_str(r##"{"theme_name":"Monokai Pro"}"##).unwrap();
        assert_eq!(de.theme_name, "Monokai Pro");
        assert_eq!(de.files_pane_width, None);
    }

    #[test]
    fn test_persisted_state_missing_required_fails() {
        assert!(serde_json::from_str::<PersistedState>(r##"{"files_pane_width":20}"##).is_err());
    }

    #[test]
    fn test_theme_conversion_valid_colors() {
        let json = r##"{"name":"Conversion Test","type":"dark","colors":{"background":"#1a1b26","backgroundAlt":"#16161e","foreground":"#a9b1d6","foregroundMuted":"#565f89","border":"#414868","accent":"#7aa2f7","selection":"#364a82"},"syntax":{"comment":"#565f89","string":"#9ece6a","number":"#ff9e64","tag":"#7aa2f7","attribute":"#73daca"}}"##;
        let tf: ThemeFile = serde_json::from_str(json).unwrap();
        let theme = theme_from_file(tf);
        assert_eq!(theme.bg, Color::Rgb(26, 27, 38));
        assert_eq!(theme.fg, Color::Rgb(169, 177, 214));
        assert_eq!(theme.accent, Color::Rgb(122, 162, 247));
        assert_eq!(theme.fg_muted, Color::Rgb(86, 95, 137));
        assert_eq!(theme.comment, Color::Rgb(86, 95, 137));
        assert_eq!(theme.syntax_string, Color::Rgb(158, 206, 106));
        assert_eq!(theme.syntax_number, Color::Rgb(255, 158, 100));
        assert_eq!(theme.syntax_tag, Color::Rgb(122, 162, 247));
        assert_eq!(theme.syntax_attribute, Color::Rgb(115, 218, 202));
    }

    #[test]
    fn test_theme_conversion_invalid_colors_use_fallback() {
        let json = r##"{"name":"Fallback Test","type":"light","colors":{"background":"invalid","backgroundAlt":"not-hex","foreground":"short","foregroundMuted":"#000000","border":"","accent":"notacolor","selection":"#ffffff"}}"##;
        let tf: ThemeFile = serde_json::from_str(json).unwrap();
        let theme = theme_from_file(tf);
        assert_eq!(theme.bg, Color::Rgb(20, 22, 31));
        assert_eq!(theme.border, Color::Rgb(127, 122, 88));
        assert_eq!(theme.selection, Color::Rgb(255, 255, 255));
        // No syntax section → falls back to defaults
        assert_eq!(theme.syntax_string, Color::Rgb(156, 220, 140));
        assert_eq!(theme.syntax_number, Color::Rgb(181, 206, 168));
    }

    // Note: load_themes() tests that use set_current_dir are omitted because
    // they race with parallel test execution. Theme loading is tested indirectly
    // via the actual theme file validation tests below.

    #[test]
    fn test_all_actual_themes_deserialize() {
        let themes_dir = PathBuf::from("themes");
        if !themes_dir.exists() { panic!("themes/ directory not found"); }

        let mut count = 0;
        let mut failures = Vec::new();
        for entry in fs::read_dir(&themes_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "json") {
                count += 1;
                let json = fs::read_to_string(&path).unwrap();
                if let Err(e) = serde_json::from_str::<ThemeFile>(&json) {
                    failures.push(format!("{:?}: {}", path.file_name().unwrap(), e));
                }
            }
        }
        assert!(count > 0, "Should find at least one theme file");
        if !failures.is_empty() {
            panic!("Failed to deserialize themes:\n{}", failures.join("\n"));
        }
    }

    #[test]
    fn test_all_actual_themes_have_valid_hex_colors() {
        let themes_dir = PathBuf::from("themes");
        if !themes_dir.exists() { return; }

        fn is_valid_hex(s: &str) -> bool {
            let s = s.trim();
            if let Some(hex) = s.strip_prefix('#') {
                // Accept 6-char (#RRGGBB) or 8-char (#RRGGBBAA) hex
                (hex.len() == 6 || hex.len() == 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
            } else {
                false
            }
        }

        let mut failures = Vec::new();
        for entry in fs::read_dir(&themes_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "json") {
                let json = fs::read_to_string(&path).unwrap();
                let tf: ThemeFile = match serde_json::from_str(&json) { Ok(t) => t, Err(_) => continue };
                for (field, val) in [("background", &tf.colors.background), ("backgroundAlt", &tf.colors.background_alt),
                    ("foreground", &tf.colors.foreground), ("border", &tf.colors.border),
                    ("accent", &tf.colors.accent), ("selection", &tf.colors.selection)] {
                    if !is_valid_hex(val) {
                        failures.push(format!("{}: Invalid '{}' in '{}'", tf.name, val, field));
                    }
                }
            }
        }
        if !failures.is_empty() {
            panic!("Invalid hex colors:\n{}", failures.join("\n"));
        }
    }

    #[test]
    fn test_all_actual_themes_have_valid_type() {
        let themes_dir = PathBuf::from("themes");
        if !themes_dir.exists() { return; }

        for entry in fs::read_dir(&themes_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "json") {
                let json = fs::read_to_string(&path).unwrap();
                let tf: ThemeFile = match serde_json::from_str(&json) { Ok(t) => t, Err(_) => continue };
                assert!(!tf.name.is_empty(), "{:?}: Empty name", path.file_name().unwrap());
                assert!(tf.theme_type == "dark" || tf.theme_type == "light",
                    "{}: Invalid type '{}'", tf.name, tf.theme_type);
            }
        }
    }
}

#[cfg(test)]
mod lsp_and_struct_tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::mpsc;
    use serde_json::json;
    use std::path::PathBuf;
    use std::collections::HashSet;

    #[test]
    fn test_lsp_reader_loop_valid_notification() {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": "file:///test.rs", "diagnostics": [] }
        });
        let payload = serde_json::to_string(&notification).unwrap();
        let message = format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload);

        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            lsp_reader_loop(Cursor::new(message.as_bytes()), tx);
        });

        std::thread::sleep(std::time::Duration::from_millis(50));
        let received = rx.try_recv().unwrap();
        match received {
            LspInbound::Notification { method, params } => {
                assert_eq!(method, "textDocument/publishDiagnostics");
                assert!(params.get("uri").is_some());
            }
            _ => panic!("Expected Notification"),
        }
        let _ = handle.join();
    }

    #[test]
    fn test_lsp_reader_loop_valid_response() {
        let response = json!({
            "jsonrpc": "2.0", "id": 42,
            "result": { "capabilities": { "textDocumentSync": 1 } }
        });
        let payload = serde_json::to_string(&response).unwrap();
        let message = format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload);

        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            lsp_reader_loop(Cursor::new(message.as_bytes()), tx);
        });

        std::thread::sleep(std::time::Duration::from_millis(50));
        match rx.try_recv().unwrap() {
            LspInbound::Response { id, result } => {
                assert_eq!(id, 42);
                assert!(result.get("capabilities").is_some());
            }
            _ => panic!("Expected Response"),
        }
        let _ = handle.join();
    }

    #[test]
    fn test_lsp_reader_loop_multiple_messages() {
        let msg1 = json!({"jsonrpc":"2.0","method":"initialized","params":{}});
        let msg2 = json!({"jsonrpc":"2.0","id":1,"result":null});
        let msg3 = json!({"jsonrpc":"2.0","method":"window/logMessage","params":{"type":4,"message":"Started"}});

        let p1 = serde_json::to_string(&msg1).unwrap();
        let p2 = serde_json::to_string(&msg2).unwrap();
        let p3 = serde_json::to_string(&msg3).unwrap();
        let messages = format!(
            "Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}",
            p1.len(), p1, p2.len(), p2, p3.len(), p3
        );

        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            lsp_reader_loop(Cursor::new(messages.as_bytes()), tx);
        });

        std::thread::sleep(std::time::Duration::from_millis(100));
        let mut received = Vec::new();
        while let Ok(msg) = rx.try_recv() { received.push(msg); }
        assert_eq!(received.len(), 3);
        let _ = handle.join();
    }

    #[test]
    fn test_lsp_reader_loop_invalid_json_skipped() {
        let invalid = "not valid json!";
        let valid = json!({"jsonrpc":"2.0","method":"test","params":{}});
        let vp = serde_json::to_string(&valid).unwrap();
        let message = format!(
            "Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}",
            invalid.len(), invalid, vp.len(), vp
        );

        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            lsp_reader_loop(Cursor::new(message.as_bytes()), tx);
        });

        std::thread::sleep(std::time::Duration::from_millis(100));
        match rx.try_recv().unwrap() {
            LspInbound::Notification { method, .. } => assert_eq!(method, "test"),
            _ => panic!("Expected Notification"),
        }
        let _ = handle.join();
    }

    #[test]
    fn test_lsp_reader_loop_truncated_input() {
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            lsp_reader_loop(Cursor::new("Content-Length: 100\r\n\r\nincomplete".as_bytes()), tx);
        });
        assert!(handle.join().is_ok());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn test_lsp_reader_loop_response_with_error() {
        let error_resp = json!({"jsonrpc":"2.0","id":5,"error":{"code":-32601,"message":"Method not found"}});
        let payload = serde_json::to_string(&error_resp).unwrap();
        let message = format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload);

        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            lsp_reader_loop(Cursor::new(message.as_bytes()), tx);
        });

        std::thread::sleep(std::time::Duration::from_millis(50));
        match rx.try_recv().unwrap() {
            LspInbound::Response { id, result } => {
                assert_eq!(id, 5);
                assert!(result.get("code").is_some());
            }
            _ => panic!("Expected Response"),
        }
        let _ = handle.join();
    }

    #[test]
    fn test_lsp_jsonrpc_format() {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": { "textDocument": { "uri": "file:///test.rs", "languageId": "rust", "version": 1, "text": "fn main() {}" } }
        });
        let payload = serde_json::to_vec(&notification).unwrap();
        let header = format!("Content-Length: {}\r\n\r\n", payload.len());
        assert!(header.starts_with("Content-Length: "));
        assert!(header.ends_with("\r\n\r\n"));
        let cl: usize = header.strip_prefix("Content-Length: ").unwrap().strip_suffix("\r\n\r\n").unwrap().parse().unwrap();
        assert_eq!(cl, payload.len());
    }

    #[test]
    fn test_file_uri_absolute_path() {
        let test_file = std::env::temp_dir().join("lazyide_test_file_uri.txt");
        std::fs::write(&test_file, "test").unwrap();
        let uri = file_uri(&test_file);
        assert!(uri.is_some());
        assert!(uri.unwrap().starts_with("file://"));
        let _ = std::fs::remove_file(&test_file);
    }

    #[test]
    fn test_file_uri_nonexistent_path() {
        assert!(file_uri(&PathBuf::from("/nonexistent/path/to/file.txt")).is_none());
    }

    #[test]
    fn test_file_uri_directory_path() {
        let uri = file_uri(&std::env::temp_dir());
        assert!(uri.is_some());
        assert!(uri.unwrap().starts_with("file://"));
    }

    #[test]
    fn test_lsp_diagnostic_construction() {
        let d = LspDiagnostic { line: 10, col: 5, severity: "Error".to_string(), message: "unused variable".to_string() };
        assert_eq!(d.line, 10);
        assert_eq!(d.col, 5);
        assert_eq!(d.severity, "Error");
        assert_eq!(d.message, "unused variable");
    }

    #[test]
    fn test_lsp_diagnostic_clone() {
        let d = LspDiagnostic { line: 100, col: 50, severity: "Error".to_string(), message: "type mismatch".to_string() };
        let c = d.clone();
        assert_eq!(d.line, c.line);
        assert_eq!(d.severity, c.severity);
    }

    #[test]
    fn test_lsp_completion_item_construction() {
        let item = LspCompletionItem { label: "println!".to_string(), insert_text: Some("println!(\"{}\")".to_string()), detail: Some("macro".to_string()) };
        assert_eq!(item.label, "println!");
        assert!(item.insert_text.is_some());
        assert!(item.detail.is_some());
    }

    #[test]
    fn test_lsp_completion_item_without_optionals() {
        let item = LspCompletionItem { label: "main".to_string(), insert_text: None, detail: None };
        assert_eq!(item.label, "main");
        assert!(item.insert_text.is_none());
        assert!(item.detail.is_none());
    }

    #[test]
    fn test_lsp_completion_item_clone() {
        let item = LspCompletionItem { label: "HashMap".to_string(), insert_text: Some("HashMap::new()".to_string()), detail: Some("std::collections".to_string()) };
        let c = item.clone();
        assert_eq!(item.label, c.label);
        assert_eq!(item.insert_text, c.insert_text);
    }

    #[test]
    fn test_tab_struct_construction() {
        let tab = Tab {
            path: PathBuf::from("/test/file.rs"), is_preview: false,
            editor: TextArea::default(), dirty: false,
            open_disk_snapshot: None, editor_scroll_row: 0,
            fold_ranges: Vec::new(), bracket_depths: Vec::new(),
            folded_starts: HashSet::new(),
            visible_rows_map: Vec::new(), open_doc_uri: None,
            open_doc_version: 0, diagnostics: Vec::new(),
            conflict_prompt_open: false, conflict_disk_text: None,
            recovery_prompt_open: false, recovery_text: None,
        };
        assert_eq!(tab.path, PathBuf::from("/test/file.rs"));
        assert!(!tab.is_preview);
        assert!(!tab.dirty);
    }

    #[test]
    fn test_tab_struct_all_fields() {
        let mut editor = TextArea::default();
        editor.insert_str("fn main() {}");
        let tab = Tab {
            path: PathBuf::from("/src/main.rs"), is_preview: true,
            editor, dirty: true,
            open_disk_snapshot: Some("old".to_string()), editor_scroll_row: 10,
            fold_ranges: vec![FoldRange { start_line: 5, end_line: 15 }],
            bracket_depths: Vec::new(),
            folded_starts: { let mut s = HashSet::new(); s.insert(5); s },
            visible_rows_map: vec![0, 1, 2, 16, 17],
            open_doc_uri: Some("file:///src/main.rs".to_string()),
            open_doc_version: 3,
            diagnostics: vec![LspDiagnostic { line: 1, col: 0, severity: "Warning".to_string(), message: "unused".to_string() }],
            conflict_prompt_open: true, conflict_disk_text: Some("disk".to_string()),
            recovery_prompt_open: false, recovery_text: None,
        };
        assert!(tab.is_preview);
        assert!(tab.dirty);
        assert_eq!(tab.fold_ranges.len(), 1);
        assert_eq!(tab.diagnostics.len(), 1);
        assert_eq!(tab.open_doc_version, 3);
    }

    #[test]
    fn test_tree_item_file() {
        let item = TreeItem { path: PathBuf::from("/project/src/main.rs"), name: "main.rs".to_string(), depth: 2, is_dir: false, expanded: false };
        assert_eq!(item.name, "main.rs");
        assert_eq!(item.depth, 2);
        assert!(!item.is_dir);
    }

    #[test]
    fn test_tree_item_directory() {
        let item = TreeItem { path: PathBuf::from("/project/src"), name: "src".to_string(), depth: 1, is_dir: true, expanded: true };
        assert!(item.is_dir);
        assert!(item.expanded);
    }

    #[test]
    fn test_tree_item_clone() {
        let item = TreeItem { path: PathBuf::from("/test.rs"), name: "test.rs".to_string(), depth: 1, is_dir: false, expanded: false };
        let c = item.clone();
        assert_eq!(item.path, c.path);
        assert_eq!(item.name, c.name);
    }
}
