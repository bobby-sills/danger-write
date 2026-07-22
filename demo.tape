# Demo script for VHS (https://github.com/charmbracelet/vhs)
# Build first:  cargo build --release
# Render with:  vhs demo.tape   ->   produces demo.gif

Output demo.gif

Set FontSize 18
Set Width 1100
Set Height 640
Set Padding 16
Set TypingSpeed 85ms
# Set Theme "Gruvbox Dark"   # uncomment if your VHS has this theme

# Launch with a 4s idle limit so the fade/erasure is easy to see on camera.
Type "./target/release/danger-write -i 4"
Enter
Sleep 1.5s

Type "The cursor is a loaded gun. Do not stop typing,"
Sleep 400ms
Type " or you will lose everything you have written"
Sleep 4s          # stop typing — watch the words fade out and vanish
Sleep 1s

Type "gone. so keep the words alive."
Sleep 2.5s
Ctrl+C
