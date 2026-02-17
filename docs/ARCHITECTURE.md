# Architecture

This document describes the internal architecture of lazyide for contributors who want to understand or modify the codebase.

## Module Structure

```
src/
  main.rs              Entry point (3 lines, calls lazyide::run())
  lib.rs               Terminal lifecycle, main event loop, setup command
  app.rs               App struct definition (all application state)
  app/
    core.rs            Constructor, persistence, autosave, fs polling, fold helpers
    input.rs           Top-level key/mouse event dispatch
    input_handlers.rs  Modal/menu/context handlers, run_key_action() dispatcher
    editor.rs          File open/save/close, clipboard, fold, scroll, comment, dedent
    file_tree.rs       Tree build, navigation, file create/rename/delete
    lsp.rs             LSP lifecycle, completion, diagnostics, go-to-definition
    search.rs          Find/replace in file, project search (ripgrep)
  ui/
    mod.rs             Main draw() function (layout, tree pane, editor pane, bars)
    overlays.rs        Overlays: command palette, theme browser, help, prompts, etc.
    helpers.rs         UI utilities (centered_rect, label helpers)
  keybinds.rs          KeyAction enum, KeyBind, KeyBindings, JSON load/save
  types.rs             Focus, PendingAction, PromptMode, CommandAction enums
  tab.rs               Tab struct, FoldRange, ProjectSearchHit
  tree_item.rs         TreeItem struct
  theme.rs             Theme structs, color parsing, theme loading
  syntax.rs            SyntaxLang, highlight_line(), keyword lists
  lsp_client.rs        LspClient (JSON-RPC over stdin/stdout), rust-analyzer spawning
  persistence.rs       PersistedState, state file paths, autosave paths
  util.rs              Fold computation, fuzzy scoring, path helpers, geometry
```

## Key Design Decisions

**Single App struct, split `impl` blocks.** All application state lives in one `App` struct defined in `app.rs`. Methods are spread across `app/*.rs` files using Rust's multiple-impl-block feature. This avoids borrow checker issues that arise when sub-state structs have their own `&mut self` methods needing cross-access.

**UI never mutates state.** The `draw()` function and all `render_*` functions in `ui/` take `&App` (or clone data from it) and only write to the `Frame`. State mutations happen exclusively in `app/` methods.

**Keybindings are data-driven.** ~40 actions are defined in the `KeyAction` enum. Default key mappings are in `KeyBindings::defaults()`. User overrides layer on top from `~/.config/lazyide/keybinds.json`. The `lookup()` method resolves a `KeyEvent` to a `KeyAction`.

## Event Loop

```
lib.rs: run_app()
  loop {
    app.poll_lsp()          // Check for LSP responses (non-blocking)
    app.poll_fs_changes()   // Check file watcher (debounced 120ms)
    app.poll_autosave()     // Write dirty buffers every 2s
    terminal.draw(|f| draw(&mut app, f))  // Render frame
    if app.quit { break }
    event::poll(100ms)      // Wait for terminal event
      Key  -> app.handle_key(key)
      Mouse -> app.handle_mouse(mouse)
  }
```

## Input Routing

`handle_key()` in `app/input.rs` routes events top-to-bottom by priority:

1. **Modal overlays** (keybind editor, file picker, prompts, completion, search results, context menus, theme browser, command palette, help) — first open modal wins
2. **Pending actions** (quit confirmation, delete confirmation)
3. **Global keybinds** — `keybinds.lookup(key, Global)` -> `run_key_action()`
4. **Non-remappable keys** (Esc, Tab for focus switch, Delete in tree)
5. **Focus-specific** — `Focus::Tree` -> `handle_tree_key()`, `Focus::Editor` -> `handle_editor_key()`

Inside `handle_editor_key()`, editor-scoped keybinds are checked before falling through to `tui_textarea::Input` for basic text editing (arrow keys, typing characters, etc.).

## Rendering Pipeline

`draw()` in `ui/mod.rs` builds the frame in layers:

```
1. Layout         3-row vertical: top bar | main content | status bar
2. Main split     If file tree open: [tree | divider | editor], else [editor]
3. Top bar        "lazyide  root: ...  branch: ...  file: ..."
4. File tree      ListWidget with TreeItem names, indent by depth
5. Tab bar        Horizontal tab names with click rects, [x] close buttons
6. Editor         Line-by-line rendering:
                    - Line number (gutter)
                    - Fold indicator (triangle)
                    - Diagnostic marker (colored dot)
                    - Syntax-highlighted text (via highlight_line())
                    - Cursor row highlight, selection highlight
                    - Fold summary ("... [N lines]")
7. Status bar     Dynamic keybind hints + status message + cursor position
8. Overlays       Modals rendered last (on top): menus, prompts, help, etc.
```

## LSP Integration

`lsp_client.rs` manages a rust-analyzer child process:

- **Spawn**: `LspClient::new_rust_analyzer()` starts the process, initializes JSON-RPC
- **Background reader**: A thread reads stdout, parses `Content-Length` headers, sends parsed `LspInbound` messages to a channel
- **Polling**: `app.poll_lsp()` calls `rx.try_recv()` each frame, matching response IDs to pending requests
- **Requests**: `send_request()` returns an ID; the response is matched later via `pending_completion_request` / `pending_definition_request` fields

Supported LSP methods: `initialize`, `textDocument/didOpen`, `textDocument/didChange`, `textDocument/didSave`, `textDocument/completion`, `textDocument/definition`, `textDocument/publishDiagnostics`.

## Theme System

Themes are JSON files with color definitions. Loading priority:
1. `themes/` directory (local, for development)
2. System paths (`/opt/homebrew/share/lazyide/themes/`, etc.)
3. Embedded themes via `include_dir!("$CARGO_MANIFEST_DIR/themes")` (fallback, always available)

Each theme defines: background, foreground, accent, selection, border colors + syntax colors (comment, string, number, tag, attribute) + bracket pair colors (yellow, purple, cyan).

## Syntax Highlighting

Lightweight, line-at-a-time highlighting in `highlight_line()`. No AST — just keyword matching, string/comment detection, and bracket depth tracking. Supports 11 language families detected by file extension.

Bracket colorization uses a depth counter computed per-file in `compute_fold_ranges()`, cycling through 3 theme-defined colors.

## File Tree

The tree is a flat `Vec<TreeItem>` built by `walk_dir()` (depth-first). Each item stores its `depth` for indentation. Expanded state is tracked in a `HashSet<PathBuf>`. The tree is rebuilt on file system changes (via `notify` crate watcher with 120ms debounce).

## Testing

Tests are inline with source using `#[cfg(test)] mod tests`. Run with:

```bash
cargo test              # all tests
cargo test keybind      # tests matching "keybind"
cargo test syntax       # tests matching "syntax"
```

191 tests cover keybindings, syntax detection, highlighting, folding, theme loading, LSP message parsing, and utilities.

## Adding a New Feature

1. Add any new types to `types.rs` (or the relevant domain module)
2. Add state fields to `App` in `app.rs`
3. Implement logic in the appropriate `app/` submodule
4. If it needs a keybind: add a `KeyAction` variant, wire it in `run_key_action()`, add a default in `KeyBindings::defaults()`
5. Add UI rendering in `ui/mod.rs` or `ui/overlays.rs`
6. Write tests in the same file under `#[cfg(test)]`
7. Update `README.md` keyboard shortcuts if applicable
