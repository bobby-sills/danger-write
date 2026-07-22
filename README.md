# danger-write

A terminal version of the Most Dangerous Writing App. Keep typing — if you
stop for too long, the words you've written fade out and are erased. Reach your
goal to survive and copy your text.

## Build

```bash
cargo build --release
```

The binary is at `target/release/danger-write`.

## Usage

```bash
danger-write              # write for 5 minutes (default)
danger-write -w 250       # write until you reach 250 words
danger-write -i 5         # allow 5 seconds idle before erasure (default: 3)
```

### Options

| Flag | Description |
|------|-------------|
| `-t, --time <MINUTES>` | Survive by writing for this long (default: 5) |
| `-w, --words <N>` | Survive by reaching this many words |
| `-i, --idle <SECONDS>` | Idle time before your words are erased (default: 3) |
| `-h, --help` | Show help |

## Keys

- **While writing:** just type. `Ctrl+C` to quit.
- **After you win:** `c` to copy your text to the clipboard, `q` to quit.
- **After you fail:** `r` to restart, `q` to quit.

## Clipboard

Copying uses whatever clipboard tool is available: `wl-copy`, `xclip`, or
`xsel` on Linux, `pbcopy` on macOS, `clip` on Windows. On Linux you may need to
install one (e.g. `wl-clipboard`); macOS and Windows work out of the box.
