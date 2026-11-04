//! The terminal UI. `App` is plain state and `handle_key` a pure transition, so everything but
//! `run` is testable without a terminal. The engine itself lives in `batch`; the TUI only starts
//! it on a background thread and reads its Trials from a channel.

mod candidates;
mod dataset;
mod progress;
mod results;
mod setup;
pub mod theme;

use std::path::PathBuf;
use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Tabs};

use crate::batch;

pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Candidates,
    Dataset,
    Setup,
    Run,
    Results,
}

impl Screen {
    const ALL: [Self; 5] = [
        Self::Candidates,
        Self::Dataset,
        Self::Setup,
        Self::Run,
        Self::Results,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Candidates => "1 Candidates",
            Self::Dataset => "2 Dataset",
            Self::Setup => "3 Setup",
            Self::Run => "4 Run",
            Self::Results => "5 Results",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap()
    }

    fn shifted(self, by: isize) -> Self {
        Self::ALL[(self.index() as isize + by).rem_euclid(Self::ALL.len() as isize) as usize]
    }
}

pub struct App {
    pub screen: Screen,
    pub config: PathBuf,
    pub out: PathBuf,
    pub candidates: candidates::State,
    pub dataset: dataset::State,
    pub setup: setup::State,
    pub run: progress::State,
    pub results: results::State,
    /// One line shown above the key hints: the last result or error.
    pub message: String,
    /// The last key asked to quit while Candidate edits were unsaved; a second one confirms.
    quit_armed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    None,
    Quit,
}

impl App {
    /// A missing config file is an empty Candidate list; an unreadable or invalid one is an error.
    pub fn new(config: Option<PathBuf>, out: Option<PathBuf>) -> Result<Self, String> {
        let config = batch::config_path(config.as_deref())?;
        let out = batch::runs_dir(out.as_deref())?;
        let items = if config.exists() {
            batch::load_config(&config)?
        } else {
            Vec::new()
        };
        let mut app = Self {
            screen: Screen::Candidates,
            candidates: candidates::State::new(items),
            dataset: dataset::State::new()?,
            setup: setup::State::default(),
            run: progress::State::default(),
            results: results::State::default(),
            message: String::new(),
            quit_armed: false,
            config,
            out,
        };
        app.refresh_results();
        Ok(app)
    }

    fn refresh_results(&mut self) {
        if let Err(e) = self.results.refresh(&self.out) {
            self.message = e;
        }
    }

    /// True while a text field takes the keys, so letters are not commands.
    pub fn editing(&self) -> bool {
        match self.screen {
            Screen::Candidates => self.candidates.form.is_some(),
            Screen::Setup => self.setup.editing.is_some(),
            _ => false,
        }
    }

    /// Moves Trials from the engine thread into the view; call between frames.
    pub fn tick(&mut self) {
        if let Some(finished) = self.run.pump() {
            self.refresh_results();
            self.message = finished;
        }
    }
}

/// Quitting during a Run first cancels it; a second request leaves without waiting. Unsaved
/// Candidate edits also need a second request, made right after the first; with both, that one
/// request covers the run and the edits.
fn request_quit(app: &mut App, confirmed: bool) -> Effect {
    if app.run.active() && app.candidates.dirty {
        if !confirmed {
            app.quit_armed = true;
            app.message = "run in progress and unsaved candidate edits: press again to cancel the run and quit without saving".into();
            return Effect::None;
        }
        app.run.cancel();
        return Effect::Quit;
    }
    if app.run.cancel() {
        app.message = "cancelling: finishing in-flight trials; quit again to leave now".into();
        return Effect::None;
    }
    if app.candidates.dirty && !confirmed {
        app.quit_armed = true;
        app.message =
            "unsaved candidate edits: press q again to quit without saving, or s to save".into();
        return Effect::None;
    }
    Effect::Quit
}

pub fn handle_key(app: &mut App, key: KeyEvent) -> Effect {
    let confirmed = std::mem::take(&mut app.quit_armed);
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return request_quit(app, confirmed);
    }
    if !app.editing() {
        let screen = match key.code {
            KeyCode::Char('q') => return request_quit(app, confirmed),
            KeyCode::Char(c @ '1'..='5') => Some(Screen::ALL[c as usize - '1' as usize]),
            KeyCode::Tab => Some(app.screen.shifted(1)),
            KeyCode::BackTab => Some(app.screen.shifted(-1)),
            _ => None,
        };
        if let Some(screen) = screen {
            app.screen = screen;
            return Effect::None;
        }
    }
    match app.screen {
        Screen::Candidates => candidates::handle_key(app, key),
        Screen::Dataset => dataset::handle_key(app, key),
        Screen::Setup => setup::handle_key(app, key),
        Screen::Run => progress::handle_key(app, key),
        Screen::Results => results::handle_key(app, key),
    }
    Effect::None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Nav {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Top,
    Bottom,
}

fn nav(key: KeyEvent) -> Option<Nav> {
    Some(match key.code {
        KeyCode::Up | KeyCode::Char('k') => Nav::Up,
        KeyCode::Down | KeyCode::Char('j') => Nav::Down,
        KeyCode::Left | KeyCode::Char('h') => Nav::Left,
        KeyCode::Right | KeyCode::Char('l') => Nav::Right,
        KeyCode::PageUp => Nav::PageUp,
        KeyCode::PageDown => Nav::PageDown,
        KeyCode::Home | KeyCode::Char('g') => Nav::Top,
        KeyCode::End | KeyCode::Char('G') => Nav::Bottom,
        _ => return None,
    })
}

/// The selection after a vertical move, kept inside `0..len`.
fn step(selected: usize, len: usize, nav: Nav) -> usize {
    let last = len.saturating_sub(1);
    match nav {
        Nav::Up => selected.saturating_sub(1),
        Nav::Down => (selected + 1).min(last),
        Nav::PageUp => selected.saturating_sub(10),
        Nav::PageDown => (selected + 10).min(last),
        Nav::Top => 0,
        Nav::Bottom => last,
        Nav::Left | Nav::Right => selected,
    }
    .min(last)
}

/// Types into `text`: characters append, Backspace deletes.
fn edit(text: &mut String, key: KeyEvent) {
    match key.code {
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => text.push(c),
        KeyCode::Backspace => {
            text.pop();
        }
        _ => {}
    }
}

fn block(title: &str) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Plain)
        .border_style(theme::border())
        .title(Span::styled(format!(" {title} "), theme::dim()))
        .style(theme::base())
}

/// `rows` cut to the `height` lines that keep `selected` visible, with a `▸` on the selected one.
fn list_lines<'a>(rows: Vec<Line<'a>>, selected: usize, height: usize) -> Vec<Line<'a>> {
    let start = (selected + 1).saturating_sub(height.max(1));
    rows.into_iter()
        .enumerate()
        .skip(start)
        .take(height)
        .map(|(i, mut line)| {
            if i == selected {
                line = line.style(theme::focus());
                line.spans.insert(0, Span::raw("▸ "));
            } else {
                line.spans.insert(0, Span::raw("  "));
            }
            line
        })
        .collect()
}

fn cut(text: &str, width: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= width {
        return text;
    }
    let mut cut: String = text.chars().take(width.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    frame.render_widget(Clear, area);
    frame.render_widget(Block::new().style(theme::base()), area);
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let text = format!(
            "terminal too small: need {MIN_WIDTH}x{MIN_HEIGHT}, have {}x{}",
            area.width, area.height
        );
        frame.render_widget(Paragraph::new(text).style(theme::base()), area);
        return;
    }
    let [tabs, body, message, hints] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    frame.render_widget(
        Tabs::new(Screen::ALL.map(Screen::title))
            .select(app.screen.index())
            .style(theme::dim())
            .highlight_style(theme::focus())
            .divider("│"),
        tabs,
    );
    match app.screen {
        Screen::Candidates => candidates::render(frame, body, app),
        Screen::Dataset => dataset::render(frame, body, app),
        Screen::Setup => setup::render(frame, body, app),
        Screen::Run => progress::render(frame, body, app),
        Screen::Results => results::render(frame, body, app),
    }
    frame.render_widget(
        Paragraph::new(app.message.as_str()).style(theme::base()),
        message,
    );
    let keys = match app.screen {
        Screen::Candidates => candidates::HINTS,
        Screen::Dataset => dataset::HINTS,
        Screen::Setup => setup::HINTS,
        Screen::Run => progress::HINTS,
        Screen::Results => results::HINTS,
    };
    frame.render_widget(Paragraph::new(keys).style(theme::dim()), hints);
}

/// Runs the TUI until the user quits. The terminal is restored on every way out; `ratatui::try_init`
/// also installs a panic hook that restores it before the panic message prints.
pub fn run(config: Option<PathBuf>, out: Option<PathBuf>) -> Result<(), String> {
    let mut app = App::new(config, out)?;
    let mut terminal = ratatui::try_init().map_err(|e| format!("cannot open the terminal: {e}"))?;
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    if let Some(report) = app.run.unsaved_report() {
        eprint!("{report}");
    }
    result.map_err(|e| format!("terminal error: {e}"))
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> std::io::Result<()> {
    loop {
        app.tick();
        terminal.draw(|frame| render(frame, app))?;
        // A resize event just falls through to the next draw.
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && handle_key(app, key) == Effect::Quit
        {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests;
