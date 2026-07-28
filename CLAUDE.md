# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`danger-write` is a single-binary terminal writing app (Rust + ratatui). You keep
typing; if you go idle longer than the idle limit, the text fades to gray and then
dissolves away — game over. Reach the goal (a time limit or word count) to survive
and unlock copying your text to the clipboard.

All logic lives in `src/main.rs` (~940 lines). The only submodule is a tiny inline
`theme` palette (two `cfg`-gated copies); there are no test files and no lib target —
the whole program is one binary crate. The same crate compiles two ways: a native
terminal app (crossterm) and a browser app (WebAssembly via
[ratzilla](https://github.com/orhun/ratzilla)), split by `cfg(target_arch = "wasm32")`.

## Commands

```bash
cargo build --release        # native binary at target/release/danger-write
cargo run -- -w 250          # run with args (note the `--` before app flags)
cargo clippy                 # lint
cargo fmt                    # format

# Regenerate the demo GIF after a visual/behavior change (requires charmbracelet/vhs):
cargo build --release && vhs demo.tape   # writes demo.gif

# --- Web build (WebAssembly) ---
rustup target add wasm32-unknown-unknown     # one-time
cargo install --locked trunk                 # one-time: the wasm bundler
cargo build --target wasm32-unknown-unknown  # compile-check the wasm build
trunk serve                                  # dev server at http://localhost:8080
trunk build --release                        # static site into dist/
```

There is no test suite. `cargo test` currently does nothing.

## Architecture

The program is a single synchronous render loop in `run()` — no async, no threads.
Each iteration: compute frame delta → `terminal.draw()` → `event::poll(50ms)` for a
keystroke → `app.tick()`. The 50ms poll timeout is what keeps fades/timers/effects
animating while the user isn't pressing keys.

**Two entry points, one `App`.** The state machine, drawing (`draw*`), and helpers
are backend-agnostic and shared. Only the shell differs, gated by `cfg`:
- **Native** (`cfg(not(target_arch = "wasm32"))`): `main()` → `run()`, the crossterm
  poll loop above; clipboard shells out to CLI tools; `parse_args()`/`HELP` read argv.
- **Web** (`cfg(target_arch = "wasm32")`): `main()` awaits the webfont (`wait_for_font`)
  then calls `start_web()`, which builds a ratzilla `DomBackend` and hands the loop to
  `draw_web` (re-runs each animation frame, replacing the 50ms poll). Clipboard uses the
  browser Clipboard API (`web-sys`). `Instant` comes from `web-time` (std's panics on
  wasm). See `index.html` (Trunk entry). Two ratzilla gotchas shaped this:
  - **Input is our own `keydown` listener on `window`**, not ratzilla's `on_key_event`.
    ratzilla listens on the grid element, which only gets keys while focused *and* which
    it destroys/rebuilds on a window `resize` (dropping the listener + `tabindex`). A
    window listener needs no focus and survives rebuilds. We convert the `web_sys`
    event to ratzilla's `KeyEvent` (`ev.into()`) and call `handle_web_key()`; there is
    **no** focus/"click to focus" logic anymore.
  - **Await the font before measuring.** ratzilla measures its cell size from the font
    when the backend is built and only re-measures on `resize` (see the rebuild issue
    above). On a cold load the webfont isn't ready, so it would mis-measure and the grid
    would overflow; `wait_for_font()` avoids that so no resize is ever needed.
  - **Start menu:** web has no CLI args, so it boots into `Phase::Menu` — a goal picker
    (`MENU_PRESETS` / `draw_menu`, wasm-only). `start_session()` leaves the menu; on the
    end screens `r` returns to the menu (`handle_web_key`) rather than replaying, and
    there's no `q` (can't close a tab).
When editing shared code, remember it must compile for both targets; keep anything
touching `std::process`, argv, or crossterm behind the native `cfg`. `Phase::Menu` and
the menu/key helpers are `cfg(wasm)`-only, so shared `match app.phase` arms that mention
`Menu` need a `#[cfg(target_arch = "wasm32")]` attribute.

**State machine.** Everything hangs off the `Phase` enum on `App` (key hints below
are native; web drops `q` and its `r` goes to the menu — see the web bullet above):
- `Menu` (web only) → goal picker; ↑/↓ move, Enter starts (`start_session`).
- `Writing` → keys append to `app.text`; each keystroke calls `touch()` to reset the idle clock.
- `Won` → goal reached, text frozen; `c` copies, `q` quits.
- `Dying` → idle limit exceeded; the built-in `Dissolve` effect is destroying the on-screen text. Input is ignored here.
- `Dead` → text wiped, game-over banner shown; `r` restarts, `q` quits.

Transitions happen only in `tick()` (goal check, idle-timeout → `Dying`, and
`Dying` → `Dead` once the effect reports `done()`). `run()`'s match on `app.phase`
decides which keys are live in each phase. `Ctrl+C` always quits, regardless of phase.

**Key details worth knowing before editing:**
- The dissolve animation is the hand-rolled `Dissolve` struct (no external effects crate): it blanks each cell in the body area at a deterministic per-cell point in its timeline (`cell_noise` + quad-in `progress`), scattering the text away. It needs the text to still be on screen while it plays, so `tick()` sets `phase = Dying` but does **not** clear `app.text` — the text is cleared only when entering `Dead`. `draw_body` advances (`tick`) and applies (`render`) the effect over `inner` during `Dying`.
- Fading is manual color interpolation, not an fx: `fade_color()` and `border_color()` lerp RGB from normal toward a dim/red danger tone over `fade_window` (the last 80% of `idle_limit`). All palette values come from the `cfg`-gated `theme` module (`theme::SAFE_FG`, `FADE_TO`, `DANGER`, etc.): the **native** build stays theme-neutral (`Color::Reset` + ANSI names, so it inherits the user's terminal theme), while the **web** build ships an explicit **gruvbox** palette because a browser has no terminal palette to borrow. The web page background and default foreground are set to match in `index.html`. When changing colors, edit both `theme` copies (or just the web one if the change is web-only).
- Scrolling is hand-rolled: `wrap_lines()` wraps text to the body width so the code knows the exact visual row count, then `draw_body` renders only the last screenful (keeps the cursor `█` visible). Don't replace this with ratatui's built-in wrap without also fixing the scroll-to-bottom behavior.
- Clipboard copy (`copy()` / `pipe_to_command()`) deliberately shells out to whatever CLI tool exists (`wl-copy`/`xclip`/`xsel`/`pbcopy`/`clip`) instead of using a clipboard crate — native crates drop the Linux selection when the process exits. It does not `wait()` on the child because those tools daemonize to keep serving the selection. `clip`/`clip.exe` get CRLF-normalized payloads. The wasm build has its own `copy()` that writes the browser clipboard via `web-sys` instead.

**CLI.** `parse_args()` is a hand-written arg parser (no clap). Flags: `-t/--time`
(minutes, default 5), `-w/--words`, `-i/--idle` (seconds, default 3). `-w` overrides
`-t` if both are given (last goal wins). Help text is the `HELP` const.

## Conventions

- Rust 2024 edition. Keep the single-file structure and the existing `// --- section ---` comment banners.
- The README's flag table, the `HELP` const, and `parse_args()` must stay in sync — update all three when changing CLI options.
