//! Run notebook / Stop run: execute a whole notebook with
//! `jupyter nbconvert --execute` in a child process, then apply the result.
//! Design: `docs/plans/jupyter-notebooks.md` (*Run notebook*).

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use super::App;
use crate::notebook::{digest, project};
use crate::util::relative_path;

/// One active `nbconvert` run. It lives on `App`, not on the tab, so a run
/// still applies and reports after its tab closes.
pub(crate) struct NotebookRun {
    pub(crate) path: PathBuf,
    tmp: PathBuf,
    start_digest: u64,
    started: Instant,
    child: Child,
    stderr_rx: Receiver<String>,
    stopped: bool,
}

/// `.<stem>.lazyide-run.ipynb` next to the notebook, so the final rename
/// stays on one filesystem.
pub(crate) fn run_tmp_path(path: &Path) -> PathBuf {
    path.with_file_name(format!(".{}.lazyide-run.ipynb", file_stem(path)))
}

/// `<stem>.executed.ipynb`: where a result goes when the notebook changed
/// during the run.
pub(crate) fn executed_path(path: &Path) -> PathBuf {
    path.with_file_name(format!("{}.executed.ipynb", file_stem(path)))
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// The nbconvert command. `program` is `["jupyter"]` in the app; tests pass
/// `["sh", <fake script>]`. The cwd is the notebook's directory, and
/// `--output` is a file name, so nbconvert writes `<tmp>` next to the notebook.
pub(crate) fn nbconvert_command(program: &[String], path: &Path, tmp: &Path) -> Command {
    let (exe, lead) = program
        .split_first()
        .map_or(("jupyter", &[][..]), |(e, l)| (e.as_str(), l));
    let mut cmd = Command::new(exe);
    cmd.args(lead)
        .arg("nbconvert")
        .arg("--to")
        .arg("notebook")
        .arg("--execute")
        .arg("--allow-errors")
        .arg("--output")
        .arg(tmp.file_name().unwrap_or_default())
        .arg(path.file_name().unwrap_or_default());
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        cmd.current_dir(dir);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    cmd
}

/// The most useful stderr line: the last non-empty one (nbconvert logs
/// `[NbConvertApp] Converting ...` first and the exception last).
fn stderr_summary(stderr: &str) -> Option<&str> {
    stderr.lines().map(str::trim).rfind(|l| !l.is_empty())
}

impl App {
    pub(crate) fn run_notebook(&mut self) {
        let Some(tab) = self.active_tab() else {
            self.set_status("Not a notebook");
            return;
        };
        if tab.notebook.is_none() {
            self.set_status("Not a notebook");
            return;
        }
        if tab.dirty {
            self.set_status("Save before running");
            return;
        }
        let path = tab.path.clone();
        let name = relative_path(&self.root, &path).display().to_string();
        if self.notebook_runs.iter().any(|r| r.path == path) {
            self.set_status(format!("{name} is already running"));
            return;
        }
        let start_digest = match fs::read(&path) {
            Ok(bytes) => digest(&bytes),
            Err(e) => {
                self.set_status(format!("Run failed: {e}"));
                return;
            }
        };
        let tmp = run_tmp_path(&path);
        let mut child = match nbconvert_command(&self.jupyter_cmd, &path, &tmp).spawn() {
            Ok(child) => child,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                self.set_status("jupyter not found -- install: pip install nbconvert ipykernel");
                return;
            }
            Err(e) => {
                self.set_status(format!("Run failed: {e}"));
                return;
            }
        };
        // Drain stderr on a thread so a chatty run never blocks on a full pipe.
        let (tx, stderr_rx) = mpsc::channel();
        if let Some(mut stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = stderr.read_to_end(&mut buf);
                let _ = tx.send(String::from_utf8_lossy(&buf).to_string());
            });
        }
        self.notebook_runs.push(NotebookRun {
            path,
            tmp,
            start_digest,
            started: Instant::now(),
            child,
            stderr_rx,
            stopped: false,
        });
        self.set_status(format!(
            "Running {name}\u{2026} (Notebook: Stop run to cancel)"
        ));
    }

    /// Stop the active tab's run, or the only run when the active tab has none
    /// (for example after its tab was closed).
    pub(crate) fn stop_notebook_run(&mut self) {
        let active = self.open_path().cloned();
        let idx = self
            .notebook_runs
            .iter()
            .position(|r| Some(&r.path) == active.as_ref())
            .or_else(|| (self.notebook_runs.len() == 1).then_some(0));
        let Some(idx) = idx else {
            self.set_status("No notebook run to stop");
            return;
        };
        let run = &mut self.notebook_runs[idx];
        run.stopped = true;
        let _ = run.child.kill();
    }

    /// Event-loop poll: classify and apply every run whose process exited.
    pub(crate) fn poll_notebook_runs(&mut self) {
        let mut i = 0;
        while i < self.notebook_runs.len() {
            let status = match self.notebook_runs[i].child.try_wait() {
                Ok(Some(status)) => Ok(status),
                Ok(None) => {
                    i += 1;
                    continue;
                }
                Err(e) => Err(e),
            };
            let run = self.notebook_runs.remove(i);
            let msg = self.finish_notebook_run(&run, status);
            self.set_status(msg);
        }
    }

    fn finish_notebook_run(
        &self,
        run: &NotebookRun,
        status: io::Result<std::process::ExitStatus>,
    ) -> String {
        let name = relative_path(&self.root, &run.path).display().to_string();
        if run.stopped {
            let _ = fs::remove_file(&run.tmp);
            return "Run stopped".to_string();
        }
        let status = match status {
            Ok(s) => s,
            Err(e) => {
                let _ = fs::remove_file(&run.tmp);
                return format!("Run of {name} failed: {e}");
            }
        };
        if !status.success() {
            let _ = fs::remove_file(&run.tmp);
            // The pipe closes when the process exits; wait briefly for it.
            let stderr = run
                .stderr_rx
                .recv_timeout(Duration::from_millis(500))
                .unwrap_or_default();
            let reason = stderr_summary(&stderr)
                .map(str::to_string)
                .unwrap_or_else(|| status.to_string());
            return format!("Run of {name} failed: {reason}");
        }
        let Ok(result) = fs::read(&run.tmp) else {
            return format!("Run of {name} failed: nbconvert wrote no output");
        };
        let disk_digest = fs::read(&run.path).ok().map(|b| digest(&b));
        if disk_digest != Some(run.start_digest) {
            // The notebook changed (or vanished) during the run: never
            // overwrite it; keep the result next to it.
            let dest = executed_path(&run.path);
            let dest_name = relative_path(&self.root, &dest).display().to_string();
            return match fs::rename(&run.tmp, &dest) {
                Ok(()) => {
                    format!("Notebook changed during run; results saved to {dest_name}")
                }
                Err(e) => {
                    let _ = fs::remove_file(&run.tmp);
                    format!("Notebook changed during run; saving {dest_name} failed: {e}")
                }
            };
        }
        if let Err(e) = fs::rename(&run.tmp, &run.path) {
            let _ = fs::remove_file(&run.tmp);
            return format!("Run of {name} finished, but writing it failed: {e}");
        }
        // Report from the written file's outputs, never from the exit code.
        let errors = std::str::from_utf8(&result)
            .ok()
            .and_then(|text| project(text).ok())
            .map(|doc| doc.error_cells())
            .unwrap_or_default();
        match errors.first() {
            Some((cell, ename)) => format!("Run finished: cell {} raised {ename}", cell + 1),
            None => format!("Run finished in {}s", run.started.elapsed().as_secs()),
        }
    }

    /// On quit: kill active runs and remove their temp files.
    pub(crate) fn kill_notebook_runs(&mut self) {
        for run in &mut self.notebook_runs {
            let _ = run.child.kill();
            let _ = run.child.wait();
            let _ = fs::remove_file(&run.tmp);
        }
        self.notebook_runs.clear();
    }

    /// Keep active runs aligned with a file-tree rename or move of the
    /// notebook or one of its parent directories (nbconvert writes into its
    /// cwd, which moves with the directory).
    pub(crate) fn retarget_notebook_runs(&mut self, from: &Path, to: &Path) {
        let retarget = |path: &Path| match path.strip_prefix(from) {
            Ok(rest) if rest.as_os_str().is_empty() => to.to_path_buf(),
            Ok(rest) => to.join(rest),
            Err(_) => path.to_path_buf(),
        };
        for run in &mut self.notebook_runs {
            run.path = retarget(&run.path);
            run.tmp = retarget(&run.tmp);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::types::PromptMode;
    use std::thread::sleep;
    use tempfile::tempdir;

    const SOURCE_NB: &str = r#"{
 "cells": [
  {
   "cell_type": "code",
   "execution_count": null,
   "id": "c1",
   "metadata": {},
   "outputs": [],
   "source": ["print(1)"]
  },
  {
   "cell_type": "code",
   "execution_count": null,
   "id": "c2",
   "metadata": {},
   "outputs": [],
   "source": ["d = {}\n", "d['k']"]
  }
 ],
 "metadata": {},
 "nbformat": 4,
 "nbformat_minor": 5
}
"#;

    const EXECUTED_NB: &str = r#"{
 "cells": [
  {
   "cell_type": "code",
   "execution_count": 1,
   "id": "c1",
   "metadata": {},
   "outputs": [{"name": "stdout", "output_type": "stream", "text": ["1\n"]}],
   "source": ["print(1)"]
  },
  {
   "cell_type": "code",
   "execution_count": 2,
   "id": "c2",
   "metadata": {},
   "outputs": [],
   "source": ["d = {}\n", "d['k']"]
  }
 ],
 "metadata": {},
 "nbformat": 4,
 "nbformat_minor": 5
}
"#;

    /// A temp project with `nb.ipynb` open as a notebook, a fake `jupyter`
    /// script with `body` (args: `$7` = tmp name, `$8` = notebook name, both
    /// relative to the cwd), and `executed.json` / `errored.json` to copy.
    fn setup(body: &str) -> (tempfile::TempDir, App, PathBuf) {
        let tmp = tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let nb = root.join("nb.ipynb");
        fs::write(&nb, SOURCE_NB).unwrap();
        fs::write(root.join("executed.json"), EXECUTED_NB).unwrap();
        let errored = EXECUTED_NB.replace(
            r#""outputs": [],
   "source": ["d = {}\n""#,
            r#""outputs": [{"ename": "KeyError", "evalue": "'k'", "output_type": "error", "traceback": []}],
   "source": ["d = {}\n""#,
        );
        assert_ne!(errored, EXECUTED_NB);
        fs::write(root.join("errored.json"), errored).unwrap();
        let script = root.join("fake_jupyter.sh");
        fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut app = App::new(root.clone()).unwrap();
        app.jupyter_cmd = vec!["sh".to_string(), script.display().to_string()];
        app.open_file(nb.clone()).unwrap();
        assert!(app.tabs[0].is_notebook());
        (tmp, app, nb)
    }

    fn wait_for_runs(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !app.notebook_runs.is_empty() {
            assert!(Instant::now() < deadline, "run did not finish");
            app.poll_notebook_runs();
            sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn nbconvert_command_args_and_cwd() {
        let cmd = nbconvert_command(
            &["jupyter".to_string()],
            Path::new("/p/dir/a.ipynb"),
            Path::new("/p/dir/.a.lazyide-run.ipynb"),
        );
        assert_eq!(cmd.get_program(), "jupyter");
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy()).collect();
        assert_eq!(
            args,
            [
                "nbconvert",
                "--to",
                "notebook",
                "--execute",
                "--allow-errors",
                "--output",
                ".a.lazyide-run.ipynb",
                "a.ipynb"
            ]
        );
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/p/dir")));
        assert_eq!(
            run_tmp_path(Path::new("/p/a.ipynb")),
            Path::new("/p/.a.lazyide-run.ipynb")
        );
        assert_eq!(
            executed_path(Path::new("/p/a.ipynb")),
            Path::new("/p/a.executed.ipynb")
        );
    }

    #[test]
    fn run_success_replaces_notebook_with_outputs() {
        let (_t, mut app, nb) = setup(r#"cp executed.json "$7""#);
        app.run_command_action(crate::types::CommandAction::NotebookRunAll)
            .unwrap();
        assert!(app.status.starts_with("Running nb.ipynb"), "{}", app.status);
        wait_for_runs(&mut app);
        assert!(app.status.starts_with("Run finished in "), "{}", app.status);
        assert_eq!(fs::read_to_string(&nb).unwrap(), EXECUTED_NB);
        assert!(!run_tmp_path(&nb).exists());
        let doc = project(EXECUTED_NB).unwrap();
        let token = &doc.tokens()[0];
        assert_eq!(doc.cell_summary(token).unwrap().execution_count, Some(1));
    }

    /// The run result reaches the open pristine tab through the watcher
    /// rules: the doc takes the new bytes, the buffer and tokens stay.
    #[test]
    fn run_success_reloads_pristine_tab_through_watcher() {
        let (_t, mut app, _nb) = setup(r#"cp executed.json "$7""#);
        let before = app.tabs[0].editor.lines().join("\n");
        app.run_command_action(crate::types::CommandAction::NotebookRunAll)
            .unwrap();
        wait_for_runs(&mut app);
        app.reload_open_file_from_disk_if_pristine().unwrap();
        let tab = &app.tabs[0];
        assert!(tab.is_notebook());
        assert!(!tab.dirty);
        assert_eq!(tab.editor.lines().join("\n"), before);
        let doc = tab.notebook.as_ref().unwrap();
        assert_eq!(doc.disk_digest(), digest(EXECUTED_NB.as_bytes()));
        assert_eq!(app.status, "Notebook outputs updated on disk");
    }

    #[test]
    fn run_status_reports_error_cell_from_written_file() {
        let (_t, mut app, nb) = setup(r#"cp errored.json "$7""#);
        app.run_notebook();
        wait_for_runs(&mut app);
        assert_eq!(app.status, "Run finished: cell 2 raised KeyError");
        assert!(fs::read_to_string(&nb).unwrap().contains("KeyError"));
    }

    #[test]
    fn run_nonzero_exit_leaves_notebook_and_removes_tmp() {
        let (_t, mut app, nb) = setup(
            r#"cp executed.json "$7"
echo "[NbConvertApp] Converting notebook $8 to notebook" >&2
echo "jupyter_client.kernelspec.NoSuchKernel: No such kernel named rust" >&2
exit 1"#,
        );
        app.run_notebook();
        wait_for_runs(&mut app);
        assert_eq!(
            app.status,
            "Run of nb.ipynb failed: jupyter_client.kernelspec.NoSuchKernel: No such kernel named rust"
        );
        assert_eq!(fs::read_to_string(&nb).unwrap(), SOURCE_NB);
        assert!(!run_tmp_path(&nb).exists());
    }

    #[test]
    fn run_not_found_shows_install_hint() {
        let (_t, mut app, nb) = setup("exit 0");
        app.jupyter_cmd = vec!["/nonexistent/lazyide-test/jupyter".to_string()];
        app.run_notebook();
        assert!(app.notebook_runs.is_empty());
        assert_eq!(
            app.status,
            "jupyter not found -- install: pip install nbconvert ipykernel"
        );
        assert_eq!(fs::read_to_string(&nb).unwrap(), SOURCE_NB);
    }

    #[test]
    fn run_disk_change_keeps_result_as_executed_copy() {
        let (_t, mut app, nb) = setup(
            r#"echo " " >> "$8"
cp executed.json "$7""#,
        );
        app.run_notebook();
        wait_for_runs(&mut app);
        assert_eq!(
            app.status,
            "Notebook changed during run; results saved to nb.executed.ipynb"
        );
        assert_eq!(fs::read_to_string(&nb).unwrap(), format!("{SOURCE_NB} \n"));
        assert_eq!(fs::read_to_string(executed_path(&nb)).unwrap(), EXECUTED_NB);
        assert!(!run_tmp_path(&nb).exists());
    }

    #[test]
    fn stop_kills_run_and_removes_tmp() {
        let (_t, mut app, nb) = setup(
            r#"cp executed.json "$7"
exec sleep 30"#,
        );
        app.run_notebook();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !run_tmp_path(&nb).exists() {
            assert!(Instant::now() < deadline, "fake run never wrote tmp");
            sleep(Duration::from_millis(20));
        }
        app.run_command_action(crate::types::CommandAction::NotebookStopRun)
            .unwrap();
        wait_for_runs(&mut app);
        assert_eq!(app.status, "Run stopped");
        assert!(!run_tmp_path(&nb).exists());
        assert_eq!(fs::read_to_string(&nb).unwrap(), SOURCE_NB);
    }

    #[test]
    fn run_refuses_dirty_tab_active_run_and_raw_tab() {
        let (_t, mut app, nb) = setup("exec sleep 30");
        app.active_tab_mut().unwrap().dirty = true;
        app.run_notebook();
        assert_eq!(app.status, "Save before running");
        assert!(app.notebook_runs.is_empty());

        app.active_tab_mut().unwrap().dirty = false;
        app.run_notebook();
        assert_eq!(app.notebook_runs.len(), 1);
        app.run_notebook();
        assert_eq!(app.status, "nb.ipynb is already running");
        assert_eq!(app.notebook_runs.len(), 1);
        app.stop_notebook_run();
        wait_for_runs(&mut app);

        app.active_tab_mut().unwrap().notebook = None;
        app.run_notebook();
        assert_eq!(app.status, "Not a notebook");
        assert!(app.notebook_runs.is_empty());
        assert_eq!(fs::read_to_string(&nb).unwrap(), SOURCE_NB);
    }

    #[test]
    fn run_applies_after_tab_closed() {
        let (_t, mut app, nb) = setup(
            r#"sleep 0.3
cp executed.json "$7""#,
        );
        app.run_notebook();
        app.close_file();
        assert!(app.active_tab().is_none());
        wait_for_runs(&mut app);
        assert!(app.status.starts_with("Run finished in "), "{}", app.status);
        assert_eq!(fs::read_to_string(&nb).unwrap(), EXECUTED_NB);
    }

    #[test]
    fn run_follows_parent_directory_rename() {
        let (_t, mut app, nb) = setup(
            r#"echo ready > ../run.ready
sleep 0.3
cp ../executed.json "$7""#,
        );
        let root = nb.parent().unwrap().to_path_buf();
        let old_dir = root.join("old");
        fs::create_dir(&old_dir).unwrap();
        let old_nb = old_dir.join("nb.ipynb");
        fs::rename(&nb, &old_nb).unwrap();
        app.active_tab_mut().unwrap().path = old_nb.clone();

        app.run_notebook();
        let ready = root.join("run.ready");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.exists() {
            assert!(Instant::now() < deadline, "fake run never entered its cwd");
            sleep(Duration::from_millis(20));
        }
        app.apply_prompt(
            PromptMode::Rename {
                target: old_dir.clone(),
            },
            "new".to_string(),
        )
        .unwrap();
        wait_for_runs(&mut app);

        let new_nb = root.join("new/nb.ipynb");
        assert_eq!(fs::read_to_string(&new_nb).unwrap(), EXECUTED_NB);
        assert!(!run_tmp_path(&new_nb).exists());
        assert!(app.status.starts_with("Run finished in "), "{}", app.status);
    }

    #[test]
    fn drop_kills_active_run_and_removes_tmp() {
        let (_t, mut app, nb) = setup(
            r#"cp executed.json "$7"
exec sleep 30"#,
        );
        app.run_notebook();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !run_tmp_path(&nb).exists() {
            assert!(Instant::now() < deadline, "fake run never wrote tmp");
            sleep(Duration::from_millis(20));
        }
        drop(app);
        assert!(!run_tmp_path(&nb).exists());
        assert_eq!(fs::read_to_string(&nb).unwrap(), SOURCE_NB);
    }
}
