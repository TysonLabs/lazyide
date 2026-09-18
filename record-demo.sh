#!/bin/sh
# Record the lazyide demo: swap in demo settings, run VHS, restore yours.
set -eu

STATE="$HOME/.config/lazyide/state.json"
BACKUP="$STATE.demo-backup"
mkdir -p "$(dirname "$STATE")"
[ -f "$STATE" ] && cp "$STATE" "$BACKUP"
restore() {
    if [ -f "$BACKUP" ]; then
        mv "$BACKUP" "$STATE"
    else
        rm -f "$STATE"
    fi
}
trap restore EXIT

# Demo settings: One Dark Pro, narrow file pane, no wrap, minimap on
printf '%s\n' '{"theme_name":"One Dark Pro","files_pane_width":28,"word_wrap":false,"minimap":true}' > "$STATE"

# Clean autosave so no recovery prompts appear
for f in "$HOME/.config/lazyide/autosave/"*.autosave; do
    [ -e "$f" ] && rm -f "$f"
done

cargo build --release
vhs demo.tape

echo "Done! Output: demo.gif + demo.mp4 + demo.png"
