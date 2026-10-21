use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ratatui_textarea::TextArea;

use crate::lsp_client::LspDiagnostic;
use crate::notebook::NotebookDoc;
use crate::syntax::{SyntaxLang, comment_start_for_lang, syntax_lang_for_path};
use crate::util::comment_prefix_for_path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum GitLineStatus {
    #[default]
    None,
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitFileStatus {
    Modified,
    Added,
    Untracked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct GitChangeSummary {
    pub(crate) files_changed: usize,
    pub(crate) insertions: usize,
    pub(crate) deletions: usize,
}

impl GitChangeSummary {
    pub(crate) fn is_clean(&self) -> bool {
        self.files_changed == 0 && self.insertions == 0 && self.deletions == 0
    }
}

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
    pub(crate) editor_scroll_col: usize,
    pub(crate) fold_ranges: Vec<FoldRange>,
    pub(crate) bracket_depths: Vec<u16>,
    pub(crate) folded_starts: HashSet<usize>,
    pub(crate) visible_rows_map: Vec<usize>,
    pub(crate) visible_row_starts: Vec<usize>,
    pub(crate) visible_row_ends: Vec<usize>,
    pub(crate) open_doc_uri: Option<String>,
    pub(crate) open_doc_version: i32,
    pub(crate) diagnostics: Vec<LspDiagnostic>,
    pub(crate) conflict_prompt_open: bool,
    pub(crate) conflict_disk_text: Option<String>,
    pub(crate) recovery_prompt_open: bool,
    pub(crate) recovery_text: Option<String>,
    pub(crate) git_line_status: Vec<GitLineStatus>,
    /// Set when this tab shows a projected notebook; None for raw text (incl. .ipynb fallback).
    #[allow(dead_code)] // TEMPORARY (skeleton): removed once open_file_as reads it.
    pub(crate) notebook: Option<NotebookDoc>,
}

/// Syntax language of a tab's text: the notebook's kernel language for a
/// notebook tab, the path's extension otherwise. Shared by `Tab::syntax_lang`
/// and `open_file_as` (which needs it before the `Tab` exists).
pub(crate) fn tab_syntax_lang(path: &Path, notebook: Option<&NotebookDoc>) -> SyntaxLang {
    match notebook {
        Some(doc) => doc.lang(),
        None => syntax_lang_for_path(Some(path)),
    }
}

impl Tab {
    pub(crate) fn is_notebook(&self) -> bool {
        self.notebook.is_some()
    }

    pub(crate) fn syntax_lang(&self) -> SyntaxLang {
        tab_syntax_lang(&self.path, self.notebook.as_ref())
    }

    /// Line-comment prefix for toggle comment: the notebook language's prefix
    /// on a notebook tab, the path-based prefix otherwise.
    pub(crate) fn comment_prefix(&self) -> Option<&'static str> {
        match &self.notebook {
            Some(doc) => comment_start_for_lang(doc.lang()),
            None => comment_prefix_for_path(&self.path),
        }
    }
}
