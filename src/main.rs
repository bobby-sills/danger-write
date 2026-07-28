// danger-write — a writing app that erases your words if you stop typing.
//
// Keep typing. If you stop for too long, the words you've written fade out
// and are erased. Reach your goal (a time limit or a word count) to survive
// and unlock the ability to copy what you wrote.

use std::io;

// A monotonic clock. Native uses std; the web build uses web-time
// (performance.now()) because std::time::Instant panics in the browser.
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};
#[cfg(target_arch = "wasm32")]
use web_time::{Duration, Instant};

// Native-only: shelling out to a clipboard CLI.
#[cfg(not(target_arch = "wasm32"))]
use std::borrow::Cow;
#[cfg(not(target_arch = "wasm32"))]
use std::io::Write;
#[cfg(not(target_arch = "wasm32"))]
use std::process::{Command, Stdio};

// Web-only: shared, callback-driven state for ratzilla's render/event loop.
#[cfg(target_arch = "wasm32")]
use std::{cell::RefCell, rc::Rc};

use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};

// Native uses crossterm for the terminal event loop.
#[cfg(not(target_arch = "wasm32"))]
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

// On the web, ratzilla supplies the backend, the render loop, and key events.
#[cfg(target_arch = "wasm32")]
use ratzilla::{DomBackend, WebRenderer};

/// What you have to do to survive the session.
#[derive(Clone, Copy)]
enum Goal {
    /// Keep writing until this much time has elapsed.
    Time(Duration),
    /// Keep writing until you've written this many words.
    // Only reachable through CLI args, which the web build has no way to pass.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Words(usize),
}

#[derive(PartialEq)]
enum Phase {
    /// Web only: the start menu for choosing a goal before the first session.
    /// Native gets its goal from CLI args, so it never enters this phase.
    #[cfg(target_arch = "wasm32")]
    Menu,
    Writing,
    /// You reached the goal. Text is frozen and safe.
    Won,
    /// You paused too long. The dissolve animation is destroying your text.
    Dying,
    /// The text is gone. Game over screen is up.
    Dead,
}

/// A self-contained dissolve animation: over `duration`, every cell in the
/// target area is blanked, each at its own deterministic point in the timeline
/// so the text scatters away rather than vanishing all at once.
struct Dissolve {
    duration: Duration,
    elapsed: Duration,
}

impl Dissolve {
    fn new(duration: Duration) -> Self {
        Self {
            duration,
            elapsed: Duration::ZERO,
        }
    }

    /// Advance the animation by one frame's worth of time.
    fn tick(&mut self, dt: Duration) {
        self.elapsed = (self.elapsed + dt).min(self.duration);
    }

    fn done(&self) -> bool {
        self.elapsed >= self.duration
    }

    /// How far along we are, eased with a quad-in curve so the dissolve starts
    /// slow and accelerates.
    fn progress(&self) -> f64 {
        let t = self.elapsed.as_secs_f64() / self.duration.as_secs_f64();
        (t * t).clamp(0.0, 1.0)
    }

    /// Blank every cell in `area` whose per-cell threshold has been passed.
    fn render(&self, buf: &mut Buffer, area: Rect) {
        let p = self.progress();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if cell_noise(x, y) < p {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.reset();
                    }
                }
            }
        }
    }
}

/// A stable pseudo-random value in `[0, 1)` for a cell position. Deterministic,
/// so each cell dissolves at the same moment on every frame.
fn cell_noise(x: u16, y: u16) -> f64 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2545_F491);
    h ^= h >> 13;
    h as f64 / (u32::MAX as f64 + 1.0)
}

struct App {
    text: String,
    goal: Goal,
    /// How long you may pause before your words start to die.
    idle_limit: Duration,
    /// The last part of the idle window during which text visibly fades.
    fade_window: Duration,
    start: Instant,
    last_key: Instant,
    phase: Phase,
    /// Elapsed session time captured when the session ends, so the frozen
    /// end screen doesn't keep ticking.
    frozen_elapsed: Option<Duration>,
    /// How many words existed at the moment of erasure (for the game-over screen).
    lost_words: usize,
    /// Set once the text has been copied to the clipboard, to confirm on screen.
    copied: bool,
    /// The dissolve effect that destroys the text on game over. Present only
    /// during the Dying phase.
    death_fx: Option<Dissolve>,
    /// Timestamp of the previous frame, for computing the effect's time delta.
    last_frame: Instant,
    /// Web only: which entry in the start menu is highlighted.
    #[cfg(target_arch = "wasm32")]
    menu_index: usize,
}

impl App {
    fn new(goal: Goal, idle_limit: Duration) -> Self {
        let now = Instant::now();
        Self {
            text: String::new(),
            goal,
            idle_limit,
            fade_window: idle_limit.mul_f64(0.8),
            start: now,
            last_key: now,
            phase: Phase::Writing,
            frozen_elapsed: None,
            lost_words: 0,
            copied: false,
            death_fx: None,
            last_frame: now,
            #[cfg(target_arch = "wasm32")]
            menu_index: 0,
        }
    }

    /// Start a brand-new session with the same goal and idle settings.
    fn restart(&mut self) {
        let now = Instant::now();
        self.text.clear();
        self.start = now;
        self.last_key = now;
        self.phase = Phase::Writing;
        self.frozen_elapsed = None;
        self.lost_words = 0;
        self.copied = false;
        self.death_fx = None;
        self.last_frame = now;
    }

    /// Session time to display: live while writing, frozen once it ends.
    fn elapsed(&self) -> Duration {
        self.frozen_elapsed.unwrap_or_else(|| self.start.elapsed())
    }

    fn word_count(&self) -> usize {
        self.text.split_whitespace().count()
    }

    fn goal_reached(&self) -> bool {
        match &self.goal {
            Goal::Time(d) => self.start.elapsed() >= *d,
            Goal::Words(n) => self.word_count() >= *n,
        }
    }

    /// Register a keystroke: resets the idle clock.
    fn touch(&mut self) {
        self.last_key = Instant::now();
    }

    /// Advance time-based state. Call every tick.
    fn tick(&mut self) {
        // Hold on the Dying phase until the dissolve animation finishes, then
        // wipe the (now invisible) text and show the game-over screen.
        if self.phase == Phase::Dying {
            if self.death_fx.as_ref().map_or(true, |e| e.done()) {
                self.text.clear();
                self.death_fx = None;
                self.phase = Phase::Dead;
            }
            return;
        }
        if self.phase != Phase::Writing {
            return;
        }
        if self.goal_reached() {
            self.frozen_elapsed = Some(self.start.elapsed());
            self.phase = Phase::Won;
            return;
        }
        if self.last_key.elapsed() >= self.idle_limit && !self.text.is_empty() {
            // Game over: freeze the timer, then dissolve the words away. The text
            // stays in place so the effect has something to destroy; it's cleared
            // once the animation completes (see the Dying branch above).
            self.lost_words = self.word_count();
            self.frozen_elapsed = Some(self.start.elapsed());
            // A slow dissolve so the destruction has some weight to it.
            self.death_fx = Some(Dissolve::new(Duration::from_millis(1400)));
            self.phase = Phase::Dying;
        }
    }

    /// Copy the surviving text to the system clipboard.
    ///
    /// Rather than depend on a native clipboard crate (which needs X11/Wayland
    /// system libraries and, on Linux, drops the selection when the process
    /// exits), we pipe the text to whichever standard clipboard CLI is present.
    /// Each of these owns the selection persistently after we're gone. We try
    /// them in order and the first one installed wins, so the same binary works
    /// on Wayland, X11, macOS, and Windows.
    #[cfg(not(target_arch = "wasm32"))]
    fn copy(&mut self) -> io::Result<()> {
        let mut candidates: Vec<(&str, &[&str])> = Vec::new();
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            candidates.push(("wl-copy", &[]));
        }
        if std::env::var_os("DISPLAY").is_some() {
            candidates.push(("xclip", &["-selection", "clipboard"]));
            candidates.push(("xsel", &["--clipboard", "--input"]));
        }
        candidates.push(("pbcopy", &[])); // macOS
        candidates.push(("clip.exe", &[])); // Windows / WSL
        candidates.push(("clip", &[])); // Windows

        for (cmd, args) in candidates {
            if pipe_to_command(cmd, args, &self.text).is_ok() {
                self.copied = true;
                return Ok(());
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no clipboard tool available",
        ))
    }

    /// Copy the surviving text to the browser clipboard via the async Clipboard
    /// API. Fire-and-forget: we don't await the returned promise (the keydown
    /// that triggered this counts as the user gesture the API requires).
    #[cfg(target_arch = "wasm32")]
    fn copy(&mut self) -> io::Result<()> {
        if let Some(win) = web_sys::window() {
            let _ = win.navigator().clipboard().write_text(&self.text);
            self.copied = true;
        }
        Ok(())
    }

    /// Move the start-menu highlight, wrapping around the preset list.
    #[cfg(target_arch = "wasm32")]
    fn menu_move(&mut self, delta: isize) {
        let n = MENU_PRESETS.len() as isize;
        self.menu_index = (self.menu_index as isize + delta).rem_euclid(n) as usize;
    }

    /// Leave the start menu and begin a fresh session with the chosen goal.
    #[cfg(target_arch = "wasm32")]
    fn start_session(&mut self, goal: Goal) {
        self.goal = goal;
        self.restart(); // resets the clock, text, and phase (→ Writing).
    }

    /// Handle a key event delivered by ratzilla in the browser. Mirrors the
    /// native key handling in `run()`, minus quitting (you can't close a tab).
    #[cfg(target_arch = "wasm32")]
    fn handle_web_key(&mut self, key: ratzilla::event::KeyEvent) {
        use ratzilla::event::KeyCode;
        // Let browser shortcuts (copy/paste/reload) pass through untouched.
        if key.ctrl || key.alt {
            return;
        }
        match self.phase {
            Phase::Menu => match key.code {
                KeyCode::Up => self.menu_move(-1),
                KeyCode::Down => self.menu_move(1),
                KeyCode::Char('k') => self.menu_move(-1),
                KeyCode::Char('j') => self.menu_move(1),
                KeyCode::Enter => {
                    let goal = MENU_PRESETS[self.menu_index].1;
                    self.start_session(goal);
                }
                _ => {}
            },
            Phase::Writing => match key.code {
                KeyCode::Char(c) => {
                    self.touch();
                    self.text.push(c);
                }
                KeyCode::Enter => {
                    self.touch();
                    self.text.push('\n');
                }
                KeyCode::Tab => {
                    self.touch();
                    self.text.push_str("    ");
                }
                KeyCode::Backspace => {
                    self.touch();
                    self.text.pop();
                }
                _ => {}
            },
            Phase::Won => match key.code {
                KeyCode::Char('c') => {
                    let _ = self.copy();
                }
                // Back to the goal picker (web has no "quit").
                KeyCode::Char('r') => self.phase = Phase::Menu,
                _ => {}
            },
            Phase::Dead => {
                // Restart returns to the goal picker rather than replaying the
                // same goal, since that's where web sessions are configured.
                if key.code == KeyCode::Char('r') {
                    self.phase = Phase::Menu;
                }
            }
            // Ignore input while the death animation plays out.
            Phase::Dying => {}
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> io::Result<()> {
    let (goal, idle_limit) = match parse_args() {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };

    let mut terminal = ratatui::init();
    let mut app = App::new(goal, idle_limit);
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    result
}

/// Web entry point. We wait for the Fira Code webfont to load *before* building
/// the terminal, then start. This matters because ratzilla measures its cell
/// size from the font when the backend is created and only re-measures on a
/// window `resize` — and its resize path rebuilds the grid element, dropping the
/// `tabindex` and key listener (breaking focus and input). Measuring against the
/// real font up front means we never need that resize, so the grid stays intact.
#[cfg(target_arch = "wasm32")]
fn main() -> io::Result<()> {
    wasm_bindgen_futures::spawn_local(async {
        wait_for_font().await;
        // Errors here would only be DOM setup failures; nothing useful to do but
        // stop, and the blank page makes it obvious.
        let _ = start_web();
    });
    Ok(())
}

/// Resolve once the Fira Code webfont is loaded (or has failed to). Calling
/// `load` both kicks off the fetch and yields a promise for its completion.
#[cfg(target_arch = "wasm32")]
async fn wait_for_font() {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        let promise = doc.fonts().load("16px \"Fira Code\"");
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
    }
}

/// Build the ratzilla backend and hand the render loop to it. `draw_web` re-runs
/// each animation frame (replacing native's 50ms poll).
///
/// Keyboard input is our own `keydown` listener on `window`, *not* ratzilla's
/// `on_key_event`. Ratzilla listens on the grid element, which only receives
/// keys while it's focused and which ratzilla throws away and rebuilds on a
/// window resize (dropping the listener). A window listener needs no focus and
/// survives those rebuilds, so typing just works — no click-to-focus required.
#[cfg(target_arch = "wasm32")]
fn start_web() -> io::Result<()> {
    use web_sys::wasm_bindgen::{JsCast, closure::Closure};

    // Start in the menu so the player picks a goal; the goal passed here is just
    // a placeholder that `start_session` replaces on selection.
    let mut app = App::new(Goal::Time(Duration::from_secs(300)), Duration::from_secs(3));
    app.phase = Phase::Menu;
    let app = Rc::new(RefCell::new(app));

    let backend = DomBackend::new()?;
    let terminal = ratatui::Terminal::new(backend)?;

    if let Some(window) = web_sys::window() {
        let app = app.clone();
        let on_key =
            Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
                // Leave browser shortcuts (copy/paste/reload/devtools) alone.
                if ev.ctrl_key() || ev.alt_key() || ev.meta_key() {
                    return;
                }
                // Swallow keys we act on so they don't scroll the page, move
                // focus, or trigger Firefox's quick-find on "/".
                let key = ev.key();
                let handled = key.chars().count() == 1
                    || matches!(
                        key.as_str(),
                        "Enter" | "Tab" | "Backspace" | "ArrowUp" | "ArrowDown"
                    );
                if handled {
                    ev.prevent_default();
                }
                app.borrow_mut().handle_web_key(ev.into());
            });
        window
            .add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref())
            .ok();
        on_key.forget(); // keep the listener alive for the page's lifetime
    }

    terminal.draw_web(move |frame| {
        let mut app = app.borrow_mut();
        let now = Instant::now();
        let frame_dt = now.duration_since(app.last_frame);
        app.last_frame = now;
        draw(frame, &mut app, frame_dt);
        app.tick();
    });

    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> io::Result<()> {
    loop {
        let now = Instant::now();
        let frame_dt = now.duration_since(app.last_frame);
        app.last_frame = now;
        terminal.draw(|frame| draw(frame, app, frame_dt))?;

        // Poll on a short timeout so fades and timers keep animating even
        // when the user isn't pressing anything.
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                // Ctrl+C always quits.
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    return Ok(());
                }

                match app.phase {
                    Phase::Writing => match key.code {
                        KeyCode::Char(c) => {
                            app.touch();
                            app.text.push(c);
                        }
                        KeyCode::Enter => {
                            app.touch();
                            app.text.push('\n');
                        }
                        KeyCode::Tab => {
                            app.touch();
                            app.text.push_str("    ");
                        }
                        KeyCode::Backspace => {
                            app.touch();
                            app.text.pop();
                        }
                        _ => {}
                    },
                    Phase::Won => match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('c') => {
                            let _ = app.copy();
                        }
                        _ => {}
                    },
                    Phase::Dead => match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('r') => app.restart(),
                        _ => {}
                    },
                    // Ignore input while the death animation plays out.
                    Phase::Dying => {}
                }
            }
        }

        app.tick();
    }
}

fn draw(frame: &mut Frame, app: &mut App, frame_dt: Duration) {
    // Web only: the start menu replaces the whole screen until a goal is chosen.
    #[cfg(target_arch = "wasm32")]
    if app.phase == Phase::Menu {
        draw_menu(frame, app);
        return;
    }

    let area = frame.area();
    let [header, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);

    draw_header(frame, app, header);
    draw_body(frame, app, body, frame_dt);

    match app.phase {
        Phase::Won => draw_end_banner(frame, app, area, false),
        Phase::Dead => draw_end_banner(frame, app, area, true),
        // During Writing/Dying the body is shown on its own (Dying is running
        // the dissolve, which we don't want the banner to cover).
        Phase::Writing | Phase::Dying => {}
        // Handled by the early return above; here only for match exhaustiveness.
        #[cfg(target_arch = "wasm32")]
        Phase::Menu => {}
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let words = app.word_count();
    let text = match &app.goal {
        Goal::Time(d) => {
            let left = d.saturating_sub(app.elapsed());
            format!("{} left    ·    {words} words", fmt_dur(left))
        }
        Goal::Words(n) => format!("{words} / {n} words"),
    };

    let line = Line::from(Span::styled(text, Style::default().fg(theme::MUTED)));
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}

fn draw_body(frame: &mut Frame, app: &mut App, area: Rect, frame_dt: Duration) {
    let block = Block::bordered().border_style(Style::default().fg(border_color(app)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Fade the text toward the background as the idle clock runs out.
    let fg = fade_color(app);
    let display = format!("{}█", app.text);

    // Wrap the text ourselves so we know the exact number of visual rows, then
    // render only the last screenful. This keeps the cursor (and the newest
    // words) visible no matter how long the text grows.
    let wrapped = wrap_lines(&display, inner.width as usize);
    let start = wrapped.len().saturating_sub(inner.height as usize);
    let visible: Vec<Line> = wrapped[start..].iter().cloned().map(Line::from).collect();

    frame.render_widget(
        Paragraph::new(visible).style(Style::default().fg(fg)),
        inner,
    );

    // On game over, dissolve the rendered text away before the banner appears.
    if app.phase == Phase::Dying {
        if let Some(effect) = app.death_fx.as_mut() {
            effect.tick(frame_dt);
            effect.render(frame.buffer_mut(), inner);
        }
    }
}

fn draw_end_banner(frame: &mut Frame, app: &App, area: Rect, dead: bool) {
    let w = 44.min(area.width);
    let h = 7.min(area.height);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    frame.render_widget(Clear, rect);

    let (accent, title, detail) = if dead {
        (
            theme::LOST,
            "YOUR WORDS ARE GONE",
            format!(
                "{} words lost · lasted {}",
                app.lost_words,
                fmt_dur(app.elapsed())
            ),
        )
    } else {
        (
            theme::WON,
            "YOU SURVIVED",
            format!("{} words written", app.word_count()),
        )
    };

    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            title,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(detail, Style::default().fg(theme::DETAIL))),
    ];
    // Instructions live here (and only here). Native can quit (Ctrl+C or q) and
    // its restart replays the same goal. The web build can't close its own tab,
    // and its restart returns to the goal picker ("menu") instead.
    lines.push(Line::from(""));
    #[cfg(not(target_arch = "wasm32"))]
    let hint = if dead {
        "r restart    q quit"
    } else if app.copied {
        "copied ✓    q quit"
    } else {
        "c copy    q quit"
    };
    #[cfg(target_arch = "wasm32")]
    let hint = if dead {
        "r menu"
    } else if app.copied {
        "copied ✓    r menu"
    } else {
        "c copy    r menu"
    };
    lines.push(Line::from(Span::styled(
        hint,
        Style::default().fg(theme::MUTED),
    )));

    let block = Block::bordered().border_style(Style::default().fg(accent));
    let para = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .block(block);
    frame.render_widget(para, rect);
}

/// Web start-menu choices. Native picks its goal from CLI args instead, so this
/// (and the menu) is web-only.
#[cfg(target_arch = "wasm32")]
const MENU_PRESETS: &[(&str, Goal)] = &[
    ("1 minute", Goal::Time(Duration::from_secs(60))),
    ("3 minutes", Goal::Time(Duration::from_secs(180))),
    ("5 minutes", Goal::Time(Duration::from_secs(300))),
    ("10 minutes", Goal::Time(Duration::from_secs(600))),
    ("100 words", Goal::Words(100)),
    ("250 words", Goal::Words(250)),
    ("500 words", Goal::Words(500)),
];

/// Draw the web start menu: a centered list of goals with one highlighted.
#[cfg(target_arch = "wasm32")]
fn draw_menu(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let w = 34.min(area.width);
    // title + subtitle + blank + presets + blank + footer, plus 2 border rows.
    let h = (MENU_PRESETS.len() as u16 + 7).min(area.height);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    frame.render_widget(Clear, rect);

    let mut lines = vec![
        Line::from(Span::styled(
            "danger-write",
            Style::default().fg(theme::WON).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "pick your goal",
            Style::default().fg(theme::DETAIL),
        )),
        Line::from(""),
    ];
    for (i, (label, _)) in MENU_PRESETS.iter().enumerate() {
        let selected = i == app.menu_index;
        let style = if selected {
            Style::default()
                .fg(theme::SAFE_FG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::MUTED)
        };
        let marker = if selected { "▸ " } else { "  " };
        lines.push(Line::from(Span::styled(format!("{marker}{label}"), style)));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "↑/↓ move · enter start",
        Style::default().fg(theme::MUTED),
    )));

    let border = Color::Rgb(
        theme::BORDER_CALM.0,
        theme::BORDER_CALM.1,
        theme::BORDER_CALM.2,
    );
    let block = Block::bordered().border_style(Style::default().fg(border));
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .block(block),
        rect,
    );
}

// --- helpers ---------------------------------------------------------------

/// Spawn `cmd args` and feed `text` to its stdin. Returns Err if the command
/// isn't installed (or the pipe fails), so the caller can try the next one.
#[cfg(not(target_arch = "wasm32"))]
fn pipe_to_command(cmd: &str, args: &[&str], text: &str) -> io::Result<()> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Windows apps expect CRLF line endings, so normalize when feeding `clip`.
    // macOS/Linux tools take our bare LF unchanged.
    let payload = if cmd.starts_with("clip") {
        Cow::Owned(text.replace('\n', "\r\n"))
    } else {
        Cow::Borrowed(text)
    };
    // Write, then drop the pipe so the tool sees EOF. We don't wait(): several
    // of these (wl-copy, xclip, xsel) daemonize to keep serving the selection.
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(payload.as_bytes())?;
    }
    Ok(())
}

// --- theme -----------------------------------------------------------------
// Palette, split by target. The native (terminal) build stays theme-neutral:
// it borrows the terminal's own colors via `Color::Reset` and ANSI names, so it
// looks right in whatever theme the user runs. The web build has no terminal to
// borrow from, so it ships an explicit gruvbox palette (morhetz/gruvbox); the
// page background and default foreground are set to match in `index.html`.
//
// Flat `Color`s where we render directly; `(u8, u8, u8)` where we interpolate.

#[cfg(not(target_arch = "wasm32"))]
mod theme {
    use super::Color;
    pub const MUTED: Color = Color::DarkGray; // header + end-screen hints
    pub const SAFE_FG: Color = Color::Reset; // live text, before any fade
    pub const FADE_FROM: (u8, u8, u8) = (220, 220, 220);
    pub const FADE_TO: (u8, u8, u8) = (90, 90, 90);
    pub const BORDER_CALM: (u8, u8, u8) = (60, 60, 60);
    pub const DANGER: (u8, u8, u8) = (200, 40, 40);
    pub const WON: Color = Color::Green;
    pub const LOST: Color = Color::Red;
    pub const DETAIL: Color = Color::Gray; // end-screen sub-line
}

#[cfg(target_arch = "wasm32")]
mod theme {
    use super::Color;
    pub const MUTED: Color = Color::Rgb(0x92, 0x83, 0x74); // gray_245
    pub const SAFE_FG: Color = Color::Rgb(0xeb, 0xdb, 0xb2); // light1
    pub const FADE_FROM: (u8, u8, u8) = (0xeb, 0xdb, 0xb2); // light1
    pub const FADE_TO: (u8, u8, u8) = (0x66, 0x5c, 0x54); // dark3
    pub const BORDER_CALM: (u8, u8, u8) = (0x50, 0x49, 0x45); // dark2
    pub const DANGER: (u8, u8, u8) = (0xfb, 0x49, 0x34); // bright_red
    pub const WON: Color = Color::Rgb(0xb8, 0xbb, 0x26); // bright_green
    pub const LOST: Color = Color::Rgb(0xfb, 0x49, 0x34); // bright_red
    pub const DETAIL: Color = Color::Rgb(0xa8, 0x99, 0x84); // light4
}

/// Interpolate the writing color from bright to danger-dim as idle time rises.
fn fade_color(app: &App) -> Color {
    if app.phase == Phase::Won {
        return theme::SAFE_FG;
    }
    let idle = app.last_key.elapsed();
    let fade_start = app.idle_limit.saturating_sub(app.fade_window);
    if idle <= fade_start {
        return theme::SAFE_FG;
    }
    // t: 0 at fade start, 1 at erasure. Stop at a dim tone, not black, so the
    // text stays readable right up until it's wiped.
    let t = ((idle - fade_start).as_secs_f64() / app.fade_window.as_secs_f64()).clamp(0.0, 1.0);
    lerp_rgb(theme::FADE_FROM, theme::FADE_TO, t)
}

fn border_color(app: &App) -> Color {
    let base = theme::BORDER_CALM;
    if matches!(app.phase, Phase::Dying | Phase::Dead) {
        let d = theme::DANGER;
        return Color::Rgb(d.0, d.1, d.2);
    }
    if app.phase != Phase::Writing {
        return Color::Rgb(base.0, base.1, base.2);
    }
    let idle = app.last_key.elapsed();
    let fade_start = app.idle_limit.saturating_sub(app.fade_window);
    if idle <= fade_start {
        return Color::Rgb(base.0, base.1, base.2);
    }
    // Push the border toward red over the same window the text fades, so the
    // whole frame reddens as erasure approaches.
    let t = ((idle - fade_start).as_secs_f64() / app.fade_window.as_secs_f64()).clamp(0.0, 1.0);
    lerp_rgb(base, theme::DANGER, t)
}

fn lerp_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f64) -> Color {
    let f = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    Color::Rgb(f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}

fn fmt_dur(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// Word-wrap `text` to `width` columns, hard-breaking any word longer than the
/// line. Returns one String per visual row, and always at least one row per
/// logical line, so callers can rely on the count for scrolling.
fn wrap_lines(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    for logical in text.split('\n') {
        let mut cur = String::new();
        for word in logical.split(' ') {
            if word.chars().count() > width {
                // A single word longer than the line: flush, then hard-break it.
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                for ch in word.chars() {
                    if cur.chars().count() == width {
                        out.push(std::mem::take(&mut cur));
                    }
                    cur.push(ch);
                }
                continue;
            }
            let need = if cur.is_empty() {
                word.chars().count()
            } else {
                cur.chars().count() + 1 + word.chars().count()
            };
            if need > width {
                out.push(std::mem::take(&mut cur));
                cur = word.to_string();
            } else {
                if !cur.is_empty() {
                    cur.push(' ');
                }
                cur.push_str(word);
            }
        }
        out.push(cur);
    }
    out
}

// --- CLI -------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
fn parse_args() -> Result<(Goal, Duration), String> {
    let mut goal: Option<Goal> = None;
    let mut idle = Duration::from_secs(3);
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-t" | "--time" => {
                let v = args.next().ok_or("--time needs a value in minutes")?;
                let mins: f64 = v.parse().map_err(|_| "invalid minutes")?;
                goal = Some(Goal::Time(Duration::from_secs_f64(mins * 60.0)));
            }
            "-w" | "--words" => {
                let v = args.next().ok_or("--words needs a value")?;
                let n: usize = v.parse().map_err(|_| "invalid word count")?;
                goal = Some(Goal::Words(n));
            }
            "-i" | "--idle" => {
                let v = args.next().ok_or("--idle needs a value in seconds")?;
                let s: f64 = v.parse().map_err(|_| "invalid idle seconds")?;
                idle = Duration::from_secs_f64(s);
            }
            "-h" | "--help" => {
                return Err(HELP.to_string());
            }
            other => return Err(format!("unknown argument: {other}\n\n{HELP}")),
        }
    }

    Ok((goal.unwrap_or(Goal::Time(Duration::from_secs(300))), idle))
}

#[cfg(not(target_arch = "wasm32"))]
const HELP: &str = "\
danger-write: a writing app that erases your words if you stop typing

USAGE:
    danger-write [options]

OPTIONS:
    -t, --time <MINUTES>   survive by writing for this long (default: 5)
    -w, --words <N>        survive by reaching this many words
    -i, --idle <SECONDS>   idle time before erasure (default: 3)
    -h, --help             show this help

Stop typing longer than the idle limit and everything you wrote is erased.
Reach your goal to unlock copying your words to the clipboard.";
