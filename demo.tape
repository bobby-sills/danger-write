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
Type "./target/release/danger-write -i 3"
Enter
Sleep 1.5s

# Write for a bit...
Type "Lorem ipsum dolor sit amet, consectetur adipiscing elit."
Sleep 3.5s        # stop typing — the words fade out, then game over

# Game-over screen is showing; restart and keep going.
Sleep 1s
Type "r"
Sleep 800ms
Type "Sed do eiusmod tempor incididunt ut labore."
Sleep 1.5s
Ctrl+C
