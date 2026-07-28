# danger-write

A writing app that forces you to keep writing. If you stop typing for too long,
the words you've written fade out and are erased. Reach your goal (a time limit
or a word count) to survive and copy your text.

![demo](https://raw.githubusercontent.com/bobby-sills/danger-write/master/demo.gif)

**▶ [Play in your browser](https://bobbysills.dev/danger-write/)** — no install required.

## Install

```bash
cargo install danger-write
```

Or build from source:

```bash
git clone https://github.com/bobby-sills/danger-write
cd danger-write
cargo build --release   # binary at target/release/danger-write
```

### Run in the browser

The same code also compiles to WebAssembly and runs in a browser via
[ratzilla](https://github.com/orhun/ratzilla) — no terminal required. A hosted
build is live at **[bobbysills.dev/danger-write](https://bobbysills.dev/danger-write/)**;
to run it yourself:

```bash
rustup target add wasm32-unknown-unknown
cargo install --locked trunk
trunk serve            # dev server at http://localhost:8080
trunk build --release  # static site into dist/ (deploy anywhere)
```

The web version opens on a menu to pick your goal (time or word count), then
plays with a 3-second idle limit. `c` copies to the browser clipboard after you
win, and `r` takes you back to the menu.

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
