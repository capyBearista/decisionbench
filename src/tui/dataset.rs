//! Dataset inspection: the bundled Cases with their label per Target Model. Read-only.

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};

use super::{App, block, cut, list_lines, nav, step, theme};
use crate::batch::load_dataset;
use crate::dataset::{Case, Summary, summarize};
use crate::grade::Target;

pub const HINTS: &str = "j/k move · g/G first/last · Tab next screen · q quit";

pub struct State {
    cases: Vec<Case>,
    summary: Summary,
    selected: usize,
}

impl State {
    pub fn new() -> Result<Self, String> {
        let (_, _, cases) = load_dataset(None)?;
        Ok(Self {
            summary: summarize(&cases),
            cases,
            selected: 0,
        })
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    if let Some(nav) = nav(key) {
        let state = &mut app.dataset;
        state.selected = step(state.selected, state.cases.len(), nav);
    }
}

fn label_status(case: &Case, target: Target) -> Span<'static> {
    match case.ok.get(target) {
        Some(range) => theme::status(theme::Status::Ok, &format!("{target} {range}")),
        None => theme::status(theme::Status::Unscored, &format!("{target} unlabelled")),
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let state = &app.dataset;
    let [list, detail] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(area);
    let counts: Vec<String> = Target::ALL
        .iter()
        .zip(state.summary.labels)
        .map(|(t, n)| format!("{}{n}", t.to_string().chars().next().unwrap()))
        .collect();
    let rows: Vec<Line> = state
        .cases
        .iter()
        .map(|case| {
            // One letter per target: labelled shows its initial, unlabelled a dot.
            let marks: String = Target::ALL
                .iter()
                .map(|t| {
                    if case.ok.get(*t).is_some() {
                        t.to_string().chars().next().unwrap().to_ascii_uppercase()
                    } else {
                        '·'
                    }
                })
                .collect();
            Line::from(format!(
                "{marks} {:<6} {}",
                case.id,
                cut(&case.message, list.width.saturating_sub(16) as usize)
            ))
        })
        .collect();
    // Initials, as in the row marks: the full names would be cut off in a narrow list pane.
    let title = format!("{} cases · {}", state.summary.cases, counts.join(" "));
    let height = list.height.saturating_sub(2) as usize;
    frame.render_widget(
        Paragraph::new(list_lines(rows, state.selected, height)).block(block(&title)),
        list,
    );
    let case = &state.cases[state.selected];
    let mut text = Text::from(vec![Line::from(case.id.clone()).style(theme::focus())]);
    text.extend(Target::ALL.map(|t| Line::from(label_status(case, t))));
    text.push_line("");
    text.push_line(case.message.as_str());
    if let Some(context) = &case.context {
        text.push_line("");
        text.push_line(Span::styled("context", theme::dim()));
        text.extend(context.lines().map(Line::from));
    }
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(block("case")),
        detail,
    );
}
