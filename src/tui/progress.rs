//! The Run screen: starts the engine on a background thread and shows its Trials as they finish.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Gauge, Paragraph};

use super::{App, Nav, Screen, block, nav, theme};
use crate::batch::{Control, Prepared};
use crate::cost::plural;
use crate::evidence::{self, RunState};

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
        /// The Trial's evidence line when the file did not take it.
        unsaved: Option<String>,
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
    /// Evidence lines of Trials the file did not take: the failed write and every Trial after it.
    unsaved: Vec<String>,
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
            unsaved: Vec::new(),
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
            let outcome = prepared.run(&control, |trial, done, unsaved| {
                let _ = tx.send(Msg::Trial {
                    unsaved: unsaved
                        .then(|| evidence::printable(&evidence::Line::Trial(trial.clone()))),
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
        while let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok(Msg::Trial {
                    line,
                    answered,
                    retries,
                    unsaved,
                }) => {
                    self.unsaved.extend(unsaved);
                    self.done += 1;
                    self.errors += usize::from(!answered);
                    self.retries += retries;
                    self.lines.push(line);
                }
                Ok(Msg::Done { state, error }) => {
                    self.phase = match (state, error) {
                        (RunState::Complete, _) => Phase::Complete,
                        (RunState::Partial, _) => Phase::Cancelled,
                        (RunState::Failed, e) => Phase::Failed(e.unwrap_or_default()),
                    };
                    self.rx = None;
                    finished = Some(self.ended());
                }
                Err(TryRecvError::Empty) => break,
                // The sender is gone without a Done: the thread died (panic) mid-run.
                Err(TryRecvError::Disconnected) => {
                    self.phase = Phase::Failed("engine thread stopped".into());
                    self.rx = None;
                    finished = Some(self.ended());
                }
            }
        }
        finished
    }

    /// What to print to stderr once the terminal is restored, so no paid Trial is lost.
    pub fn unsaved_report(&self) -> Option<String> {
        let path = self.path.as_ref()?;
        let mut report = String::new();
        if !self.unsaved.is_empty() {
            report = format!(
                "warning: {} not saved to {}; their records follow:\n{}",
                plural(self.unsaved.len(), "trial was", "trials were"),
                path.display(),
                self.unsaved.concat()
            );
        }
        // Only after the second cancel: the engine is still waiting on these, and they are lost.
        let lost = match (&self.phase, &self.control) {
            (Phase::Cancelled, Some(control)) => control.in_flight(),
            _ => 0,
        };
        if lost > 0 {
            report += &format!(
                "warning: {} not recorded in {}\n",
                plural(
                    lost,
                    "trial still in flight was",
                    "trials still in flight were"
                ),
                path.display()
            );
        }
        (!report.is_empty()).then_some(report)
    }

    fn ended(&self) -> String {
        format!(
            "run ended: {}; Enter on the Run screen opens its report",
            self.phase_label().1
        )
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
            if state.unsaved.is_empty() {
                Span::raw("")
            } else {
                theme::status(
                    theme::Status::Fail,
                    " · write failed: unsaved trials print on exit",
                )
            },
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
                unsaved: None,
            })
            .unwrap();
            tx.send(Msg::Done { state, error }).unwrap();
            assert!(view.pump().is_some());
            assert_eq!(view.phase, phase);
            assert_eq!((view.done, view.errors, view.retries), (1, 1, 2));
        }
    }

    #[test]
    fn trials_the_file_did_not_take_are_kept_for_the_exit_report() {
        let (tx, rx) = channel();
        let mut view = State {
            phase: Phase::Running,
            rx: Some(rx),
            path: Some(PathBuf::from("runs/r.jsonl")),
            ..State::default()
        };
        assert_eq!(view.unsaved_report(), None);
        for unsaved in [
            None,
            Some("{\"a\":1}\n".to_string()),
            Some("{\"b\":2}\n".to_string()),
        ] {
            tx.send(Msg::Trial {
                line: "t".into(),
                answered: true,
                retries: 0,
                unsaved,
            })
            .unwrap();
        }
        view.pump();
        assert_eq!(
            view.unsaved_report().unwrap(),
            "warning: 2 trials were not saved to runs/r.jsonl; their records follow:\n{\"a\":1}\n{\"b\":2}\n"
        );
    }

    #[test]
    fn stopping_the_wait_reports_the_trials_still_in_flight() {
        let control = Arc::new(Control::default());
        control.started.store(5, Ordering::Relaxed);
        control.recorded.store(4, Ordering::Relaxed);
        let mut view = State {
            phase: Phase::Cancelled,
            control: Some(control.clone()),
            path: Some(PathBuf::from("runs/r.jsonl")),
            ..State::default()
        };
        assert_eq!(
            view.unsaved_report().unwrap(),
            "warning: 1 trial still in flight was not recorded in runs/r.jsonl\n"
        );
        // Nothing in flight, and a cancel that is still waiting, report nothing.
        control.recorded.store(5, Ordering::Relaxed);
        assert_eq!(view.unsaved_report(), None);
        control.recorded.store(0, Ordering::Relaxed);
        view.phase = Phase::Cancelling;
        assert_eq!(view.unsaved_report(), None);
        // Unsaved records and lost in-flight Trials are both reported.
        view.phase = Phase::Cancelled;
        view.unsaved = vec!["{\"a\":1}\n".into()];
        let report = view.unsaved_report().unwrap();
        assert!(
            report.starts_with("warning: 1 trial was not saved"),
            "{report}"
        );
        assert!(
            report
                .ends_with("warning: 5 trials still in flight were not recorded in runs/r.jsonl\n"),
            "{report}"
        );
    }

    #[test]
    fn an_engine_thread_that_vanishes_fails_the_run() {
        let (tx, rx) = channel();
        let mut view = State {
            phase: Phase::Running,
            rx: Some(rx),
            ..State::default()
        };
        drop(tx);
        assert!(view.pump().is_some());
        assert_eq!(view.phase, Phase::Failed("engine thread stopped".into()));
        assert!(view.rx.is_none() && !view.active());
    }
}
