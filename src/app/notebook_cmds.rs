//! Notebook commands on a projected notebook tab: move between cells, insert
//! a cell, toggle a cell between code and markdown, clear all outputs.
//! Buffer changes go through the `TextArea` editing API so Ctrl+Z undoes them.

use ratatui_textarea::CursorMove;

use super::App;
use crate::notebook::{CellKind, Marker, cell_rows, is_marker_like, marker_line, parse_marker};
use crate::util::to_u16_saturating;

const NOT_A_NOTEBOOK: &str = "Not a notebook";

impl App {
    /// The active tab's lines and cursor when it is a notebook tab; else
    /// shows "Not a notebook".
    fn notebook_lines_and_cursor(&mut self) -> Option<(Vec<String>, (usize, usize))> {
        match self.active_tab() {
            Some(tab) if tab.is_notebook() => {
                Some((tab.editor.lines().to_vec(), tab.editor.cursor()))
            }
            _ => {
                self.set_status(NOT_A_NOTEBOOK);
                None
            }
        }
    }

    fn move_editor_cursor(&mut self, row: usize, col: usize) {
        if let Some(tab) = self.active_tab_mut() {
            tab.editor.cancel_selection();
            tab.editor.move_cursor(CursorMove::Jump(
                to_u16_saturating(row),
                to_u16_saturating(col),
            ));
        }
        self.sync_editor_scroll_guess();
    }

    /// Move the cursor to the first line after the next marker.
    pub(crate) fn notebook_next_cell(&mut self) {
        let Some((lines, (row, _))) = self.notebook_lines_and_cursor() else {
            return;
        };
        match (row + 1..lines.len()).find(|&r| is_marker_like(&lines[r])) {
            Some(next) => self.move_editor_cursor((next + 1).min(lines.len() - 1), 0),
            None => self.set_status("No next cell"),
        }
    }

    /// Move the cursor to the first line after the current cell's marker, or
    /// after the previous marker when the cursor is already there.
    pub(crate) fn notebook_prev_cell(&mut self) {
        let Some((lines, cursor)) = self.notebook_lines_and_cursor() else {
            return;
        };
        let last = lines.len() - 1;
        let (marker, _) = cell_rows(&lines, cursor.0);
        if let Some(m) = marker {
            let target = ((m + 1).min(last), 0);
            if cursor.0 > m && cursor != target {
                self.move_editor_cursor(target.0, 0);
                return;
            }
            if let Some(prev) = (0..m).rev().find(|&r| is_marker_like(&lines[r])) {
                self.move_editor_cursor((prev + 1).min(last), 0);
                return;
            }
        }
        self.set_status("No previous cell");
    }

    /// Insert an empty code cell after the current cell's last line and put
    /// the cursor in it. The new marker has no token; the save assigns one.
    pub(crate) fn notebook_insert_cell_below(&mut self) {
        let Some((lines, (row, _))) = self.notebook_lines_and_cursor() else {
            return;
        };
        let (_, end) = cell_rows(&lines, row);
        // Keep one blank separator line before the new marker. An empty last
        // line already is one (or is the line after the final newline, which
        // `rebuild` ignores), so the current cell's text never changes.
        let insert = if lines[end].is_empty() {
            "\n# %%\n"
        } else {
            "\n\n# %%\n"
        };
        if let Some(tab) = self.active_tab_mut() {
            tab.editor.cancel_selection();
            tab.editor.move_cursor(CursorMove::Jump(
                to_u16_saturating(end),
                to_u16_saturating(lines[end].chars().count()),
            ));
            tab.editor.insert_str(insert);
        }
        self.on_editor_content_changed();
        self.sync_editor_scroll_guess();
        self.set_status("Inserted cell below");
    }

    /// Rewrite the current cell's marker between code and markdown (raw
    /// becomes code) and add or remove the `# ` line prefix. The token is
    /// kept; the save treats a changed type as a new cell, so code outputs
    /// are dropped. Undo takes two steps (delete, then insert).
    pub(crate) fn notebook_toggle_cell_type(&mut self) {
        let Some((lines, (row, col))) = self.notebook_lines_and_cursor() else {
            return;
        };
        let (Some(marker_row), end) = cell_rows(&lines, row) else {
            self.set_status("Cursor is not in a cell");
            return;
        };
        let Some(Marker { kind, token }) = parse_marker(&lines[marker_row]) else {
            self.set_status(format!("Malformed cell marker at line {}", marker_row + 1));
            return;
        };
        let to = if kind == CellKind::Code {
            CellKind::Markdown
        } else {
            CellKind::Code
        };
        // Leave the trailing blank line (the separator before the next
        // marker, or the line after the final newline) as it is.
        let mut body_end = end;
        if body_end > marker_row && lines[end].is_empty() {
            body_end -= 1;
        }
        let mut new_lines = vec![marker_line(to, token.as_deref())];
        for (i, line) in lines
            .iter()
            .enumerate()
            .take(body_end + 1)
            .skip(marker_row + 1)
        {
            let converted = match to {
                CellKind::Code if line == "#" => String::new(),
                CellKind::Code => line.strip_prefix("# ").unwrap_or(line).to_string(),
                _ if line.is_empty() => "#".to_string(),
                _ => format!("# {line}"),
            };
            // A code line `%%time` would become `# %%time`, a marker.
            if is_marker_like(&converted) {
                self.set_status(format!(
                    "Cannot toggle: line {} would become a cell marker",
                    i + 1
                ));
                return;
            }
            new_lines.push(converted);
        }
        let new_col = if row == marker_row {
            // The type tag is inserted or removed after the unchanged `# %%`
            // stem. Keep a cursor in that stem where it is, and keep a cursor
            // in the token suffix attached to the same suffix.
            let tag_width = |kind| match kind {
                CellKind::Code => 0,
                CellKind::Markdown => " [markdown]".len(),
                CellKind::Raw => " [raw]".len(),
            };
            let stem_width = "# %%".len();
            let old_tag_width = tag_width(kind);
            let new_tag_width = tag_width(to);
            if col < stem_width {
                col
            } else if col <= stem_width + old_tag_width {
                stem_width + new_tag_width
            } else {
                col - old_tag_width + new_tag_width
            }
        } else if (marker_row..=body_end).contains(&row) {
            let delta = new_lines[row - marker_row].chars().count() as isize
                - lines[row].chars().count() as isize;
            (col as isize + delta).max(0) as usize
        } else {
            col
        };
        if let Some(tab) = self.active_tab_mut() {
            let editor = &mut tab.editor;
            editor.cancel_selection();
            editor.move_cursor(CursorMove::Jump(to_u16_saturating(marker_row), 0));
            editor.start_selection();
            editor.move_cursor(CursorMove::Jump(
                to_u16_saturating(body_end),
                to_u16_saturating(lines[body_end].chars().count()),
            ));
            editor.insert_str(new_lines.join("\n"));
            editor.move_cursor(CursorMove::Jump(
                to_u16_saturating(row),
                to_u16_saturating(new_col),
            ));
        }
        self.on_editor_content_changed();
        self.sync_editor_scroll_guess();
        self.set_status(match to {
            CellKind::Code => "Cell is now code",
            _ => "Cell is now markdown",
        });
    }

    /// Clear every code cell's outputs and execution count on the next save.
    /// A reload drops the pending clear; a buffer undo does not.
    pub(crate) fn notebook_clear_outputs(&mut self) {
        let Some(doc) = self.active_tab_mut().and_then(|t| t.notebook.as_mut()) else {
            self.set_status(NOT_A_NOTEBOOK);
            return;
        };
        doc.set_clear_outputs(true);
        self.mark_dirty();
        self.set_status("Outputs will be cleared on save (reload to cancel)");
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use serde_json::Value;
    use tempfile::tempdir;

    use super::App;
    use crate::keybinds::KeyAction;
    use crate::notebook::{project, rebuild};
    use crate::types::CommandAction;
    use crate::util::text_to_lines;

    const EXECUTED: &str = include_str!("../notebook/testdata/lab45_executed.ipynb");

    /// Slice E stand-in for the notebook open path (slice B): open the file,
    /// then load its projection as the tab's notebook and text.
    fn open_notebook_for_cmds_test(app: &mut App, path: &Path) {
        app.open_file(path.to_path_buf()).expect("open");
        let doc = project(&fs::read_to_string(path).expect("read")).expect("project");
        let lines = text_to_lines(doc.projected_text());
        app.active_tab_mut().expect("tab").notebook = Some(doc);
        app.replace_editor_text(lines, (0, 0));
        app.active_tab_mut().expect("tab").dirty = false;
    }

    fn notebook_app(root: &Path) -> (App, PathBuf) {
        let path = root.join("nb.ipynb");
        fs::write(&path, EXECUTED).expect("write");
        let mut app = App::new(root.to_path_buf()).expect("app");
        open_notebook_for_cmds_test(&mut app, &path);
        (app, path)
    }

    fn lines(app: &App) -> Vec<String> {
        app.active_tab().expect("tab").editor.lines().to_vec()
    }

    fn cursor(app: &App) -> (usize, usize) {
        app.active_tab().expect("tab").editor.cursor()
    }

    fn marker_rows(app: &App) -> Vec<usize> {
        lines(app)
            .iter()
            .enumerate()
            .filter(|(_, l)| l.starts_with("# %%"))
            .map(|(i, _)| i)
            .collect()
    }

    fn rebuilt_cells(app: &App) -> Vec<Value> {
        let tab = app.active_tab().expect("tab");
        let text = tab.editor.lines().join("\n");
        let bytes = rebuild(tab.notebook.as_ref().expect("doc"), &text)
            .expect("rebuild")
            .bytes;
        let root: Value = serde_json::from_str(&bytes).expect("json");
        root["cells"].as_array().expect("cells").clone()
    }

    fn original_cells() -> Vec<Value> {
        let root: Value = serde_json::from_str(EXECUTED).expect("json");
        root["cells"].as_array().expect("cells").clone()
    }

    fn jump(app: &mut App, row: usize, col: usize) {
        app.active_tab_mut()
            .expect("tab")
            .editor
            .move_cursor(ratatui_textarea::CursorMove::Jump(row as u16, col as u16));
    }

    #[test]
    fn next_and_prev_cell_move_between_markers() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, _) = notebook_app(tmp.path());
        let m = marker_rows(&app);
        assert_eq!(m.len(), 7);

        app.notebook_next_cell();
        assert_eq!(cursor(&app), (m[1] + 1, 0));
        app.notebook_next_cell();
        assert_eq!(cursor(&app), (m[2] + 1, 0));

        // Mid-cell: previous goes to the top of the current cell first.
        jump(&mut app, m[2] + 2, 3);
        app.notebook_prev_cell();
        assert_eq!(cursor(&app), (m[2] + 1, 0));
        app.notebook_prev_cell();
        assert_eq!(cursor(&app), (m[1] + 1, 0));
        // On a marker line: previous goes to the cell above.
        jump(&mut app, m[1], 0);
        app.notebook_prev_cell();
        assert_eq!(cursor(&app), (m[0] + 1, 0));
        app.notebook_prev_cell();
        assert_eq!(app.status, "No previous cell");
        assert_eq!(cursor(&app), (m[0] + 1, 0));

        jump(&mut app, m[6], 0);
        app.notebook_next_cell();
        assert_eq!(app.status, "No next cell");
        assert!(!app.active_tab().expect("tab").dirty);
    }

    #[test]
    fn insert_cell_below_adds_a_marker_and_undo_removes_it() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, _) = notebook_app(tmp.path());
        let before = lines(&app);
        let m = marker_rows(&app);
        jump(&mut app, m[1] + 1, 2);

        app.notebook_insert_cell_below();
        let after = lines(&app);
        // Blank separator kept, new marker, cursor in the new cell's body.
        assert_eq!(after[m[2] - 1], "");
        assert_eq!(after[m[2]], "# %%");
        assert_eq!(after[m[2] + 1], "");
        assert_eq!(after[m[2] + 2], before[m[2]]);
        assert_eq!(cursor(&app), (m[2] + 1, 0));
        assert!(app.active_tab().expect("tab").dirty);

        // Every original cell keeps its JSON; the new cell is empty code.
        let cells = rebuilt_cells(&app);
        let orig = original_cells();
        assert_eq!(cells.len(), 8);
        assert_eq!(&cells[..2], &orig[..2]);
        assert_eq!(&cells[3..], &orig[2..]);
        assert_eq!(cells[2]["cell_type"], "code");
        assert_eq!(cells[2]["source"], Value::Array(vec![]));

        app.run_key_action(KeyAction::Undo).expect("undo");
        assert_eq!(lines(&app), before);
    }

    #[test]
    fn insert_cell_below_the_last_cell_keeps_its_text() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, _) = notebook_app(tmp.path());
        // Below the raw cell, then below the last (empty) cell, then below a
        // last cell whose last line is not empty.
        for cell in [5, 7] {
            let row = marker_rows(&app)[cell];
            jump(&mut app, row, 0);
            app.notebook_insert_cell_below();
        }
        app.active_tab_mut()
            .expect("tab")
            .editor
            .insert_str("x = 1");
        app.notebook_insert_cell_below();
        let cells = rebuilt_cells(&app);
        let orig = original_cells();
        assert_eq!(cells.len(), 10);
        assert_eq!(&cells[..6], &orig[..6]);
        assert_eq!(cells[7], orig[6]);
        assert_eq!(cells[8]["source"], Value::Array(vec!["x = 1".into()]));
        assert_eq!(cells[9]["source"], Value::Array(vec![]));
        let after = lines(&app);
        let last_marker = *marker_rows(&app).last().expect("marker");
        assert_eq!(after[last_marker - 2..last_marker], ["x = 1", ""]);
    }

    #[test]
    fn toggle_cell_type_rewrites_prefixes_and_save_drops_outputs() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, _) = notebook_app(tmp.path());
        let before = lines(&app);
        let m = marker_rows(&app);
        jump(&mut app, m[1] + 1, 5);

        app.notebook_toggle_cell_type();
        let after = lines(&app);
        assert!(after[m[1]].starts_with("# %% [markdown] #"));
        assert_eq!(after[m[1] + 1], "# print(\"hello\")");
        assert_eq!(after[m[1] + 2], "# print(\"world\")");
        assert_eq!(after[m[1] + 3], "", "separator stays blank");
        assert_eq!(cursor(&app), (m[1] + 1, 7));
        assert_eq!(app.status, "Cell is now markdown");

        let cells = rebuilt_cells(&app);
        let orig = original_cells();
        assert_eq!(cells.len(), 7);
        assert_eq!(cells[1]["cell_type"], "markdown");
        assert!(cells[1].get("outputs").is_none());
        assert_eq!(cells[1]["source"], orig[1]["source"]);
        assert_eq!(&cells[2..], &orig[2..]);

        // Markdown back to code restores the text; two undo steps also do.
        app.notebook_toggle_cell_type();
        assert_eq!(lines(&app), before);
        app.notebook_toggle_cell_type();
        app.run_key_action(KeyAction::Undo).expect("undo");
        app.run_key_action(KeyAction::Undo).expect("undo");
        assert_eq!(lines(&app), before);

        // The markdown cell becomes code: `#` lines become empty lines.
        jump(&mut app, m[0], 0);
        app.notebook_toggle_cell_type();
        let after = lines(&app);
        assert!(after[m[0]].starts_with("# %% #"));
        assert_eq!(after[1..4], ["# Executed", "", "Some *markdown* text."]);
    }

    #[test]
    fn toggle_refuses_a_line_that_would_become_a_marker() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, _) = notebook_app(tmp.path());
        let m = marker_rows(&app);
        jump(&mut app, m[1] + 1, 0);
        app.active_tab_mut()
            .expect("tab")
            .editor
            .insert_str("%%time\n");
        let before = lines(&app);
        app.notebook_toggle_cell_type();
        assert_eq!(
            app.status,
            format!(
                "Cannot toggle: line {} would become a cell marker",
                m[1] + 2
            )
        );
        assert_eq!(lines(&app), before);
    }

    #[test]
    fn toggle_cell_type_keeps_a_cursor_at_the_start_of_the_marker() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, _) = notebook_app(tmp.path());
        let marker = marker_rows(&app)[1];
        jump(&mut app, marker, 0);

        app.notebook_toggle_cell_type();

        assert_eq!(cursor(&app), (marker, 0));
    }

    #[test]
    fn clear_outputs_then_save_clears_code_cells_only() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, path) = notebook_app(tmp.path());
        app.run_command_action(CommandAction::NotebookClearOutputs)
            .expect("clear");
        assert_eq!(
            app.status,
            "Outputs will be cleared on save (reload to cancel)"
        );
        assert!(app.active_tab().expect("tab").dirty);

        let cells = rebuilt_cells(&app);
        let orig = original_cells();
        for (new, old) in cells.iter().zip(&orig) {
            if old["cell_type"] == "code" {
                assert_eq!(new["outputs"], Value::Array(vec![]));
                assert_eq!(new["execution_count"], Value::Null);
                assert_eq!(new["source"], old["source"]);
            } else {
                assert_eq!(new, old);
            }
        }

        // A reload (a fresh projection of the file) drops the pending clear.
        app.close_file();
        open_notebook_for_cmds_test(&mut app, &path);
        let tab = app.active_tab().expect("tab");
        let doc = tab.notebook.as_ref().expect("doc");
        assert!(!doc.clear_outputs_pending());
        let text = tab.editor.lines().join("\n");
        assert!(rebuild(doc, &text).expect("rebuild").unchanged);
    }

    #[test]
    fn palette_commands_on_a_raw_tab_say_not_a_notebook() {
        let tmp = tempdir().expect("tempdir");
        let file = tmp.path().join("a.rs");
        fs::write(&file, "fn main() {}\n# %%\nx\n").expect("write");
        let mut app = App::new(tmp.path().to_path_buf()).expect("app");
        app.open_file(file).expect("open");
        let before = lines(&app);
        for action in [
            CommandAction::NotebookNextCell,
            CommandAction::NotebookPrevCell,
            CommandAction::NotebookInsertCellBelow,
            CommandAction::NotebookToggleCellType,
            CommandAction::NotebookClearOutputs,
        ] {
            app.status.clear();
            app.run_command_action(action).expect("run");
            assert_eq!(app.status, "Not a notebook");
            assert_eq!(lines(&app), before);
            assert_eq!(cursor(&app), (0, 0));
            assert!(!app.active_tab().expect("tab").dirty);
        }
    }

    /// Guardrail 3: on a raw tab the notebook keys reach the editor exactly
    /// as they would with the actions unbound (today's behaviour).
    #[test]
    fn notebook_keys_fall_through_on_a_raw_tab() {
        let tmp = tempdir().expect("tempdir");
        let file = tmp.path().join("a.rs");
        fs::write(&file, "fn a() {}\n# %%\nfn b() {}\n").expect("write");
        let open = |unbind: bool| {
            let mut app = App::new(tmp.path().to_path_buf()).expect("app");
            if unbind {
                for action in [
                    KeyAction::NextCell,
                    KeyAction::PrevCell,
                    KeyAction::InsertCellBelow,
                ] {
                    app.keybinds.set(action, Vec::new());
                }
            }
            app.open_file(file.clone()).expect("open");
            jump(&mut app, 1, 2);
            app.status.clear();
            app
        };
        for code in [KeyCode::Down, KeyCode::Up, KeyCode::Char('c')] {
            let key = KeyEvent::new(code, KeyModifiers::ALT);
            let mut app = open(false);
            let mut reference = open(true);
            app.handle_editor_key(key).expect("key");
            reference.handle_editor_key(key).expect("key");
            assert_eq!(app.status, reference.status, "{code:?}");
            assert_eq!(app.status, "", "{code:?}");
            assert_eq!(lines(&app), lines(&reference), "{code:?}");
            assert_eq!(cursor(&app), cursor(&reference), "{code:?}");
            let dirty = |a: &App| a.active_tab().expect("tab").dirty;
            assert_eq!(dirty(&app), dirty(&reference), "{code:?}");
        }
    }

    #[test]
    fn notebook_keys_act_on_a_notebook_tab() {
        let tmp = tempdir().expect("tempdir");
        let (mut app, _) = notebook_app(tmp.path());
        let m = marker_rows(&app);
        app.handle_editor_key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT))
            .expect("key");
        assert_eq!(cursor(&app), (m[1] + 1, 0));
        app.handle_editor_key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT))
            .expect("key");
        assert_eq!(cursor(&app), (m[0] + 1, 0));
        app.handle_editor_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::ALT))
            .expect("key");
        assert_eq!(marker_rows(&app).len(), 8);
    }
}
