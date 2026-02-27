# Contributing to lazyide

## Getting Started

```bash
git clone https://github.com/TysonLabs/lazyide.git
cd lazyide
cargo build && cargo test
cargo run  # opens current directory
```

Requires Rust 2024 edition. Optional: `rust-analyzer` (LSP), `ripgrep` (project search), `git` (branch display). Run `cargo run -- --setup` to detect and install these.

## Theme Submissions

The easiest way to contribute is to submit a new theme. Themes are single JSON files — no Rust code changes required.

### Required Fields

```json
{
  "name": "Your Theme Name",
  "type": "dark",
  "colors": {
    "background": "#1a1b26",
    "backgroundAlt": "#24283b",
    "foreground": "#c0caf5",
    "foregroundMuted": "#565f89",
    "border": "#3b4261",
    "accent": "#7aa2f7",
    "selection": "#33467c"
  }
}
```

- `type` must be `"dark"` or `"light"`

### Recommended Fields

Add syntax highlighting and bracket pair colors for a polished experience:

```json
{
  "name": "Your Theme Name",
  "type": "dark",
  "colors": {
    "background": "#1a1b26",
    "backgroundAlt": "#24283b",
    "foreground": "#c0caf5",
    "foregroundMuted": "#565f89",
    "border": "#3b4261",
    "accent": "#7aa2f7",
    "accentSecondary": "#73daca",
    "selection": "#33467c",
    "yellow": "#e0af68",
    "purple": "#bb9af7",
    "cyan": "#7dcfff"
  },
  "syntax": {
    "comment": "#565f89",
    "string": "#9ece6a",
    "number": "#ff9e64",
    "tag": "#7aa2f7",
    "attribute": "#73daca"
  }
}
```

- `accentSecondary` — used for keybind highlights in the help screen (falls back to a blue default)
- `yellow`, `purple`, `cyan` — used for bracket pair colorization (nesting depth 1, 2, 3)
- `syntax` — controls keyword, string, number, tag, and attribute highlighting

### Steps

1. Fork this repository
2. Add your theme as `themes/your-theme-name.json` (use kebab-case)
3. Test it: `cargo run`, then press `Ctrl+P` and select "Theme Picker"
4. Submit a pull request

### Tips

- Look at existing themes in `themes/` for reference
- Extra color fields (like `red`, `green`, `blue`, `orange`, etc.) are ignored but welcome for future use
- All hex color values must be 6-digit with `#` prefix (e.g. `#ff00aa`)

## Bug Reports

Please open an issue on [GitHub Issues](https://github.com/TysonLabs/lazyide/issues) with:
- Steps to reproduce
- Expected vs actual behavior
- Terminal emulator and OS
- Output of `lazyide --setup` if relevant

## Code Contributions

### Project Structure

The app uses a modular structure — see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full breakdown. Key entry points:

- `src/lib.rs` — main event loop and terminal lifecycle
- `src/app.rs` — the `App` struct (all application state)
- `src/app/input.rs` — where key/mouse events are routed
- `src/app/input_handlers.rs` — where actions are dispatched (`run_key_action()`)
- `src/ui/mod.rs` — the main `draw()` function

### Code Style

- Use `pub(crate)` for internal APIs, not `pub`
- Keep functions focused — if a method grows past ~50 lines, consider splitting
- Write `#[cfg(test)]` tests in the same file as the code they test
- Explicit imports only — no glob `use crate::*` patterns

### Workflow

1. Open an issue first for large changes so we can discuss the approach
2. Fork and create a feature branch
3. Make your changes, keeping them minimal — lazyide aims to stay lightweight
4. Run `cargo build && cargo test` to verify
5. Submit a pull request

### Adding a Keybinding

1. Add a variant to `KeyAction` in `src/keybinds.rs`
2. Add the default mapping in `KeyBindings::defaults()`
3. Add `is_global()` / `is_editor()` classification and a `label()` for UI display
4. Add the variant to `KeyAction::all()`
5. Wire the action in `run_key_action()` in `src/app/input_handlers.rs`
6. Update `README.md` keyboard shortcuts

### Adding a Language

1. Add a variant to `SyntaxLang` in `src/syntax.rs`
2. Map file extensions in `syntax_lang_for_path()`
3. Add keywords in `keywords_for_lang()`
4. Add comment style in `comment_start_for_lang()`
5. If it uses `//` or `#` comments, add the extension to `comment_prefix_for_path()` in `src/util.rs`
