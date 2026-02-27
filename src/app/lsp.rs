use super::App;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::TryRecvError;

use serde_json::{Value, json};
use url::Url;

use crate::lsp_client::{LspClient, LspCompletionItem, LspDiagnostic, LspInbound};
use crate::syntax::{is_ident_char, keywords_for_lang, syntax_lang_for_path};
use crate::util::{file_uri, to_u16_saturating};

impl App {
    pub(crate) fn request_lsp_definition(&mut self) {
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

    pub(crate) fn handle_definition_response(&mut self, result: Value) -> io::Result<()> {
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
            let range = item
                .get("range")
                .or_else(|| item.get("targetSelectionRange"));
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
            tab.editor.move_cursor(ratatui_textarea::CursorMove::Jump(
                to_u16_saturating(line),
                to_u16_saturating(col),
            ));
        }
        self.sync_editor_scroll_guess();
        self.set_status("Jumped to definition");
        Ok(())
    }

    pub(crate) fn try_local_definition_jump(&mut self) -> bool {
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
        let Some(tab) = self.active_tab() else {
            return false;
        };
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
                self.tabs[self.active_tab]
                    .editor
                    .move_cursor(ratatui_textarea::CursorMove::Jump(
                        to_u16_saturating(i),
                        to_u16_saturating(col),
                    ));
                self.sync_editor_scroll_guess();
                self.set_status("Jumped to local definition");
                return true;
            }
        }
        false
    }

    pub(crate) fn ensure_lsp_for_path(&mut self, path: &Path) {
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
            self.completion.reset();
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

    pub(crate) fn notify_lsp_did_change(&mut self) {
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

    pub(crate) fn poll_lsp(&mut self) {
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

    pub(crate) fn handle_publish_diagnostics(&mut self, params: Value) {
        let uri = params
            .get("uri")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        // Find the tab that matches this URI
        let tab_idx = self
            .tabs
            .iter()
            .position(|t| t.open_doc_uri.as_deref() == Some(uri.as_str()));
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
                    severity,
                    message,
                });
            }
        }
        self.tabs[tab_idx].diagnostics = diagnostics;
    }

    pub(crate) fn request_lsp_completion(&mut self) {
        let uri = self.active_tab().and_then(|t| t.open_doc_uri.clone());
        let Some((row, col)) = self.active_tab().map(|t| t.editor.cursor()) else {
            return;
        };
        let prefix = self.current_identifier_prefix();
        self.completion.prefix = prefix.clone();
        self.completion.ghost = None;
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

    pub(crate) fn handle_completion_response(&mut self, result: Value) {
        if result.get("code").is_some() && result.get("message").is_some() {
            let msg = result
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("LSP completion error");
            self.completion.items.clear();
            self.completion.reset();
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
        self.completion.items = items_out;
        self.completion.index = 0;
        self.completion.open = !self.completion.items.is_empty();
        self.completion.ghost = self.completion.items.first().and_then(|item| {
            let label = item.insert_text.as_deref().unwrap_or(&item.label);
            self.ghost_suffix(label, &self.completion.prefix)
        });
        if self.completion.open {
            self.set_status(format!("{} completion items", self.completion.items.len()));
        } else {
            self.set_status("No completions");
        }
    }

    pub(crate) fn fallback_completion_items(&self) -> Vec<LspCompletionItem> {
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
        let editor_lines = self
            .active_tab()
            .map(|t| t.editor.lines())
            .unwrap_or(&empty_lines);
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

    pub(crate) fn current_identifier_prefix(&self) -> String {
        let Some(tab) = self.active_tab() else {
            return String::new();
        };
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

    pub(crate) fn current_identifier_at_cursor(&self) -> String {
        let Some(tab) = self.active_tab() else {
            return String::new();
        };
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

    pub(crate) fn apply_completion(&mut self) {
        let Some(item) = self.completion.items.get(self.completion.index).cloned() else {
            self.completion.reset();
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
        let inserted = self
            .active_tab_mut()
            .is_some_and(|t| t.editor.insert_str(insert));
        if inserted {
            self.on_editor_content_changed();
        }
        self.completion.reset();
        self.set_status(format!("Inserted completion: {}", item.label));
    }

    pub(crate) fn update_completion_ghost_from_selection(&mut self) {
        self.completion.ghost = self
            .completion
            .items
            .get(self.completion.index)
            .and_then(|item| {
                let label = item.insert_text.as_deref().unwrap_or(&item.label);
                self.ghost_suffix(label, &self.completion.prefix)
            });
    }

    pub(crate) fn refresh_inline_ghost(&mut self) {
        let prefix = self.current_identifier_prefix();
        if prefix.chars().count() < Self::INLINE_GHOST_MIN_PREFIX {
            self.completion.prefix.clear();
            self.completion.ghost = None;
            return;
        }
        self.completion.prefix = prefix.clone();
        self.completion.ghost = self
            .fallback_completion_items()
            .into_iter()
            .filter_map(|item| {
                let text = item.insert_text.unwrap_or(item.label);
                self.ghost_suffix(&text, &prefix)
            })
            .min_by_key(|s| s.len());
    }

    fn ghost_suffix(&self, label: &str, prefix: &str) -> Option<String> {
        if prefix.is_empty() {
            return None;
        }
        label
            .strip_prefix(prefix)
            .filter(|suffix| !suffix.is_empty())
            .map(ToString::to_string)
    }
}
