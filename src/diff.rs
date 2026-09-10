//! Unified-diff parsing into side-by-side rows for the diff view, plus small
//! helpers for jumping between changed regions in the editor.

use std::path::Path;
use std::process::{Command, Stdio};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::tab::GitLineStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiffLineKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiffLine {
    pub(crate) kind: DiffLineKind,
    /// 1-based line number in the old (HEAD) side, if present.
    pub(crate) old_no: Option<usize>,
    /// 1-based line number in the new (working tree) side, if present.
    pub(crate) new_no: Option<usize>,
    pub(crate) text: String,
}

/// One rendered row of the side-by-side view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DiffRow {
    Header(String),
    Pair {
        left: Option<DiffLine>,
        right: Option<DiffLine>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiffHunk {
    /// Index into `FileDiff::rows` of this hunk's header row.
    pub(crate) start_row: usize,
    /// 1-based first line of the hunk in the new file.
    pub(crate) new_start: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FileDiff {
    pub(crate) rows: Vec<DiffRow>,
    pub(crate) hunks: Vec<DiffHunk>,
    pub(crate) added: usize,
    pub(crate) removed: usize,
}

impl FileDiff {
    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// Parse `git diff` output for a single file into side-by-side rows.
/// Within a hunk, runs of removed and added lines are paired row by row;
/// context lines appear on both sides.
pub(crate) fn parse_unified_diff(diff: &str) -> FileDiff {
    let mut out = FileDiff::default();
    let mut old_no = 0usize;
    let mut new_no = 0usize;
    let mut in_hunk = false;
    let mut removed: Vec<DiffLine> = Vec::new();
    let mut added: Vec<DiffLine> = Vec::new();

    fn flush(out: &mut FileDiff, removed: &mut Vec<DiffLine>, added: &mut Vec<DiffLine>) {
        let n = removed.len().max(added.len());
        let mut r = removed.drain(..);
        let mut a = added.drain(..);
        for _ in 0..n {
            out.rows.push(DiffRow::Pair {
                left: r.next(),
                right: a.next(),
            });
        }
    }

    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("@@") {
            flush(&mut out, &mut removed, &mut added);
            // @@ -a,b +c,d @@ optional context
            let mut old_start = 0usize;
            let mut new_start = 0usize;
            for tok in rest.split_whitespace().take(2) {
                if let Some(o) = tok.strip_prefix('-') {
                    old_start = o
                        .split(',')
                        .next()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                } else if let Some(nw) = tok.strip_prefix('+') {
                    new_start = nw
                        .split(',')
                        .next()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                }
            }
            old_no = old_start;
            new_no = new_start;
            in_hunk = true;
            out.hunks.push(DiffHunk {
                start_row: out.rows.len(),
                new_start,
            });
            out.rows.push(DiffRow::Header(line.to_string()));
            continue;
        }
        if !in_hunk || line.starts_with('\\') {
            continue;
        }
        if let Some(text) = line.strip_prefix('-') {
            removed.push(DiffLine {
                kind: DiffLineKind::Removed,
                old_no: Some(old_no),
                new_no: None,
                text: text.to_string(),
            });
            old_no += 1;
            out.removed += 1;
            continue;
        }
        if let Some(text) = line.strip_prefix('+') {
            added.push(DiffLine {
                kind: DiffLineKind::Added,
                old_no: None,
                new_no: Some(new_no),
                text: text.to_string(),
            });
            new_no += 1;
            out.added += 1;
            continue;
        }
        // Context line: leading space, or an empty line from a blank source line.
        flush(&mut out, &mut removed, &mut added);
        let text = line.strip_prefix(' ').unwrap_or(line).to_string();
        let ctx = DiffLine {
            kind: DiffLineKind::Context,
            old_no: Some(old_no),
            new_no: Some(new_no),
            text,
        };
        out.rows.push(DiffRow::Pair {
            left: Some(ctx.clone()),
            right: Some(ctx),
        });
        old_no += 1;
        new_no += 1;
    }
    flush(&mut out, &mut removed, &mut added);
    out
}

/// A diff where every line of `content` is new (untracked files).
pub(crate) fn all_added_diff(content: &str) -> FileDiff {
    let lines: Vec<&str> = content.lines().collect();
    let mut out = FileDiff {
        rows: Vec::with_capacity(lines.len() + 1),
        hunks: Vec::new(),
        added: lines.len(),
        removed: 0,
    };
    out.hunks.push(DiffHunk {
        start_row: 0,
        new_start: 1,
    });
    out.rows.push(DiffRow::Header(format!(
        "@@ -0,0 +1,{} @@ (untracked)",
        lines.len()
    )));
    for (i, text) in lines.iter().enumerate() {
        out.rows.push(DiffRow::Pair {
            left: None,
            right: Some(DiffLine {
                kind: DiffLineKind::Added,
                old_no: None,
                new_no: Some(i + 1),
                text: (*text).to_string(),
            }),
        });
    }
    out
}

/// Diff of `file_path` against HEAD. Untracked files come back as all-added.
/// Returns `None` when git is unavailable or the file is clean.
pub(crate) fn git_diff_for_file(root: &Path, file_path: &Path) -> Option<FileDiff> {
    let rel = file_path.strip_prefix(root).unwrap_or(file_path);
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "HEAD", "--"])
        .arg(rel)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if output.status.success() && !output.stdout.is_empty() {
        let diff = parse_unified_diff(&String::from_utf8_lossy(&output.stdout));
        return (!diff.is_empty()).then_some(diff);
    }
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain", "--"])
        .arg(rel)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&status.stdout);
    if s.trim_start().starts_with("??") {
        let content = std::fs::read_to_string(file_path).ok()?;
        return Some(all_added_diff(&content));
    }
    None
}

/// Row index of the next (or previous) start of a changed run relative to
/// `from`, using the editor's per-line git status.
pub(crate) fn next_change_row(
    status: &[GitLineStatus],
    from: usize,
    forward: bool,
) -> Option<usize> {
    let is_start = |i: usize| {
        status[i] != GitLineStatus::None && (i == 0 || status[i - 1] == GitLineStatus::None)
    };
    if forward {
        ((from + 1)..status.len()).find(|&i| is_start(i))
    } else {
        (0..from.min(status.len())).rev().find(|&i| is_start(i))
    }
}

/// Truncate `s` to at most `width` display columns, marking the cut with `…`.
pub(crate) fn fit_width(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if used + cw > width - 1 {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
diff --git a/f.rs b/f.rs
index 1..2 100644
--- a/f.rs
+++ b/f.rs
@@ -1,4 +1,4 @@ fn head() {
 fn a() {}
-fn b() {}
-fn c() {}
+fn b2() {}
 fn d() {}
+fn e() {}
@@ -10,2 +10,1 @@
 x
-y
\\ No newline at end of file
";

    #[test]
    fn parse_pairs_removed_and_added_runs() {
        let d = parse_unified_diff(SAMPLE);
        assert_eq!(d.hunks.len(), 2);
        assert_eq!(d.added, 2);
        assert_eq!(d.removed, 3);
        assert_eq!(d.hunks[0].start_row, 0);
        assert_eq!(d.hunks[0].new_start, 1);
        // header, ctx a, pair(b,b2), pair(c,-), ctx d, pair(-,e)
        assert!(matches!(&d.rows[0], DiffRow::Header(h) if h.starts_with("@@ -1,4 +1,4 @@")));
        match &d.rows[2] {
            DiffRow::Pair {
                left: Some(l),
                right: Some(r),
            } => {
                assert_eq!(
                    (l.kind, l.old_no, l.text.as_str()),
                    (DiffLineKind::Removed, Some(2), "fn b() {}")
                );
                assert_eq!(
                    (r.kind, r.new_no, r.text.as_str()),
                    (DiffLineKind::Added, Some(2), "fn b2() {}")
                );
            }
            other => panic!("unexpected row: {other:?}"),
        }
        match &d.rows[3] {
            DiffRow::Pair {
                left: Some(l),
                right: None,
            } => assert_eq!(l.old_no, Some(3)),
            other => panic!("unexpected row: {other:?}"),
        }
        match &d.rows[4] {
            DiffRow::Pair {
                left: Some(l),
                right: Some(r),
            } => {
                assert_eq!(l.kind, DiffLineKind::Context);
                assert_eq!((l.old_no, r.new_no), (Some(4), Some(3)));
            }
            other => panic!("unexpected row: {other:?}"),
        }
        match &d.rows[5] {
            DiffRow::Pair {
                left: None,
                right: Some(r),
            } => assert_eq!(r.new_no, Some(4)),
            other => panic!("unexpected row: {other:?}"),
        }
        // second hunk: header, ctx, removed; the "\ No newline" marker is skipped
        assert_eq!(d.hunks[1].start_row, 6);
        assert_eq!(d.rows.len(), 9);
    }

    #[test]
    fn all_added_diff_numbers_every_line() {
        let d = all_added_diff("a\nb\n");
        assert_eq!(d.added, 2);
        assert_eq!(d.hunks.len(), 1);
        assert_eq!(d.rows.len(), 3);
        match &d.rows[2] {
            DiffRow::Pair {
                left: None,
                right: Some(r),
            } => assert_eq!((r.new_no, r.text.as_str()), (Some(2), "b")),
            other => panic!("unexpected row: {other:?}"),
        }
    }

    #[test]
    fn next_change_row_finds_run_starts_both_ways() {
        use GitLineStatus::{Added as A, Modified as M, None as N};
        let s = [N, A, A, N, N, M, N];
        assert_eq!(next_change_row(&s, 0, true), Some(1));
        assert_eq!(next_change_row(&s, 1, true), Some(5));
        assert_eq!(next_change_row(&s, 5, true), None);
        assert_eq!(next_change_row(&s, 6, false), Some(5));
        assert_eq!(next_change_row(&s, 5, false), Some(1));
        assert_eq!(next_change_row(&s, 1, false), None);
        assert_eq!(next_change_row(&[], 0, true), None);
        assert_eq!(next_change_row(&s, 99, false), Some(5));
    }

    #[test]
    fn fit_width_truncates_with_ellipsis() {
        assert_eq!(fit_width("hello", 10), "hello");
        assert_eq!(fit_width("hello world", 6), "hello…");
        assert_eq!(fit_width("日本語", 4), "日…");
        assert_eq!(fit_width("abc", 0), "");
    }
}
