# lazyide — ~14s loop demo
Output demo.gif
Output demo.mp4

Set Shell "bash"
Set FontSize 15
Set Width 1400
Set Height 900
Set Padding 0
Set Framerate 30
Set TypingSpeed 30ms
Set PlaybackSpeed 1

# --- Hidden setup ---
Hide
Type "export TERM=xterm-256color"
Enter
Sleep 200ms
Type@1ms "export PATH=/Users/tgeorge/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
Enter
Sleep 200ms
Type "clear"
Enter
Sleep 200ms
Type@1ms "./target/release/lazyide ~/demo-project"
Enter
Sleep 2s
# Open a file with uncommitted changes so gutter markers show
Ctrl+o
Sleep 200ms
Type@10ms "users.rs"
Sleep 200ms
Enter
Sleep 800ms
Show

# Start: tree + users.rs, git gutter, minimap, status bar
Sleep 700ms

# Scroll: minimap viewport band follows
Down 8
Sleep 60ms
Down 8
Sleep 60ms
Down 8
Sleep 60ms
Down 8
Sleep 400ms

# Diff view against HEAD, step hunks, jump
Ctrl+p
Sleep 250ms
Type "git diff"
Sleep 200ms
Enter
Sleep 900ms
Type "n"
Sleep 500ms
Type "n"
Sleep 600ms
Enter
Sleep 700ms

# Split vertically and open a second file in the new pane
Ctrl+p
Sleep 250ms
Type "split editor vert"
Sleep 200ms
Enter
Sleep 500ms
Ctrl+p
Sleep 250ms
Type "focus other"
Sleep 200ms
Enter
Sleep 300ms
Ctrl+o
Sleep 200ms
Type "main.rs"
Sleep 200ms
Enter
Sleep 900ms
Screenshot demo.png

# Theme switch via the live-preview browser
Ctrl+p
Sleep 250ms
Type "theme"
Sleep 150ms
Enter
Sleep 300ms
Down
Sleep 250ms
Down
Sleep 250ms
Down
Sleep 250ms
Down
Sleep 250ms
Down
Sleep 250ms
Down
Sleep 300ms
Enter
Sleep 700ms

# Hold final frame
Sleep 800ms
