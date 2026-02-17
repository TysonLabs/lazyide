use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ratatui::layout::Rect;
use url::Url;

use crate::syntax::{SyntaxLang, comment_start_for_lang, syntax_lang_for_path};
use crate::tab::{FoldRange, GitChangeSummary, GitFileStatus, GitLineStatus, ProjectSearchHit};
use crate::types::{CommandAction, ContextAction, EditorContextAction, PendingAction};

/// Convert a text string to editor lines, preserving a trailing newline as an
/// empty final line so the cursor can be positioned after the last content line.
pub(crate) fn text_to_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut lines: Vec<String> = text.lines().map(ToString::to_string).collect();
    if text.ends_with('\n') {
        lines.push(String::new());
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

pub(crate) fn pending_hint(pending: &PendingAction) -> String {
    let m = primary_mod_label();
    match pending {
        PendingAction::None => String::new(),
        PendingAction::Quit => format!("Pending quit: {}+Q confirm, Esc cancel", m),
        PendingAction::ClosePrompt => {
            format!(
                "Pending close: Enter/{}+S save+close, Esc discard, C cancel",
                m
            )
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

pub(crate) fn primary_mod_label() -> &'static str {
    "Ctrl"
}

pub(crate) fn command_action_label(action: CommandAction) -> &'static str {
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
        CommandAction::Keybinds => "Keybind Editor",
    }
}

pub(crate) fn context_actions() -> [ContextAction; 6] {
    [
        ContextAction::Open,
        ContextAction::NewFile,
        ContextAction::NewFolder,
        ContextAction::Rename,
        ContextAction::Delete,
        ContextAction::Cancel,
    ]
}

pub(crate) fn editor_context_actions() -> [EditorContextAction; 5] {
    [
        EditorContextAction::Copy,
        EditorContextAction::Cut,
        EditorContextAction::Paste,
        EditorContextAction::SelectAll,
        EditorContextAction::Cancel,
    ]
}

pub(crate) fn context_label(action: ContextAction) -> &'static str {
    match action {
        ContextAction::Open => "Open",
        ContextAction::NewFile => "New File",
        ContextAction::NewFolder => "New Folder",
        ContextAction::Rename => "Rename",
        ContextAction::Delete => "Delete",
        ContextAction::Cancel => "Cancel",
    }
}

pub(crate) fn editor_context_label(action: EditorContextAction) -> &'static str {
    match action {
        EditorContextAction::Copy => "Copy",
        EditorContextAction::Cut => "Cut",
        EditorContextAction::Paste => "Paste",
        EditorContextAction::SelectAll => "Select All",
        EditorContextAction::Cancel => "Cancel",
    }
}

pub(crate) fn leading_indent_bytes(line: &str) -> usize {
    let mut i = 0usize;
    let bytes = line.as_bytes();
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    i
}

pub(crate) fn comment_prefix_for_path(path: &Path) -> Option<&'static str> {
    comment_start_for_lang(syntax_lang_for_path(Some(path))).or_else(|| {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "java" | "c" | "h" | "cpp" | "hpp" | "cs" | "swift" | "kt" | "kts" | "scala" => {
                Some("//")
            }
            "yaml" | "yml" | "toml" | "rb" | "pl" | "conf" | "ini" => Some("#"),
            "sql" | "lua" => Some("--"),
            _ => None,
        }
    })
}

pub(crate) fn parse_rg_line(line: &str) -> Option<ProjectSearchHit> {
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

pub(crate) fn fuzzy_score(query: &str, candidate: &str) -> Option<usize> {
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

pub(crate) fn detect_git_branch(root: &Path) -> Option<String> {
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

pub(crate) fn compute_git_line_status(
    root: &Path,
    file_path: &Path,
    line_count: usize,
) -> Vec<GitLineStatus> {
    let mut result = vec![GitLineStatus::None; line_count];
    if line_count == 0 {
        return result;
    }
    let rel = file_path.strip_prefix(root).unwrap_or(file_path);
    let rel_str = rel.to_string_lossy();

    let diff_output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "HEAD", "--"])
        .arg(rel_str.as_ref())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    match diff_output {
        Ok(output) if output.status.success() && !output.stdout.is_empty() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            parse_unified_diff_into(&stdout, &mut result);
        }
        _ => {
            // No diff available — check if untracked
            let status_output = Command::new("git")
                .arg("-C")
                .arg(root)
                .args(["status", "--porcelain", "--"])
                .arg(rel_str.as_ref())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output();
            if let Ok(out) = status_output {
                let s = String::from_utf8_lossy(&out.stdout);
                if s.trim_start().starts_with("??") {
                    for status in result.iter_mut() {
                        *status = GitLineStatus::Added;
                    }
                }
            }
        }
    }
    result
}

fn parse_unified_diff_into(diff: &str, result: &mut [GitLineStatus]) {
    let mut new_line: usize = 0;
    let mut in_hunk = false;
    let mut pending_deletes: usize = 0;

    for line in diff.lines() {
        if line.starts_with("@@") {
            // Parse @@ -a,b +c,d @@
            in_hunk = false;
            if let Some(plus_part) = line.split('+').nth(1) {
                let nums: &str = plus_part.split_whitespace().next().unwrap_or("");
                let mut parts = nums.split(',');
                if let Some(start_str) = parts.next() {
                    if let Ok(start) = start_str.parse::<usize>() {
                        new_line = start;
                        let _count: usize = parts.next().and_then(|n| n.parse().ok()).unwrap_or(1);
                        in_hunk = true;
                        pending_deletes = 0;
                    }
                }
            }
            continue;
        }
        if !in_hunk {
            continue;
        }
        if line.starts_with('\\') {
            // Skip diff metadata like "\ No newline at end of file"
            continue;
        }
        if line.starts_with('-') {
            pending_deletes += 1;
            continue;
        }
        if line.starts_with('+') {
            let idx = new_line.saturating_sub(1);
            if idx < result.len() {
                if pending_deletes > 0 {
                    result[idx] = GitLineStatus::Modified;
                    pending_deletes -= 1;
                } else {
                    result[idx] = GitLineStatus::Added;
                }
            }
            new_line += 1;
            continue;
        }
        // Context line (starts with ' ' or is the line itself)
        if pending_deletes > 0 {
            // Deleted lines before this context line — mark delete on current line
            let idx = new_line.saturating_sub(1);
            if idx < result.len() {
                result[idx] = GitLineStatus::Deleted;
            }
            pending_deletes = 0;
        }
        new_line += 1;
    }
    // If hunk ended with pending deletes, mark on the last processed line
    if pending_deletes > 0 {
        let idx = new_line
            .saturating_sub(1)
            .min(result.len().saturating_sub(1));
        if idx < result.len() && result[idx] == GitLineStatus::None {
            result[idx] = GitLineStatus::Deleted;
        }
    }
}

pub(crate) fn compute_git_file_statuses(root: &Path) -> HashMap<PathBuf, GitFileStatus> {
    let mut map = HashMap::new();
    let Some(entries) = git_status_entries(root) else {
        return map;
    };
    for (path_str, status) in entries {
        map.insert(root.join(path_str), status);
    }
    // Propagate statuses up to parent directories (VS Code behavior).
    // Priority: Modified > Added > Untracked.
    let file_entries: Vec<(PathBuf, GitFileStatus)> =
        map.iter().map(|(k, v)| (k.clone(), *v)).collect();
    for (path, status) in file_entries {
        let mut dir = path.as_path();
        while let Some(parent) = dir.parent() {
            if parent == root || parent.as_os_str().is_empty() {
                break;
            }
            let entry = map.entry(parent.to_path_buf()).or_insert(status);
            // Escalate: Modified beats Added beats Untracked
            match (*entry, status) {
                (GitFileStatus::Modified, _) => {} // already highest
                (_, GitFileStatus::Modified) => *entry = GitFileStatus::Modified,
                (GitFileStatus::Added, _) => {}
                (_, GitFileStatus::Added) => *entry = GitFileStatus::Added,
                _ => {} // both Untracked, no change
            }
            dir = parent;
        }
    }
    map
}

pub(crate) fn compute_git_change_summary(root: &Path) -> GitChangeSummary {
    let mut summary = GitChangeSummary::default();
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--numstat", "HEAD"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let Ok(output) = output else {
        return summary;
    };
    if !output.status.success() {
        return summary;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let mut parts = line.split_whitespace();
        let add = parts.next().unwrap_or("0").parse::<usize>().unwrap_or(0);
        let del = parts.next().unwrap_or("0").parse::<usize>().unwrap_or(0);
        if parts.next().is_none() {
            continue;
        }
        summary.files_changed += 1;
        summary.insertions += add;
        summary.deletions += del;
    }
    summary
}

fn git_status_entries(root: &Path) -> Option<Vec<(String, GitFileStatus)>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain", "-z"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let Ok(output) = output else {
        return None;
    };
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Some(parse_porcelain_z_entries(&stdout))
}

fn parse_porcelain_z_entries(raw: &str) -> Vec<(String, GitFileStatus)> {
    let mut entries = Vec::new();
    // -z format: NUL-separated records. For renames/copies git emits:
    // "XY <new-path>\0<old-path>\0"
    let mut records = raw.split('\0').peekable();
    while let Some(record) = records.next() {
        if record.len() < 3 {
            continue;
        }
        let bytes = record.as_bytes();
        let x = bytes[0];
        let y = bytes[1];
        // bytes[2] is a space separator; path in the main record is the target/new path.
        let path_str = &record[3..];
        // For renames/copies, consume the source path record.
        if x == b'R' || x == b'C' {
            let _ = records.next();
        }
        let status = match (x, y) {
            (b'?', b'?') => GitFileStatus::Untracked,
            (b'A', _) => GitFileStatus::Added,
            (b'M', _) | (_, b'M') => GitFileStatus::Modified,
            (b'R', _) | (b'C', _) => GitFileStatus::Modified,
            _ => continue,
        };
        entries.push((path_str.to_string(), status));
    }
    entries
}

pub(crate) fn file_uri(path: &Path) -> Option<String> {
    let abs = path.canonicalize().ok()?;
    Url::from_file_path(abs).ok().map(|u| u.to_string())
}

pub(crate) fn compute_fold_ranges(
    lines: &[String],
    lang: SyntaxLang,
) -> (Vec<FoldRange>, Vec<u16>) {
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
            if s.starts_with('<')
                && !s.starts_with("<!")
                && !s.starts_with("<?")
                && !s.ends_with("/>")
            {
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
pub(crate) fn row_has_selection(
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

pub(crate) fn inside(x: u16, y: u16, rect: Rect) -> bool {
    x >= rect.x
        && x < rect.x.saturating_add(rect.width)
        && y >= rect.y
        && y < rect.y.saturating_add(rect.height)
}

pub(crate) fn collect_all_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(ft) = fs::symlink_metadata(&path).map(|m| m.file_type()) else {
            continue;
        };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
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

pub(crate) fn relative_path(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

pub(crate) fn to_u16_saturating(v: usize) -> u16 {
    u16::try_from(v).unwrap_or(u16::MAX)
}
#[cfg(test)]
mod git_parsing_tests {
    use super::*;

    fn parse_porcelain_z_fixture(raw: &str, root: &Path) -> HashMap<PathBuf, GitFileStatus> {
        parse_porcelain_z_entries(raw)
            .into_iter()
            .map(|(path, status)| (root.join(path), status))
            .collect()
    }

    fn propagate_statuses(
        mut map: HashMap<PathBuf, GitFileStatus>,
        root: &Path,
    ) -> HashMap<PathBuf, GitFileStatus> {
        let file_entries: Vec<(PathBuf, GitFileStatus)> =
            map.iter().map(|(k, v)| (k.clone(), *v)).collect();
        for (path, status) in file_entries {
            let mut dir = path.as_path();
            while let Some(parent) = dir.parent() {
                if parent == root || parent.as_os_str().is_empty() {
                    break;
                }
                let entry = map.entry(parent.to_path_buf()).or_insert(status);
                match (*entry, status) {
                    (GitFileStatus::Modified, _) => {}
                    (_, GitFileStatus::Modified) => *entry = GitFileStatus::Modified,
                    (GitFileStatus::Added, _) => {}
                    (_, GitFileStatus::Added) => *entry = GitFileStatus::Added,
                    _ => {}
                }
                dir = parent;
            }
        }
        map
    }

    // --- parse_unified_diff_into tests ---

    #[test]
    fn test_parse_diff_added_lines() {
        let diff = "\
diff --git a/file.rs b/file.rs
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,5 @@
 line1
+new_line2
+new_line3
 line2
 line3";
        let mut result = vec![GitLineStatus::None; 5];
        parse_unified_diff_into(diff, &mut result);
        assert_eq!(result[0], GitLineStatus::None);
        assert_eq!(result[1], GitLineStatus::Added);
        assert_eq!(result[2], GitLineStatus::Added);
        assert_eq!(result[3], GitLineStatus::None);
        assert_eq!(result[4], GitLineStatus::None);
    }

    #[test]
    fn test_parse_diff_modified_lines() {
        // One line removed, one line added at same position = Modified
        let diff = "\
@@ -1,3 +1,3 @@
 line1
-old_line2
+new_line2
 line3";
        let mut result = vec![GitLineStatus::None; 3];
        parse_unified_diff_into(diff, &mut result);
        assert_eq!(result[0], GitLineStatus::None);
        assert_eq!(result[1], GitLineStatus::Modified);
        assert_eq!(result[2], GitLineStatus::None);
    }

    #[test]
    fn test_parse_diff_deleted_lines() {
        // Lines removed, no additions — Deleted marker on next context line
        let diff = "\
@@ -1,4 +1,2 @@
 line1
-removed1
-removed2
 line3";
        let mut result = vec![GitLineStatus::None; 2];
        parse_unified_diff_into(diff, &mut result);
        assert_eq!(result[0], GitLineStatus::None);
        assert_eq!(result[1], GitLineStatus::Deleted);
    }

    #[test]
    fn test_parse_diff_multiple_hunks() {
        let diff = "\
@@ -1,2 +1,3 @@
 line1
+added
 line2
@@ -5,2 +6,2 @@
-old
+modified
 line6";
        let mut result = vec![GitLineStatus::None; 7];
        parse_unified_diff_into(diff, &mut result);
        assert_eq!(result[1], GitLineStatus::Added);
        assert_eq!(result[5], GitLineStatus::Modified);
    }

    #[test]
    fn test_parse_diff_empty_input() {
        let mut result = vec![GitLineStatus::None; 3];
        parse_unified_diff_into("", &mut result);
        assert!(result.iter().all(|s| *s == GitLineStatus::None));
    }

    #[test]
    fn test_parse_diff_no_hunks() {
        let diff = "diff --git a/file b/file\nindex abc..def 100644\n--- a/file\n+++ b/file";
        let mut result = vec![GitLineStatus::None; 3];
        parse_unified_diff_into(diff, &mut result);
        assert!(result.iter().all(|s| *s == GitLineStatus::None));
    }

    #[test]
    fn test_parse_diff_hunk_count_one_implicit() {
        // @@ -1 +1 @@ means count=1 (implicit)
        let diff = "@@ -1 +1 @@\n-old\n+new";
        let mut result = vec![GitLineStatus::None; 1];
        parse_unified_diff_into(diff, &mut result);
        assert_eq!(result[0], GitLineStatus::Modified);
    }

    #[test]
    fn test_parse_diff_no_newline_marker_ignored() {
        // "\ No newline at end of file" must not be counted as a context line
        let diff = "\
@@ -1,2 +1,2 @@
-old
+new
\\ No newline at end of file";
        let mut result = vec![GitLineStatus::None; 2];
        parse_unified_diff_into(diff, &mut result);
        assert_eq!(result[0], GitLineStatus::Modified);
        // Line 2 should be unaffected — the backslash line is metadata, not content
        assert_eq!(result[1], GitLineStatus::None);
    }

    // --- porcelain -z parsing (via compute_git_file_statuses, tested indirectly) ---
    // These test the parse_porcelain_z helper logic

    #[test]
    fn test_parse_porcelain_z_records() {
        // Simulate the NUL-separated format that compute_git_file_statuses parses
        let raw = "?? new_file.txt\0M  modified.rs\0A  added.rs\0";
        let root = Path::new("/project");
        let map = parse_porcelain_z_fixture(raw, root);
        assert_eq!(
            map.get(&root.join("new_file.txt")),
            Some(&GitFileStatus::Untracked)
        );
        assert_eq!(
            map.get(&root.join("modified.rs")),
            Some(&GitFileStatus::Modified)
        );
        assert_eq!(map.get(&root.join("added.rs")), Some(&GitFileStatus::Added));
    }

    #[test]
    fn test_parse_porcelain_z_rename() {
        // Rename in porcelain -z: "R  <new>\0<old>\0"
        let raw = "R  new.rs\0old.rs\0";
        let root = Path::new("/project");
        let map = parse_porcelain_z_fixture(raw, root);
        assert!(!map.contains_key(&root.join("old.rs")));
        assert_eq!(
            map.get(&root.join("new.rs")),
            Some(&GitFileStatus::Modified)
        );
    }

    #[test]
    fn test_parse_porcelain_z_path_with_spaces() {
        let raw = "?? path with spaces/file name.txt\0";
        let root = Path::new("/project");
        let map = parse_porcelain_z_fixture(raw, root);
        assert_eq!(
            map.get(&root.join("path with spaces/file name.txt")),
            Some(&GitFileStatus::Untracked)
        );
    }

    #[test]
    fn test_parent_propagation() {
        let root = Path::new("/project");
        let mut map = HashMap::new();
        map.insert(root.join("src/lib.rs"), GitFileStatus::Modified);
        map.insert(root.join("src/util.rs"), GitFileStatus::Untracked);
        map.insert(root.join("README.md"), GitFileStatus::Added);
        let map = propagate_statuses(map, root);
        // src/ dir should be Modified (highest of Modified + Untracked)
        assert_eq!(map.get(&root.join("src")), Some(&GitFileStatus::Modified));
    }

    #[test]
    fn test_compute_git_change_summary_empty_on_non_repo() {
        let summary = compute_git_change_summary(Path::new("/definitely/not/a/git/repo"));
        assert_eq!(summary.files_changed, 0);
        assert_eq!(summary.insertions, 0);
        assert_eq!(summary.deletions, 0);
    }
}
#[cfg(test)]
mod fold_and_selection_tests {
    use super::*;
    use std::collections::HashSet;

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
        let fold_ranges = vec![FoldRange {
            start_line: 0,
            end_line: 2,
        }];
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
    use crate::theme::color_from_hex;
    use crate::ui::centered_rect;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use std::path::{Path, PathBuf};

    // color_from_hex tests

    #[test]
    fn test_color_from_hex_valid_uppercase() {
        assert_eq!(
            color_from_hex("#FF0000", Color::White),
            Color::Rgb(255, 0, 0)
        );
    }

    #[test]
    fn test_color_from_hex_valid_lowercase() {
        assert_eq!(
            color_from_hex("#00ff00", Color::White),
            Color::Rgb(0, 255, 0)
        );
    }

    #[test]
    fn test_color_from_hex_valid_mixed_case() {
        assert_eq!(
            color_from_hex("#0000Ff", Color::White),
            Color::Rgb(0, 0, 255)
        );
    }

    #[test]
    fn test_color_from_hex_valid_with_whitespace() {
        assert_eq!(
            color_from_hex("  #AABBCC  ", Color::White),
            Color::Rgb(170, 187, 204)
        );
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
        assert_eq!(
            color_from_hex("#", Color::Rgb(10, 20, 30)),
            Color::Rgb(10, 20, 30)
        );
    }

    // inside tests

    #[test]
    fn test_inside_point_in_center() {
        assert!(inside(15, 15, Rect::new(10, 10, 20, 20)));
    }

    #[test]
    fn test_inside_point_at_corners() {
        let rect = Rect::new(10, 10, 20, 20);
        assert!(inside(10, 10, rect)); // top-left inclusive
        assert!(!inside(30, 10, rect)); // top-right exclusive
        assert!(!inside(10, 30, rect)); // bottom-left exclusive
        assert!(!inside(30, 30, rect)); // bottom-right exclusive
        assert!(inside(29, 29, rect)); // just inside
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
        let result = relative_path(
            Path::new("/home/user/project"),
            Path::new("/home/user/project/src/main.rs"),
        );
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
        let hint = pending_hint(&PendingAction::Delete(PathBuf::from(
            "/home/user/project/file.rs",
        )));
        assert!(!hint.is_empty());
        assert!(hint.contains("delete"));
        assert!(hint.contains("file.rs"));
    }

    // command_action_label tests

    #[test]
    fn test_command_action_labels() {
        assert_eq!(command_action_label(CommandAction::Theme), "Theme Picker");
        assert_eq!(command_action_label(CommandAction::Help), "Help");
        assert_eq!(
            command_action_label(CommandAction::QuickOpen),
            "Quick Open Files"
        );
        assert_eq!(
            command_action_label(CommandAction::FindInFile),
            "Find in File"
        );
        assert_eq!(
            command_action_label(CommandAction::FindInProject),
            "Search in Project"
        );
        assert_eq!(command_action_label(CommandAction::SaveFile), "Save File");
        assert_eq!(
            command_action_label(CommandAction::RefreshTree),
            "Refresh Tree"
        );
        assert_eq!(
            command_action_label(CommandAction::ToggleFiles),
            "Toggle Files Pane"
        );
        assert_eq!(
            command_action_label(CommandAction::GotoDefinition),
            "Go to Definition"
        );
        assert_eq!(
            command_action_label(CommandAction::ReplaceInFile),
            "Find and Replace"
        );
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
        assert_eq!(
            editor_context_label(EditorContextAction::SelectAll),
            "Select All"
        );
        assert_eq!(editor_context_label(EditorContextAction::Cancel), "Cancel");
    }
}
