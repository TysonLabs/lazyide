# lazyide demo
Output demo.gif

Set Shell "bash"
Set FontSize 14
Set Width 1200
Set Height 700
Set Theme "Catppuccin Mocha"
Set WindowBar Colorful
Set WindowBarSize 40
Set BorderRadius 8
Set Padding 10
Set PlaybackSpeed 1
Set Framerate 30

# Launch
Hide
Type "rm -f ~/.config/lazyide/autosave/*.autosave"
Enter
Sleep 300ms
Type "export TERM=xterm-256color"
Enter
Sleep 300ms
Type "export PATH=/Users/tgeorge/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
Enter
Sleep 300ms
Type "cd /Users/tgeorge/test-project"
Enter
Sleep 300ms
Type "clear"
Enter
Sleep 300ms
Type "/Users/tgeorge/src/github.com/tgeorge06/lazyide/target/debug/lazyide ."
Enter
Show
Sleep 2.5s

# Navigate tree: . > .git > docs > src > brackets.rs
Down
Sleep 300ms
Down
Sleep 300ms
Down
Sleep 300ms
Right
Sleep 400ms
Down
Sleep 300ms

# Open brackets.rs
Enter
Sleep 1.5s

# Scroll down to depth_0 function and fold it
Down 5
Sleep 300ms
Ctrl+j
Sleep 1.5s

# Unfold it (Ctrl+J toggles)
Ctrl+j
Sleep 1.5s

# Scroll to mixed_brackets and fold it
Down 16
Sleep 300ms
Ctrl+j
Sleep 1.5s

# Unfold it
Ctrl+j
Sleep 1.5s

# Switch to Dark Mech theme via command palette
Ctrl+p
Sleep 600ms
Type "theme"
Sleep 400ms
Enter
Sleep 600ms
Down
Sleep 200ms
Down
Sleep 200ms
Down
Sleep 200ms
Down
Sleep 200ms
Enter
Sleep 1.5s

# Open main.rs via quick open (show tabs)
Ctrl+o
Sleep 600ms
Type "main"
Sleep 400ms
Enter
Sleep 1.5s

# Toggle file tree off/on
Ctrl+b
Sleep 1s
Ctrl+b
Sleep 1s

# Quit
Ctrl+q
Sleep 200ms
Ctrl+q
Sleep 300ms
