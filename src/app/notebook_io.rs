//! Notebook tab I/O: save, external changes, autosave tokens and reopen.
//! Every function here acts only on a tab with `notebook.is_some()`; raw
//! tabs (incl. a `.ipynb` that fell back to raw JSON) never reach it.

use super::App;
use std::fs;
use std::io;
use std::path::Path;

use ratatui_textarea::CursorMove;

use crate::notebook::{self, NotebookDoc, Rebuilt};
use crate::persistence::{NotebookTokens, nbtokens_path_for};
use crate::types::{PendingAction, SaveOutcome};
use crate::util::{compute_git_line_status, pending_hint, relative_path, text_to_lines};

/// True for a path that opens as a notebook (when it projects).
pub(crate) fn is_ipynb_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("ipynb"))
}

/// Read a file the way `open_file_as` does, so digests match.
fn read_lossy(path: &Path) -> io::Result<String> {
    Ok(String::from_utf8_lossy(&fs::read(path)?).to_string())
}

/// True when the buffer has non-blank text above its first marker line;
/// `rebuild` saves that text as a code cell without a marker.
fn has_preamble(text: &str) -> bool {
    text.lines()
        .take_while(|l| !notebook::is_marker_like(l))
        .any(|l| !l.trim().is_empty())
}

/// Write the autosave sidecar for a notebook tab: its tokens and the digest
/// of the notebook bytes they belong to.
pub(crate) fn write_nbtokens_sidecar(path: &Path, doc: &NotebookDoc) -> io::Result<()> {
    let sidecar = NotebookTokens {
        disk_digest: doc.disk_digest(),
        tokens: doc.tokens(),
    };
    let json = serde_json::to_string(&sidecar).map_err(io::Error::other)?;
    fs::write(nbtokens_path_for(path), json)
}

impl App {
    /// Save the active notebook tab: rebuild the JSON from the buffer, ask
    /// before a whitespace reformat, write, then give new cells their tokens.
    /// The caller runs the common after-save steps on `Saved`.
    pub(crate) fn save_notebook(&mut self) -> io::Result<SaveOutcome> {
        let idx = self.active_tab;
        let Some(doc) = self.tabs[idx].notebook.as_ref() else {
            return Ok(SaveOutcome::Failed);
        };
        let path = self.tabs[idx].path.clone();

        // The watcher is debounced, so saving can race ahead of its external-
        // change handling. Reconcile the disk here too: otherwise a rebuild
        // from the stale doc can discard newly written outputs or structural
        // changes. A version explicitly kept by K/D/Esc is already recorded
        // in open_disk_snapshot and is intentionally overwritten below.
        if let Ok(disk_text) = read_lossy(&path) {
            let disk_changed = notebook::digest(disk_text.as_bytes()) != doc.disk_digest();
            let acknowledged =
                self.tabs[idx].open_disk_snapshot.as_deref() == Some(disk_text.as_str());
            if disk_changed && !acknowledged {
                if self.tabs[idx].dirty {
                    self.flag_notebook_conflict(&path)?;
                    if self.tabs[idx].conflict_prompt_open {
                        self.set_status(
                            "Notebook changed on disk; resolve the conflict before saving",
                        );
                        return Ok(SaveOutcome::Failed);
                    }
                } else {
                    // Nothing to write: the clean buffer now shows the disk
                    // version; keep the reload status.
                    self.reload_pristine_notebook(&path)?;
                    return Ok(SaveOutcome::Failed);
                }
            }
        }

        let tab = &self.tabs[idx];
        let doc = tab.notebook.as_ref().expect("notebook mode checked above");
        let edited = tab.editor.lines().join("\n");
        let rebuilt = match notebook::rebuild(doc, &edited) {
            Ok(rebuilt) => rebuilt,
            Err(e) => {
                self.set_status(e.to_string());
                return Ok(SaveOutcome::Failed);
            }
        };
        if !rebuilt.unchanged && !doc.layout_canonical() && !tab.reformat_confirmed {
            self.pending = PendingAction::NotebookReformat { then_close: false };
            self.set_status(pending_hint(&self.pending));
            return Ok(SaveOutcome::Deferred);
        }
        // A no-op save does not rewrite the file, unless the disk no longer
        // holds the doc's bytes (the user kept the local version in a conflict).
        let on_disk = read_lossy(&path)
            .ok()
            .map(|t| notebook::digest(t.as_bytes()));
        if !rebuilt.unchanged || on_disk != Some(doc.disk_digest()) {
            fs::write(&path, &rebuilt.bytes)?;
        }
        let preamble_token = has_preamble(&edited)
            .then(|| rebuilt.cell_tokens_after.first().cloned())
            .flatten();
        let tab = &mut self.tabs[idx];
        if let Some(doc) = tab.notebook.as_mut() {
            doc.commit_save(&rebuilt);
        }
        tab.dirty = false;
        tab.open_disk_snapshot = Some(rebuilt.bytes.clone());
        tab.conflict_prompt_open = false;
        tab.conflict_disk_text = None;
        self.insert_saved_tokens(&rebuilt, preamble_token);
        Ok(SaveOutcome::Saved)
    }

    /// Put the tokens of new cells on their marker lines through the
    /// `TextArea` editing API (so undo history is kept), then restore the
    /// cursor and selection. Text above the first marker gets a marker line.
    fn insert_saved_tokens(&mut self, rebuilt: &Rebuilt, preamble_token: Option<String>) {
        if rebuilt.new_tokens.is_empty()
            && rebuilt.replaced_tokens.is_empty()
            && preamble_token.is_none()
        {
            return;
        }
        let ed = &mut self.tabs[self.active_tab].editor;
        let mut cursor = ed.cursor();
        let mut anchor = ed
            .selection_range()
            .map(|(s, e)| if s == cursor { e } else { s });
        ed.cancel_selection();
        // Rows past u16::MAX cannot be addressed by `Jump`; such a cell
        // stays tokenless and is simply new again on the next save.
        let jump = |row: usize, col: usize| {
            Some(CursorMove::Jump(
                u16::try_from(row).ok()?,
                u16::try_from(col).unwrap_or(u16::MAX),
            ))
        };
        let marker_token = |ed: &ratatui_textarea::TextArea<'_>, row: usize| {
            ed.lines()
                .get(row)
                .and_then(|l| notebook::parse_marker(l))
                .map(|m| m.token.is_some())
        };
        for (row, token) in &rebuilt.new_tokens {
            if marker_token(ed, *row) == Some(false)
                && let Some(to) = jump(*row, usize::MAX)
            {
                ed.move_cursor(to);
                ed.insert_str(format!(" #{token}"));
            }
        }
        for (row, token) in &rebuilt.replaced_tokens {
            if marker_token(ed, *row) == Some(true)
                && let Some(to) = jump(*row, ed.lines()[*row].chars().count() - 6)
            {
                ed.move_cursor(to);
                ed.start_selection();
                ed.move_cursor(CursorMove::End);
                ed.insert_str(token);
            }
        }
        if let Some(token) = preamble_token {
            ed.move_cursor(CursorMove::Jump(0, 0));
            ed.insert_str(format!("# %% #{token}\n"));
            cursor.0 += 1;
            if let Some(a) = anchor.as_mut() {
                a.0 += 1;
            }
        }
        if let Some((row, col)) = anchor
            && let Some(to) = jump(row, col)
        {
            ed.move_cursor(to);
            ed.start_selection();
        }
        if let Some(to) = jump(cursor.0, cursor.1) {
            ed.move_cursor(to);
        }
        self.recompute_folds();
    }

    /// Replace the active buffer (undo history is dropped, as for any
    /// reload), keeping the cursor inside the new text.
    fn replace_text_keep_cursor(&mut self, text: &str) {
        let lines = text_to_lines(text);
        let (row, col) = self.tabs[self.active_tab].editor.cursor();
        let row = row.min(lines.len().saturating_sub(1));
        let col = col.min(lines[row].chars().count());
        self.replace_editor_text(lines, (row, col));
    }

    /// Pristine notebook tab whose file changed: reload it. When only
    /// outputs or metadata changed, the buffer and its tokens stay.
    pub(crate) fn reload_pristine_notebook(&mut self, path: &Path) -> io::Result<()> {
        let disk_text = read_lossy(path)?;
        let idx = self.active_tab;
        let Some(doc) = self.tabs[idx].notebook.as_ref() else {
            return Ok(());
        };
        let disk_digest = notebook::digest(disk_text.as_bytes());
        if disk_digest == doc.disk_digest() {
            return Ok(());
        }
        let old_tokens = doc.tokens();
        let current = self.tabs[idx].editor.lines().join("\n");
        let rel = relative_path(&self.root, path).display().to_string();
        let (doc, shown, status) = match notebook::project(&disk_text) {
            Ok(fresh) => {
                let mut same = fresh.clone();
                if same.adopt_tokens(&old_tokens) && same.projected_text() == current {
                    let tab = &mut self.tabs[idx];
                    tab.notebook = Some(same);
                    tab.open_disk_snapshot = Some(disk_text);
                    if !self.take_run_result(path, disk_digest) {
                        self.set_status("Notebook outputs updated on disk");
                    }
                    return Ok(());
                }
                let shown = fresh.projected_text().to_string();
                (Some(fresh), shown, format!("Reloaded {rel} from disk"))
            }
            Err(e) => (
                None,
                disk_text.clone(),
                format!("Reloaded {rel} as raw JSON: {e}"),
            ),
        };
        self.tabs[idx].notebook = doc;
        self.replace_text_keep_cursor(&shown);
        let tab = &mut self.tabs[idx];
        tab.dirty = false;
        tab.open_disk_snapshot = Some(disk_text);
        self.notify_lsp_did_change();
        self.set_status(status);
        Ok(())
    }

    /// A notebook tab that just became active: the watcher reconciles only
    /// the active tab, so a change made while it was inactive (e.g. a run
    /// that finished meanwhile) is applied now. No-op on raw tabs.
    pub(crate) fn reconcile_activated_notebook(&mut self) {
        if !self.active_tab().is_some_and(|t| t.is_notebook()) {
            return;
        }
        let result = if self.is_dirty() {
            self.maybe_flag_external_conflict()
        } else {
            self.reload_open_file_from_disk_if_pristine()
        };
        if let Err(e) = result {
            self.set_status(format!("Could not check notebook on disk: {e}"));
        }
    }

    /// Dirty notebook tab whose file changed: adopt the disk outputs when
    /// the cell mapping is provable, otherwise raise the conflict prompt.
    /// `conflict_disk_text` holds the raw disk JSON for `R`.
    pub(crate) fn flag_notebook_conflict(&mut self, path: &Path) -> io::Result<()> {
        let disk_text = read_lossy(path)?;
        let disk_digest = notebook::digest(disk_text.as_bytes());
        let tab = &mut self.tabs[self.active_tab];
        let Some(doc) = tab.notebook.as_mut() else {
            return Ok(());
        };
        // Our own save, or a version the user already chose to overwrite.
        if disk_digest == doc.disk_digest()
            || tab.open_disk_snapshot.as_deref() == Some(disk_text.as_str())
        {
            return Ok(());
        }
        match notebook::project(&disk_text) {
            Ok(disk) if doc.outputs_only_change(&disk) => {
                doc.adopt_disk_outputs(disk);
                tab.open_disk_snapshot = Some(disk_text);
                if !self.take_run_result(path, disk_digest) {
                    self.set_status("Notebook outputs updated on disk");
                }
            }
            _ => {
                tab.conflict_prompt_open = true;
                tab.conflict_disk_text = Some(disk_text);
            }
        }
        Ok(())
    }

    /// Conflict `R` on a notebook tab: load the disk notebook with fresh
    /// tokens (or as raw JSON when it no longer projects).
    pub(crate) fn reload_notebook_from_conflict(&mut self, disk: String) {
        let (doc, shown, status) = match notebook::project(&disk) {
            Ok(doc) => {
                let shown = doc.projected_text().to_string();
                (Some(doc), shown, "Reloaded file from disk".to_string())
            }
            Err(e) => (
                None,
                disk.clone(),
                format!("Reloaded file from disk as raw JSON: {e}"),
            ),
        };
        self.tabs[self.active_tab].notebook = doc;
        self.replace_text_keep_cursor(&shown);
        let tab = &mut self.tabs[self.active_tab];
        tab.dirty = false;
        tab.open_disk_snapshot = Some(disk);
        self.clear_autosave_for_open_file();
        self.notify_lsp_did_change();
        self.set_status(status);
    }

    /// Recovery on a notebook tab: adopt the autosave's tokens when the
    /// sidecar matches the notebook on disk; otherwise strip the tokens from
    /// the recovered text so every cell is new and no outputs misattach.
    pub(crate) fn recover_notebook_tokens(&mut self, recovered: String) -> String {
        let idx = self.active_tab;
        let sidecar = fs::read_to_string(nbtokens_path_for(&self.tabs[idx].path))
            .ok()
            .and_then(|s| serde_json::from_str::<NotebookTokens>(&s).ok());
        let tab = &mut self.tabs[idx];
        let Some(doc) = tab.notebook.as_mut() else {
            return recovered;
        };
        let adopted = sidecar
            .is_some_and(|s| s.disk_digest == doc.disk_digest() && doc.adopt_tokens(&s.tokens));
        tab.recovery_tokens_stale = !adopted;
        if !adopted {
            return notebook::strip_tokens(&recovered);
        }
        let shown = doc.projected_text().to_string();
        self.replace_editor_text(text_to_lines(&shown), (0, 0));
        recovered
    }

    /// Palette "Reopen as raw JSON" / "Reopen as notebook": switch the
    /// active tab's mode in place. Refused on a dirty tab.
    pub(crate) fn reopen_active_tab(&mut self, as_notebook: bool) -> io::Result<()> {
        let Some(tab) = self.active_tab() else {
            self.set_status("No file open");
            return Ok(());
        };
        if tab.is_notebook() == as_notebook {
            self.set_status(if as_notebook {
                "Already open as a notebook"
            } else {
                "Not a notebook"
            });
            return Ok(());
        }
        if as_notebook && !is_ipynb_path(&tab.path) {
            self.set_status("Not a notebook file");
            return Ok(());
        }
        if tab.dirty {
            self.set_status("Save before reopening");
            return Ok(());
        }
        let path = tab.path.clone();
        let rel = relative_path(&self.root, &path).display().to_string();
        let text = read_lossy(&path)?;
        let (doc, shown) = if as_notebook {
            match notebook::project(&text) {
                Ok(doc) => {
                    let shown = doc.projected_text().to_string();
                    (Some(doc), shown)
                }
                Err(e) => {
                    self.set_status(format!("Cannot open {rel} as a notebook: {e}"));
                    return Ok(());
                }
            }
        } else {
            (None, text.clone())
        };
        let tab = &mut self.tabs[self.active_tab];
        tab.notebook = doc;
        tab.reformat_confirmed = false;
        tab.folded_starts.clear();
        self.replace_editor_text(text_to_lines(&shown), (0, 0));
        let lines = self.tabs[self.active_tab].editor.lines().len();
        let git_line_status = compute_git_line_status(&self.root, &path, lines, as_notebook);
        let tab = &mut self.tabs[self.active_tab];
        tab.dirty = false;
        tab.open_disk_snapshot = Some(text);
        tab.git_line_status = git_line_status;
        self.notify_lsp_did_change();
        self.set_status(if as_notebook {
            format!("Reopened {rel} as notebook")
        } else {
            format!("Reopened {rel} as raw JSON")
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::autosave_path_for;
    use crate::types::CommandAction;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use serde_json::Value;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    const V44: &str = include_str!("../notebook/testdata/v44_no_ids.ipynb");
    const LAB45: &str = include_str!("../notebook/testdata/lab45_executed.ipynb");
    const INDENT2: &str = include_str!("../notebook/testdata/indent2.ipynb");
    const PERCENT: &str = include_str!("../notebook/testdata/percent_in_code.ipynb");

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Write `text` as `name` in `root`, open it, and return the app and path.
    fn open(root: &Path, name: &str, text: &str) -> (App, PathBuf) {
        let path = root.join(name);
        fs::write(&path, text).expect("write");
        let mut app = App::new(root.to_path_buf()).expect("app");
        app.open_file(path.clone()).expect("open");
        (app, path)
    }

    fn buffer(app: &App) -> String {
        app.tabs[app.active_tab].editor.lines().join("\n")
    }

    fn lines(app: &App) -> Vec<String> {
        app.tabs[app.active_tab].editor.lines().to_vec()
    }

    /// Replace one buffer line through the editing API and mark the tab dirty.
    fn edit_line(app: &mut App, row: usize, text: &str) {
        let ed = &mut app.tabs[app.active_tab].editor;
        ed.move_cursor(CursorMove::Jump(row as u16, 0));
        ed.start_selection();
        ed.move_cursor(CursorMove::End);
        ed.insert_str(text);
        app.mark_dirty();
    }

    fn row_of(app: &App, line: &str) -> usize {
        lines(app)
            .iter()
            .position(|l| l == line)
            .expect("line in buffer")
    }

    fn cells(path: &Path) -> Vec<Value> {
        let v: Value =
            serde_json::from_str(&fs::read_to_string(path).expect("read")).expect("json");
        v["cells"].as_array().expect("cells").clone()
    }

    fn source(cell: &Value) -> String {
        match &cell["source"] {
            Value::String(s) => s.clone(),
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => String::new(),
        }
    }

    fn markers(app: &App) -> Vec<String> {
        lines(app)
            .into_iter()
            .filter(|l| notebook::is_marker_like(l))
            .collect()
    }

    #[test]
    fn open_projects_notebook_and_falls_back_to_raw_json() {
        let tmp = tempdir().expect("tempdir");
        let (app, _) = open(tmp.path(), "a.ipynb", V44);
        let tab = &app.tabs[0];
        assert!(tab.is_notebook());
        assert_eq!(app.status, "Opened a.ipynb");
        let doc = tab.notebook.as_ref().expect("doc");
        assert_eq!(buffer(&app), doc.projected_text());
        assert_eq!(markers(&app).len(), 3);

        let (app, _) = open(tmp.path(), "b.ipynb", PERCENT);
        let tab = &app.tabs[0];
        assert!(!tab.is_notebook());
        assert_eq!(buffer(&app), PERCENT);
        assert_eq!(
            app.status,
            "Opened b.ipynb as raw JSON: cell 1 contains a `# %%` line"
        );
    }

    /// Plan test 10 + own save: the buffer stays projected, existing tokens
    /// are unchanged, a new cell gets a token, the watcher does not reload,
    /// and Ctrl+Z after the save still reaches the user's own edit.
    #[test]
    fn save_keeps_tokens_tokenizes_new_cell_and_keeps_undo() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, path) = open(tmp.path(), "a.ipynb", LAB45);
        let before = markers(&app);
        let last = lines(&app).len() - 1;
        let ed = &mut app.tabs[0].editor;
        ed.move_cursor(CursorMove::Jump(last as u16, 0));
        ed.insert_str("\n# %%\nz = 3");
        app.mark_dirty();
        let cursor = app.tabs[0].editor.cursor();

        assert_eq!(app.save_file().expect("save"), SaveOutcome::Saved);
        let after = markers(&app);
        assert_eq!(&after[..before.len()], &before[..]);
        let new_marker = after.last().expect("marker");
        let token = new_marker.strip_prefix("# %% #").expect("token added");
        assert_eq!(token.len(), 6);
        assert_eq!(app.tabs[0].editor.cursor(), cursor);
        let written = cells(&path);
        assert_eq!(written.len(), 8);
        assert_eq!(source(&written[7]), "z = 3");
        assert_eq!(written[7]["outputs"], Value::Array(vec![]));
        let doc = app.tabs[0].notebook.as_ref().expect("doc");
        assert_eq!(doc.projected_text(), format!("{}\n", buffer(&app)));
        assert!(!app.tabs[0].dirty);

        // Own save: the watcher sees our digest and leaves the tab alone.
        let shown = buffer(&app);
        app.reload_open_file_from_disk_if_pristine().expect("poll");
        assert_eq!(buffer(&app), shown);
        assert!(app.status.starts_with("Saved"), "{}", app.status);

        // Undo: first the token insertion, then the user's own edit.
        app.tabs[0].editor.undo();
        assert_eq!(markers(&app).last().map(String::as_str), Some("# %%"));
        app.tabs[0].editor.undo();
        assert_eq!(markers(&app), before);
    }

    /// A save with no edits does not rewrite the file.
    #[test]
    fn noop_save_does_not_rewrite() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, path) = open(tmp.path(), "a.ipynb", INDENT2);
        assert_eq!(app.save_file().expect("save"), SaveOutcome::Saved);
        assert_eq!(fs::read_to_string(&path).expect("read"), INDENT2);
        assert!(matches!(app.pending, PendingAction::None));
    }

    /// Text above the first marker becomes a code cell, and gets a marker
    /// line with its token, so the next save is a no-op.
    #[test]
    fn preamble_text_gets_a_marker_on_save() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, path) = open(tmp.path(), "a.ipynb", V44);
        let ed = &mut app.tabs[0].editor;
        ed.move_cursor(CursorMove::Jump(0, 0));
        ed.insert_str("import os\n\n");
        app.mark_dirty();
        assert_eq!(app.save_file().expect("save"), SaveOutcome::Saved);
        let first = lines(&app)[0].clone();
        assert!(first.starts_with("# %% #"), "{first}");
        assert_eq!(lines(&app)[1], "import os");
        assert_eq!(app.tabs[0].editor.cursor(), (3, 0));
        let written = cells(&path);
        assert_eq!(written.len(), 4);
        assert_eq!(source(&written[0]), "import os");
        let bytes = fs::read_to_string(&path).expect("read");

        let doc = app.tabs[0].notebook.as_ref().expect("doc");
        let rebuilt = notebook::rebuild(doc, &buffer(&app)).expect("rebuild");
        assert!(rebuilt.unchanged);
        app.mark_dirty();
        assert_eq!(app.save_file().expect("save"), SaveOutcome::Saved);
        assert_eq!(fs::read_to_string(&path).expect("read"), bytes);
    }

    /// Plan test 12b: a token from another notebook is a new cell.
    #[test]
    fn foreign_token_saves_as_new_cell() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, path) = open(tmp.path(), "a.ipynb", V44);
        let last = lines(&app).len() - 1;
        let ed = &mut app.tabs[0].editor;
        ed.move_cursor(CursorMove::Jump(last as u16, 0));
        ed.insert_str("\n# %% #zzzzzz\nq = 1");
        app.mark_dirty();
        assert_eq!(app.save_file().expect("save"), SaveOutcome::Saved);
        let written = cells(&path);
        assert_eq!(written.len(), 4);
        assert_eq!(source(&written[3]), "q = 1");
        assert_eq!(written[3]["outputs"], Value::Array(vec![]));
        assert_eq!(written[0]["outputs"][0]["data"]["text/plain"][0], "1");
    }

    /// Plan test 12c: a half-typed marker fails the save; nothing is written.
    #[test]
    fn malformed_marker_fails_save() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, path) = open(tmp.path(), "a.ipynb", V44);
        let row = row_of(&app, "y = 2");
        edit_line(&mut app, row, "# %% #");
        assert_eq!(app.save_file().expect("save"), SaveOutcome::Failed);
        assert_eq!(
            app.status,
            format!("Malformed cell marker at line {}", row + 1)
        );
        assert_eq!(fs::read_to_string(&path).expect("read"), V44);
        assert!(app.tabs[0].dirty);
    }

    /// Plan test 13: a non-canonical notebook asks before reformatting.
    #[test]
    fn noncanonical_close_prompt_asks_before_reformat() {
        for answer in [KeyCode::Char('y'), KeyCode::Char('n')] {
            let tmp = tempdir().expect("tempdir");
            let (mut app, path) = open(tmp.path(), "a.ipynb", INDENT2);
            let row = row_of(&app, "a = 1");
            edit_line(&mut app, row, "a = 2");
            app.pending = PendingAction::ClosePrompt;
            app.handle_pending_key(key(KeyCode::Enter)).expect("enter");
            assert!(matches!(
                app.pending,
                PendingAction::NotebookReformat { then_close: true }
            ));
            assert_eq!(app.tabs.len(), 1);
            assert_eq!(fs::read_to_string(&path).expect("read"), INDENT2);

            app.handle_pending_key(key(answer)).expect("answer");
            if answer == KeyCode::Char('y') {
                assert!(app.tabs.is_empty());
                assert_eq!(source(&cells(&path)[0]), "a = 2");
            } else {
                assert_eq!(app.tabs.len(), 1);
                assert!(app.tabs[0].dirty);
                assert_eq!(fs::read_to_string(&path).expect("read"), INDENT2);
            }
        }
    }

    /// Plan test 11: an outputs-only change on disk under a dirty tab is
    /// adopted; the next save keeps the new outputs and the local edit.
    #[test]
    fn outputs_only_external_change_is_adopted() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, path) = open(tmp.path(), "a.ipynb", V44);
        let row = row_of(&app, "y = 2");
        edit_line(&mut app, row, "y = 20");
        let disk = V44.replace("\"1\"\n", "\"42\"\n");
        assert_ne!(disk, V44);
        fs::write(&path, &disk).expect("write");
        app.maybe_flag_external_conflict().expect("poll");
        assert!(!app.tabs[0].conflict_prompt_open);
        assert_eq!(app.status, "Notebook outputs updated on disk");
        assert!(app.tabs[0].dirty);

        assert_eq!(app.save_file().expect("save"), SaveOutcome::Saved);
        let written = cells(&path);
        assert_eq!(written[0]["outputs"][0]["data"]["text/plain"][0], "42");
        assert_eq!(source(&written[2]), "y = 20");
    }

    /// Saving can happen before the debounced watcher handles a disk event.
    /// The save path itself must adopt output-only updates and must stop for
    /// a structural conflict instead of rebuilding from the stale document.
    #[test]
    fn save_preflights_external_notebook_change() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, path) = open(tmp.path(), "outputs.ipynb", V44);
        let row = row_of(&app, "y = 2");
        edit_line(&mut app, row, "y = 20");
        let disk = V44.replace("\"1\"\n", "\"42\"\n");
        fs::write(&path, &disk).expect("external output write");

        // Do not poll the watcher: save must still merge the disk outputs.
        assert_eq!(app.save_file().expect("save"), SaveOutcome::Saved);
        let written = cells(&path);
        assert_eq!(written[0]["outputs"][0]["data"]["text/plain"][0], "42");
        assert_eq!(source(&written[2]), "y = 20");

        let (mut app, path) = open(tmp.path(), "structural.ipynb", V44);
        let row = row_of(&app, "y = 2");
        edit_line(&mut app, row, "y = 20");
        let mut changed: Value = serde_json::from_str(V44).expect("json");
        changed["cells"].as_array_mut().expect("cells").remove(0);
        let disk = serde_json::to_string_pretty(&changed).expect("serialize");
        fs::write(&path, &disk).expect("external structural write");

        assert_eq!(app.save_file().expect("save"), SaveOutcome::Failed);
        assert!(app.tabs[0].conflict_prompt_open);
        assert_eq!(fs::read_to_string(&path).expect("read"), disk);
        assert!(app.tabs[0].dirty);
        // A clean tab reloads instead of saving, and says so.
        let (mut app, path) = open(tmp.path(), "clean.ipynb", V44);
        fs::write(&path, V44.replace("\"y = 2\"", "\"y = 3\"")).expect("write");
        assert_eq!(app.save_file().expect("save"), SaveOutcome::Failed);
        assert_eq!(app.status, "Reloaded clean.ipynb from disk");
        assert!(lines(&app).contains(&"y = 3".to_string()));
    }

    /// Plan test 12: a structural change raises the conflict prompt; `K`
    /// keeps the old doc, so a save writes the local version and no cell
    /// takes another cell's outputs.
    #[test]
    fn structural_external_change_conflict_keep_mine() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, path) = open(tmp.path(), "a.ipynb", V44);
        let row = row_of(&app, "y = 2");
        edit_line(&mut app, row, "y = 20");
        let mut v: Value = serde_json::from_str(V44).expect("json");
        let new_cell = serde_json::json!({
            "cell_type": "code", "execution_count": 9, "metadata": {},
            "outputs": [{"output_type": "stream", "name": "stdout", "text": ["new\n"]}],
            "source": ["inserted"]
        });
        v["cells"]
            .as_array_mut()
            .expect("cells")
            .insert(0, new_cell);
        fs::write(&path, serde_json::to_string_pretty(&v).expect("ser")).expect("write");

        app.maybe_flag_external_conflict().expect("poll");
        assert!(app.tabs[0].conflict_prompt_open);
        app.handle_conflict_prompt_key(key(KeyCode::Char('k')))
            .expect("k");
        assert_eq!(
            app.status,
            "Kept local version; saving will overwrite the notebook on disk"
        );
        // The kept-over disk version does not prompt again.
        app.maybe_flag_external_conflict().expect("poll");
        assert!(!app.tabs[0].conflict_prompt_open);

        assert_eq!(app.save_file().expect("save"), SaveOutcome::Saved);
        let written = cells(&path);
        assert_eq!(written.len(), 3);
        assert_eq!(source(&written[0]), "x = 1\nx");
        assert_eq!(written[0]["outputs"][0]["data"]["text/plain"][0], "1");
        assert_eq!(source(&written[2]), "y = 20");
        assert_eq!(written[2]["outputs"], Value::Array(vec![]));
    }

    /// Keep mine, undo back to the loaded text, save: the disk still gets
    /// the local version (a no-op rebuild is written when the disk differs).
    #[test]
    fn keep_mine_then_unchanged_save_still_overwrites_disk() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, path) = open(tmp.path(), "a.ipynb", V44);
        let row = row_of(&app, "y = 2");
        edit_line(&mut app, row, "y = 20");
        fs::write(&path, V44.replace("\"y = 2\"", "\"y = 3\"")).expect("write");
        app.maybe_flag_external_conflict().expect("poll");
        assert!(app.tabs[0].conflict_prompt_open);
        app.handle_conflict_prompt_key(key(KeyCode::Esc))
            .expect("esc");
        edit_line(&mut app, row, "y = 2");
        assert_eq!(app.save_file().expect("save"), SaveOutcome::Saved);
        assert_eq!(fs::read_to_string(&path).expect("read"), V44);
    }

    /// Conflict `R` loads the disk notebook with fresh tokens.
    #[test]
    fn conflict_reload_loads_disk_notebook() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, path) = open(tmp.path(), "a.ipynb", V44);
        let row = row_of(&app, "y = 2");
        edit_line(&mut app, row, "y = 20");
        let disk = V44.replace("\"y = 2\"", "\"y = 3\"");
        fs::write(&path, &disk).expect("write");
        app.maybe_flag_external_conflict().expect("poll");
        app.handle_conflict_prompt_key(key(KeyCode::Char('r')))
            .expect("r");
        assert!(!app.tabs[0].dirty);
        assert!(lines(&app).contains(&"y = 3".to_string()));
        let doc = app.tabs[0].notebook.as_ref().expect("doc");
        assert_eq!(doc.disk_digest(), notebook::digest(disk.as_bytes()));
        assert_eq!(app.save_file().expect("save"), SaveOutcome::Saved);
        assert_eq!(fs::read_to_string(&path).expect("read"), disk);
    }

    /// Plan test 12a: swapping two identical-source cells of a pre-4.5
    /// notebook is not provable, so it raises the conflict prompt.
    #[test]
    fn swap_of_identical_cells_without_ids_conflicts() {
        let tmp = tempdir().expect("tempdir");
        let text = V44.replace("\"y = 2\"", "\"x = 1\\n\",\n    \"x\"");
        let (mut app, path) = open(tmp.path(), "a.ipynb", &text);
        assert!(app.tabs[0].is_notebook());
        let row = row_of(&app, "# ## Notes");
        edit_line(&mut app, row, "# ## Notes!");
        let mut v: Value = serde_json::from_str(&text).expect("json");
        v["cells"].as_array_mut().expect("cells").swap(0, 2);
        fs::write(&path, serde_json::to_string_pretty(&v).expect("ser")).expect("write");
        app.maybe_flag_external_conflict().expect("poll");
        assert!(app.tabs[0].conflict_prompt_open);
    }

    /// A pristine tab reloads a structural change, and keeps its buffer and
    /// tokens when only outputs changed.
    #[test]
    fn pristine_tab_reloads_from_disk() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, path) = open(tmp.path(), "a.ipynb", V44);
        let shown = buffer(&app);
        fs::write(&path, V44.replace("\"1\"\n", "\"42\"\n")).expect("write");
        app.reload_open_file_from_disk_if_pristine().expect("poll");
        assert_eq!(buffer(&app), shown);
        assert_eq!(app.status, "Notebook outputs updated on disk");

        fs::write(&path, V44.replace("\"y = 2\"", "\"y = 3\"")).expect("write");
        app.reload_open_file_from_disk_if_pristine().expect("poll");
        assert!(lines(&app).contains(&"y = 3".to_string()));
        assert_eq!(app.status, "Reloaded a.ipynb from disk");

        fs::write(&path, "{ not json").expect("write");
        app.reload_open_file_from_disk_if_pristine().expect("poll");
        assert!(!app.tabs[0].is_notebook());
        assert_eq!(buffer(&app), "{ not json");
        assert!(app.status.starts_with("Reloaded a.ipynb as raw JSON: "));
    }

    /// Simulate a crash: autosave a dirty notebook tab, then drop the app.
    fn autosave_and_crash(root: &Path, text: &str, edit: (&str, &str)) -> PathBuf {
        let (mut app, path) = open(root, "a.ipynb", text);
        let row = row_of(&app, edit.0);
        edit_line(&mut app, row, edit.1);
        app.autosave_last_write = Instant::now()
            .checked_sub(Duration::from_secs(10))
            .expect("instant");
        app.poll_autosave().expect("autosave");
        assert!(nbtokens_path_for(&path).exists());
        path
    }

    fn cleanup_autosave(path: &Path) {
        let _ = fs::remove_file(autosave_path_for(path));
        let _ = fs::remove_file(nbtokens_path_for(path));
    }

    /// Recovery with a matching digest adopts the autosave's tokens, so the
    /// recovered cells keep their outputs.
    #[test]
    fn recovery_with_matching_digest_keeps_outputs() {
        let tmp = tempdir().expect("tempdir");
        let path = autosave_and_crash(tmp.path(), V44, ("x", "x + 1"));
        let mut app = App::new(tmp.path().to_path_buf()).expect("app");
        app.open_file(path.clone()).expect("open");
        assert!(app.tabs[0].recovery_prompt_open);
        assert!(!app.tabs[0].recovery_tokens_stale);
        app.handle_recovery_prompt_key(key(KeyCode::Enter))
            .expect("recover");
        assert_eq!(app.save_file().expect("save"), SaveOutcome::Saved);
        let written = cells(&path);
        assert_eq!(source(&written[0]), "x = 1\nx + 1");
        assert_eq!(written[0]["outputs"][0]["data"]["text/plain"][0], "1");
        assert!(!nbtokens_path_for(&path).exists());
        cleanup_autosave(&path);
    }

    /// Plan test 14: recovery with a stale digest strips the tokens; the
    /// text is recovered and no outputs attach to it.
    #[test]
    fn recovery_with_stale_digest_strips_tokens() {
        let tmp = tempdir().expect("tempdir");
        let path = autosave_and_crash(tmp.path(), V44, ("x", "x + 1"));
        fs::write(&path, V44.replace("\"y = 2\"", "\"y = 3\"")).expect("write");
        let mut app = App::new(tmp.path().to_path_buf()).expect("app");
        app.open_file(path.clone()).expect("open");
        let tab = &app.tabs[0];
        assert!(tab.recovery_prompt_open);
        assert!(tab.recovery_tokens_stale);
        let text = tab.recovery_text.clone().expect("text");
        assert!(text.lines().all(|l| !l.starts_with("# %% #")), "{text}");
        app.handle_recovery_prompt_key(key(KeyCode::Enter))
            .expect("recover");
        assert_eq!(app.save_file().expect("save"), SaveOutcome::Saved);
        let written = cells(&path);
        assert_eq!(source(&written[0]), "x = 1\nx + 1");
        assert_eq!(written[0]["outputs"], Value::Array(vec![]));
        assert_eq!(source(&written[2]), "y = 2");
        cleanup_autosave(&path);
    }

    /// Upgrade case: a pre-notebook raw autosave of an `.ipynb` (no
    /// sidecar) is not offered to the notebook tab, and is kept on disk.
    #[test]
    fn raw_autosave_is_not_offered_to_notebook_tab() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("a.ipynb");
        fs::write(&path, V44).expect("write");
        let autosave = autosave_path_for(&path);
        fs::create_dir_all(autosave.parent().expect("parent")).expect("mkdir");
        let raw = V44.replace("\"x = 1\"", "\"x = 9\"");
        fs::write(&autosave, &raw).expect("autosave");
        let mut app = App::new(tmp.path().to_path_buf()).expect("app");
        app.open_file(path.clone()).expect("open");
        assert!(app.tabs[0].is_notebook());
        assert!(!app.tabs[0].recovery_prompt_open);
        assert_eq!(app.tabs[0].recovery_text, None);
        assert_eq!(
            app.status,
            "Autosave for a.ipynb was written in a different view; not recovered"
        );
        assert_eq!(fs::read_to_string(&autosave).expect("kept"), raw);
        cleanup_autosave(&path);
    }

    /// A notebook-view autosave (projected text + sidecar) is not offered to
    /// a raw tab; both files are kept.
    #[test]
    fn notebook_autosave_is_not_offered_to_raw_tab() {
        let tmp = tempdir().expect("tempdir");
        let path = autosave_and_crash(tmp.path(), V44, ("x", "x + 1"));
        let projected = fs::read_to_string(autosave_path_for(&path)).expect("autosave");
        fs::write(&path, "{ not json").expect("write");
        let mut app = App::new(tmp.path().to_path_buf()).expect("app");
        app.open_file(path.clone()).expect("open");
        assert!(!app.tabs[0].is_notebook());
        assert!(!app.tabs[0].recovery_prompt_open);
        assert_eq!(
            app.status,
            "Autosave for a.ipynb was written in a different view; not recovered"
        );
        assert_eq!(
            fs::read_to_string(autosave_path_for(&path)).expect("kept"),
            projected
        );
        assert!(nbtokens_path_for(&path).exists());
        cleanup_autosave(&path);
    }

    /// A raw-JSON autosave replaces a projected one, so it drops the stale
    /// sidecar and recovery then sees the raw view.
    #[test]
    fn raw_autosave_drops_stale_sidecar() {
        let tmp = tempdir().expect("tempdir");
        let path = autosave_and_crash(tmp.path(), V44, ("x", "x + 1"));
        let mut app = App::new(tmp.path().to_path_buf()).expect("app");
        app.open_file(path.clone()).expect("open");
        app.handle_recovery_prompt_key(key(KeyCode::Esc))
            .expect("cancel");
        app.run_command_action(CommandAction::ReopenAsRawJson)
            .expect("reopen");
        assert!(!app.tabs[0].is_notebook());
        edit_line(&mut app, 0, "{ ");
        app.autosave_last_write = Instant::now()
            .checked_sub(Duration::from_secs(10))
            .expect("instant");
        app.poll_autosave().expect("autosave");
        assert!(!nbtokens_path_for(&path).exists());
        cleanup_autosave(&path);
    }

    /// Guardrail 7: reopen as raw JSON and back; refused on a dirty tab.
    #[test]
    fn reopen_as_raw_json_and_back() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, path) = open(tmp.path(), "a.ipynb", V44);
        app.mark_dirty();
        app.run_command_action(CommandAction::ReopenAsRawJson)
            .expect("reopen");
        assert_eq!(app.status, "Save before reopening");
        assert!(app.tabs[0].is_notebook());

        app.tabs[0].dirty = false;
        app.run_command_action(CommandAction::ReopenAsRawJson)
            .expect("reopen");
        assert!(!app.tabs[0].is_notebook());
        assert_eq!(buffer(&app), V44);
        assert_eq!(app.status, "Reopened a.ipynb as raw JSON");
        app.run_command_action(CommandAction::ReopenAsRawJson)
            .expect("reopen");
        assert_eq!(app.status, "Not a notebook");

        app.run_command_action(CommandAction::ReopenAsNotebook)
            .expect("reopen");
        assert!(app.tabs[0].is_notebook());
        assert_eq!(markers(&app).len(), 3);
        assert_eq!(fs::read_to_string(&path).expect("read"), V44);

        let (mut app, _) = open(tmp.path(), "b.rs", "fn b() {}\n");
        app.run_command_action(CommandAction::ReopenAsNotebook)
            .expect("reopen");
        assert_eq!(app.status, "Not a notebook file");
    }

    /// Plan test 16: a raw-fallback `.ipynb` keeps its git gutter.
    #[test]
    fn raw_fallback_ipynb_keeps_git_gutter() {
        use crate::tab::GitLineStatus;
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .status()
                .is_ok_and(|s| s.success())
        };
        let path = root.join("b.ipynb");
        fs::write(&path, PERCENT).expect("write");
        if !(git(&["init", "-q"]) && git(&["add", "."]) && git(&["commit", "-qm", "x"])) {
            return; // git not available
        }
        fs::write(&path, PERCENT.replace("\"x = 1\"", "\"x = 2\"")).expect("write");
        let mut app = App::new(root.to_path_buf()).expect("app");
        app.open_file(path).expect("open");
        let tab = &app.tabs[0];
        assert!(!tab.is_notebook());
        assert!(
            tab.git_line_status.contains(&GitLineStatus::Modified),
            "{:?}",
            tab.git_line_status
        );
    }

    /// Guardrail: a raw `.rs` tab's save, watcher, conflict and autosave
    /// paths behave exactly as before (no sidecar, no projection).
    #[test]
    fn raw_rs_tab_paths_are_unchanged() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, path) = open(tmp.path(), "a.rs", "fn a() {}\n");
        assert!(!app.tabs[0].is_notebook());
        edit_line(&mut app, 0, "fn b() {}");
        assert_eq!(app.save_file().expect("save"), SaveOutcome::Saved);
        assert_eq!(fs::read_to_string(&path).expect("read"), "fn b() {}\n");

        fs::write(&path, "fn c() {}\n").expect("write");
        app.reload_open_file_from_disk_if_pristine().expect("poll");
        assert_eq!(buffer(&app), "fn c() {}\n");
        assert_eq!(app.status, "Reloaded a.rs from disk");

        edit_line(&mut app, 0, "fn d() {}");
        fs::write(&path, "fn e() {}\n").expect("write");
        app.maybe_flag_external_conflict().expect("poll");
        assert!(app.tabs[0].conflict_prompt_open);
        assert_eq!(
            app.tabs[0].conflict_disk_text.as_deref(),
            Some("fn e() {}\n")
        );
        app.handle_conflict_prompt_key(key(KeyCode::Char('k')))
            .expect("k");
        assert_eq!(app.status, "Keeping local edits");

        app.autosave_last_write = Instant::now()
            .checked_sub(Duration::from_secs(10))
            .expect("instant");
        app.poll_autosave().expect("autosave");
        assert_eq!(
            fs::read_to_string(autosave_path_for(&path)).expect("autosave"),
            buffer(&app)
        );
        assert!(!nbtokens_path_for(&path).exists());
        let mut again = App::new(tmp.path().to_path_buf()).expect("app");
        again.open_file(path.clone()).expect("open");
        assert_eq!(again.tabs[0].recovery_text.as_deref(), Some("fn d() {}\n"));
        assert!(!again.tabs[0].recovery_tokens_stale);
        cleanup_autosave(&path);
    }
}
