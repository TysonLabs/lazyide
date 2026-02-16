# Contributing to lazyide

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

Please open an issue on [GitHub Issues](https://github.com/tgeorge06/lazyide/issues) with steps to reproduce.

## Code Contributions

The entire app is a single file: `src/main.rs`. To get started:

```bash
cargo build && cargo test
```

Keep changes minimal — lazyide aims to stay lightweight. Open an issue first for large changes so we can discuss the approach.
