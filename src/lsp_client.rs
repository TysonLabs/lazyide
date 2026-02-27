use std::env;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use url::Url;

#[derive(Debug, Clone)]
pub(crate) struct LspDiagnostic {
    pub(crate) line: usize,
    pub(crate) severity: String,
    pub(crate) message: String,
}

#[derive(Debug, Clone)]
pub(crate) struct LspCompletionItem {
    pub(crate) label: String,
    pub(crate) insert_text: Option<String>,
    pub(crate) detail: Option<String>,
}

#[derive(Debug)]
pub(crate) enum LspInbound {
    Notification { method: String, params: Value },
    Response { id: i64, result: Value },
}

pub(crate) struct LspClient {
    pub(crate) writer: Arc<Mutex<ChildStdin>>,
    pub(crate) rx: Receiver<LspInbound>,
    pub(crate) next_id: i64,
}

impl LspClient {
    pub(crate) fn new_rust_analyzer(root: &Path) -> io::Result<Self> {
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

    pub(crate) fn wait_for_initialize(&self, init_id: i64) -> io::Result<()> {
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
                        return Err(io::Error::other(format!(
                            "LSP initialize error: {}",
                            result
                        )));
                    }
                    return Ok(());
                }
                Ok(_) => continue,
                Err(_) => return Err(io::Error::other("LSP initialize response missing")),
            }
        }
    }

    pub(crate) fn send_notification(&self, method: &str, params: Value) -> io::Result<()> {
        self.send_raw(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    pub(crate) fn send_request(&mut self, method: &str, params: Value) -> io::Result<i64> {
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

    pub(crate) fn send_raw(&self, value: Value) -> io::Result<()> {
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

pub(crate) fn resolve_rust_analyzer_bin() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            candidates.push(dir.join("rust-analyzer"));
        }
    }
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.push(home.join(".cargo/bin/rust-analyzer"));
        candidates
            .push(home.join(".rustup/toolchains/stable-aarch64-apple-darwin/bin/rust-analyzer"));
        candidates
            .push(home.join(".rustup/toolchains/stable-x86_64-apple-darwin/bin/rust-analyzer"));
        candidates.push(
            home.join(".rustup/toolchains/stable-aarch64-unknown-linux-gnu/bin/rust-analyzer"),
        );
        candidates.push(
            home.join(".rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rust-analyzer"),
        );
    }
    candidates.into_iter().find(|p| p.is_file())
}

pub(crate) fn lsp_reader_loop(stdout: impl Read, tx: Sender<LspInbound>) {
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
#[cfg(test)]
mod lsp_and_struct_tests {
    use super::*;
    use crate::tab::{FoldRange, Tab};
    use crate::tree_item::TreeItem;
    use crate::util::file_uri;
    use serde_json::json;
    use std::collections::HashSet;
    use std::io::Cursor;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use ratatui_textarea::TextArea;

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
            p1.len(),
            p1,
            p2.len(),
            p2,
            p3.len(),
            p3
        );

        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            lsp_reader_loop(Cursor::new(messages.as_bytes()), tx);
        });

        std::thread::sleep(std::time::Duration::from_millis(100));
        let mut received = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            received.push(msg);
        }
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
            invalid.len(),
            invalid,
            vp.len(),
            vp
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
            lsp_reader_loop(
                Cursor::new("Content-Length: 100\r\n\r\nincomplete".as_bytes()),
                tx,
            );
        });
        assert!(handle.join().is_ok());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn test_lsp_reader_loop_response_with_error() {
        let error_resp =
            json!({"jsonrpc":"2.0","id":5,"error":{"code":-32601,"message":"Method not found"}});
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
        let cl: usize = header
            .strip_prefix("Content-Length: ")
            .unwrap()
            .strip_suffix("\r\n\r\n")
            .unwrap()
            .parse()
            .unwrap();
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
        let d = LspDiagnostic {
            line: 10,
            severity: "Error".to_string(),
            message: "unused variable".to_string(),
        };
        assert_eq!(d.line, 10);
        assert_eq!(d.severity, "Error");
        assert_eq!(d.message, "unused variable");
    }

    #[test]
    fn test_lsp_diagnostic_clone() {
        let d = LspDiagnostic {
            line: 100,
            severity: "Error".to_string(),
            message: "type mismatch".to_string(),
        };
        let c = d.clone();
        assert_eq!(d.line, c.line);
        assert_eq!(d.severity, c.severity);
    }

    #[test]
    fn test_lsp_completion_item_construction() {
        let item = LspCompletionItem {
            label: "println!".to_string(),
            insert_text: Some("println!(\"{}\")".to_string()),
            detail: Some("macro".to_string()),
        };
        assert_eq!(item.label, "println!");
        assert!(item.insert_text.is_some());
        assert!(item.detail.is_some());
    }

    #[test]
    fn test_lsp_completion_item_without_optionals() {
        let item = LspCompletionItem {
            label: "main".to_string(),
            insert_text: None,
            detail: None,
        };
        assert_eq!(item.label, "main");
        assert!(item.insert_text.is_none());
        assert!(item.detail.is_none());
    }

    #[test]
    fn test_lsp_completion_item_clone() {
        let item = LspCompletionItem {
            label: "HashMap".to_string(),
            insert_text: Some("HashMap::new()".to_string()),
            detail: Some("std::collections".to_string()),
        };
        let c = item.clone();
        assert_eq!(item.label, c.label);
        assert_eq!(item.insert_text, c.insert_text);
    }

    #[test]
    fn test_tab_struct_construction() {
        let tab = Tab {
            path: PathBuf::from("/test/file.rs"),
            is_preview: false,
            editor: TextArea::default(),
            dirty: false,
            open_disk_snapshot: None,
            editor_scroll_row: 0,
            editor_scroll_col: 0,
            fold_ranges: Vec::new(),
            bracket_depths: Vec::new(),
            folded_starts: HashSet::new(),
            visible_rows_map: Vec::new(),
            visible_row_starts: Vec::new(),
            visible_row_ends: Vec::new(),
            open_doc_uri: None,
            open_doc_version: 0,
            diagnostics: Vec::new(),
            conflict_prompt_open: false,
            conflict_disk_text: None,
            recovery_prompt_open: false,
            recovery_text: None,
            git_line_status: Vec::new(),
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
            path: PathBuf::from("/src/main.rs"),
            is_preview: true,
            editor,
            dirty: true,
            open_disk_snapshot: Some("old".to_string()),
            editor_scroll_row: 10,
            editor_scroll_col: 0,
            fold_ranges: vec![FoldRange {
                start_line: 5,
                end_line: 15,
            }],
            bracket_depths: Vec::new(),
            folded_starts: {
                let mut s = HashSet::new();
                s.insert(5);
                s
            },
            visible_rows_map: vec![0, 1, 2, 16, 17],
            visible_row_starts: vec![0, 0, 0, 0, 0],
            visible_row_ends: vec![10, 10, 10, 10, 10],
            open_doc_uri: Some("file:///src/main.rs".to_string()),
            open_doc_version: 3,
            diagnostics: vec![LspDiagnostic {
                line: 1,
                severity: "Warning".to_string(),
                message: "unused".to_string(),
            }],
            conflict_prompt_open: true,
            conflict_disk_text: Some("disk".to_string()),
            recovery_prompt_open: false,
            recovery_text: None,
            git_line_status: Vec::new(),
        };
        assert!(tab.is_preview);
        assert!(tab.dirty);
        assert_eq!(tab.fold_ranges.len(), 1);
        assert_eq!(tab.diagnostics.len(), 1);
        assert_eq!(tab.open_doc_version, 3);
    }

    #[test]
    fn test_tree_item_file() {
        let item = TreeItem {
            path: PathBuf::from("/project/src/main.rs"),
            name: "main.rs".to_string(),
            depth: 2,
            is_dir: false,
            expanded: false,
        };
        assert_eq!(item.name, "main.rs");
        assert_eq!(item.depth, 2);
        assert!(!item.is_dir);
    }

    #[test]
    fn test_tree_item_directory() {
        let item = TreeItem {
            path: PathBuf::from("/project/src"),
            name: "src".to_string(),
            depth: 1,
            is_dir: true,
            expanded: true,
        };
        assert!(item.is_dir);
        assert!(item.expanded);
    }

    #[test]
    fn test_tree_item_clone() {
        let item = TreeItem {
            path: PathBuf::from("/test.rs"),
            name: "test.rs".to_string(),
            depth: 1,
            is_dir: false,
            expanded: false,
        };
        let c = item.clone();
        assert_eq!(item.path, c.path);
        assert_eq!(item.name, c.name);
    }
}
