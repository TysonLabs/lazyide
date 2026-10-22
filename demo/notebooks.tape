# lazyide — Jupyter notebooks demo (v0.4.0), ~19s
# Record with: demo/record-notebooks.sh (sets DEMO_DIR and a throwaway config)
Output demo/notebooks.gif
Output demo/notebooks.mp4

Set Shell "bash"
Set FontSize 15
Set Width 1400
Set Height 900
Set Padding 0
Set Framerate 30
Set TypingSpeed 45ms
Set PlaybackSpeed 1

# --- Hidden setup ---
Hide
Type "export TERM=xterm-256color"
Enter
Sleep 200ms
Type "clear"
Enter
Sleep 200ms
Type@1ms "./target/release/lazyide $DEMO_DIR"
Enter
Sleep 2s
Show

# Start: file tree with the notebook
Sleep 600ms

# Open the notebook: cells as plain text, one marker per cell
Ctrl+o
Sleep 250ms
Type "sales"
Sleep 250ms
Enter
Sleep 1200ms

# Jump to the data cell; once the "Opened" message clears, the status bar
# summarises the cell's outputs (text/html)
Down@120ms 11
Sleep 3800ms

# Edit the forecast (growth 1.10 -> 1.25): status shows "edited, outputs stale"
Down@120ms 7
Sleep 300ms
Right 13
Backspace 2
Type "25"
Sleep 1800ms

# Next cell from the palette: the chart cell (image/png)
Ctrl+p
Sleep 300ms
Type "next cell"
Sleep 300ms
Enter
Sleep 1800ms

# Insert a new cell below (palette) and type into it, then save
Ctrl+p
Sleep 300ms
Type "insert cell"
Sleep 300ms
Enter
Sleep 400ms
Type "forecast.q4.sum("
Sleep 700ms
Ctrl+S
Sleep 1000ms

# Run the whole notebook with Jupyter
Ctrl+p
Sleep 300ms
Type "run all"
Sleep 300ms
Enter
Sleep 300ms
Screenshot demo/notebooks-running.png
Sleep 2700ms
Screenshot demo/notebooks.png

# After the message clears: the new cell has run (its output summary)
Sleep 3500ms
Sleep 2000ms
