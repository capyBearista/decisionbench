//! Results and history: the saved Runs in the evidence directory. A Run is shown with exactly the
//! text `show` prints, and marked Runs with the text `compare` prints, so CLI and TUI agree.

use std::collections::BTreeSet;
use std::path::Path;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{App, block, cut, list_lines, nav, step, theme};
use crate::batch::{load_run, read_runs};
use crate::evidence::{RunFile, RunState};
use crate::report;

pub const HINTS: &str =
    "j/k move · Space mark · Enter report (2+ marked: compare) · m compare · Esc back · q quit";

struct Entry {
    name: String,
    run: Result<RunFile, String>,
}

struct View {
    title: String,
    text: String,
    scroll: usize,
}

#[derive(Default)]
pub struct State {
    entries: Vec<Entry>,
    selected: usize,
    /// File names marked for comparison.
    marked: BTreeSet<String>,
    view: Option<View>,
}

impl State {
    /// Re-reads `dir`, newest first, unreadable files last. Selection and marks follow the file names.
    pub fn refresh(&mut self, dir: &Path) -> Result<(), String> {
        let keep = self.entries.get(self.selected).map(|e| e.name.clone());
        let mut entries: Vec<Entry> = read_runs(dir)?
            .into_iter()
            .map(|(name, run)| Entry { name, run })
            .collect();
        entries.sort_by_key(|e| {
            let created = e.run.as_ref().map_or(0, |r| r.manifest.created_unix_ms);
            (e.run.is_err(), std::cmp::Reverse(created), e.name.clone())
        });
        self.entries = entries;
        self.selected = keep
            .and_then(|k| self.entries.iter().position(|e| e.name == k))
            .unwrap_or(0);
        self.marked
            .retain(|m| self.entries.iter().any(|e| &e.name == m));
        Ok(())
    }

    /// Selects `name` and shows its report.
    pub fn open(&mut self, name: &str, dir: &Path) {
        if let Some(i) = self.entries.iter().position(|e| e.name == name) {
            self.selected = i;
            self.show_selected(dir);
        }
    }

    fn show_selected(&mut self, dir: &Path) {
        let Some(entry) = self.entries.get(self.selected) else {
            return;
        };
        // Read again so the text is what `show` prints for the file as it is now.
        let text = load_run(&dir.join(&entry.name)).map(|run| report::report(&run).to_string());
        self.view = Some(match text {
            Ok(text) => View {
                title: format!("report {}", entry.name),
                text,
                scroll: 0,
            },
            Err(e) => View {
                title: format!("report {}", entry.name),
                text: e,
                scroll: 0,
            },
        });
    }

    fn compare_marked(&mut self, dir: &Path) -> Result<(), String> {
        let runs: Vec<RunFile> = self
            .entries
            .iter()
            .filter(|e| self.marked.contains(&e.name))
            .map(|e| load_run(&dir.join(&e.name)))
            .collect::<Result<_, _>>()?;
        if runs.len() < 2 {
            return Err("mark two or more readable runs with Space to compare".into());
        }
        self.view = Some(View {
            title: format!("compare {} runs", runs.len()),
            text: report::Comparison(&runs).to_string(),
            scroll: 0,
        });
        Ok(())
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let dir = app.out.clone();
    let state = &mut app.results;
    if let Some(view) = state.view.as_mut() {
        if key.code == KeyCode::Esc {
            state.view = None;
        } else if let Some(n) = nav(key) {
            let last = view.text.lines().count().saturating_sub(1);
            view.scroll = step(view.scroll, last + 1, n);
        }
        return;
    }
    match key.code {
        KeyCode::Char(' ') => {
            if let Some(Entry { name, run: Ok(_) }) = state.entries.get(state.selected)
                && !state.marked.remove(name)
            {
                state.marked.insert(name.clone());
            }
        }
        KeyCode::Char('r') => app.refresh_results(),
        KeyCode::Char('m') => {
            if let Err(e) = state.compare_marked(&dir) {
                app.message = e;
            }
        }
        KeyCode::Enter => {
            if state.marked.len() >= 2 {
                if let Err(e) = state.compare_marked(&dir) {
                    app.message = e;
                }
            } else {
                state.show_selected(&dir);
            }
        }
        _ => {
            if let Some(n) = nav(key) {
                state.selected = step(state.selected, state.entries.len(), n);
            }
        }
    }
}

fn state_status(run: &RunFile) -> Span<'static> {
    match run.end.as_ref().map(|e| e.state) {
        Some(RunState::Complete) => theme::status(theme::Status::Ok, "complete"),
        Some(RunState::Failed) => theme::status(theme::Status::Fail, "failed"),
        Some(RunState::Partial) => theme::status(theme::Status::Warn, "partial (cancelled)"),
        None => theme::status(theme::Status::Warn, "partial (interrupted)"),
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let state = &app.results;
    if let Some(view) = &state.view {
        // No wrapping: every report line is shown as `show` prints it, cut only by the terminal.
        let lines: Vec<Line> = view
            .text
            .lines()
            .skip(view.scroll)
            .map(Line::from)
            .collect();
        frame.render_widget(Paragraph::new(lines).block(block(&view.title)), area);
        return;
    }
    let rows: Vec<Line> = state
        .entries
        .iter()
        .map(|e| {
            let mark = if state.marked.contains(&e.name) {
                "[x]"
            } else {
                "[ ]"
            };
            match &e.run {
                Ok(run) => {
                    let m = &run.manifest;
                    let names: Vec<&str> = m
                        .candidates
                        .iter()
                        .map(|c| c.candidate.name.as_str())
                        .collect();
                    Line::from(vec![
                        Span::raw(format!("{mark} {:<24} ", cut(&e.name, 24))),
                        state_status(run),
                        Span::raw(format!(
                            " {}/{} {}",
                            run.trials.len(),
                            m.planned_trials,
                            names.join(",")
                        )),
                    ])
                }
                Err(err) => Line::from(vec![
                    Span::raw(format!("{mark} {:<24} ", cut(&e.name, 24))),
                    theme::status(
                        theme::Status::Fail,
                        &format!("unreadable: {}", cut(err, 30)),
                    ),
                ]),
            }
        })
        .collect();
    let rows = if rows.is_empty() {
        vec![Line::from(Span::styled(
            format!("no runs in {}", app.out.display()),
            theme::dim(),
        ))]
    } else {
        rows
    };
    let height = area.height.saturating_sub(2) as usize;
    frame.render_widget(
        Paragraph::new(list_lines(rows, state.selected, height))
            .block(block(&format!("runs · {}", app.out.display()))),
        area,
    );
}
