use std::collections::HashSet;
use std::path::PathBuf;

use tui_textarea::TextArea;

use crate::lsp_client::LspDiagnostic;

#[derive(Debug, Clone)]
pub(crate) struct ProjectSearchHit {
    pub(crate) path: PathBuf,
    pub(crate) line: usize,
    pub(crate) preview: String,
}

#[derive(Debug, Clone)]
pub(crate) struct FoldRange {
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
}

pub(crate) struct Tab {
    pub(crate) path: PathBuf,
    pub(crate) is_preview: bool,
    pub(crate) editor: TextArea<'static>,
    pub(crate) dirty: bool,
    pub(crate) open_disk_snapshot: Option<String>,
    pub(crate) editor_scroll_row: usize,
    pub(crate) fold_ranges: Vec<FoldRange>,
    pub(crate) bracket_depths: Vec<u16>,
    pub(crate) folded_starts: HashSet<usize>,
    pub(crate) visible_rows_map: Vec<usize>,
    pub(crate) open_doc_uri: Option<String>,
    pub(crate) open_doc_version: i32,
    pub(crate) diagnostics: Vec<LspDiagnostic>,
    pub(crate) conflict_prompt_open: bool,
    pub(crate) conflict_disk_text: Option<String>,
    pub(crate) recovery_prompt_open: bool,
    pub(crate) recovery_text: Option<String>,
}
