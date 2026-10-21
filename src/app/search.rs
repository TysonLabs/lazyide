use super::App;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::notebook;
use crate::tab::ProjectSearchHit;
use crate::util::{parse_rg_line, relative_path, to_u16_saturating};

/// Most notebooks one project search projects and searches; the rest are
/// skipped with a status note.
const NOTEBOOK_SEARCH_CAP: usize = 500;

impl App {
    pub(crate) fn search_in_open_file(&mut self, query: &str) {
        if self.open_path().is_none() {
            self.set_status("Open a file first");
            return;
        }
        if query.trim().is_empty() {
            if let Some(tab) = self.active_tab_mut() {
                let _ = tab.editor.set_search_pattern("");
            }
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

    pub(crate) fn replace_in_open_file(&mut self, search: &str, replacement: &str) {
        if self.open_path().is_none() {
            self.set_status("Open a file first");
            return;
        }
        if search.is_empty() {
            self.set_status("Search pattern cannot be empty");
            return;
        }
        let mut lines = self.tabs[self.active_tab].editor.lines().to_vec();
        let mut count = 0usize;
        for line in &mut lines {
            let occurrences = line.matches(search).count();
            if occurrences > 0 {
                *line = line.replace(search, replacement);
                count += occurrences;
            }
        }
        if count > 0 {
            let cursor = self.tabs[self.active_tab].editor.cursor();
            self.replace_editor_text(lines, cursor);
            self.on_editor_content_changed();
            self.set_status(format!("Replaced {} occurrence(s)", count));
        } else {
            self.set_status(format!("No occurrences of '{}' found", search));
        }
    }

    pub(crate) fn search_in_project(&mut self, query: &str) {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            self.set_status("Search query is empty");
            return;
        }
        let Ok(search) = run_project_search(&self.root, trimmed) else {
            self.set_status(
                "rg (ripgrep) not found -- install: https://github.com/BurntSushi/ripgrep#installation",
            );
            return;
        };
        self.search_results.query = trimmed.to_string();
        self.search_results.results = search.hits;
        self.search_results.index = 0;
        self.search_results.open = true;
        let mut status = if self.search_results.results.is_empty() {
            format!("No results for '{}'", trimmed)
        } else {
            format!(
                "{} results for '{}'",
                self.search_results.results.len(),
                trimmed
            )
        };
        if search.skipped_notebooks > 0 {
            status.push_str(&format!(
                " ({} notebooks searched, {} skipped)",
                NOTEBOOK_SEARCH_CAP, search.skipped_notebooks
            ));
        }
        if let Some(err) = search.errors.first() {
            status.push_str(&format!(" -- rg error: {}", err));
        }
        self.set_status(status);
    }

    pub(crate) fn open_selected_search_result(&mut self) -> io::Result<()> {
        let Some(hit) = self
            .search_results
            .results
            .get(self.search_results.index)
            .cloned()
        else {
            return Ok(());
        };
        self.open_file(hit.path.clone())?;
        let target_row = hit.line.saturating_sub(1);
        if let Some(tab) = self.active_tab_mut() {
            tab.editor.move_cursor(ratatui_textarea::CursorMove::Jump(
                to_u16_saturating(target_row),
                0,
            ));
        }
        self.sync_editor_scroll_guess();
        self.search_results.open = false;
        self.set_status(format!(
            "Opened {}:{}",
            relative_path(&self.root, &hit.path).display(),
            hit.line
        ));
        Ok(())
    }
}

/// How an rg run ended: exit 0 = match, 1 = no match, anything else (2, or
/// killed by a signal) = error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RgExit {
    Matched,
    NoMatch,
    Error,
}

fn rg_exit_kind(code: Option<i32>) -> RgExit {
    match code {
        Some(0) => RgExit::Matched,
        Some(1) => RgExit::NoMatch,
        _ => RgExit::Error,
    }
}

struct RgOutput {
    stdout: String,
    exit: RgExit,
    /// Last non-empty stderr line, kept for the status bar on `RgExit::Error`.
    error: String,
}

#[derive(Debug, Default)]
struct ProjectSearch {
    hits: Vec<ProjectSearchHit>,
    /// One message per rg run that exited with an error or failed to start.
    errors: Vec<String>,
    /// Notebooks past `NOTEBOOK_SEARCH_CAP` that were not searched.
    skipped_notebooks: usize,
}

/// Arguments of the project search rg run. With `skip_notebooks == false` and
/// `paths == [root]` this is exactly the command used before notebook support.
fn rg_search_args(query: &str, paths: &[&Path], skip_notebooks: bool) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["--line-number", "--no-heading", "--color", "never"]
        .iter()
        .map(OsString::from)
        .collect();
    args.push("--smart-case".into());
    if skip_notebooks {
        args.push("--glob".into());
        args.push("!*.ipynb".into());
    }
    args.push(query.into());
    args.extend(paths.iter().map(|p| p.as_os_str().to_os_string()));
    args
}

/// Lists notebooks under `root` with the same ignore rules as the search.
fn rg_list_notebooks_args(root: &Path) -> Vec<OsString> {
    vec![
        "--files".into(),
        "--glob".into(),
        "*.ipynb".into(),
        root.as_os_str().to_os_string(),
    ]
}

/// Searches projected notebook text piped on stdin; output is `line:text`.
fn rg_stdin_args(query: &str) -> Vec<OsString> {
    [
        "--text",
        "--line-number",
        "--no-heading",
        "--color",
        "never",
        "--smart-case",
        "--no-filename",
        query,
        "-",
    ]
    .iter()
    .map(OsString::from)
    .collect()
}

/// Blanks generated notebook marker lines while preserving every projected
/// line number. Markers and their identity tokens are editor scaffolding, not
/// cell text, so they must not produce project-search hits.
fn notebook_search_text(projected: &str) -> String {
    let mut searchable = String::with_capacity(projected.len());
    for line in projected.split_inclusive('\n') {
        if line.starts_with("# %%") {
            searchable.push('\n');
        } else {
            searchable.push_str(line);
        }
    }
    searchable
}

/// Runs rg, feeding `stdin` from a separate thread so a large input cannot
/// deadlock against rg filling its stdout pipe.
fn run_rg(args: &[OsString], stdin: Option<&str>) -> io::Result<RgOutput> {
    let mut cmd = Command::new("rg");
    cmd.args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let mut child = cmd.spawn()?;
    let output = std::thread::scope(|s| {
        if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
            // A write error means rg exited early; its exit code reports why.
            s.spawn(move || {
                let _ = pipe.write_all(text.as_bytes());
            });
        }
        child.wait_with_output()
    })?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok(RgOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        exit: rg_exit_kind(output.status.code()),
        error: stderr
            .lines()
            .rev()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("rg failed")
            .to_string(),
    })
}

impl ProjectSearch {
    /// Runs one rg invocation, recording an error exit or spawn failure.
    fn run(&mut self, args: &[OsString], stdin: Option<&str>) -> Option<String> {
        match run_rg(args, stdin) {
            Ok(out) => {
                if out.exit == RgExit::Error {
                    self.errors.push(out.error);
                }
                Some(out.stdout)
            }
            Err(err) => {
                self.errors.push(format!("could not run rg: {}", err));
                None
            }
        }
    }

    fn push_rg_hits(&mut self, stdout: &str) {
        self.hits.extend(stdout.lines().filter_map(parse_rg_line));
    }
}

/// Project search. Without notebooks under `root` this runs exactly the
/// pre-notebook rg command. Otherwise notebooks are excluded from that run and
/// each one is searched by its projected cell text (outputs and metadata are
/// never searched), so hits carry projected line numbers; a notebook that fails
/// to project is searched raw. Errs only when rg cannot be started.
fn run_project_search(root: &Path, query: &str) -> io::Result<ProjectSearch> {
    run_project_search_capped(root, query, NOTEBOOK_SEARCH_CAP)
}

fn run_project_search_capped(root: &Path, query: &str, cap: usize) -> io::Result<ProjectSearch> {
    // Listing errors are not recorded: the main search below reports them.
    let listing = run_rg(&rg_list_notebooks_args(root), None)?;
    let mut notebooks: Vec<PathBuf> = listing
        .stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect();
    let mut search = ProjectSearch::default();
    if notebooks.is_empty() {
        if let Some(stdout) = search.run(&rg_search_args(query, &[root], false), None) {
            search.push_rg_hits(&stdout);
        }
        return Ok(search);
    }
    notebooks.sort();
    notebooks.dedup();
    search.skipped_notebooks = notebooks.len().saturating_sub(cap);
    notebooks.truncate(cap);
    if let Some(stdout) = search.run(&rg_search_args(query, &[root], true), None) {
        search.push_rg_hits(&stdout);
    }
    let stdin_args = rg_stdin_args(query);
    let mut failed: Vec<&Path> = Vec::new();
    for path in &notebooks {
        let doc = fs::read_to_string(path)
            .ok()
            .and_then(|text| notebook::project(&text).ok());
        let Some(doc) = doc else {
            failed.push(path);
            continue;
        };
        let searchable = notebook_search_text(doc.projected_text());
        let errors_before = search.errors.len();
        let Some(stdout) = search.run(&stdin_args, Some(&searchable)) else {
            break;
        };
        if search.errors.len() > errors_before {
            // stdin has no I/O to fail, so this is the query (e.g. a bad regex):
            // every other notebook would fail the same way.
            break;
        }
        search.hits.extend(stdout.lines().filter_map(|l| {
            let (line, preview) = l.split_once(':')?;
            Some(ProjectSearchHit {
                path: path.clone(),
                line: line.parse().ok()?,
                preview: preview.to_string(),
            })
        }));
    }
    if !failed.is_empty() {
        // Explicit file paths: force `path:` prefixes even for a single file.
        let mut args = rg_search_args(query, &failed, false);
        args.insert(0, "--with-filename".into());
        if let Some(stdout) = search.run(&args, None) {
            search.push_rg_hits(&stdout);
        }
    }
    search
        .hits
        .sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
    Ok(search)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn new_app(root: &Path) -> App {
        App::new(root.to_path_buf()).expect("app should initialize")
    }

    fn have_rg() -> bool {
        let ok = Command::new("rg").arg("--version").output().is_ok();
        if !ok {
            eprintln!("skipping: rg (ripgrep) not installed");
        }
        ok
    }

    /// A v4.4 notebook: a code cell (with `needle` in its output and metadata
    /// only), a markdown cell whose second line holds `needle`, and a last
    /// code cell (so a jump clamped to the end cannot pass by accident).
    fn notebook_json() -> &'static str {
        r##"{
 "cells": [
  {
   "cell_type": "code",
   "execution_count": 1,
   "metadata": {"tag": "needle_meta"},
   "outputs": [{"name": "stdout", "output_type": "stream", "text": ["needle_out\n"]}],
   "source": ["import os\n", "x = 1"]
  },
  {
   "cell_type": "markdown",
   "metadata": {},
   "source": ["# Title\n", "the needle_cell line"]
  },
  {
   "cell_type": "code",
   "execution_count": null,
   "metadata": {},
   "outputs": [],
   "source": ["y = 2"]
  }
 ],
 "metadata": {"needle_root": true},
 "nbformat": 4,
 "nbformat_minor": 4
}
"##
    }

    /// 1-based projected line holding `needle` in `nb`'s projection.
    fn projected_line(nb: &str, needle: &str) -> usize {
        let doc = notebook::project(nb).expect("project");
        doc.projected_text()
            .lines()
            .position(|l| l.contains(needle))
            .expect("needle in projection")
            + 1
    }

    /// Slice-D test helper until the real notebook open path lands: open the
    /// file, then make the tab a projected notebook tab.
    fn open_notebook_for_search_test(app: &mut App, path: &Path) {
        app.open_file(path.to_path_buf()).expect("open");
        let doc = notebook::project(&fs::read_to_string(path).expect("read")).expect("project");
        let lines: Vec<String> = doc.projected_text().lines().map(String::from).collect();
        let idx = app.active_tab;
        app.tabs[idx].notebook = Some(doc);
        app.replace_editor_text(lines, (0, 0));
    }

    /// Guardrail 5: with no notebooks the search command is exactly the
    /// pre-notebook one.
    #[test]
    fn no_notebook_search_args_match_pre_notebook_command() {
        let root = Path::new("/proj");
        let want: Vec<OsString> = [
            "--line-number",
            "--no-heading",
            "--color",
            "never",
            "--smart-case",
            "foo",
            "/proj",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        assert_eq!(rg_search_args("foo", &[root], false), want);
        let with_skip = rg_search_args("foo", &[root], true);
        assert_eq!(with_skip.len(), want.len() + 2);
        assert_eq!(
            with_skip[5..7],
            [OsString::from("--glob"), "!*.ipynb".into()]
        );
    }

    #[test]
    fn rg_exit_codes_are_classified() {
        assert_eq!(rg_exit_kind(Some(0)), RgExit::Matched);
        assert_eq!(rg_exit_kind(Some(1)), RgExit::NoMatch);
        assert_eq!(rg_exit_kind(Some(2)), RgExit::Error);
        assert_eq!(rg_exit_kind(None), RgExit::Error);
    }

    #[test]
    fn notebook_hits_use_projected_lines_and_skip_outputs_and_metadata() {
        if !have_rg() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let nb = tmp.path().join("a.ipynb");
        fs::write(&nb, notebook_json()).expect("write");
        fs::write(tmp.path().join("b.txt"), "needle_txt\n").expect("write");
        let mut app = new_app(tmp.path());

        app.search_in_project("needle");
        let hits = &app.search_results.results;
        assert_eq!(hits.len(), 2, "cell text + b.txt only: {:?}", hits);
        assert_eq!(hits[0].path, nb);
        assert_eq!(hits[0].line, projected_line(notebook_json(), "needle_cell"));
        assert_eq!(hits[0].preview, "# the needle_cell line");
        assert_eq!(hits[1].path, tmp.path().join("b.txt"));
        assert_eq!(app.status, "2 results for 'needle'");

        app.search_in_project("needle_out");
        assert!(app.search_results.results.is_empty());
        app.search_in_project("needle_meta");
        assert!(app.search_results.results.is_empty());
    }

    #[test]
    fn notebook_search_does_not_match_generated_cell_markers() {
        if !have_rg() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        fs::write(tmp.path().join("a.ipynb"), notebook_json()).expect("write");

        let search = run_project_search(tmp.path(), "markdown").expect("rg");

        assert!(
            search.hits.is_empty(),
            "generated marker matched: {:?}",
            search.hits
        );
    }

    #[test]
    fn notebook_search_treats_nul_in_cell_source_as_text() {
        if !have_rg() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let nb = tmp.path().join("a.ipynb");
        let json = notebook_json().replace("the needle_cell line", "before\\u0000needle_nul");
        fs::write(&nb, json).expect("write");

        let search = run_project_search(tmp.path(), "needle_nul").expect("rg");

        assert_eq!(search.hits.len(), 1, "{:?}", search.hits);
        assert_eq!(search.hits[0].path, nb);
    }

    #[test]
    fn notebook_hit_jumps_to_projected_line() {
        if !have_rg() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let nb = tmp.path().join("a.ipynb");
        fs::write(&nb, notebook_json()).expect("write");
        let mut app = new_app(tmp.path());
        open_notebook_for_search_test(&mut app, &nb);

        app.search_in_project("needle_cell");
        app.open_selected_search_result().expect("jump");
        let tab = app.active_tab().expect("tab");
        assert!(tab.is_notebook());
        let row = tab.editor.cursor().0;
        assert_eq!(tab.editor.lines()[row], "# the needle_cell line");
    }

    #[test]
    fn notebook_that_fails_to_project_is_searched_raw() {
        if !have_rg() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let bad = tmp.path().join("bad.ipynb");
        fs::write(&bad, "{\n \"nbformat\": 3,\n \"needle\": 1\n}\n").expect("write");
        let good = tmp.path().join("good.ipynb");
        fs::write(&good, notebook_json()).expect("write");
        let mut app = new_app(tmp.path());

        app.search_in_project("needle");
        let hits = &app.search_results.results;
        assert_eq!(hits.len(), 2, "{:?}", hits);
        assert_eq!((hits[0].path.as_path(), hits[0].line), (bad.as_path(), 3));
        assert_eq!(hits[0].preview, " \"needle\": 1");
        assert_eq!(hits[1].path, good);
    }

    #[test]
    fn notebook_cap_skips_the_rest_with_a_count() {
        if !have_rg() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        for name in ["a.ipynb", "b.ipynb", "c.ipynb"] {
            fs::write(tmp.path().join(name), notebook_json()).expect("write");
        }
        let search = run_project_search_capped(tmp.path(), "needle_cell", 2).expect("rg");
        assert_eq!(search.skipped_notebooks, 1);
        let paths: Vec<_> = search.hits.iter().map(|h| h.path.clone()).collect();
        assert_eq!(
            paths,
            vec![tmp.path().join("a.ipynb"), tmp.path().join("b.ipynb")]
        );
        assert!(search.errors.is_empty());
    }

    #[test]
    fn rg_errors_are_reported_not_shown_as_no_match() {
        if !have_rg() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        fs::write(tmp.path().join("a.txt"), "x\n").expect("write");
        let mut app = new_app(tmp.path());
        app.search_in_project("zzz_absent");
        assert_eq!(app.status, "No results for 'zzz_absent'");
        app.search_in_project("(");
        assert!(app.status.starts_with("No results for '(' -- rg error: "));

        fs::write(tmp.path().join("n.ipynb"), notebook_json()).expect("write");
        let search = run_project_search(tmp.path(), "(").expect("rg");
        // Main run errors, the first notebook run errors, then the loop stops.
        assert_eq!(search.errors.len(), 2, "{:?}", search.errors);
        let search = run_project_search(tmp.path(), "zzz_absent").expect("rg");
        assert!(search.errors.is_empty() && search.hits.is_empty());
    }
}
