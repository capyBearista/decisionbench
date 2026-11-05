//! Run setup: which Candidates, Targets and settings, then Start.

use std::collections::BTreeSet;
use std::path::PathBuf;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{App, Nav, Screen, block, cut, edit, list_lines, nav, step};
use crate::batch::{RunArgs, prepare};
use crate::grade::Target;
use crate::run::Settings;

pub const HINTS: &str =
    "j/k move · Space toggle · Enter edit/start · h/l -/+ · Esc cancel edit · q quit";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Candidate(usize),
    Target(Target),
    Instructions,
    Repetitions,
    Concurrency,
    Timeout,
    Retries,
    Unscored,
    Start,
}

pub struct State {
    /// Chosen Candidate names.
    pub chosen: BTreeSet<String>,
    pub targets: BTreeSet<Target>,
    /// A Judge Instructions file; blank is the bundled `neutral`.
    pub instructions: String,
    pub repetitions: u32,
    pub concurrency: u32,
    pub timeout_secs: u64,
    pub retries: u32,
    pub include_unscored: bool,
    cursor: usize,
    /// The text being typed into the row under the cursor.
    pub editing: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            chosen: BTreeSet::new(),
            targets: Target::ALL.into_iter().collect(),
            instructions: String::new(),
            repetitions: 3,
            concurrency: 1,
            timeout_secs: 60,
            retries: 1,
            include_unscored: false,
            cursor: 0,
            editing: None,
        }
    }
}

fn rows(candidates: usize) -> Vec<Row> {
    (0..candidates)
        .map(Row::Candidate)
        .chain(Target::ALL.map(Row::Target))
        .chain([
            Row::Instructions,
            Row::Repetitions,
            Row::Concurrency,
            Row::Timeout,
            Row::Retries,
            Row::Unscored,
            Row::Start,
        ])
        .collect()
}

impl State {
    fn text(&self, row: Row) -> Option<String> {
        Some(match row {
            Row::Instructions => self.instructions.clone(),
            Row::Repetitions => self.repetitions.to_string(),
            Row::Concurrency => self.concurrency.to_string(),
            Row::Timeout => self.timeout_secs.to_string(),
            Row::Retries => self.retries.to_string(),
            _ => return None,
        })
    }

    /// Stores `text` into the row's field. Counts must be whole numbers, and all but retries at least 1.
    fn set(&mut self, row: Row, text: &str) -> Result<(), String> {
        let text = text.trim();
        if row == Row::Instructions {
            self.instructions = text.to_string();
            return Ok(());
        }
        let n: u64 = text
            .parse()
            .map_err(|_| format!("`{text}` is not a whole number"))?;
        if n == 0 && row != Row::Retries {
            return Err("must be at least 1".into());
        }
        match row {
            Row::Repetitions => self.repetitions = u32::try_from(n).map_err(|e| e.to_string())?,
            Row::Concurrency => self.concurrency = u32::try_from(n).map_err(|e| e.to_string())?,
            Row::Timeout => self.timeout_secs = n,
            Row::Retries => self.retries = u32::try_from(n).map_err(|e| e.to_string())?,
            _ => {}
        }
        Ok(())
    }
}

pub(super) fn args(app: &App) -> Result<RunArgs, String> {
    let state = &app.setup;
    if app.candidates.dirty {
        return Err("unsaved candidate edits: press s on the Candidates screen first".into());
    }
    // Config order, so the evidence lists Candidates as the config does.
    let candidates: Vec<String> = app
        .candidates
        .items
        .iter()
        .filter(|c| state.chosen.contains(&c.name))
        .map(|c| c.name.clone())
        .collect();
    if candidates.is_empty() {
        return Err("choose at least one candidate".into());
    }
    if state.targets.is_empty() {
        return Err("choose at least one target".into());
    }
    Ok(RunArgs {
        candidates,
        instructions: if state.instructions.is_empty() {
            Vec::new()
        } else {
            vec![PathBuf::from(&state.instructions)]
        },
        cases: app.dataset.marked.iter().cloned().collect(),
        targets: state.targets.iter().copied().collect(),
        settings: Settings {
            repetitions: state.repetitions,
            concurrency: state.concurrency,
            timeout_secs: state.timeout_secs,
            retries: state.retries,
            include_unscored: state.include_unscored,
        },
        config: Some(app.config.clone()),
        dataset: None,
        out: Some(app.out.clone()),
    })
}

/// Validates everything, writes the manifest and starts the engine thread.
fn start(app: &mut App) {
    if app.run.active() {
        app.message = "a run is already in progress".into();
        return;
    }
    match args(app).and_then(|a| prepare(&a)) {
        Ok(prepared) => {
            app.message = format!("started {}", prepared.path.display());
            app.run.begin(prepared);
            app.screen = Screen::Run;
        }
        Err(e) => app.message = e,
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let rows = rows(app.candidates.items.len());
    let cursor = app.setup.cursor.min(rows.len() - 1);
    let row = rows[cursor];
    if let Some(buffer) = app.setup.editing.as_mut() {
        match key.code {
            KeyCode::Esc => app.setup.editing = None,
            KeyCode::Enter => {
                let text = std::mem::take(buffer);
                match app.setup.set(row, &text) {
                    Ok(()) => app.setup.editing = None,
                    Err(e) => {
                        app.setup.editing = Some(text);
                        app.message = e;
                    }
                }
            }
            _ => edit(buffer, key),
        }
        return;
    }
    let state = &mut app.setup;
    match key.code {
        KeyCode::Char(' ') => match row {
            Row::Candidate(i) => {
                let name = app.candidates.items[i].name.clone();
                if !state.chosen.remove(&name) {
                    state.chosen.insert(name);
                }
            }
            Row::Target(t) => {
                if !state.targets.remove(&t) {
                    state.targets.insert(t);
                }
            }
            Row::Unscored => state.include_unscored = !state.include_unscored,
            _ => {}
        },
        KeyCode::Enter => match row {
            Row::Start => start(app),
            Row::Unscored => state.include_unscored = !state.include_unscored,
            _ => state.editing = state.text(row),
        },
        _ => match nav(key) {
            Some(n @ (Nav::Left | Nav::Right)) => {
                if let Some(value) = state.text(row).filter(|_| row != Row::Instructions) {
                    let n_now: u64 = value.parse().unwrap_or(1);
                    let next = if n == Nav::Right {
                        n_now + 1
                    } else {
                        n_now.saturating_sub(1)
                    };
                    // Out-of-range steps (below the minimum) are ignored.
                    let _ = state.set(row, &next.to_string());
                }
            }
            Some(n) => state.cursor = step(cursor, rows.len(), n),
            None => {}
        },
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let state = &app.setup;
    let rows_all = rows(app.candidates.items.len());
    let cursor = state.cursor.min(rows_all.len() - 1);
    let check = |on: bool| if on { "[x]" } else { "[ ]" };
    let lines: Vec<Line> = rows_all
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let typing = state.editing.as_ref().filter(|_| i == cursor);
            let field = |label: &str, row: Row| {
                let value = typing
                    .cloned()
                    .map_or_else(|| state.text(row).unwrap_or_default(), |t| format!("{t}▏"));
                Line::from(format!("{label:<22} {value}"))
            };
            match *row {
                Row::Candidate(c) => {
                    let name = &app.candidates.items[c].name;
                    Line::from(format!(
                        "{} candidate {name}",
                        check(state.chosen.contains(name))
                    ))
                }
                Row::Target(t) => {
                    Line::from(format!("{} target {t}", check(state.targets.contains(&t))))
                }
                Row::Instructions => {
                    let shown = if state.instructions.is_empty() && typing.is_none() {
                        "bundled neutral".to_string()
                    } else {
                        state.instructions.clone()
                    };
                    let mut line = field("instructions file", Row::Instructions);
                    if typing.is_none() {
                        line = Line::from(format!("{:<22} {shown}", "instructions file"));
                    }
                    line
                }
                Row::Repetitions => field("repetitions", *row),
                Row::Concurrency => field("concurrency", *row),
                Row::Timeout => field("timeout (seconds)", *row),
                Row::Retries => field("retries", *row),
                Row::Unscored => Line::from(format!(
                    "{} include unscored cases",
                    check(state.include_unscored)
                )),
                Row::Start => Line::from(Span::raw("▶ start run")),
            }
        })
        .collect();
    let height = area.height.saturating_sub(2) as usize;
    frame.render_widget(
        Paragraph::new(list_lines(lines, cursor, height)).block(block(&cut(
            &format!(
                "run setup · cases: {} (chosen on Dataset)",
                app.dataset.choice()
            ),
            // Corners and the title's padding take four columns.
            area.width.saturating_sub(4) as usize,
        ))),
        area,
    );
}
