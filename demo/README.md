# Demos

Recorded demos for posts, release notes and docs. Each demo has a VHS tape,
the recorded outputs, and a script that re-records it. The main README demo
(`demo.gif` in the repo root, recorded with `record-demo.sh`) is separate.

| Demo | Files | Shows |
|---|---|---|
| Jupyter notebooks (v0.4.0) | `notebooks.gif`, `notebooks.mp4`, `notebooks.png`, `notebooks-running.png` | Open a `.ipynb` as cell text, cell output summaries in the status bar, edit a cell (`edited, outputs stale`), insert a cell, save, and **Notebook: Run all** with Jupyter (`Run finished`, then the new cell's output summary). |

## Using the files

- **Social posts:** most platforms prefer the `.mp4` (smaller, sharper, and it
  loops). Use the `.gif` where only images work, for example in a README or a
  forum.
- **Stills:** `notebooks.png` is the "Run finished" frame and
  `notebooks-running.png` is the frame during the run.
- **Links:** reference files from the default branch, for example
  `https://github.com/TysonLabs/lazyide/raw/main/demo/notebooks.gif`.

## Re-recording the notebooks demo

You need [VHS](https://github.com/charmbracelet/vhs), and `jupyter` with
`nbconvert`, `ipykernel`, `pandas` and `matplotlib` on `PATH`.

```sh
pip install nbconvert ipykernel pandas matplotlib
demo/record-notebooks.sh
```

The script builds a release binary and copies the fixture
`demo/notebooks/sales.ipynb` to `~/demo-notebooks` (override with
`DEMO_DIR`). It runs lazyide with a throwaway config folder, so your own
settings are not touched. The project folder name appears in the top bar of
the recording.

The tape avoids `Alt+…` key chords, because VHS sends them as Escape followed
by the key. Use palette commands (`Ctrl+p`) instead.
