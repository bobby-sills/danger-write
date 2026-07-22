# Demo script for VHS (https://github.com/charmbracelet/vhs)
# Build first:  cargo build --release
# Render with:  vhs demo.tape   ->   produces demo.gif

Output demo.gif

Set FontSize 18
Set Width 1100
Set Height 640
Set Padding 16
Set TypingSpeed 75ms
# Set Theme "Gruvbox Dark"   # uncomment if your VHS has this theme

# Launch with a 3s idle limit so the fade + erasure is quick to show.
# Hidden so the GIF opens straight into the app, not the shell command.
Hide
Type "./target/release/danger-write -i 3"
Enter
Sleep 1s
Show
Sleep 500ms

# Write for a bit...
Type "Lorem ipsum dolor sit amet, consectetur adipiscing elit."
Sleep 600ms
Type " Sed do eiusmod tempor incididunt ut labore."

# ...then stop. The words fade, then dissolve away — game over.
Sleep 3s   # idle runs out: the text fades to gray
Sleep 2s   # the dissolve destroys it
Sleep 2s   # hold on the game-over screen
