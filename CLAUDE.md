# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Lazyide is a lightweight, simple terminal-native IDE built in Rust using ratatui. The goal is to be a little more than bare bones — keep things minimal and avoid over-engineering. Don't add heavy features or abstractions. It provides file tree navigation, text editing, LSP integration (rust-analyzer), syntax highlighting, code folding, project search (via ripgrep), and a theme system.

## Build & Run

```bash
cargo build            # Debug build
cargo build --release  # Release build
cargo run              # Run the application
cargo run -- <path>    # Open a specific project directory
```

Rust 2024 edition. No custom build scripts, no CI/CD.

## Architecture

The app is now modular (library + thin binary launcher):

- `src/main.rs` — minimal entrypoint (`lazyide::run()`)
- `src/lib.rs` — runtime entry (`run`, setup flow, terminal lifecycle) and crate wiring
- `src/app.rs` + `src/app/*` — `App` state + split impls:
  - `core.rs` (constructor, persistence/autosave/fs poll, folding core helpers)
  - `input.rs` (top-level key/mouse dispatch)
  - `input_handlers.rs` (modal/menu/context/keybind handlers + key action routing)
  - `editor.rs` (editor ops, file open/save/close, clipboard, paging/selection helpers)
  - `file_tree.rs` (tree build/navigation/context actions)
  - `search.rs` (find/replace/project search)
  - `lsp.rs` (LSP lifecycle, diagnostics, completion, definition handling)
- `src/ui/mod.rs` + `src/ui/overlays.rs` + `src/ui/helpers.rs` — drawing and overlays
- Shared domain/util modules:
  - `src/types.rs`, `src/tab.rs`, `src/tree_item.rs`
  - `src/theme.rs`, `src/syntax.rs`, `src/persistence.rs`, `src/lsp_client.rs`, `src/keybinds.rs`, `src/util.rs`

Core model remains the same:

- **App struct** — central application state (tree/editor/theme/LSP/fs watcher/UI state)
- **Main event loop** (`run_app`) — poll LSP/fs/autosave, draw, then process events
- **Focus model** — `Focus::{Tree, Editor}` controls key handling paths
- **Keybinding system** — `KeyAction`, `KeyBind`, `KeyBindings` with user overrides at `~/.config/lazyide/keybinds.json`

## External Tool Dependencies

- **rust-analyzer** — LSP server (resolved from multiple known paths including rustup and brew)
- **ripgrep (`rg`)** — Powers project-wide search (Ctrl+Shift+F)
- **System clipboard** — Via `arboard` crate

## File Tree

- The root directory is implicit — its children appear directly at the top level of the Files pane
- **[+]/[-] buttons** in the tree header bar (right-aligned) expand/collapse all folders via mouse click
- **Keyboard shortcuts** (when tree is focused):
  - `Up/Down/K/J` — navigate
  - `Right/L/Enter` — expand dir or open file
  - `Left/H` — collapse dir or jump to parent
  - `Shift+Right` — recursively expand selected dir and all descendants
  - `Shift+Left` — recursively collapse selected dir and all descendants
  - `Ctrl+Shift+E` — expand all folders globally
  - `Ctrl+Shift+C` — collapse all folders globally
  - `Delete` — delete selected item

## Editor Features

- **Gutter line selection** — click the gutter to select a line, drag to select multiple lines
- **Cut line** — Ctrl+K cuts the entire current line
- **Auto-pair insertion** — automatically pairs `()`, `[]`, `{}`, `""`, `''`
- **Bracketed paste** — multi-line paste without auto-pair doubling
- **Inline ghost completion** — LSP ghost text shown inline, Tab to accept

## Themes

`themes/*.json` — Each file defines background, foreground, accent, selection, border, and UI element colors. Parsed via `ThemeFile` struct with serde. Fallback to built-in dark theme if no files found.

## Key Constants

```
INLINE_GHOST_MIN_PREFIX: 3      // Min chars before showing inline completion
EDITOR_GUTTER_WIDTH: 10         // Line number + fold marker gutter width
MIN_FILES_PANE_WIDTH: 18
MIN_EDITOR_PANE_WIDTH: 28
FS_REFRESH_DEBOUNCE_MS: 120
AUTOSAVE_INTERVAL_MS: 2000
```
