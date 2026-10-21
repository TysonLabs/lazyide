//! Jupyter notebook (`.ipynb`) projection: a notebook is shown in a normal
//! editor tab as percent-format cell text with opaque identity tokens, and
//! saved back by splicing only changed cells into the original JSON.
//!
//! See `docs/plans/jupyter-notebooks.md` for the design. This module is pure:
//! no `App` access, no filesystem access.
//!
//! JSON values are never re-serialized unless they changed: the root object
//! and each cell are parsed as ordered `(key, RawValue)` lists, and the writer
//! emits the Jupyter layout (indent 1) around the raw values.
//!
//! The public items carry `#[allow(dead_code)]` until the App wiring slices
//! (B/E) call them; those slices remove the attributes.

use std::collections::HashSet;
use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::Value;
use serde_json::value::RawValue;

use crate::syntax::SyntaxLang;

/// Kind of a notebook cell, as written in `cell_type` and in the marker line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CellKind {
    Code,
    Markdown,
    Raw,
}

/// A parsed marker line: `# %%`, `# %% [markdown]`, `# %% [raw]`, each with
/// an optional ` #xxxxxx` token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Marker {
    pub(crate) kind: CellKind,
    pub(crate) token: Option<String>,
}

/// Why a file could not be projected; the caller opens it as raw JSON and
/// shows `Opened x.ipynb as raw JSON: <reason>`.
#[allow(dead_code)] // Wired by slice B (open_file_as); remove then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotebookOpenError {
    InvalidJson(String),
    Unsupported(String),
    Invalid(String),
    MarkerCollision { cell_index: usize },
}

impl std::fmt::Display for NotebookOpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJson(m) => write!(f, "invalid JSON ({m})"),
            Self::Unsupported(m) => write!(f, "unsupported notebook ({m})"),
            Self::Invalid(m) => write!(f, "invalid notebook ({m})"),
            Self::MarkerCollision { cell_index } => {
                write!(f, "cell {} contains a `# %%` line", cell_index + 1)
            }
        }
    }
}

/// Why a save was refused; nothing is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotebookSaveError {
    /// A line starting with `# %%` that is not a valid marker (0-based row).
    /// Also used for a markdown/raw line typed as `%%…` without the `# `
    /// prefix, which would reopen as a marker.
    MalformedMarker { row: usize },
}

impl std::fmt::Display for NotebookSaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedMarker { row } => write!(f, "Malformed cell marker at line {}", row + 1),
        }
    }
}

/// One-line output summary for the status bar.
#[allow(dead_code)] // Wired by slice E (status bar); remove then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CellSummary {
    /// 0-based index of the cell in the notebook on disk.
    pub(crate) index: usize,
    pub(crate) cell_count: usize,
    pub(crate) kind: CellKind,
    pub(crate) execution_count: Option<i64>,
    /// Output kinds in order, e.g. `text`, `image/png`, `stream`, `error`.
    pub(crate) output_kinds: Vec<String>,
    /// `ename` of the first error output, if any.
    pub(crate) error: Option<String>,
}

/// Result of [`rebuild`]: the bytes to write and the tokens to append to the
/// marker lines of new (tokenless) cells.
#[allow(dead_code)] // Wired by slice B (save_file); remove then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rebuilt {
    pub(crate) bytes: String,
    /// True when `bytes` are the original bytes (no-op save).
    pub(crate) unchanged: bool,
    /// (0-based buffer row of a tokenless marker line, token to append).
    pub(crate) new_tokens: Vec<(usize, String)>,
    /// (0-based buffer row of a marker whose token was a duplicate, token
    /// that replaces it). The later copy of a duplicated token is a new cell
    /// and needs its own token; if the caller leaves the old one in place,
    /// that cell is simply new again on the next save.
    pub(crate) replaced_tokens: Vec<(usize, String)>,
    /// Internal plan for [`NotebookDoc::commit_save`]; opaque to callers.
    pub(crate) cell_tokens_after: Vec<String>,
}

type Fields = Vec<(String, Box<RawValue>)>;

/// One cell of a loaded notebook.
#[derive(Debug, Clone)]
struct Cell {
    /// The cell object as ordered raw JSON entries.
    fields: Fields,
    kind: CellKind,
    /// Body lines as shown in the buffer (markdown/raw with the `# ` prefix).
    lines: Vec<String>,
    /// `lines` joined with `\n`.
    text: String,
    id: Option<String>,
    execution_count: Option<i64>,
    output_kinds: Vec<String>,
    error: Option<String>,
}

/// A loaded notebook: the original bytes, raw JSON pieces, and the token table.
#[derive(Debug, Clone)]
pub(crate) struct NotebookDoc {
    original_bytes: String,
    disk_digest: u64,
    root: Fields,
    cells: Vec<Cell>,
    /// Identity token per cell index (disk order).
    tokens: Vec<String>,
    nbformat_minor: u64,
    layout_canonical: bool,
    lang: SyntaxLang,
    projected_text: String,
    clear_outputs: bool,
}

#[allow(dead_code)] // Wired by slices B/E; remove when App calls these.
impl NotebookDoc {
    /// The projected editor text (with tokens).
    pub(crate) fn projected_text(&self) -> &str {
        &self.projected_text
    }
    /// Language for highlighting/comment/folds/completion of this tab.
    pub(crate) fn lang(&self) -> SyntaxLang {
        self.lang
    }
    /// Digest of the raw bytes this doc was loaded from or last saved as.
    pub(crate) fn disk_digest(&self) -> u64 {
        self.disk_digest
    }
    /// True when re-assembling the unchanged doc reproduces the original bytes.
    pub(crate) fn layout_canonical(&self) -> bool {
        self.layout_canonical
    }
    pub(crate) fn cell_count(&self) -> usize {
        self.cells.len()
    }
    fn index_of(&self, token: &str) -> Option<usize> {
        self.tokens.iter().position(|t| t == token)
    }
    /// Summary for the cell that owns `token`, or None for an unknown token.
    pub(crate) fn cell_summary(&self, token: &str) -> Option<CellSummary> {
        let index = self.index_of(token)?;
        let cell = &self.cells[index];
        Some(CellSummary {
            index,
            cell_count: self.cells.len(),
            kind: cell.kind,
            execution_count: cell.execution_count,
            output_kinds: cell.output_kinds.clone(),
            error: cell.error.clone(),
        })
    }
    /// Projected visible text (without the marker line) of the cell that owns
    /// `token`, used to show "edited, outputs stale". The body lines exactly
    /// as projected (markdown/raw with `# `), joined with `\n`, without the
    /// blank separator line before the next marker.
    pub(crate) fn cell_visible_text(&self, token: &str) -> Option<&str> {
        self.index_of(token).map(|i| self.cells[i].text.as_str())
    }
    /// Pending "clear all outputs" applied by the next [`rebuild`].
    pub(crate) fn set_clear_outputs(&mut self, on: bool) {
        self.clear_outputs = on;
    }
    pub(crate) fn clear_outputs_pending(&self) -> bool {
        self.clear_outputs
    }
    /// Tokens in disk cell order (written to the autosave sidecar).
    pub(crate) fn tokens(&self) -> Vec<String> {
        self.tokens.clone()
    }
    /// Recovery: replace this doc's tokens with `tokens` (disk cell order).
    /// Returns false (and changes nothing) when the length differs, or when a
    /// token is malformed or repeated.
    pub(crate) fn adopt_tokens(&mut self, tokens: &[String]) -> bool {
        let mut seen = HashSet::new();
        let valid = tokens.len() == self.cells.len()
            && tokens
                .iter()
                .all(|t| is_token(t) && seen.insert(t.as_str()));
        if valid {
            self.tokens = tokens.to_vec();
            self.projected_text = render(&self.cells, &self.tokens);
        }
        valid
    }
    /// True when `disk` differs from `self` only in outputs, metadata or
    /// execution counts and the cell mapping is provable (plan: *External
    /// changes*).
    pub(crate) fn outputs_only_change(&self, disk: &NotebookDoc) -> bool {
        if self.cells.len() != disk.cells.len() || self.nbformat_minor != disk.nbformat_minor {
            return false;
        }
        let same_cells = self
            .cells
            .iter()
            .zip(&disk.cells)
            .all(|(a, b)| a.kind == b.kind && a.lines == b.lines);
        if !same_cells {
            return false;
        }
        if disk.nbformat_minor >= 5 {
            return self
                .cells
                .iter()
                .zip(&disk.cells)
                .all(|(a, b)| a.id == b.id);
        }
        // Without ids, two cells with the same type and text could have
        // swapped outputs; the mapping is only provable when none repeat.
        let mut seen = HashSet::new();
        disk.cells
            .iter()
            .all(|c| seen.insert((c.kind, c.lines.as_slice())))
    }
    /// Take `disk`'s raw cells, bytes and digest; keep this doc's tokens.
    /// Only valid when [`Self::outputs_only_change`] returned true.
    pub(crate) fn adopt_disk_outputs(&mut self, disk: NotebookDoc) {
        let tokens = std::mem::take(&mut self.tokens);
        let clear_outputs = self.clear_outputs;
        *self = disk;
        if tokens.len() == self.cells.len() {
            self.tokens = tokens;
            self.projected_text = render(&self.cells, &self.tokens);
        }
        self.clear_outputs = clear_outputs;
    }
    /// After a successful write of `rebuilt.bytes`: point every token at its
    /// written cell and record the new bytes and digest.
    pub(crate) fn commit_save(&mut self, rebuilt: &Rebuilt) {
        if rebuilt.unchanged {
            return;
        }
        match project(&rebuilt.bytes) {
            Ok(mut doc) if doc.cells.len() == rebuilt.cell_tokens_after.len() => {
                doc.tokens = rebuilt.cell_tokens_after.clone();
                doc.projected_text = render(&doc.cells, &doc.tokens);
                *self = doc;
            }
            // Unreachable by construction (rebuild only writes projectable
            // notebooks); still record what is on disk.
            _ => {
                self.disk_digest = digest(rebuilt.bytes.as_bytes());
                self.original_bytes = rebuilt.bytes.clone();
                self.clear_outputs = false;
            }
        }
    }
    /// (0-based cell index, ename) of every code cell with an error output.
    pub(crate) fn error_cells(&self) -> Vec<(usize, String)> {
        self.cells
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.error.clone().map(|e| (i, e)))
            .collect()
    }
}

/// An object parsed as ordered raw entries (no `preserve_order` needed).
struct OrderedObject(Fields);

impl<'de> Deserialize<'de> for OrderedObject {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor;
        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = OrderedObject;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<OrderedObject, A::Error> {
                let mut fields = Vec::new();
                while let Some(entry) = map.next_entry::<String, Box<RawValue>>()? {
                    fields.push(entry);
                }
                Ok(OrderedObject(fields))
            }
        }
        deserializer.deserialize_map(ObjectVisitor)
    }
}

fn invalid(m: impl Into<String>) -> NotebookOpenError {
    NotebookOpenError::Invalid(m.into())
}

/// Parse a raw JSON value that must be an object with unique keys.
fn parse_object(raw: &str, what: &str) -> Result<Fields, NotebookOpenError> {
    if !raw.starts_with('{') {
        return Err(invalid(format!("{what} is not an object")));
    }
    let fields = serde_json::from_str::<OrderedObject>(raw)
        .map_err(|e| invalid(format!("{what}: {e}")))?
        .0;
    let mut keys = HashSet::new();
    if let Some((k, _)) = fields.iter().find(|(k, _)| !keys.insert(k.as_str())) {
        return Err(invalid(format!("{what} has duplicate key \"{k}\"")));
    }
    Ok(fields)
}

fn field<'a>(fields: &'a Fields, key: &str) -> Option<&'a RawValue> {
    fields.iter().find(|(k, _)| k == key).map(|(_, v)| &**v)
}

fn value_of(raw: &RawValue) -> Value {
    // A RawValue is already valid JSON.
    serde_json::from_str(raw.get()).unwrap_or(Value::Null)
}

/// Parse notebook bytes into a doc, or say why it must open as raw JSON.
#[allow(dead_code)] // Wired by slice B (open_file_as); remove then.
pub(crate) fn project(text: &str) -> Result<NotebookDoc, NotebookOpenError> {
    let root_raw = serde_json::from_str::<Box<RawValue>>(text)
        .map_err(|e| NotebookOpenError::InvalidJson(e.to_string()))?;
    let root = parse_object(root_raw.get(), "root")?;
    let nbformat = field(&root, "nbformat").map(value_of);
    if nbformat.as_ref().and_then(Value::as_u64) != Some(4) {
        let shown = nbformat.map_or("missing".to_string(), |v| v.to_string());
        return Err(NotebookOpenError::Unsupported(format!("nbformat {shown}")));
    }
    let nbformat_minor = field(&root, "nbformat_minor")
        .map(value_of)
        .and_then(|v| v.as_u64())
        .ok_or_else(|| invalid("nbformat_minor is not an integer"))?;
    let metadata = field(&root, "metadata").map(value_of);
    let Some(metadata @ Value::Object(_)) = metadata else {
        return Err(invalid("metadata is not an object"));
    };
    let cells_raw = field(&root, "cells").ok_or_else(|| invalid("cells is missing"))?;
    let raw_cells = serde_json::from_str::<Vec<Box<RawValue>>>(cells_raw.get())
        .map_err(|_| invalid("cells is not an array"))?;
    let mut ids = HashSet::new();
    let mut cells = Vec::with_capacity(raw_cells.len());
    for (i, raw) in raw_cells.iter().enumerate() {
        let cell = parse_cell(raw, i, nbformat_minor)?;
        if let Some(id) = &cell.id
            && nbformat_minor >= 5
            && !ids.insert(id.clone())
        {
            return Err(invalid(format!("cell {}: duplicate id \"{id}\"", i + 1)));
        }
        cells.push(cell);
    }
    let mut used = HashSet::new();
    let tokens: Vec<String> = cells.iter().map(|_| fresh_token(&mut used)).collect();
    let layout_canonical = assemble(&root, cells.iter().map(|c| &c.fields)) == text;
    let projected_text = render(&cells, &tokens);
    Ok(NotebookDoc {
        original_bytes: text.to_string(),
        disk_digest: digest(text.as_bytes()),
        root,
        cells,
        tokens,
        nbformat_minor,
        layout_canonical,
        lang: lang_from_metadata(&metadata),
        projected_text,
        clear_outputs: false,
    })
}

fn parse_cell(raw: &RawValue, i: usize, nbformat_minor: u64) -> Result<Cell, NotebookOpenError> {
    let n = i + 1;
    let fields = parse_object(raw.get(), &format!("cell {n}"))?;
    let kind = match field(&fields, "cell_type").map(value_of) {
        Some(Value::String(s)) if s == "code" => CellKind::Code,
        Some(Value::String(s)) if s == "markdown" => CellKind::Markdown,
        Some(Value::String(s)) if s == "raw" => CellKind::Raw,
        _ => return Err(invalid(format!("cell {n}: unknown cell_type"))),
    };
    let source = match field(&fields, "source").map(value_of) {
        Some(Value::String(s)) => s,
        Some(Value::Array(parts)) => {
            let mut s = String::new();
            for p in parts {
                let Value::String(p) = p else {
                    return Err(invalid(format!("cell {n}: source is not a string list")));
                };
                s.push_str(&p);
            }
            s
        }
        _ => return Err(invalid(format!("cell {n}: source is not a string or list"))),
    };
    if !matches!(
        field(&fields, "metadata").map(value_of),
        Some(Value::Object(_))
    ) {
        return Err(invalid(format!("cell {n}: metadata is not an object")));
    }
    let id = match field(&fields, "id").map(value_of) {
        Some(Value::String(s)) if !s.is_empty() => Some(s),
        _ if nbformat_minor < 5 => None,
        _ => return Err(invalid(format!("cell {n}: missing or empty id"))),
    };
    let mut cell = Cell {
        fields: Vec::new(),
        kind,
        lines: project_lines(kind, &source),
        text: String::new(),
        id,
        execution_count: None,
        output_kinds: Vec::new(),
        error: None,
    };
    if cell.lines.iter().any(|l| is_marker_like(l)) {
        return Err(NotebookOpenError::MarkerCollision { cell_index: i });
    }
    cell.text = cell.lines.join("\n");
    if kind == CellKind::Code {
        let Some(Value::Array(outputs)) = field(&fields, "outputs").map(value_of) else {
            return Err(invalid(format!("cell {n}: outputs is not an array")));
        };
        cell.execution_count = match field(&fields, "execution_count").map(value_of) {
            Some(Value::Null) => None,
            Some(Value::Number(c)) if c.is_i64() || c.is_u64() => c.as_i64(),
            _ => return Err(invalid(format!("cell {n}: bad execution_count"))),
        };
        for out in &outputs {
            cell.output_kinds.push(output_kind(out));
            if cell.error.is_none()
                && out.get("output_type").and_then(Value::as_str) == Some("error")
            {
                let ename = out.get("ename").and_then(Value::as_str).unwrap_or("error");
                cell.error = Some(ename.to_string());
            }
        }
    }
    cell.fields = fields;
    Ok(cell)
}

/// `text` for a text/plain-only result, else the richest mime type.
fn output_kind(out: &Value) -> String {
    const RICH: [&str; 8] = [
        "image/png",
        "image/jpeg",
        "image/svg+xml",
        "image/gif",
        "text/html",
        "text/markdown",
        "text/latex",
        "application/json",
    ];
    let output_type = out
        .get("output_type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if output_type != "execute_result" && output_type != "display_data" {
        return output_type.to_string();
    }
    let Some(data) = out.get("data").and_then(Value::as_object) else {
        return "text".to_string();
    };
    if let Some(m) = RICH.iter().find(|m| data.contains_key(**m)) {
        return m.to_string();
    }
    data.keys()
        .find(|k| *k != "text/plain")
        .map_or("text".to_string(), |k| k.clone())
}

fn lang_from_metadata(metadata: &Value) -> SyntaxLang {
    ["/kernelspec/language", "/language_info/name"]
        .iter()
        .filter_map(|p| metadata.pointer(p).and_then(Value::as_str))
        .find_map(lang_for_name)
        .unwrap_or(SyntaxLang::Python)
}

fn lang_for_name(name: &str) -> Option<SyntaxLang> {
    match name.to_ascii_lowercase().as_str() {
        "python" | "python3" | "ipython" | "ipython3" => Some(SyntaxLang::Python),
        "rust" => Some(SyntaxLang::Rust),
        "javascript" | "typescript" | "js" | "ts" => Some(SyntaxLang::JsTs),
        "go" | "golang" => Some(SyntaxLang::Go),
        "php" => Some(SyntaxLang::Php),
        "bash" | "sh" | "shell" | "zsh" => Some(SyntaxLang::Shell),
        "html" => Some(SyntaxLang::HtmlXml),
        "css" => Some(SyntaxLang::Css),
        _ => None,
    }
}

/// Body lines of a cell as shown in the buffer.
fn project_lines(kind: CellKind, source: &str) -> Vec<String> {
    source
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .map(|l| match kind {
            CellKind::Code => l.to_string(),
            _ if l.is_empty() => "#".to_string(),
            _ => format!("# {l}"),
        })
        .collect()
}

/// Inverse of [`project_lines`] for one buffer line.
fn unproject_line(kind: CellKind, line: &str) -> &str {
    match kind {
        CellKind::Code => line,
        _ if line == "#" => "",
        _ => line.strip_prefix("# ").unwrap_or(line),
    }
}

fn marker_line(kind: CellKind, token: Option<&str>) -> String {
    let mut s = String::from("# %%");
    match kind {
        CellKind::Code => {}
        CellKind::Markdown => s.push_str(" [markdown]"),
        CellKind::Raw => s.push_str(" [raw]"),
    }
    if let Some(t) = token {
        s.push_str(" #");
        s.push_str(t);
    }
    s
}

/// The projected text: a marker per cell, its body lines, and one blank line
/// before every marker but the first. Always ends with `\n` (when non-empty).
fn render(cells: &[Cell], tokens: &[String]) -> String {
    let mut out = String::new();
    for (i, (cell, token)) in cells.iter().zip(tokens).enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&marker_line(cell.kind, Some(token)));
        out.push('\n');
        for l in &cell.lines {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

/// The Jupyter layout (indent 1, `": "`) around raw values.
fn assemble<'a>(root: &Fields, cells: impl Iterator<Item = &'a Fields>) -> String {
    let mut cells_json = String::new();
    for (i, fields) in cells.enumerate() {
        cells_json.push_str(if i == 0 { "[\n  " } else { ",\n  " });
        write_object(&mut cells_json, fields, 2, None);
    }
    cells_json.push_str(if cells_json.is_empty() { "[]" } else { "\n ]" });
    let mut out = String::new();
    write_object(&mut out, root, 0, Some(&cells_json));
    out.push('\n');
    out
}

/// Write `fields` as an object whose braces sit at `indent` spaces. When
/// `cells` is given it replaces the value of the root `cells` key.
fn write_object(out: &mut String, fields: &Fields, indent: usize, cells: Option<&str>) {
    if fields.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push('{');
    for (i, (key, value)) in fields.iter().enumerate() {
        out.push_str(if i == 0 { "\n" } else { ",\n" });
        out.push_str(&" ".repeat(indent + 1));
        out.push_str(&json_string(key));
        out.push_str(": ");
        match cells {
            Some(c) if key == "cells" => out.push_str(c),
            _ => out.push_str(value.get()),
        }
    }
    out.push('\n');
    out.push_str(&" ".repeat(indent));
    out.push('}');
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

fn raw(json: String) -> Box<RawValue> {
    RawValue::from_string(json).expect("writer builds valid JSON")
}

fn set_field(fields: &mut Fields, key: &str, value: Box<RawValue>) {
    match fields.iter_mut().find(|(k, _)| k == key) {
        Some((_, v)) => *v = value,
        None => fields.push((key.to_string(), value)),
    }
}

/// The `source` JSON for edited lines: LF endings, no trailing newline unless
/// the last line is blank (so the lines project back exactly), and the list
/// form (one entry per line, at cell-key depth) unless `as_string`.
fn source_json(kind: CellKind, lines: &[&str], as_string: bool) -> String {
    let mut src = lines
        .iter()
        .map(|l| unproject_line(kind, l))
        .collect::<Vec<_>>()
        .join("\n");
    if lines
        .last()
        .is_some_and(|l| unproject_line(kind, l).is_empty())
    {
        src.push('\n');
    }
    if as_string {
        return json_string(&src);
    }
    if src.is_empty() {
        return "[]".to_string();
    }
    let items: Vec<String> = src
        .split_inclusive('\n')
        .map(|p| format!("    {}", json_string(p)))
        .collect();
    format!("[\n{}\n   ]", items.join(",\n"))
}

/// One cell parsed from the edited buffer.
struct BufCell<'a> {
    kind: CellKind,
    token: Option<&'a str>,
    /// Row of the marker line; None for text above the first marker.
    row: Option<usize>,
    lines: Vec<&'a str>,
}

impl BufCell<'_> {
    fn close(mut self, followed_by_marker: bool, cells: &mut Vec<Self>) {
        if followed_by_marker && self.lines.last() == Some(&"") {
            self.lines.pop();
        }
        let blank_preamble = self.row.is_none() && self.lines.iter().all(|l| l.trim().is_empty());
        if !blank_preamble {
            cells.push(self);
        }
    }
}

fn parse_buffer(text: &str) -> Result<Vec<BufCell<'_>>, NotebookSaveError> {
    let mut cells = Vec::new();
    let mut cur = BufCell {
        kind: CellKind::Code,
        token: None,
        row: None,
        lines: Vec::new(),
    };
    for (row, line) in text.lines().enumerate() {
        if is_marker_like(line) {
            let marker = parse_marker(line).ok_or(NotebookSaveError::MalformedMarker { row })?;
            let token = marker.token.as_ref().map(|_| &line[line.len() - 6..]);
            let next = BufCell {
                kind: marker.kind,
                token,
                row: Some(row),
                lines: Vec::new(),
            };
            std::mem::replace(&mut cur, next).close(true, &mut cells);
            continue;
        }
        // A markdown/raw line `%%…` without the prefix would reopen as a
        // marker (`# %%…`), so the notebook could not be projected again.
        if cur.kind != CellKind::Code && unproject_line(cur.kind, line).starts_with("%%") {
            return Err(NotebookSaveError::MalformedMarker { row });
        }
        cur.lines.push(line);
    }
    cur.close(false, &mut cells);
    Ok(cells)
}

/// Build the bytes to write from the edited buffer text.
#[allow(dead_code)] // Wired by slice B (save_file); remove then.
pub(crate) fn rebuild(doc: &NotebookDoc, edited: &str) -> Result<Rebuilt, NotebookSaveError> {
    let buf = parse_buffer(edited)?;
    // Identity: the first occurrence of a known token with the same type.
    let mut used: HashSet<String> = doc.tokens.iter().cloned().collect();
    used.extend(buf.iter().filter_map(|c| c.token.map(str::to_string)));
    let mut seen = HashSet::new();
    let mut new_tokens = Vec::new();
    let mut replaced_tokens = Vec::new();
    let mut plan: Vec<(Option<usize>, String)> = Vec::with_capacity(buf.len());
    for c in &buf {
        let (kept, token) = match c.token {
            Some(t) if seen.insert(t) => {
                let kept = doc.index_of(t).filter(|&i| doc.cells[i].kind == c.kind);
                (kept, t.to_string())
            }
            Some(_) => {
                let t = fresh_token(&mut used);
                if let Some(row) = c.row {
                    replaced_tokens.push((row, t.clone()));
                }
                (None, t)
            }
            None => {
                let t = fresh_token(&mut used);
                if let Some(row) = c.row {
                    new_tokens.push((row, t.clone()));
                }
                (None, t)
            }
        };
        plan.push((kept, token));
    }
    let unchanged_text = |c: &BufCell, i: usize| {
        c.lines
            .iter()
            .copied()
            .eq(doc.cells[i].lines.iter().map(String::as_str))
    };
    let noop = !doc.clear_outputs
        && buf.len() == doc.cells.len()
        && plan
            .iter()
            .zip(&buf)
            .enumerate()
            .all(|(pos, ((kept, _), c))| *kept == Some(pos) && unchanged_text(c, pos));
    if noop {
        return Ok(Rebuilt {
            bytes: doc.original_bytes.clone(),
            unchanged: true,
            new_tokens,
            replaced_tokens,
            cell_tokens_after: doc.tokens.clone(),
        });
    }
    let mut ids: HashSet<String> = plan
        .iter()
        .filter_map(|(kept, _)| kept.and_then(|i| doc.cells[i].id.clone()))
        .collect();
    let mut out_cells: Vec<Fields> = Vec::with_capacity(buf.len());
    for ((kept, _), c) in plan.iter().zip(&buf) {
        let mut fields = match kept {
            Some(i) => {
                let cell = &doc.cells[*i];
                let mut fields = cell.fields.clone();
                if !unchanged_text(c, *i) {
                    let as_string =
                        field(&fields, "source").is_some_and(|s| s.get().starts_with('"'));
                    set_field(
                        &mut fields,
                        "source",
                        raw(source_json(c.kind, &c.lines, as_string)),
                    );
                }
                fields
            }
            None => new_cell_fields(doc.nbformat_minor, c, &mut ids),
        };
        if doc.clear_outputs && c.kind == CellKind::Code {
            set_field(&mut fields, "outputs", raw("[]".to_string()));
            set_field(&mut fields, "execution_count", raw("null".to_string()));
        }
        out_cells.push(fields);
    }
    Ok(Rebuilt {
        bytes: assemble(&doc.root, out_cells.iter()),
        unchanged: false,
        new_tokens,
        replaced_tokens,
        cell_tokens_after: plan.into_iter().map(|(_, t)| t).collect(),
    })
}

/// A new cell in nbformat key order; ids only for `nbformat_minor >= 5`.
fn new_cell_fields(nbformat_minor: u64, c: &BufCell, ids: &mut HashSet<String>) -> Fields {
    let cell_type = match c.kind {
        CellKind::Code => "code",
        CellKind::Markdown => "markdown",
        CellKind::Raw => "raw",
    };
    let mut fields = vec![("cell_type".to_string(), raw(json_string(cell_type)))];
    if c.kind == CellKind::Code {
        fields.push(("execution_count".to_string(), raw("null".to_string())));
    }
    if nbformat_minor >= 5 {
        let id = fresh_cell_id(ids);
        fields.push(("id".to_string(), raw(json_string(&id))));
    }
    fields.push(("metadata".to_string(), raw("{}".to_string())));
    if c.kind == CellKind::Code {
        fields.push(("outputs".to_string(), raw("[]".to_string())));
    }
    fields.push((
        "source".to_string(),
        raw(source_json(c.kind, &c.lines, false)),
    ));
    fields
}

/// Remove identity tokens from every marker line (stale recovery text).
#[allow(dead_code)] // Wired by slice B (recovery); remove then.
pub(crate) fn strip_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for piece in text.split_inclusive('\n') {
        let body = piece.trim_end_matches(['\n', '\r']);
        match parse_marker(body) {
            Some(Marker {
                kind,
                token: Some(_),
            }) => {
                out.push_str(&marker_line(kind, None));
                out.push_str(&piece[body.len()..]);
            }
            _ => out.push_str(piece),
        }
    }
    out
}

/// Parse a full marker line, or None when the line is not a valid marker.
/// Grammar: `^# %%( \[(markdown|raw)\])?( #[0-9a-z]{6})?$`.
pub(crate) fn parse_marker(line: &str) -> Option<Marker> {
    let rest = line.strip_prefix("# %%")?;
    let (kind, rest) = if let Some(r) = rest.strip_prefix(" [markdown]") {
        (CellKind::Markdown, r)
    } else if let Some(r) = rest.strip_prefix(" [raw]") {
        (CellKind::Raw, r)
    } else {
        (CellKind::Code, rest)
    };
    if rest.is_empty() {
        return Some(Marker { kind, token: None });
    }
    let token = rest.strip_prefix(" #").filter(|t| is_token(t))?;
    Some(Marker {
        kind,
        token: Some(token.to_string()),
    })
}

fn is_token(t: &str) -> bool {
    t.len() == 6
        && t.bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
}

/// True for any line that starts with `# %%` (valid marker or not).
pub(crate) fn is_marker_like(line: &str) -> bool {
    line.starts_with("# %%")
}

/// Change-detection digest of raw file bytes (not a security hash).
pub(crate) fn digest(bytes: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// Random bits from std only: a randomly keyed hash of the time and a counter.
fn random_u64() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = RandomState::new().build_hasher();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    nanos.hash(&mut h);
    COUNTER.fetch_add(1, Ordering::Relaxed).hash(&mut h);
    h.finish()
}

/// 6 random base-36 characters not yet in `used` (and added to it).
fn fresh_token(used: &mut HashSet<String>) -> String {
    loop {
        let mut n = random_u64();
        let t: String = (0..6)
            .map(|_| {
                let d = (n % 36) as u32;
                n /= 36;
                char::from_digit(d, 36).unwrap_or('0')
            })
            .collect();
        if used.insert(t.clone()) {
            return t;
        }
    }
}

/// 8 random hex characters not yet in `used` (and added to it).
fn fresh_cell_id(used: &mut HashSet<String>) -> String {
    loop {
        let id = format!("{:08x}", random_u64() as u32);
        if used.insert(id.clone()) {
            return id;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAB45: &str = include_str!("notebook/testdata/lab45_executed.ipynb");
    const V44: &str = include_str!("notebook/testdata/v44_no_ids.ipynb");
    const STRING_SOURCE: &str = include_str!("notebook/testdata/string_source.ipynb");
    const CRLF: &str = include_str!("notebook/testdata/crlf_source.ipynb");
    const NUMBERS: &str = include_str!("notebook/testdata/numbers.ipynb");
    const INDENT2: &str = include_str!("notebook/testdata/indent2.ipynb");
    const PERCENT_IN_CODE: &str = include_str!("notebook/testdata/percent_in_code.ipynb");
    const MISSING_ID45: &str = include_str!("notebook/testdata/missing_id45.ipynb");
    const RUST_KERNEL: &str = include_str!("notebook/testdata/rust_kernel.ipynb");

    const PROJECTABLE: [(&str, &str); 7] = [
        ("lab45", LAB45),
        ("v44", V44),
        ("string_source", STRING_SOURCE),
        ("crlf", CRLF),
        ("numbers", NUMBERS),
        ("indent2", INDENT2),
        ("rust_kernel", RUST_KERNEL),
    ];

    fn doc(text: &str) -> NotebookDoc {
        project(text).expect("notebook projects")
    }

    /// Projected text split into per-cell chunks (marker line, body and the
    /// blank separator that follows), so tests can drop, swap or copy cells.
    fn chunks(text: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for piece in text.split_inclusive('\n') {
            if is_marker_like(piece) || out.is_empty() {
                out.push(String::new());
            }
            out.last_mut().unwrap().push_str(piece);
        }
        out
    }

    fn cells_json(bytes: &str) -> Vec<Value> {
        let v: Value = serde_json::from_str(bytes).unwrap();
        v["cells"].as_array().unwrap().clone()
    }

    fn source_of(cell: &Value) -> String {
        match &cell["source"] {
            Value::String(s) => s.clone(),
            Value::Array(a) => a.iter().map(|p| p.as_str().unwrap()).collect(),
            _ => panic!("bad source"),
        }
    }

    /// The raw text of cell `i`'s object in `bytes` (indent-1 layout).
    fn raw_cell(bytes: &str, i: usize) -> String {
        let root = serde_json::from_str::<OrderedObject>(bytes).unwrap().0;
        let cells =
            serde_json::from_str::<Vec<Box<RawValue>>>(field(&root, "cells").unwrap().get())
                .unwrap();
        cells[i].get().to_string()
    }

    // ---- Test 1 / 2: byte identity and layout ----

    #[test]
    fn noop_save_is_byte_identical_for_every_fixture() {
        for (name, text) in PROJECTABLE {
            let d = doc(text);
            let r = rebuild(&d, d.projected_text()).unwrap();
            assert!(r.unchanged, "{name}");
            assert_eq!(r.bytes, text, "{name}");
            assert!(r.new_tokens.is_empty(), "{name}");
            assert_eq!(r.cell_tokens_after, d.tokens(), "{name}");
        }
    }

    #[test]
    fn layout_canonical_true_for_jupyter_writers_false_for_indent2() {
        for (name, text) in PROJECTABLE {
            assert_eq!(doc(text).layout_canonical(), name != "indent2", "{name}");
        }
    }

    #[test]
    fn full_reassembly_reproduces_canonical_fixtures() {
        // Clear-outputs disables the no-op shortcut; on a notebook without
        // outputs the assembled bytes must still equal the original.
        for text in [STRING_SOURCE, CRLF] {
            let mut d = doc(text);
            d.set_clear_outputs(true);
            let r = rebuild(&d, d.projected_text()).unwrap();
            assert!(!r.unchanged);
            assert_eq!(r.bytes, text);
        }
    }

    // ---- Projection format ----

    #[test]
    fn projection_format_markers_prefixes_and_separators() {
        let d = doc(LAB45);
        let t = d.tokens();
        let text = d.projected_text();
        assert!(text.starts_with(&format!(
            "# %% [markdown] #{}\n# # Executed\n#\n# Some *markdown* text.\n\n# %% #{}\nprint(\"hello\")\n",
            t[0], t[1]
        )));
        assert!(text.contains(&format!(
            "\n\n# %% [raw] #{}\n# raw text\n#\n# more raw\n",
            t[5]
        )));
        assert!(text.ends_with(&format!("\n\n# %% #{}\n", t[6])));
        assert_eq!(d.cell_count(), 7);
        assert_eq!(
            d.cell_visible_text(&t[0]),
            Some("# # Executed\n#\n# Some *markdown* text.")
        );
        assert_eq!(d.cell_visible_text("zzzzzz"), None);
    }

    #[test]
    fn tokens_are_unique_base36_and_parse_back() {
        let d = doc(LAB45);
        let tokens = d.tokens();
        let set: HashSet<_> = tokens.iter().collect();
        assert_eq!(set.len(), tokens.len());
        assert!(tokens.iter().all(|t| is_token(t)));
        let parsed: Vec<String> = d
            .projected_text()
            .lines()
            .filter_map(parse_marker)
            .map(|m| m.token.unwrap())
            .collect();
        assert_eq!(parsed, tokens);
    }

    #[test]
    fn parse_marker_grammar() {
        let m = |kind, token: Option<&str>| {
            Some(Marker {
                kind,
                token: token.map(str::to_string),
            })
        };
        assert_eq!(parse_marker("# %%"), m(CellKind::Code, None));
        assert_eq!(
            parse_marker("# %% #a1b2c3"),
            m(CellKind::Code, Some("a1b2c3"))
        );
        assert_eq!(parse_marker("# %% [markdown]"), m(CellKind::Markdown, None));
        assert_eq!(
            parse_marker("# %% [raw] #zzzzzz"),
            m(CellKind::Raw, Some("zzzzzz"))
        );
        for bad in [
            "# %% #",
            "# %% #abc",
            "# %% #ABCDEF",
            "# %% #abcdefg",
            "# %%x",
            "# %% ",
            "# %% [code]",
            "# %% [markdown]#abcdef",
            "#%%",
            "# %% [markdown] #abcdef ",
        ] {
            assert_eq!(parse_marker(bad), None, "{bad:?}");
        }
    }

    // ---- Test 3 / 6 / 7: lossless rule ----

    #[test]
    fn edit_one_cell_changes_only_its_source_bytes() {
        let d = doc(LAB45);
        let edited = d.projected_text().replacen("\n1 + 1\n", "\n1 + 2\n", 1);
        let r = rebuild(&d, &edited).unwrap();
        assert!(!r.unchanged);
        assert_eq!(r.bytes, LAB45.replacen("\"1 + 1\"", "\"1 + 2\"", 1));
    }

    #[test]
    fn edited_source_is_lf_list_form_without_trailing_newline() {
        let d = doc(LAB45);
        let edited = d
            .projected_text()
            .replacen("\n1 + 1\n", "\na = 1\nb = 2\n", 1);
        let r = rebuild(&d, &edited).unwrap();
        assert!(
            r.bytes
                .contains("   \"source\": [\n    \"a = 1\\n\",\n    \"b = 2\"\n   ]\n")
        );
        // The edited cell keeps its outputs and execution count (stale).
        let cell = &cells_json(&r.bytes)[3];
        assert_eq!(cell["execution_count"], 3);
        assert_eq!(cell["outputs"][0]["data"]["text/plain"][0], "2");
    }

    #[test]
    fn unchanged_cells_keep_crlf_trailing_newline_and_string_form() {
        // Edit the markdown cell; the code cell's `\r\n` source survives.
        let d = doc(CRLF);
        let edited = d.projected_text().replacen("# line two", "# line 2", 1);
        let r = rebuild(&d, &edited).unwrap();
        assert!(
            r.bytes
                .contains("    \"a = 1\\r\\n\",\n    \"b = 2\\r\\n\"\n")
        );
        assert_eq!(source_of(&cells_json(&r.bytes)[1]), "line one\nline 2");

        // Edit the markdown cell; the string-form code source with its
        // trailing newline survives, and the edited cell stays a string.
        let d = doc(STRING_SOURCE);
        let edited = d.projected_text().replacen("# body", "# body!", 1);
        let r = rebuild(&d, &edited).unwrap();
        assert!(r.bytes.contains("\"source\": \"a = 1\\nb = 2\\n\"\n"));
        assert!(r.bytes.contains("\"source\": \"Title\\n\\nbody!\"\n"));
    }

    #[test]
    fn numbers_survive_in_untouched_and_edited_cells() {
        let d = doc(NUMBERS);
        for (from, to) in [("\ny = 2\n", "\ny = 3\n"), ("\nx = 1\n", "\nx = 9\n")] {
            let r = rebuild(&d, &d.projected_text().replacen(from, to, 1)).unwrap();
            assert!(!r.unchanged);
            assert_eq!(
                r.bytes.matches("1e-05").count(),
                NUMBERS.matches("1e-05").count()
            );
            assert_eq!(r.bytes.matches("1180591620717411303424").count(), 2);
        }
    }

    #[test]
    fn ansi_and_quotes_escape_like_python() {
        // A new cell holding the error fixture's traceback text must encode
        // exactly as the Python writer did.
        let d = doc(LAB45);
        let tb = "\u{1b}[31mKeyError\u{1b}[39m: 'missing' \"q\" \\ \t";
        let edited = format!("{}\n# %%\n{tb}\n", d.projected_text());
        let r = rebuild(&d, &edited).unwrap();
        assert!(
            r.bytes
                .contains(r#""\u001b[31mKeyError\u001b[39m: 'missing' \"q\" \\ \t""#)
        );
        assert!(LAB45.contains(r#""\u001b[31mKeyError\u001b[39m: 'missing'""#));
    }

    // ---- Test 4 / 5: identity ----

    #[test]
    fn f1_delete_a_edit_b_keeps_b_outputs() {
        let d = doc(LAB45);
        let mut c = chunks(d.projected_text());
        c.remove(1); // A: print hello/world (stream output)
        let edited = c.concat().replacen("\n1 + 1\n", "\n1 + 3\n", 1);
        let r = rebuild(&d, &edited).unwrap();
        assert!(!r.bytes.contains("hello\\n"));
        let cells = cells_json(&r.bytes);
        assert_eq!(cells.len(), 6);
        assert_eq!(source_of(&cells[2]), "1 + 3");
        assert_eq!(cells[2]["outputs"][0]["data"]["text/plain"][0], "2");
        assert_eq!(cells[2]["execution_count"], 3);
        assert_eq!(cells[3]["outputs"][0]["ename"], "KeyError");
    }

    #[test]
    fn swap_and_insert_keep_outputs_with_their_cells() {
        let d = doc(LAB45);
        let mut c = chunks(d.projected_text());
        c.swap(1, 3); // print <-> 1 + 1
        c.insert(2, "# %%\nnew = 1\n\n".to_string());
        let r = rebuild(&d, &c.concat()).unwrap();
        let cells = cells_json(&r.bytes);
        assert_eq!(source_of(&cells[1]), "1 + 1");
        assert_eq!(cells[1]["outputs"][0]["data"]["text/plain"][0], "2");
        assert_eq!(source_of(&cells[2]), "new = 1");
        assert_eq!(cells[2]["outputs"], Value::Array(vec![]));
        assert_eq!(cells[2]["execution_count"], Value::Null);
        assert_eq!(source_of(&cells[4]), "print(\"hello\")\nprint(\"world\")");
        assert_eq!(cells[4]["outputs"][0]["output_type"], "stream");
        // Kept cells are byte-identical objects.
        assert_eq!(raw_cell(&r.bytes, 4), raw_cell(LAB45, 1));
        // The inserted marker is buffer row 13 (after markdown 5 + print 4 + 1+1 4).
        let row = c.concat().lines().position(|l| l == "# %%").unwrap();
        assert_eq!(r.new_tokens.len(), 1);
        assert_eq!(r.new_tokens[0].0, row);
    }

    #[test]
    fn token_rules_missing_foreign_duplicate_and_type_change_are_new_cells() {
        let d = doc(LAB45);
        let t = d.tokens();
        let base = d.projected_text().to_string();
        let new_cell_has_no_outputs = |edited: &str, idx: usize| {
            let r = rebuild(&d, edited).unwrap();
            let cells = cells_json(&r.bytes);
            assert_eq!(cells[idx]["outputs"], Value::Array(vec![]), "{edited}");
            assert_ne!(cells[idx]["id"], "08c1993e");
            r
        };
        // Missing token on the `1 + 1` cell.
        let missing = base.replacen(&format!("# %% #{}", t[3]), "# %%", 1);
        let r = new_cell_has_no_outputs(&missing, 3);
        let row = missing.lines().position(|l| l == "# %%").unwrap();
        assert_eq!(
            r.new_tokens.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
            vec![row]
        );
        // A foreign token (not in the table) keeps its text as the new token.
        let foreign_tok = if t.contains(&"zz9zz9".to_string()) {
            "zz8zz8"
        } else {
            "zz9zz9"
        };
        let foreign = base.replacen(
            &format!("# %% #{}", t[3]),
            &format!("# %% #{foreign_tok}"),
            1,
        );
        let r = new_cell_has_no_outputs(&foreign, 3);
        assert!(r.new_tokens.is_empty());
        assert_eq!(r.cell_tokens_after[3], foreign_tok);
        // Duplicate: the first occurrence keeps identity, the copy is new.
        let mut c = chunks(&base);
        let copy = c[3].clone();
        c.insert(4, copy);
        let dup = c.concat();
        let r = new_cell_has_no_outputs(&dup, 4);
        assert_eq!(
            cells_json(&r.bytes)[3]["outputs"][0]["data"]["text/plain"][0],
            "2"
        );
        assert_eq!(r.replaced_tokens.len(), 1);
        let dup_row = dup
            .lines()
            .enumerate()
            .filter(|(_, l)| l.ends_with(&t[3]))
            .nth(1)
            .unwrap()
            .0;
        assert_eq!(r.replaced_tokens[0].0, dup_row);
        assert_ne!(r.replaced_tokens[0].1, t[3]);
        // Type change code -> markdown: a new markdown cell, no outputs key.
        let typed = base.replacen(
            &format!("# %% #{}\n1 + 1", t[3]),
            &format!("# %% [markdown] #{}\n# 1 + 1", t[3]),
            1,
        );
        let r = rebuild(&d, &typed).unwrap();
        let cell = &cells_json(&r.bytes)[3];
        assert_eq!(cell["cell_type"], "markdown");
        assert!(cell.get("outputs").is_none());
        assert_eq!(source_of(cell), "1 + 1");
    }

    #[test]
    fn new_cell_ids_only_for_minor_5_and_unique() {
        let add_two = |text: &str| {
            let d = doc(text);
            let edited = format!(
                "{}\n# %%\nn1\n\n# %% [markdown]\n# n2\n",
                d.projected_text()
            );
            rebuild(&d, &edited).unwrap().bytes
        };
        let cells = cells_json(&add_two(LAB45));
        let ids: Vec<&str> = cells.iter().map(|c| c["id"].as_str().unwrap()).collect();
        assert_eq!(ids.len(), 9);
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), 9);
        for id in &ids[7..] {
            assert!(
                id.len() == 8 && id.bytes().all(|b| b.is_ascii_hexdigit()),
                "{id}"
            );
        }
        let cells = cells_json(&add_two(V44));
        assert!(cells.iter().all(|c| c.get("id").is_none()));
        // New code cells follow nbformat key order.
        let bytes = add_two(V44);
        assert!(bytes.contains(
            "  {\n   \"cell_type\": \"code\",\n   \"execution_count\": null,\n   \"metadata\": {},\n   \"outputs\": [],\n   \"source\": [\n    \"n1\"\n   ]\n  },"
        ));
    }

    #[test]
    fn text_above_first_marker_becomes_a_code_cell() {
        let d = doc(V44);
        let r = rebuild(&d, &format!("import os\n\n{}", d.projected_text())).unwrap();
        let cells = cells_json(&r.bytes);
        assert_eq!(cells.len(), 4);
        assert_eq!(source_of(&cells[0]), "import os");
        assert_eq!(r.cell_tokens_after.len(), 4);
        // Blank lines alone above the first marker are ignored.
        let r = rebuild(&d, &format!("\n\n{}", d.projected_text())).unwrap();
        assert_eq!(cells_json(&r.bytes).len(), 3);
    }

    // ---- Malformed markers ----

    #[test]
    fn malformed_marker_refuses_save_with_row() {
        let d = doc(V44);
        let mut lines: Vec<&str> = d.projected_text().lines().collect();
        lines.insert(2, "# %% #");
        let err = rebuild(&d, &lines.join("\n")).unwrap_err();
        assert_eq!(err, NotebookSaveError::MalformedMarker { row: 2 });
        assert_eq!(err.to_string(), "Malformed cell marker at line 3");
        // A markdown line typed as `%%…` would reopen as a marker.
        let edited = d.projected_text().replacen("# ## Notes", "%% Notes", 1);
        let row = edited.lines().position(|l| l == "%% Notes").unwrap();
        assert_eq!(
            rebuild(&d, &edited),
            Err(NotebookSaveError::MalformedMarker { row })
        );
    }

    // ---- Clear outputs ----

    #[test]
    fn clear_outputs_empties_code_cells_and_keeps_markdown_bytes() {
        let mut d = doc(LAB45);
        assert!(!d.clear_outputs_pending());
        d.set_clear_outputs(true);
        assert!(d.clear_outputs_pending());
        let r = rebuild(&d, d.projected_text()).unwrap();
        assert!(!r.unchanged);
        for (i, c) in cells_json(&r.bytes).iter().enumerate() {
            if c["cell_type"] == "code" {
                assert_eq!(c["outputs"], Value::Array(vec![]), "cell {i}");
                assert_eq!(c["execution_count"], Value::Null, "cell {i}");
            } else {
                assert_eq!(raw_cell(&r.bytes, i), raw_cell(LAB45, i));
            }
        }
        d.commit_save(&r);
        assert!(!d.clear_outputs_pending());
        assert!(d.error_cells().is_empty());
    }

    // ---- Save commit ----

    #[test]
    fn commit_save_keeps_tokens_and_records_new_ones() {
        let mut d = doc(LAB45);
        let before = d.tokens();
        let mut c = chunks(d.projected_text());
        c.insert(2, "# %%\nnew = 1\n\n".to_string());
        let edited = c.concat();
        let r = rebuild(&d, &edited).unwrap();
        d.commit_save(&r);
        let after = d.tokens();
        assert_eq!(after.len(), 8);
        assert_eq!(&after[..2], &before[..2]);
        assert_eq!(&after[3..], &before[2..]);
        assert_eq!(after[2], r.new_tokens[0].1);
        assert_eq!(d.disk_digest(), digest(r.bytes.as_bytes()));
        // The caller appends the token to the marker; that text is a no-op.
        let mut lines: Vec<String> = edited.lines().map(str::to_string).collect();
        lines[r.new_tokens[0].0].push_str(&format!(" #{}", r.new_tokens[0].1));
        let buffer = lines.join("\n") + "\n";
        assert_eq!(buffer, d.projected_text());
        let again = rebuild(&d, &buffer).unwrap();
        assert!(again.unchanged);
        assert_eq!(again.bytes, r.bytes);
    }

    // ---- Test 9: fallback ----

    #[test]
    fn fallback_reasons_return_err() {
        use NotebookOpenError::*;
        let nb = |cells: &str, minor: &str| {
            format!(
                "{{\"cells\": [{cells}], \"metadata\": {{}}, \"nbformat\": 4, \"nbformat_minor\": {minor}}}"
            )
        };
        let code = |extra: &str| {
            format!(
                "{{\"cell_type\": \"code\", \"metadata\": {{}}, \"source\": \"x\", \"outputs\": [], \"execution_count\": null{extra}}}"
            )
        };
        assert!(matches!(project("{"), Err(InvalidJson(_))));
        assert!(matches!(project("[1]"), Err(Invalid(_))));
        assert!(matches!(
            project("{\"cells\": [], \"metadata\": {}, \"nbformat\": 3, \"nbformat_minor\": 0}"),
            Err(Unsupported(_))
        ));
        assert!(matches!(
            project("{\"cells\": [], \"metadata\": {}}"),
            Err(Unsupported(_))
        ));
        assert!(matches!(project(&nb("", "\"4\"")), Err(Invalid(_))));
        assert!(matches!(project(&nb("", "-1")), Err(Invalid(_))));
        assert!(matches!(
            project("{\"cells\": [], \"metadata\": [], \"nbformat\": 4, \"nbformat_minor\": 4}"),
            Err(Invalid(_))
        ));
        assert!(matches!(
            project("{\"cells\": {}, \"metadata\": {}, \"nbformat\": 4, \"nbformat_minor\": 4}"),
            Err(Invalid(_))
        ));
        assert!(matches!(
            project(
                "{\"cells\": [], \"cells\": [], \"metadata\": {}, \"nbformat\": 4, \"nbformat_minor\": 4}"
            ),
            Err(Invalid(_))
        ));
        let bad_cells = [
            "1".to_string(),
            "{\"cell_type\": \"heading\", \"metadata\": {}, \"source\": \"\"}".to_string(),
            "{\"cell_type\": \"markdown\", \"metadata\": {}, \"source\": 1}".to_string(),
            "{\"cell_type\": \"markdown\", \"metadata\": {}, \"source\": [\"a\", 1]}".to_string(),
            "{\"cell_type\": \"markdown\", \"metadata\": [], \"source\": \"\"}".to_string(),
            "{\"cell_type\": \"code\", \"metadata\": {}, \"source\": \"\", \"execution_count\": null}".to_string(),
            "{\"cell_type\": \"code\", \"metadata\": {}, \"source\": \"\", \"outputs\": []}".to_string(),
            code(", \"execution_count\": \"1\"").replacen(", \"execution_count\": null", "", 1),
            code(", \"execution_count\": 1.5").replacen(", \"execution_count\": null", "", 1),
        ];
        for cell in &bad_cells {
            assert!(matches!(project(&nb(cell, "4")), Err(Invalid(_))), "{cell}");
        }
        // nbformat 4.5: ids required, non-empty and unique.
        assert!(matches!(project(MISSING_ID45), Err(Invalid(_))));
        assert!(matches!(
            project(&nb(&code(", \"id\": \"\""), "5")),
            Err(Invalid(_))
        ));
        let two = format!("{},{}", code(", \"id\": \"a\""), code(", \"id\": \"a\""));
        assert!(matches!(project(&nb(&two, "5")), Err(Invalid(_))));
        assert!(project(&nb(&two, "4")).is_ok());
        // Marker collisions: `# %%` in code, `%%` in markdown.
        assert_eq!(
            project(PERCENT_IN_CODE).unwrap_err(),
            MarkerCollision { cell_index: 0 }
        );
        let md = "{\"cell_type\": \"markdown\", \"metadata\": {}, \"source\": \"ok\\n%%time\"}";
        assert_eq!(
            project(&nb(&format!("{},{md}", code("")), "4")).unwrap_err(),
            MarkerCollision { cell_index: 1 }
        );
        assert_eq!(
            MarkerCollision { cell_index: 1 }.to_string(),
            "cell 2 contains a `# %%` line"
        );
    }

    // ---- Summaries and language ----

    #[test]
    fn cell_summary_and_error_cells() {
        let d = doc(LAB45);
        let t = d.tokens();
        let s = |i: usize| d.cell_summary(&t[i]).unwrap();
        assert_eq!(s(0).kind, CellKind::Markdown);
        assert!(s(0).output_kinds.is_empty());
        assert_eq!(
            (s(1).execution_count, s(1).output_kinds.clone()),
            (Some(1), vec!["stream".to_string()])
        );
        assert_eq!(s(2).output_kinds, vec!["image/png"]);
        assert_eq!(s(3).output_kinds, vec!["text"]);
        assert_eq!(s(4).output_kinds, vec!["error"]);
        assert_eq!(s(4).error.as_deref(), Some("KeyError"));
        assert_eq!(
            (s(6).execution_count, s(6).index, s(6).cell_count),
            (None, 6, 7)
        );
        assert_eq!(d.error_cells(), vec![(4, "KeyError".to_string())]);
        assert_eq!(d.cell_summary("zzzzzz"), None);
    }

    #[test]
    fn lang_from_kernel_metadata() {
        assert_eq!(doc(LAB45).lang(), SyntaxLang::Python);
        assert_eq!(doc(RUST_KERNEL).lang(), SyntaxLang::Rust);
        let other = "{\"cells\": [], \"metadata\": {\"kernelspec\": {\"language\": \"julia\"}}, \"nbformat\": 4, \"nbformat_minor\": 4}";
        assert_eq!(doc(other).lang(), SyntaxLang::Python);
        let info = "{\"cells\": [], \"metadata\": {\"language_info\": {\"name\": \"go\"}}, \"nbformat\": 4, \"nbformat_minor\": 4}";
        assert_eq!(doc(info).lang(), SyntaxLang::Go);
    }

    // ---- External changes ----

    #[test]
    fn outputs_only_change_accepts_output_and_metadata_edits() {
        let d = doc(LAB45);
        let disk = doc(&LAB45.replacen("\"hello\\n\"", "\"HELLO\\n\"", 1).replacen(
            "\"execution_count\": 3,\n   \"id\"",
            "\"execution_count\": 9,\n   \"id\"",
            1,
        ));
        assert!(d.outputs_only_change(&disk));
        let mut adopted = d.clone();
        adopted.adopt_disk_outputs(disk.clone());
        assert_eq!(adopted.tokens(), d.tokens());
        assert_eq!(adopted.projected_text(), d.projected_text());
        assert_eq!(adopted.disk_digest(), disk.disk_digest());
        assert_eq!(
            adopted
                .cell_summary(&d.tokens()[3])
                .unwrap()
                .execution_count,
            Some(9)
        );
        // A save after adopting keeps the new outputs with the local edit.
        let r = rebuild(
            &adopted,
            &d.projected_text().replacen("\n1 + 1\n", "\n1 + 4\n", 1),
        )
        .unwrap();
        assert!(r.bytes.contains("\"HELLO\\n\""));
    }

    #[test]
    fn outputs_only_change_rejects_structural_changes() {
        let d = doc(LAB45);
        let changed_source = LAB45.replacen("\"1 + 1\"", "\"1 + 5\"", 1);
        assert!(!d.outputs_only_change(&doc(&changed_source)));
        let changed_id = LAB45.replacen("08c1993e", "08c1993f", 1);
        assert!(!d.outputs_only_change(&doc(&changed_id)));
        let fewer = doc(&rebuild(&d, &chunks(d.projected_text())[1..].concat())
            .unwrap()
            .bytes);
        assert!(!d.outputs_only_change(&fewer));
        let retyped = LAB45.replacen("\"cell_type\": \"raw\"", "\"cell_type\": \"markdown\"", 1);
        assert!(!d.outputs_only_change(&doc(&retyped)));
    }

    #[test]
    fn outputs_only_change_rejects_repeated_cells_before_4_5() {
        let cell = |out: &str| {
            format!(
                "  {{\n   \"cell_type\": \"code\",\n   \"execution_count\": null,\n   \"metadata\": {{}},\n   \"outputs\": [{out}],\n   \"source\": \"x\"\n  }}"
            )
        };
        let out = |s: &str| {
            format!("{{\"name\": \"stdout\", \"output_type\": \"stream\", \"text\": \"{s}\"}}")
        };
        let nb = |a: &str, b: &str| {
            format!(
                "{{\n \"cells\": [\n{},\n{}\n ],\n \"metadata\": {{}},\n \"nbformat\": 4,\n \"nbformat_minor\": 4\n}}\n",
                cell(a),
                cell(b)
            )
        };
        let mine = doc(&nb(&out("one"), &out("two")));
        // Swapped outputs of two identical cells: the mapping is not provable.
        let swapped = doc(&nb(&out("two"), &out("one")));
        assert!(!mine.outputs_only_change(&swapped));
        // The same cells with distinct text are fine.
        let distinct = |x: &str| {
            nb(&out(x), &out("two")).replacen("\"source\": \"x\"", "\"source\": \"y\"", 1)
        };
        assert!(doc(&distinct("one")).outputs_only_change(&doc(&distinct("uno"))));
    }

    // ---- Recovery ----

    #[test]
    fn adopt_tokens_checks_length_and_format() {
        let mut d = doc(V44);
        let original = d.tokens();
        assert!(!d.adopt_tokens(&original[..2]));
        assert!(!d.adopt_tokens(&["aaaaaa".into(), "aaaaaa".into(), "bbbbbb".into()]));
        assert!(!d.adopt_tokens(&["aaaaaa".into(), "BAD".into(), "bbbbbb".into()]));
        assert_eq!(d.tokens(), original);
        let mine = vec![
            "aaaaaa".to_string(),
            "bbbbbb".to_string(),
            "cccccc".to_string(),
        ];
        assert!(d.adopt_tokens(&mine));
        assert_eq!(d.tokens(), mine);
        assert!(d.projected_text().starts_with("# %% #aaaaaa\n"));
        assert_eq!(d.cell_summary("bbbbbb").unwrap().index, 1);
    }

    #[test]
    fn strip_tokens_removes_only_marker_tokens() {
        let text = "# %% #abc123\r\nx = '# %% #abc123 '\n\n# %% [markdown] #zzzzzz\n# hi\n# %% #bad\n# %% [raw] #000000";
        assert_eq!(
            strip_tokens(text),
            "# %%\r\nx = '# %% #abc123 '\n\n# %% [markdown]\n# hi\n# %% #bad\n# %% [raw]"
        );
        // Stripped recovery text saves every cell as new: no outputs attach.
        let d = doc(LAB45);
        let r = rebuild(&d, &strip_tokens(d.projected_text())).unwrap();
        assert!(
            cells_json(&r.bytes)
                .iter()
                .all(|c| c.get("outputs").is_none_or(|o| o == &Value::Array(vec![])))
        );
        assert_eq!(r.new_tokens.len(), 7);
    }

    // ---- Property test over generated cell lists ----

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }
        fn pick(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    /// A notebook written like nbformat (indent 1, sorted keys).
    fn python_like(cells: &[(CellKind, String, bool)], minor: u64) -> String {
        let mut out = String::from("{\n \"cells\": [");
        for (i, (kind, src, as_string)) in cells.iter().enumerate() {
            out.push_str(if i == 0 { "\n  {\n" } else { ",\n  {\n" });
            let kind_name = match kind {
                CellKind::Code => "code",
                CellKind::Markdown => "markdown",
                CellKind::Raw => "raw",
            };
            out.push_str(&format!("   \"cell_type\": \"{kind_name}\",\n"));
            if *kind == CellKind::Code {
                out.push_str("   \"execution_count\": null,\n");
            }
            if minor >= 5 {
                out.push_str(&format!("   \"id\": \"c{i:07}\",\n"));
            }
            out.push_str("   \"metadata\": {},\n");
            if *kind == CellKind::Code {
                out.push_str("   \"outputs\": [],\n");
            }
            let source = if *as_string {
                json_string(src)
            } else if src.is_empty() {
                "[]".to_string()
            } else {
                let items: Vec<String> = src
                    .split_inclusive('\n')
                    .map(|p| format!("    {}", json_string(p)))
                    .collect();
                format!("[\n{}\n   ]", items.join(",\n"))
            };
            out.push_str(&format!("   \"source\": {source}\n  }}"));
        }
        out.push_str(if cells.is_empty() { "],\n" } else { "\n ],\n" });
        out.push_str(&format!(
            " \"metadata\": {{}},\n \"nbformat\": 4,\n \"nbformat_minor\": {minor}\n}}\n"
        ));
        out
    }

    #[test]
    fn projection_round_trip_property() {
        const POOL: [&str; 11] = [
            "",
            "#",
            "x = 1",
            "  indented",
            "# comment",
            "%",
            "a\tb",
            "ünï",
            "say \"q\"",
            "\\back",
            "#x",
        ];
        let kinds = [CellKind::Code, CellKind::Markdown, CellKind::Raw];
        let mut rng = Lcg(0x5eed);
        for case in 0..400 {
            let cells: Vec<(CellKind, String, bool)> = (0..rng.pick(5))
                .map(|_| {
                    let eol = if rng.pick(4) == 0 { "\r\n" } else { "\n" };
                    let lines: Vec<&str> = (0..rng.pick(4))
                        .map(|_| POOL[rng.pick(POOL.len())])
                        .collect();
                    let mut src = lines.join(eol);
                    if !lines.is_empty() && rng.pick(2) == 0 {
                        src.push_str(eol);
                    }
                    (kinds[rng.pick(3)], src, rng.pick(3) == 0)
                })
                .collect();
            let minor = if rng.pick(2) == 0 { 4 } else { 5 };
            let text = python_like(&cells, minor);
            let d = project(&text).unwrap_or_else(|e| panic!("case {case}: {e}\n{text}"));
            assert!(d.layout_canonical(), "case {case}\n{text}");
            // No-op save is byte-identical.
            let r = rebuild(&d, d.projected_text()).unwrap();
            assert!(r.unchanged && r.bytes == text, "case {case}");
            // Every cell new: the written notebook projects to the same cells.
            let stripped = strip_tokens(d.projected_text());
            let r = rebuild(&d, &stripped).unwrap();
            assert_eq!(r.unchanged, cells.is_empty(), "case {case}");
            let again =
                project(&r.bytes).unwrap_or_else(|e| panic!("case {case}: {e}\n{}", r.bytes));
            let shape = |d: &NotebookDoc| {
                d.cells
                    .iter()
                    .map(|c| (c.kind, c.lines.clone()))
                    .collect::<Vec<_>>()
            };
            assert_eq!(shape(&again), shape(&d), "case {case}\n{stripped}");
            // After commit plus token insertion the buffer is a no-op save.
            let mut committed = d.clone();
            committed.commit_save(&r);
            let mut lines: Vec<String> = stripped.lines().map(str::to_string).collect();
            for (row, tok) in &r.new_tokens {
                lines[*row].push_str(&format!(" #{tok}"));
            }
            let buffer: String = lines.iter().map(|l| format!("{l}\n")).collect();
            assert_eq!(buffer, committed.projected_text(), "case {case}");
            assert!(
                rebuild(&committed, &buffer).unwrap().unchanged,
                "case {case}"
            );
            // Editing one cell changes only that cell's projection.
            if !cells.is_empty() {
                let target = rng.pick(cells.len());
                let mut c = chunks(d.projected_text());
                let insert_at = c[target].find('\n').unwrap() + 1;
                let added = if d.cells[target].kind == CellKind::Code {
                    "edited"
                } else {
                    "# edited"
                };
                c[target].insert_str(insert_at, &format!("{added}\n"));
                let r = rebuild(&d, &c.concat()).unwrap();
                let after = project(&r.bytes).unwrap();
                for (i, (a, b)) in shape(&after).iter().zip(shape(&d)).enumerate() {
                    if i == target {
                        assert_eq!(a.1[0], added, "case {case}");
                        assert_eq!(&a.1[1..], &b.1[..], "case {case}");
                    } else {
                        assert_eq!(a, &b, "case {case}");
                        assert_eq!(raw_cell(&r.bytes, i), raw_cell(&text, i), "case {case}");
                    }
                }
            }
        }
    }
}
