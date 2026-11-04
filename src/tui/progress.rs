//! The Run screen: starts the engine on a background thread and shows its Trials as they finish.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, channel};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Gauge, Paragraph};

use super::{App, Nav, Screen, block, nav, theme};
use crate::batch::{Control, Prepared};
use crate::cost::plural;
use crate::evidence::RunState;

pub const HINTS: &str =
    "c cancel (again: stop waiting) · Enter results for this run · j/k scroll · q quit";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Running,
    /// Cancel requested; Trials already in flight are finishing.
    Cancelling,
    /// Stopped early; the evidence file holds what finished.
    Cancelled,
    Complete,
    Failed(String),
}

enum Msg {
    Trial {
        line: String,
        answered: bool,
        retries: usize,
    },
    Done {
        state: RunState,
        error: Option<String>,
    },
}

pub struct State {
    pub phase: Phase,
    control: Option<Arc<Control>>,
    rx: Option<Receiver<Msg>>,
    pub planned: usize,
    pub done: usize,
    /// Trials whose last Attempt was not answered.
    pub errors: usize,
    /// Attempts beyond the first, over all Trials.
    pub retries: usize,
    pub lines: Vec<String>,
    pub path: Option<PathBuf>,
    /// Lines scrolled up from the newest.
    scrolled: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            control: None,
            rx: None,
            planned: 0,
            done: 0,
            errors: 0,
            retries: 0,
            lines: Vec::new(),
            path: None,
            scrolled: 0,
        }
    }
}

impl State {
    pub fn active(&self) -> bool {
        matches!(self.phase, Phase::Running | Phase::Cancelling)
    }

    /// Runs `prepared` on a new thread; the thread is the only writer of the evidence file.
    pub fn begin(&mut self, prepared: Prepared) {
        let control = Arc::new(Control::default());
        let (tx, rx) = channel();
        *self = Self {
            phase: Phase::Running,
            planned: prepared.planned(),
            path: Some(prepared.path.clone()),
            control: Some(control.clone()),
            rx: Some(rx),
            ..Self::default()
        };
        let planned = self.planned;
        std::thread::spawn(move || {
            let outcome = prepared.run(&control, |trial, done, _| {
                let _ = tx.send(Msg::Trial {
                    line: trial.progress(done, planned),
                    answered: trial.answered(),
                    retries: trial.attempts.len().saturating_sub(1),
                });
            });
            let _ = tx.send(Msg::Done {
                state: outcome.state,
                error: outcome.write_error,
            });
        });
    }

    /// First call stops new Trials; a second stops waiting for the ones in flight. `false` when
    /// there is nothing to cancel or nothing left to wait for.
    pub fn cancel(&mut self) -> bool {
        match (&self.phase, &self.control) {
            (Phase::Running, Some(control)) => {
                control.cancel.store(true, Ordering::Relaxed);
                self.phase = Phase::Cancelling;
                true
            }
            (Phase::Cancelling, _) => {
                // Dropping the receiver abandons the in-flight Trials; the thread still ends the
                // file if it lives long enough, and the file reads as partial until then.
                self.rx = None;
                self.phase = Phase::Cancelled;
                false
            }
            _ => false,
        }
    }

    /// Applies what the engine thread sent. Returns a message when the Run ended.
    pub fn pump(&mut self) -> Option<String> {
        let mut finished = None;
        while let Some(msg) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            match msg {
                Msg::Trial {
                    line,
                    answered,
                    retries,
                } => {
                    self.done += 1;
                    self.errors += usize::from(!answered);
                    self.retries += retries;
                    self.lines.push(line);
                }
                Msg::Done { state, error } => {
                    self.phase = match (state, error) {
                        (RunState::Complete, _) => Phase::Complete,
                        (RunState::Partial, _) => Phase::Cancelled,
                        (RunState::Failed, e) => Phase::Failed(e.unwrap_or_default()),
                    };
                    self.rx = None;
                    finished = Some(format!(
                        "run ended: {}; Enter on the Run screen opens its report",
                        self.phase_label().1
                    ));
                }
            }
        }
        finished
    }

    fn phase_label(&self) -> (theme::Status, String) {
        use theme::Status::{Fail, Ok, Unknown, Warn};
        match &self.phase {
            Phase::Idle => (Unknown, "no run yet".into()),
            Phase::Running => (Unknown, "running".into()),
            Phase::Cancelling => (
                Warn,
                format!(
                    "cancelling ({} in flight finishing)",
                    self.control.as_ref().map_or(0, |c| c.in_flight())
                ),
            ),
            Phase::Cancelled => (Warn, "cancelled (partial evidence kept)".into()),
            Phase::Complete => (Ok, "complete".into()),
            Phase::Failed(e) => (Fail, format!("failed {e}")),
        }
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('c') => {
            if app.run.cancel() {
                app.message =
                    "cancelling: in-flight trials finish; press c again to stop waiting".into();
            } else if app.run.phase == Phase::Cancelled {
                app.message = "stopped waiting; the evidence file stays readable as partial".into();
                app.refresh_results();
            }
        }
        KeyCode::Enter => {
            if let Some(name) = app
                .run
                .path
                .as_ref()
                .filter(|_| !app.run.active())
                .and_then(|p| p.file_name())
            {
                let name = name.to_string_lossy().into_owned();
                app.refresh_results();
                app.results.open(&name, &app.out);
                app.screen = Screen::Results;
            }
        }
        _ => match nav(key) {
            Some(Nav::Up) => app.run.scrolled = (app.run.scrolled + 1).min(app.run.lines.len()),
            Some(Nav::Down) => app.run.scrolled = app.run.scrolled.saturating_sub(1),
            Some(Nav::Bottom) => app.run.scrolled = 0,
            _ => {}
        },
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let state = &app.run;
    let [top, log] = Layout::vertical([Constraint::Length(4), Constraint::Min(3)]).areas(area);
    let (status, label) = state.phase_label();
    let ratio = if state.planned == 0 {
        0.0
    } else {
        state.done as f64 / state.planned as f64
    };
    let inner = block("progress");
    let inside = inner.inner(top);
    frame.render_widget(inner, top);
    let [gauge, counts] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inside);
    frame.render_widget(
        Gauge::default()
            .ratio(ratio.min(1.0))
            .label(format!("{}/{}", state.done, state.planned))
            .gauge_style(theme::focus())
            .style(theme::dim()),
        gauge,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            theme::status(status, &label),
            Span::styled(
                format!(
                    " · {} · {}",
                    plural(state.errors, "error", "errors"),
                    plural(state.retries, "retry", "retries")
                ),
                theme::base(),
            ),
        ])),
        counts,
    );
    let height = log.height.saturating_sub(2) as usize;
    let end = state.lines.len().saturating_sub(state.scrolled);
    let shown: Vec<Line> = state.lines[end.saturating_sub(height)..end]
        .iter()
        .map(|l| Line::from(l.as_str()))
        .collect();
    frame.render_widget(Paragraph::new(shown).block(block("trials")), log);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> State {
        State {
            phase: Phase::Running,
            control: Some(Arc::new(Control::default())),
            rx: Some(channel().1),
            ..State::default()
        }
    }

    #[test]
    fn cancel_stops_new_trials_then_stops_waiting() {
        let mut state = running();
        assert!(state.active());
        assert!(state.cancel());
        assert_eq!(state.phase, Phase::Cancelling);
        assert!(
            state
                .control
                .as_ref()
                .unwrap()
                .cancel
                .load(Ordering::Relaxed)
        );
        assert!(state.active());
        // Second request: nothing more to ask of the engine, the view stops waiting.
        assert!(!state.cancel());
        assert_eq!(state.phase, Phase::Cancelled);
        assert!(state.rx.is_none() && !state.active());
        assert!(!state.cancel());
    }

    #[test]
    fn the_engine_finishing_decides_the_final_phase() {
        for (state, error, phase) in [
            (RunState::Complete, None, Phase::Complete),
            (RunState::Partial, None, Phase::Cancelled),
            (
                RunState::Failed,
                Some("disk full".to_string()),
                Phase::Failed("disk full".into()),
            ),
        ] {
            let (tx, rx) = channel();
            let mut view = State {
                phase: Phase::Cancelling,
                rx: Some(rx),
                ..State::default()
            };
            tx.send(Msg::Trial {
                line: "t".into(),
                answered: false,
                retries: 2,
            })
            .unwrap();
            tx.send(Msg::Done { state, error }).unwrap();
            assert!(view.pump().is_some());
            assert_eq!(view.phase, phase);
            assert_eq!((view.done, view.errors, view.retries), (1, 1, 2));
        }
    }
}
