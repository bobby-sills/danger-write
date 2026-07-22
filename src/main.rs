// danger-write — a terminal "Most Dangerous Writing App".
//
// Keep typing. If you stop for too long, the words you've written fade out
// and are erased. Reach your goal (a time limit or a word count) to survive
// and unlock the ability to save what you wrote.

use std::borrow::Cow;
use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Wrap},
    Frame,
};

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
    /// You paused too long. Your words were erased. Game over.
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
}

impl App {
    fn new(goal: Goal, idle_limit: Duration) -> Self {
        let now = Instant::now();
        Self {
            text: String::new(),
            goal,
            idle_limit,
            fade_window: idle_limit.mul_f64(0.4).min(Duration::from_secs(2)),
            start: now,
            last_key: now,
            phase: Phase::Writing,
            frozen_elapsed: None,
            lost_words: 0,
            copied: false,
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
        if self.phase != Phase::Writing {
            return;
        }
        if self.goal_reached() {
            self.frozen_elapsed = Some(self.start.elapsed());
            self.phase = Phase::Won;
            return;
        }
        if self.last_key.elapsed() >= self.idle_limit && !self.text.is_empty() {
            // Game over: freeze the timer, wipe the words, stay on this screen.
            self.lost_words = self.word_count();
            self.frozen_elapsed = Some(self.start.elapsed());
            self.text.clear();
            self.phase = Phase::Dead;
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
        terminal.draw(|frame| draw(frame, app))?;

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
                }
            }
        }

        app.tick();
    }
}

fn draw(frame: &mut Frame, app: &App) {
    let [header, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(frame.area());

    draw_header(frame, app, header);
    draw_body(frame, app, body);

    match app.phase {
        Phase::Won => draw_end_banner(frame, app, frame.area(), false),
        Phase::Dead => draw_end_banner(frame, app, frame.area(), true),
        Phase::Writing => {}
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

fn draw_body(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered()
        .border_style(Style::default().fg(Color::Rgb(60, 60, 60)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Fade the text toward the background as the idle clock runs out.
    let fg = fade_color(app);
    let display = format!("{}█", app.text);

    let scroll = scroll_to_bottom(&display, inner.width, inner.height);
    let paragraph = Paragraph::new(display)
        .style(Style::default().fg(fg))
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, inner);
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
        return Color::Rgb(220, 220, 220);
    }
    let idle = app.last_key.elapsed();
    let fade_start = app.idle_limit.saturating_sub(app.fade_window);
    if idle <= fade_start {
        return Color::Rgb(220, 220, 220);
    }
    // t: 0 at fade start, 1 at erasure. Stop at a dim gray, not black, so the
    // text stays readable right up until it's wiped.
    let t = ((idle - fade_start).as_secs_f64() / app.fade_window.as_secs_f64())
        .clamp(0.0, 1.0);
    lerp_rgb((220, 220, 220), (90, 90, 90), t)
}

fn lerp_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f64) -> Color {
    let f = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    Color::Rgb(f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}

fn fmt_dur(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// Estimate how many rows to scroll so the tail of the text stays visible.
fn scroll_to_bottom(text: &str, width: u16, height: u16) -> u16 {
    if width == 0 {
        return 0;
    }
    let width = width as usize;
    let mut rows = 0usize;
    for logical in text.split('\n') {
        let len = logical.chars().count();
        rows += (len / width) + 1;
    }
    (rows as u16).saturating_sub(height)
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
danger-write — a terminal Most Dangerous Writing App

USAGE:
    danger-write [options]

OPTIONS:
    -t, --time <MINUTES>   survive by writing for this long (default: 5)
    -w, --words <N>        survive by reaching this many words
    -i, --idle <SECONDS>   idle time before erasure (default: 3)
    -h, --help             show this help

Stop typing longer than the idle limit and everything you wrote is erased.
Reach your goal to unlock copying your words to the clipboard.";
