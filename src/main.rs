// danger-write — a writing app that erases your words if you stop typing.
//
// Keep typing. If you stop for too long, the words you've written fade out
// and are erased. Reach your goal (a time limit or a word count) to survive
// and unlock the ability to copy what you wrote.

use std::borrow::Cow;
use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
    Frame,
};
use tachyonfx::{fx, Effect, EffectRenderer};

/// What you have to do to survive the session.
#[derive(Clone, Copy)]
enum Goal {
    /// Keep writing until this much time has elapsed.
    Time(Duration),
    /// Keep writing until you've written this many words.
    Words(usize),
}

#[derive(PartialEq)]
enum Phase {
    Writing,
    /// You reached the goal. Text is frozen and safe.
    Won,
    /// You paused too long. The dissolve animation is destroying your text.
    Dying,
    /// The text is gone. Game over screen is up.
    Dead,
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
    death_fx: Option<Effect>,
    /// Timestamp of the previous frame, for computing the effect's time delta.
    last_frame: Instant,
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
            self.death_fx = Some(fx::dissolve(900));
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
}

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
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('c')
                {
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
    let area = frame.area();
    let [header, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(area);

    draw_header(frame, app, header);
    draw_body(frame, app, body, frame_dt);

    match app.phase {
        Phase::Won => draw_end_banner(frame, app, area, false),
        Phase::Dead => draw_end_banner(frame, app, area, true),
        // During Writing/Dying the body is shown on its own (Dying is running
        // the dissolve, which we don't want the banner to cover).
        Phase::Writing | Phase::Dying => {}
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

    let line = Line::from(Span::styled(text, Style::default().fg(Color::DarkGray)));
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}

fn draw_body(frame: &mut Frame, app: &mut App, area: Rect, frame_dt: Duration) {
    let block = Block::bordered()
        .border_style(Style::default().fg(border_color(app)));
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
            frame.render_effect(effect, inner, frame_dt.into());
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
            Color::Red,
            "YOUR WORDS ARE GONE",
            format!(
                "{} words lost · lasted {}",
                app.lost_words,
                fmt_dur(app.elapsed())
            ),
        )
    } else {
        (
            Color::Green,
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
        Line::from(Span::styled(detail, Style::default().fg(Color::Gray))),
    ];
    // Instructions live here (and only here), with Ctrl+C to quit.
    lines.push(Line::from(""));
    let hint = if dead {
        "r restart    q quit".to_string()
    } else if app.copied {
        "copied ✓    q quit".to_string()
    } else {
        "c copy    q quit".to_string()
    };
    lines.push(Line::from(Span::styled(
        hint,
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::bordered().border_style(Style::default().fg(accent));
    let para = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .block(block);
    frame.render_widget(para, rect);
}

// --- helpers ---------------------------------------------------------------

/// Spawn `cmd args` and feed `text` to its stdin. Returns Err if the command
/// isn't installed (or the pipe fails), so the caller can try the next one.
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

/// Interpolate the writing color from bright to background as danger rises.
fn fade_color(app: &App) -> Color {
    if app.phase == Phase::Won {
        return Color::Reset;
    }
    let idle = app.last_key.elapsed();
    let fade_start = app.idle_limit.saturating_sub(app.fade_window);
    if idle <= fade_start {
        // Use the terminal's own foreground color while the text is safe, so it
        // matches the user's theme instead of a hardcoded white.
        return Color::Reset;
    }
    // t: 0 at fade start, 1 at erasure. Stop at a dim gray, not black, so the
    // text stays readable right up until it's wiped.
    let t = ((idle - fade_start).as_secs_f64() / app.fade_window.as_secs_f64())
        .clamp(0.0, 1.0);
    lerp_rgb((220, 220, 220), (90, 90, 90), t)
}

fn border_color(app: &App) -> Color {
    let base = (60, 60, 60);
    if matches!(app.phase, Phase::Dying | Phase::Dead) {
        return Color::Rgb(200, 40, 40);
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
    let t = ((idle - fade_start).as_secs_f64() / app.fade_window.as_secs_f64())
        .clamp(0.0, 1.0);
    lerp_rgb(base, (200, 40, 40), t)
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
