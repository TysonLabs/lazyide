#!/bin/sh
# Record the Jupyter notebooks demo (demo/notebooks.tape).
# Runs on a throwaway copy of demo/notebooks/sales.ipynb with a throwaway
# lazyide config, so neither the fixture nor your own settings change.
# Needs: vhs, and jupyter (nbconvert, ipykernel, pandas, matplotlib) on PATH.
set -eu
cd "$(dirname "$0")/.."

if ! command -v jupyter >/dev/null 2>&1; then
    echo "jupyter not found -- pip install nbconvert ipykernel pandas matplotlib" >&2
    exit 1
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# The project folder name shows in lazyide's top bar; the fixture is copied
# fresh on every run so the recording always starts from the same notebook.
export DEMO_DIR="${DEMO_DIR:-$HOME/demo-notebooks}"
mkdir -p "$DEMO_DIR"
cp demo/notebooks/sales.ipynb "$DEMO_DIR/sales.ipynb"

# Demo settings: One Dark Pro, narrow file pane, no wrap, minimap on
export XDG_CONFIG_HOME="$WORK/config"
mkdir -p "$XDG_CONFIG_HOME/lazyide"
printf '%s\n' '{"theme_name":"One Dark Pro","files_pane_width":28,"word_wrap":false,"minimap":true}' \
    > "$XDG_CONFIG_HOME/lazyide/state.json"

cargo build --release
vhs demo/notebooks.tape

echo "Done! Output: demo/notebooks.gif + demo/notebooks.mp4 + demo/notebooks.png"
