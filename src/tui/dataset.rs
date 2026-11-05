//! Dataset inspection: the bundled Cases with their label per Target Model. Read-only.

use std::collections::BTreeSet;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};

use super::{App, block, cut, list_lines, nav, step, theme};
use crate::batch::load_dataset;
use crate::dataset::{Case, Effort, EffortRange, Summary, summarize};
use crate::grade::Target;
use crate::run::eligible;

/// Heavy and light horizontal lines: one thin line with a thicker span for the acceptable efforts.
const BAR_ON: char = '━';
const BAR_OFF: char = '─';
/// Status symbol, target name padded to 6, and a space: the bar starts after these.
const LABEL_WIDTH: usize = 2 + 6 + 1;
/// Two spaces and the longest range text (`medium-xhigh`).
const RANGE_WIDTH: usize = 2 + 12;

pub const HINTS: &str =
    "j/k move · g/G first/last · Space mark case · x clear marks · Tab next screen · q quit";

pub struct State {
    cases: Vec<Case>,
    summary: Summary,
    selected: usize,
    /// Case ids to run; empty means every eligible Case.
    pub marked: BTreeSet<String>,
}

impl State {
    pub fn new() -> Result<Self, String> {
        let (_, _, cases) = load_dataset(None)?;
        Ok(Self {
            summary: summarize(&cases),
            cases,
            selected: 0,
            marked: BTreeSet::new(),
        })
    }

    /// Cases a Run would plan: the marked ones, else all, less those no selected Target labels.
    pub fn planned(&self, targets: &[Target], include_unscored: bool) -> usize {
        self.cases
            .iter()
            .filter(|c| self.marked.is_empty() || self.marked.contains(&c.id))
            .filter(|c| eligible(c, targets, include_unscored))
            .count()
    }

    /// The Run setup's summary of the case choice.
    pub fn choice(&self) -> String {
        if self.marked.is_empty() {
            return "all eligible".into();
        }
        let ids: Vec<&str> = self.marked.iter().take(3).map(String::as_str).collect();
        let more = if self.marked.len() > 3 { ", …" } else { "" };
        format!("{} selected ({}{more})", self.marked.len(), ids.join(", "))
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let state = &mut app.dataset;
    match key.code {
        KeyCode::Char(' ') => {
            let id = state.cases[state.selected].id.clone();
            if !state.marked.remove(&id) {
                state.marked.insert(id);
            }
        }
        KeyCode::Char('x') => state.marked.clear(),
        _ => {
            if let Some(nav) = nav(key) {
                state.selected = step(state.selected, state.cases.len(), nav);
            }
        }
    }
}

/// The bar as runs of cells with whether they are inside the range: `seg` cells per effort level.
fn effort_bar(range: EffortRange, seg: usize) -> Vec<(String, bool)> {
    let (start, end) = (range.from as usize * seg, (range.to as usize + 1) * seg);
    let run = |cells: usize, glyph: char| glyph.to_string().repeat(cells);
    [
        (run(start, BAR_OFF), false),
        (run(end - start, BAR_ON), true),
        (run(Effort::ALL.len() * seg - end, BAR_OFF), false),
    ]
    .into_iter()
    .filter(|(cells, _)| !cells.is_empty())
    .collect()
}

/// Level names above the bar segments, each followed by a gap: the longest set that fits (full
/// names, 3 letters, 2 letters), or none below 3 cells.
fn effort_header(seg: usize) -> Option<String> {
    let sets = [
        Effort::ALL.map(Effort::name),
        ["low", "med", "hi", "xh", "max"],
        ["lo", "md", "hi", "xh", "mx"],
    ];
    let names = sets
        .into_iter()
        .find(|set| set.iter().all(|n| n.len() < seg))?;
    Some(names.iter().map(|name| format!("{name:<seg$}")).collect())
}

/// Cells per level and whether the range text fits after the bar, for a pane `inner` columns wide.
/// The range text goes first when space is short; the bar never does.
fn bar_layout(inner: usize) -> (usize, bool) {
    let seg =
        |text: usize| (inner.saturating_sub(LABEL_WIDTH + text) / Effort::ALL.len()).clamp(1, 7);
    match seg(RANGE_WIDTH) {
        2.. => (seg(RANGE_WIDTH), true),
        _ => (seg(0), false),
    }
}

fn target_line(case: &Case, target: Target, seg: usize, show_range: bool) -> Line<'static> {
    let Some(range) = case.ok.get(target) else {
        return Line::from(theme::status(
            theme::Status::Unscored,
            &format!("{target} unlabelled"),
        ));
    };
    let mut spans = vec![theme::status(
        theme::Status::Ok,
        &format!("{:<6} ", target.to_string()),
    )];
    spans.extend(
        effort_bar(range, seg)
            .into_iter()
            .map(|(cells, on)| Span::styled(cells, if on { theme::base() } else { theme::dim() })),
    );
    if show_range {
        spans.push(Span::raw(format!("  {range}")));
    }
    Line::from(spans)
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
                "{} {marks} {:<6} {}",
                if state.marked.contains(&case.id) {
                    "[x]"
                } else {
                    "[ ]"
                },
                case.id,
                cut(&case.message, list.width.saturating_sub(20) as usize)
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
    let (seg, show_range) = bar_layout(detail.width.saturating_sub(2) as usize);
    if let Some(header) = effort_header(seg) {
        text.push_line(Span::styled(
            format!("{:LABEL_WIDTH$}{header}", ""),
            theme::dim(),
        ));
    }
    text.extend(Target::ALL.map(|t| target_line(case, t, seg, show_range)));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn range(from: Effort, to: Effort) -> EffortRange {
        EffortRange { from, to }
    }

    fn run(on: bool, cells: usize) -> (String, bool) {
        let glyph = if on { BAR_ON } else { BAR_OFF };
        (glyph.to_string().repeat(cells), on)
    }

    #[test]
    fn the_bar_fills_the_levels_inside_the_range_with_no_gaps() {
        use Effort::*;
        assert_eq!(
            effort_bar(range(High, Xhigh), 3),
            [run(false, 6), run(true, 6), run(false, 3)]
        );
        assert_eq!(
            effort_bar(range(Max, Max), 2),
            [run(false, 8), run(true, 2)]
        );
        assert_eq!(effort_bar(range(Low, Max), 1), [run(true, 5)]);
        assert_eq!(
            effort_bar(range(Low, Low), 6),
            [run(true, 6), run(false, 24)]
        );
        assert_eq!(
            effort_bar(range(Medium, High), 6),
            [run(false, 6), run(true, 12), run(false, 12)]
        );
    }

    #[test]
    fn the_header_names_the_levels_in_full_abbreviated_or_not_at_all() {
        assert_eq!(
            effort_header(7).as_deref(),
            Some("low    medium high   xhigh  max    ")
        );
        assert_eq!(
            effort_header(6).as_deref(),
            Some("low   med   hi    xh    max   ")
        );
        assert_eq!(effort_header(4).as_deref(), Some("low med hi  xh  max "));
        assert_eq!(effort_header(3).as_deref(), Some("lo md hi xh mx "));
        assert_eq!(effort_header(2), None);
    }

    #[test]
    fn the_range_text_is_dropped_before_the_bar_shrinks_below_two_cells() {
        // 9 for symbol and target, 14 for the longest range text, 5 levels of `seg` cells.
        assert_eq!(bar_layout(52), (5, true));
        assert_eq!(bar_layout(23 + 10), (2, true));
        assert_eq!(bar_layout(23 + 9), (4, false));
        assert_eq!(bar_layout(25), (3, false));
        assert_eq!(bar_layout(200), (7, true));
        assert_eq!(bar_layout(0), (1, false));
    }
}
