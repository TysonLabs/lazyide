# Plan: Jupyter notebook (`.ipynb`) editing

Status: draft for review · Branch: `feat/jupyter-notebooks` · Issue: (linked in the PR)

## Goal

Open a `.ipynb` file in a normal editor tab as readable cell text, edit it, and
save it back as a valid notebook. Keep every cell's id, metadata and outputs
that the user did not touch, and keep the file byte-identical when nothing
changed.

## Non-goals (v1)

- Running cells or talking to a Jupyter kernel. Users run notebooks outside
  lazyide (`jupyter nbconvert --execute --inplace x.ipynb`, or their agent);
  the file watcher reloads a pristine tab afterwards.
- Rendering outputs or images in the editor. A read-only output viewer is an
  optional follow-up (Phase 3).
- nbformat v3 and older. These open as raw JSON (see *Fallback*).
- A new dependency. The `nbformat` crate was evaluated and rejected:
  `serialize_notebook` refuses nbformat 4.0–4.4 and v3 (nbformat-3.0.0
  `src/lib.rs:166-175`), always sorts keys (`:133-150`) and pulls in
  `jupyter-protocol`, `chrono` and `uuid`. lazyide's CLAUDE.md asks for minimal
  features with no heavy abstractions.

## Design

### Projection format (jupytext "percent" format, subset)

```
# %%
import pandas as pd
df = pd.read_csv("x.csv")

# %% [markdown]
# # Title
#
# Some *markdown* text.

# %% [raw]
# raw cell text
```

- One marker line per cell: `# %%` (code), `# %% [markdown]`, `# %% [raw]`.
- Code cell lines appear verbatim.
- Markdown and raw lines get a `# ` prefix; an empty line becomes `#`. On parse,
  strip `# `, or turn a lone `#` into an empty line; keep any other line
  verbatim (the user typed it without a prefix).
- Exactly one blank separator line goes before each marker except the first.
  On parse, drop exactly one trailing blank line from a cell that is followed
  by another marker. Every other blank line belongs to the cell source.
- The marker grammar is `^# %%( \[(markdown|raw)\])?$`. Any other line that
  starts with `# %%` in projected output is a collision (see *Fallback*).

Required property (tested): `parse(project(nb)) == nb.cells` for every cell
source, including empty sources, sources that end with `\n`, sources that end
with blank lines and sources with `\r\n`.

### Module and state

- New `src/notebook.rs` (pure functions, no `App` access), matching the flat
  module layout:
  - `fn project(bytes: &str) -> Result<NotebookDoc, NotebookOpenError>`.
  - `fn rebuild(doc: &NotebookDoc, edited_text: &str) -> Result<RebuiltNotebook, NotebookSaveError>`.
- `NotebookDoc` holds `original_bytes: String`, the parsed
  `root: serde_json::Value`, the original cells as `Vec<OrigCell>` (index, type,
  source as parsed lines, source form = list or string), `nbformat_minor`,
  `canonical: bool` and `projected_text: String`.
- `Tab` gets `notebook: Option<NotebookDoc>` (`src/tab.rs`). Two test `Tab`
  literals in `src/lsp_client.rs:482`, `:514` need the field.
- Notebook mode is decided once at open from the extension and stored on the
  tab. No other code re-derives it from the path.

### Save: matching edited cells to original cells

`rebuild` parses the edited text into cells `(type, source)` and matches them
to the original cells:

1. LCS over the two cell sequences keyed by `(type, source)`. A matched cell
   keeps its JSON object unchanged, including id, metadata, outputs and
   `execution_count`.
2. Inside each unmatched gap, pair cells by position when they have the same
   type. A paired cell keeps id, metadata, outputs and `execution_count`, and
   only `source` is replaced. Outputs are kept, as Jupyter and VS Code do: an
   edited cell's outputs are stale until it is run again.
3. Any other edited cell is new: `{cell_type, metadata: {}, source}`, plus
   `outputs: []` and `execution_count: null` for code. It also gets an `id`
   when `nbformat_minor >= 5`. The id is 8 random hex chars, unique within the
   notebook, generated from `std` (`RandomState` hash of time and a counter),
   so no `uuid` dependency is needed.
4. Original cells left unmatched are deleted.
5. A cell whose type changed is new (code to markdown must drop `outputs`).

Source form: write `source` in the cell's original form (list of lines with
trailing `\n` except the last, or a single string). New cells use the list form.

### Save: writing bytes

- If the rebuilt cell list equals the original (every cell matched in step 1
  and the order is unchanged), write `original_bytes` verbatim. This applies
  lesson l746: a no-op save must not rewrite the file.
- Otherwise serialize the patched `root` the way Python `nbformat.write` does:
  keys sorted, indent 1, `ensure_ascii=False`, trailing newline. serde_json's
  default `Map` (a BTreeMap, since `preserve_order` is off today) sorts keys,
  and `PrettyFormatter::with_indent(b" ")` gives indent 1.
- `canonical` is computed at open as `serialize(parse(bytes)) == bytes`. It is
  false for notebooks from writers that differ, for example on float spelling
  (`1e-05` vs `1e-5`), key order or escapes. If it is false, the first real
  save asks "This notebook's JSON layout differs from Jupyter's. Saving will
  reformat it (cell contents are kept). Save? (y/n)", and the answer is kept
  for the tab's lifetime. This prompt goes through the existing
  `PendingAction` mechanism. No `serde_json` feature flags are added, because
  `preserve_order` and `arbitrary_precision` would change behaviour for the
  keybinds, theme, state and LSP code (`src/keybinds.rs`, `src/theme.rs`,
  `src/persistence.rs`, `src/lsp_client.rs`).
- The `open_disk_snapshot` and the autosave stay in **projected** space: after
  save, `open_disk_snapshot = projected text`, and `doc` is replaced with
  `project(written_bytes)` so the next save diffs against what is on disk.

### Fallback (fail closed to raw JSON)

`project` returns an error, and the file opens as plain text like today
(status: `Opened x.ipynb as raw JSON: <reason>`) when:
- the JSON is invalid (the user can then fix it);
- `nbformat != 4` or `cells` is missing or not an array;
- a cell has an unknown `cell_type` or a non-string/non-list `source`;
- any projected line matches `^# %%` but is not one of this notebook's
  markers, for example a code cell that contains a `# %%` line or a markdown
  line that is `%%…`. A post-projection check (marker count == cell count)
  catches every case.

A tab opened as raw JSON has `notebook: None` and behaves exactly as today.

### Touchpoints (from the code survey)

| # | Site | Change |
|---|---|---|
| 1 | `src/app/editor.rs:340-348` `open_file_as` | For `.ipynb`, `project()`; on Ok, load `projected_text` and set `tab.notebook`; on Err, fall back with status |
| 2 | `src/app/editor.rs:422-450` `save_file` | For notebook tabs, `rebuild()`, the reformat prompt when `!canonical`, write, then refresh `doc` and snapshot |
| 3 | `src/app/core.rs:318-347` `reload_open_file_from_disk_if_pristine` | Project disk bytes before comparing, replacing and storing the snapshot; refresh `doc`. Without this, our own save triggers a reload into raw JSON |
| 4 | `src/app/core.rs:652-674` `maybe_flag_external_conflict` | Project disk bytes before comparing; store projected `conflict_disk_text` plus the disk `NotebookDoc` |
| 5 | `src/app/input_handlers.rs:839-881` conflict `R`/`K`/`D` | `R` also swaps in the disk `NotebookDoc` |
| 6 | `src/app/core.rs:610-644`, `input_handlers.rs:803-835` autosave and recovery | No change: they already work on editor text (projected). Recovery applied over a fresh `doc` from disk works through the step-1/2 matching |
| 7 | `src/util.rs:208-255` `compute_git_line_status` | Return all `None` for `.ipynb` (JSON line numbers do not map). Projected-vs-projected gutter is Phase 3 |
| 8 | `src/app/git_diff.rs:126-136` diff view Enter jump | Diff view still shows the raw JSON diff, which is useful; Enter on a notebook hunk shows a status and does not jump |
| 9 | `src/app/search.rs:106-131` project search jump | For an `.ipynb` hit, jump to the k-th projected line that matches the query with the same matching mode rg used (k = the hit's index among that file's hits); fall back to the first match, then line 0 with a status |
| 10 | `src/app/file_tree.rs:91-101` `retarget_tabs_for_rename` | If a rename changes whether the path is `.ipynb`: refuse when the tab is dirty ("Save before renaming"); otherwise close and reopen the tab at the new path |
| 11 | `src/syntax.rs:21-44` | `ipynb` maps to the kernel language from `metadata.kernelspec.language` / `language_info.name` when it is a supported `SyntaxLang`; Python otherwise |
| 12 | `src/util.rs:467-607` `compute_fold_ranges` callers (`editor.rs:354`, `core.rs:683-696`) | Add `compute_cell_fold_ranges(lines)` (marker to the line before the next marker) and merge it for notebook tabs only |
| 13 | `src/ui/mod.rs` editor line render | Draw marker lines with the accent color and bold, so cells are visibly separated |
| 14 | `src/app/lsp.rs:434` keyword completion | Uses the tab's notebook language (from #11) |

Blast-radius notes (grep done on the survey branch):
- `open_disk_snapshot` readers: only the conflict paths (#4, #5).
- `dirty` is a flag set by `mark_dirty`, never compared to the snapshot, so it
  needs no change.
- `poll_fs_changes` (`src/app/core.rs:226-297`) checks only the active tab.
  This is existing behaviour and is not changed here.
- `spawn_git_refresh` (`src/util.rs:437-460`) runs off-thread without tab
  state, so #7 decides by path only.
- Every `all_tabs()` iteration was checked: none read or write file bytes.

## Testing

All tests are inline `#[cfg(test)]` modules, per CONTRIBUTING.md.

Fixtures in `src/notebook/testdata/` (loaded with `include_str!`). These are
**generated by real writers, not hand-written**, so the canonical serializer
is checked against the real format:
- JupyterLab-saved nbformat 4.5 (ids, outputs incl. `image/png`, stream,
  error with ANSI).
- nbformat 4.4 without ids (Python `nbformat.v4` writer).
- Cells with string-form `source`.
- VS Code-saved notebook.
- A non-canonical notebook (float `1e-05` in metadata).
- A code cell that contains `# %%` (must fall back).

Unit tests (`src/notebook.rs`):
1. Byte identity: open then save with no edits, for each fixture. Bytes equal
   `original_bytes`.
2. Canonical check: for each Jupyter-written fixture `canonical == true`;
   for the non-canonical fixture `false`.
3. Projection round-trip property over generated cell lists (edge cases above).
4. Edit one code cell: only that cell's `source` changes in the JSON (assert
   with a structural diff of the two `Value`s). Id, metadata and outputs kept.
5. Insert a cell, delete a cell, swap two cells, change a cell's type: assert
   outputs move with their cells and are never attached to another cell.
6. New-cell id: present only for `nbformat_minor >= 5`, unique.
7. Each fallback reason returns `Err`. The open path (App-level test) shows raw
   JSON and `notebook == None`.

App-level tests (`tempdir` + `App::new` + `open_file`, the existing pattern):
8. Save then fs poll: the tab stays in projected text (#3 regression).
9. External change while dirty raises a conflict prompt only when the
   projected disk text differs (#4). External change while pristine reloads
   projected text.
10. Recovery: an autosave with an edit is applied, then a save keeps the
    outputs of untouched cells.
11. Rename `.ipynb` to `.py` while dirty is refused; while clean, it reopens as
    plain text.
12. Project-search jump lands on the matching projected line.

Each test must fail without its change. For #3, #4 and #11, run the test once
on the unmodified code and record it in the PR.

Gate (the same as CI): `cargo fmt --all -- --check`,
`cargo clippy --all-targets -- -D warnings`, `cargo test`.

## Docs (same PR)

- `README.md`: the Editor features list (`:68-75`), a new "Notebooks" section
  (format, what is kept, the reformat prompt, fallback, no execution,
  `nbconvert` tip), and the Git section note (no gutter for notebooks; the diff
  view shows JSON).
- `docs/ARCHITECTURE.md`: the module list (`notebook.rs`), Syntax Highlighting,
  Git Integration, and the test count.
- `CONTRIBUTING.md:130-136`: a note that `.ipynb` is not a language entry.
- `CLAUDE.md`: the module list and overview, and correct "no CI/CD" (CI runs
  fmt, clippy and test in `.github/workflows/ci.yml`).
- Delete this plan file, or move it under `docs/` as the design note, when the
  feature merges.

## Phases and effort

| Phase | Content | Estimate |
|---|---|---|
| 1 | `notebook.rs` (project, rebuild, serializer, fallback) and its unit tests with fixtures | 2–3 days |
| 2 | App wiring #1–#14, App-level tests, docs | 2–3 days |
| 3 (optional, separate PR) | Read-only output viewer overlay for the cursor's cell (text/plain, stream, error with ANSI stripped, `[image/png 640×480]` placeholders); projected git gutter via `git show HEAD:path` | 2–3 days |

Phases 1 and 2 ship as one PR (about 1.2–1.8K lines, mostly tests and
fixtures). Phase 3 needs a new keybinding; read the key matcher in
`src/keybinds.rs` before choosing the default (lesson: the key you type may not
be the key the code sees).

## Open questions for review

1. Keep stale outputs on an edited cell (the Jupyter and VS Code behaviour), or clear them?
2. Is the LCS-plus-positional pairing safe enough, or should markers carry a
   cell id (`# %% id=abc`)? That clutters the text but makes identity exact.
3. Is the reformat prompt for non-canonical notebooks the right default, or
   should those open read-only?
