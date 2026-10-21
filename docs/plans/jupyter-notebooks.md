# Plan: Jupyter notebook (`.ipynb`) editing

Status: revised after Codex plan review and confirmation round (rev 3) · Branch: `feat/jupyter-notebooks` ·
Issue: [#51](https://github.com/TysonLabs/lazyide/issues/51)

## Goal

Open a `.ipynb` file in a normal editor tab as readable cell text, edit it, and
save it back as a valid notebook. Keep every cell's id, metadata and outputs
byte-for-byte unless the user deleted that cell, and keep the whole file
byte-identical when nothing changed.

## Non-goals (v1)

- Running cells or talking to a Jupyter kernel. Users run notebooks outside
  lazyide (`jupyter nbconvert --execute --inplace x.ipynb`, or their agent).
  The watcher picks up the new outputs (see *External changes*).
- Rendering outputs or images in the editor. A read-only output viewer is an
  optional follow-up (Phase 3).
- nbformat v3 and older. These open as raw JSON (see *Fallback*).
- The `nbformat` crate. It was evaluated and rejected: `serialize_notebook`
  refuses nbformat 4.0–4.4 and v3 (nbformat-3.0.0 `src/lib.rs:166-175`),
  re-serializes every value (`:152-157`) and pulls in `jupyter-protocol`,
  `chrono` and `uuid`. lazyide's CLAUDE.md asks for minimal features with no
  heavy abstractions.

## Design

### Projection format (jupytext "percent" style with identity tokens)

```
# %% #k3x9qa
import pandas as pd
df = pd.read_csv("x.csv")

# %% [markdown] #p0d7mz
# # Title
#
# Some *markdown* text.

# %% [raw] #b81fte
# raw cell text
```

- One marker line per cell. The marker grammar is
  `^# %%( \[(markdown|raw)\])?( #[0-9a-z]{6})?$`.
- **Identity token** = 6 random base-36 characters, assigned to each cell when
  the tab loads the notebook (open, reload, `R`). The token is opaque, scoped
  to the doc and **never renumbered** (Codex rev-2 F3, F5). `NotebookDoc`
  keeps a token table from token to cell. Tokens are not written to the
  `.ipynb`. A token pasted from another notebook is not in this table, so it
  is a new cell (a collision has probability ≈ cells / 36⁶). Source text and
  position are never used for identity (Codex F1).
- Token rules at save:
  - No token: a new cell.
  - A token not in the table: a new cell.
  - A token used twice (for example a copy-pasted cell): the first occurrence
    in buffer order keeps the identity, and the later ones are new cells.
  - A known token whose marker type changed (code → markdown): a new cell.
    Code outputs must not move onto a markdown cell.
- **Malformed markers fail the save (Codex rev-2 F4).** Any line that starts
  with `# %%` but does not match the full grammar (for example a half-typed
  `# %% #`) makes `rebuild` return an error. `save_file` returns `Failed` with
  "Malformed cell marker at line N", and nothing is written. Projection
  fallback and save validation use the same `is_marker_like` / `parse_marker`
  pair.
- Code cell lines appear verbatim. Markdown and raw lines get a `# ` prefix,
  and an empty line becomes `#`. On parse, strip `# `, or turn a lone `#` into
  an empty line, and keep any other line verbatim.
- One blank separator line goes before each marker except the first. On parse,
  drop exactly one trailing blank line from a cell that is followed by
  another marker.

**Lossless rule (Codex F2).** The projection is not injective: `"x"` and
`"x\n"` project the same, and `text_to_lines` (`src/util.rs:19`, `str::lines`)
drops `\r`. Losslessness therefore does not depend on parsing back:
- If a token-matched cell's visible text equals the projection of its original
  source, the original JSON `source` value is written back unchanged (raw
  bytes, see *Writing bytes*). Trailing newlines, `\r\n` and string-vs-list
  form all survive.
- Only a cell whose visible text changed gets a new `source`, built from the
  visible text: LF line endings, no trailing `\n`, and the list form unless
  the original used a string. This normalization is documented in the README.

### Module and state

- New `src/notebook.rs` with pure functions and no `App` access, matching the
  flat module layout:
  - `project(bytes: &str) -> Result<NotebookDoc, NotebookOpenError>`.
  - `rebuild(doc: &NotebookDoc, edited: &str) -> Result<Rebuilt, NotebookSaveError>`.
    `Rebuilt` holds the bytes to write plus the token assignments for new cells.
  - `strip_tokens(text) -> String` (used by recovery).
- `NotebookDoc` holds:
  - `original_bytes: String` and `disk_digest: u64` (std `DefaultHasher`,
    change detection only);
  - the root as an ordered list of `(key, Box<RawValue>)`;
  - per cell: its raw object as an ordered list of `(key, Box<RawValue>)`,
    its type, and its visible projected text;
  - `nbformat_minor`, `layout_canonical: bool`, `lang: SyntaxLang` and
    `projected_text: String`.
- `Tab` gets `notebook: Option<NotebookDoc>` (`src/tab.rs`). The two test
  `Tab` literals in `src/lsp_client.rs:482`, `:514` need the field.
- **Mode lives on the tab (Codex F8).** Every notebook-specific branch checks
  `tab.notebook.is_some()`, never the `.ipynb` extension. A `.ipynb` that fell
  back to raw JSON behaves exactly as today, gutter and jumps included.
- **Language lives on the tab (Codex F11).** Add `Tab::syntax_lang()`, which
  returns `notebook.lang` for notebook tabs and `syntax_lang_for_path(path)`
  otherwise. Replace the path-based calls at:
  - `src/app/editor.rs:353` (open folds);
  - `src/app/core.rs:687` (recompute folds);
  - `src/ui/mod.rs:338` (render);
  - `src/app/lsp.rs:434` (keyword completion);
  - `src/util.rs:132` `comment_prefix_for_path`, used by `toggle_comment`
    (`src/app/editor.rs:52`). This one gets a tab-aware variant.

  `notebook.lang` comes from `metadata.kernelspec.language` or
  `language_info.name` when that maps to a supported `SyntaxLang`, and is
  Python otherwise.

### Validation and fallback (fail closed to raw JSON; Codex F6)

`project` returns `Err`, and the file opens as plain text like today (status:
`Opened x.ipynb as raw JSON: <reason>`), when any of these hold:
- the JSON is invalid;
- the root is not an object, or `nbformat != 4`, or `nbformat_minor` is not
  an integer;
- the root `metadata` is not an object, or `cells` is not an array;
- a cell is not an object, or its `cell_type` is not
  `code`/`markdown`/`raw`;
- a cell's `source` is not a string or a list of strings, or its `metadata`
  is not an object;
- a code cell lacks `outputs` (array) or `execution_count` (int or null);
- for `nbformat_minor >= 5`, a cell lacks a non-empty string `id`, or two
  cells have the same `id`;
- any projected non-marker line matches `^# %%`, or the marker count is not
  the cell count. This catches a code cell that contains `# %%` and a
  markdown line `%%…`.

A raw-JSON tab has `notebook: None`.

### Save

`save_file` returns `SaveOutcome { Saved, Deferred, Failed }` (Codex F5). All
four callers handle it:
- `src/app/input_handlers.rs:418` Save key: save only.
- `src/app/core.rs:556` palette Save: save only.
- `src/app/input_handlers.rs:229` close prompt Ctrl+S: save, then close only
  on `Saved`.
- `src/app/input_handlers.rs:235` close prompt Enter: save, then close only on
  `Saved`.

`rebuild` steps:
1. Parse the buffer into cells with tokens, using the token rules above.
2. If every original cell is present once, in the original order, with the
   same type, and with unchanged visible text: write `original_bytes`
   verbatim, then return. This applies lesson l746: a no-op save never
   rewrites the file.
3. Otherwise assemble the file:
   - Each kept cell is its original raw object. If its visible text changed,
     only its `source` entry is replaced.
   - New cells are `{cell_type, metadata: {}, source}`. Code cells also get
     `outputs: []` and `execution_count: null`, and an `id` when
     `nbformat_minor >= 5`: 8 random hex chars, unique, from std
     (`RandomState` hash of time and a counter), with no `uuid` dependency.
   - Deleted cells are dropped.
4. Edited cells keep their outputs and `execution_count`. Jupyter and VS Code
   do the same: the outputs are stale until the cell runs again.
5. After the write, `doc` is updated in place: each existing token points at
   its cell in the written file, and `original_bytes` and `disk_digest` are
   the written bytes. A doc is never re-projected after our own save, so
   existing tokens never change. Each new (tokenless) cell gets a token,
   which is appended to its marker line through the `TextArea` editing API
   (`move_cursor` + `insert_str`, then the cursor and selection are restored).
   That keeps undo history: the token insertion is one extra undo step,
   which the README notes. `replace_editor_text` is not used here, because it
   rebuilds the `TextArea` and drops undo history (`src/app/editor.rs:147`).
   Then `open_disk_snapshot` is set to the buffer text, and `dirty` is
   cleared.

### Writing bytes (Codex F4, F7)

Untouched JSON values are **never re-serialized**. Enable serde_json's
`raw_value` feature. It is additive: it only adds `RawValue` and does not
change `Value` or number parsing for the keybinds, theme, persistence or LSP
code. The root and each cell are parsed as ordered
`Vec<(String, Box<RawValue>)>` through a small `Visitor`, so key order is kept
without `preserve_order`. The writer emits:
- the original key order;
- the Jupyter layout (indent 1, `": "`, `",\n"`, trailing newline) around the
  raw values;
- raw values verbatim. A raw value carries its own internal layout, which in a
  Jupyter-written file already matches its depth.
- new or changed strings, which go through `serde_json::to_string` (that
  matches Python's `ensure_ascii=False` escaping for typical text: `\n`, `\"`,
  `\\`, `\u001b`; a fixture test proves it).

So no number is ever reformatted (`1e-05` and integers beyond u64 survive),
and floats need no Python-compatible printer.

`layout_canonical` is set at open to `assemble(unchanged doc) == original_bytes`.
It is false only when the outer layout differs from Jupyter's (another indent
width, or whitespace between keys). Then the first real save asks:
"Saving will change this notebook's JSON whitespace. Cell contents and outputs
are kept. Save? (y/n)". This is a new `PendingAction::NotebookReformat
{ then_close: bool }`, with the same input, mouse-modal, hint and render wiring
as the existing prompts:
- `src/app/input_handlers.rs:200-250` (key handling);
- the pending hint in `src/util.rs` `pending_hint`;
- the overlay in `src/ui/overlays.rs`.

The answer is kept for the tab's lifetime. The answer `y` with
`then_close: true` saves and closes. The answer `n` leaves the tab open and
dirty.

### External changes (Codex F3)

The comparison uses **raw bytes** (`disk_digest`) and projected text separately.
- **`reload_open_file_from_disk_if_pristine`** (`src/app/core.rs:318-347`,
  early return at `:328`): if the disk digest changed, re-`project`, then
  replace `doc`. Replace the buffer text only when the projected text differs.
  Our own save writes bytes whose digest is recorded, so it is a no-op.
- **`maybe_flag_external_conflict`** (`src/app/core.rs:652-674`) on a dirty
  tab whose disk digest changed:
  - **Silent adopt only when the cell mapping is provable (Codex rev-2
    F2).** The disk cells must have the same count, types and visible texts
    as the current `doc`, in the same order. For `nbformat_minor >= 5`, the
    cell-id sequence must also be unchanged. For older notebooks, no two
    cells may share the same (type, visible text). If the check passes, only
    outputs, metadata or execution counts changed (for example after
    `nbconvert --execute`). The doc's raw cells, `original_bytes` and
    `disk_digest` are then replaced, and each token keeps pointing at the
    same index. Status: "Notebook outputs updated on disk".
  - Otherwise raise the existing conflict prompt, with a projected
    `conflict_disk_text` and the disk `NotebookDoc` stored next to it.
- **Conflict keys** (`src/app/input_handlers.rs:839-881`):
  - `R` loads the disk text and the disk doc, with fresh tokens.
  - `K`, `D` and `Esc` keep the buffer **and keep the old doc** as the save
    base, so the next save overwrites the disk, as "keep mine" does for text
    files. The disk doc is dropped, and no token is ever rebound to another
    notebook's cells (Codex rev-2 F1). Status: "Kept local version; saving
    will overwrite the notebook on disk".

### Autosave and recovery

The autosave stays as projected text (`src/app/core.rs:610-626`). Tokens are
random per load, so next to the autosave write `<autosave>.nbtokens`
(JSON: `disk_digest`, plus the token for each disk cell index).

In `check_recovery_for_open_file` (`:628-644`):
- If the sidecar's digest equals the current disk digest, the new doc
  **adopts the sidecar's tokens** for its cells, so the recovered text binds
  exactly.
- If the digest differs, or the sidecar is missing or unreadable, the
  recovered text goes through `strip_tokens`, so every cell becomes new. The
  text is recovered, but no outputs attach to the wrong cell, and the
  recovery prompt says so.

The sidecar is removed with the autosave (`core.rs:646-650`, `editor.rs:438`,
`:474`).

### Other touchpoints

| # | Site | Change |
|---|---|---|
| 1 | `src/app/editor.rs:340-348` `open_file_as` | For `.ipynb`: on `project()` Ok, load `projected_text` and set `tab.notebook`; on Err, fall back with status |
| 2 | `src/util.rs:437-460` `spawn_git_refresh` + `compute_git_line_status` (`:208-255`) | Pass `is_notebook: bool` per tab (from `tab.notebook`); notebooks get an all-`None` gutter. A projected gutter is Phase 3 |
| 3 | `src/app/git_diff.rs:126-136` diff view Enter | The view still shows the raw JSON diff. On a notebook tab, Enter shows a status and does not jump |
| 4 | `src/app/search.rs:63-131` project search (Codex F9) | Run the existing rg command with `--glob '!*.ipynb'`. List notebooks with `rg --files --glob '*.ipynb'`, so ignore rules match. For each one, `project()` it and pipe the projected text to `rg --line-number --smart-case <query> -` on stdin. That gives the same regex and smart-case rules with no new dependency. Hits carry projected line numbers, so the jump is exact, and output/metadata text is never searched. A notebook that fails to project is searched raw, as today. Cap at 500 notebooks per search, with a status note when the cap is reached. Classify an rg exit code of 1 (no match) separately from 2 (error) |
| 5 | `src/app/file_tree.rs:466-490` rename (Codex F10) | **Before** `fs::rename` (`:486`), find open tabs at or under the target whose notebook mode would change. If any is dirty, refuse ("Save x.ipynb before renaming"). After the rename, close and reopen the affected clean tabs |
| 6 | `src/util.rs:467-607` fold callers | Add `compute_cell_fold_ranges(lines)` (from a marker to the line before the next marker) and merge it in for notebook tabs |
| 7 | `src/ui/mod.rs` line render | Draw marker lines in the accent color, bold, so cells are visibly separated |

Blast-radius notes:
- `open_disk_snapshot` readers are only the conflict paths.
- `dirty` is a flag set by `mark_dirty` and is never compared to the snapshot.
- `poll_fs_changes` (`src/app/core.rs:226-297`) checks only the active tab.
  This is existing behaviour, unchanged.
- `all_tabs()` iterations (autosave, git refresh, LSP diagnostics, rename
  retarget, dirty check, wrap rebuild) were each checked. Autosave gains the
  digest sidecar, git refresh gains `is_notebook`, rename gains the preflight,
  and the others need no change.
- `syntax_lang_for_path` callers are all listed above.
- `save_file` callers are all listed above.

## Testing

All tests are inline `#[cfg(test)]` modules, per CONTRIBUTING.md.

Fixtures live in `src/notebook/testdata/` and are loaded with `include_str!`.
**They are generated by real writers, not written by hand**:
- JupyterLab nbformat 4.5 with `image/png`, stream and ANSI error outputs;
- nbformat 4.4 without ids;
- string-form `source`;
- a source with `\r\n`;
- metadata with `1e-05` and an integer beyond u64;
- VS Code-saved;
- indent-2 JSON (non-canonical layout);
- a code cell containing `# %%`;
- a 4.5 notebook missing a cell id.

Unit tests (`src/notebook.rs`):
1. Byte identity: open, then save with no edits, for each fixture.
2. `layout_canonical` is true for the Jupyter and VS Code fixtures and false
   for the indent-2 fixture.
3. Edit one cell: the output differs from the original only inside that
   cell's `source`, compared at the byte level.
4. The F1 scenario: delete cell A and edit cell B. B keeps B's outputs, and
   A's outputs are gone.
5. The token rules: missing, out of range, duplicate and type change each
   produce a new cell. Cell swap, insert and delete keep outputs with the
   right cell.
6. Unchanged visible text keeps `\r\n`, a trailing `\n` and string form.
7. Numbers in untouched cells survive (`1e-05`, an integer beyond u64), as do
   numbers in an edited cell's outputs.
8. New-cell ids exist only for `nbformat_minor >= 5` and are unique.
9. Each fallback reason returns `Err`.

App-level tests (`tempdir` + `App::new` + `open_file`):
10. Save, then fs poll: the buffer stays projected, and existing tokens are
    unchanged. A new cell gets a token, and Ctrl+Z after the save still
    reaches the user's own edit (rev-2 F5).
11. Outputs-only external change on a dirty tab: no conflict, and the next
    save keeps the new outputs together with the local edit (F3).
12. A structural external change on a dirty tab (insert a cell before A)
    raises the conflict prompt. `K` then save writes the local version, and
    no cell gets another cell's outputs (rev-2 F1).
12a. External swap of two cells with identical source in a pre-4.5 notebook
     raises the conflict prompt instead of a silent adopt (rev-2 F2).
12b. A cell pasted from another notebook (foreign token) saves as a new cell
     (rev-2 F3).
12c. Save with a half-typed `# %% #` marker fails with the line number, and
     the file is unchanged (rev-2 F4).
13. A non-canonical notebook, close prompt, Enter: the reformat prompt shows
    and the tab stays open. Answering `y` saves and closes; answering `n`
    keeps the tab dirty (F5).
14. Recovery with a stale digest strips tokens, and the text is recovered
    without misattached outputs.
15. Renaming a dirty notebook from `.ipynb` to `.py` is refused before any disk
    change. When clean, it reopens as plain text (F10).
16. A raw-fallback `.ipynb` keeps its git gutter and raw search jumps (F8).
17. A project-search hit in a notebook cell jumps to the right projected line.
    A hit that appears only in an output is not listed (F9).
18. A notebook with a Rust kernel highlights as Rust and comment-toggles with
    `//` (F11).

Each test must fail without its change. Run tests 10–17 once against the
unmodified code (or with the fix commented out) and record that in the PR.

Gate (the same as CI): `cargo fmt --all -- --check`,
`cargo clippy --all-targets -- -D warnings`, `cargo test`.

## Docs (same PR)

- `README.md`:
  - the Editor features list (`:68-75`);
  - a new "Notebooks" section: the format, the `#xxxxxx` cell tokens (not saved to the file), what is kept, the
    edited-source normalization, the reformat prompt, fallback, no execution,
    the `nbconvert` tip, and outputs refreshing on disk change;
  - the Git section note: no gutter for notebooks, and the diff view shows
    JSON;
  - the project-search note: notebooks are searched by cell text.
- `docs/ARCHITECTURE.md`: the module list (`notebook.rs`), Syntax Highlighting
  (the tab-stored language), Git Integration, and the test count.
- `CONTRIBUTING.md:130-136`: a note that `.ipynb` is not a language entry.
- `CLAUDE.md`: the module list and overview, and correct "no CI/CD" (CI runs
  fmt, clippy and test in `.github/workflows/ci.yml`).
- Move this plan to `docs/notebooks-design.md` when it merges, and link it
  from the README Documentation section, which is the docs index.

## Phases and effort

| Phase | Content | Estimate |
|---|---|---|
| 1 | `notebook.rs`: projection, tokens, validation, raw-value writer and unit tests with fixtures | 3–4 days |
| 2 | App wiring (save outcome and prompt, watcher and conflict, recovery digest, language on tab, search, rename, gutter, folds, render), App-level tests, docs | 3–4 days |
| 3 (optional, separate PR) | Read-only output viewer for the cursor's cell; a projected git gutter via `git show HEAD:path` | 2–3 days |

Phases 1 and 2 ship as one PR (about 2–2.5K lines, mostly tests and fixtures).
Phase 3 needs a new keybinding: read the key matcher in `src/keybinds.rs`
before choosing the default.

## Review log

Rev 2 folds in all 11 findings from the Codex plan review (all accepted after
checking the code):
- F1: identity tokens.
- F2: the lossless rule.
- F3: raw-digest external changes.
- F4/F7: the raw-value writer replaces re-serialization (F7 amended: the plan
  had called `1e-05` non-canonical, which is wrong, because Python writes
  `1e-05`).
- F5: `SaveOutcome` plus `NotebookReformat { then_close }`.
- F6: full validation.
- F8: mode on the tab.
- F9: projected search.
- F10: rename preflight.
- F11: language on the tab.

Rev 3 folds in the Codex confirmation round, which found prior F2 and F4–F11
fixed and F1 and F3 partly fixed. All five new findings were verified and
accepted:
- rev-2 F1: `K`/`D`/`Esc` keep the old doc, and nothing is rebound.
- rev-2 F2: silent adopt only with a provable mapping.
- rev-2 F3: opaque, doc-scoped random tokens.
- rev-2 F4: malformed markers fail the save.
- rev-2 F5: tokens are never renumbered; a new cell's token is inserted via
  the editing API, so undo history survives.

This was review round 2 of 2. Rev 3 has not had a reviewer pass of its own.
The implementation PR's Codex round reviews it.

Open question carried forward: keep stale outputs on edited cells (current
choice, the same as Jupyter and VS Code) or clear them.
