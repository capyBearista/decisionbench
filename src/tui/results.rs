//! Results and history: the saved Runs in the evidence directory. A Run opens on a summary built
//! from the report engine's matrix, and a second panel shows exactly the text `show` prints; marked
//! Runs do the same with the text `compare` prints, so CLI and TUI agree.

use std::cell::Cell;
use std::path::Path;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{App, Nav, block, cut, list_lines, nav, step, theme};
use crate::batch::{load_run, read_runs};
use crate::evidence::{RunFile, RunState, RunSummary};
use crate::report;

pub const HINTS: &str =
    "j/k move · Space mark · Enter open · m compare · [ ] panel · Esc back · q quit";

struct Entry {
    name: String,
    run: Result<RunSummary, String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    Summary,
    Full,
}

/// A line of a panel. Prose wraps to the pane's width; a table line keeps its columns and is read
/// by scrolling sideways, as is every line of the full report.
struct Row {
    line: Line<'static>,
    prose: bool,
}

impl Row {
    fn table(line: Line<'static>) -> Self {
        Self { line, prose: false }
    }

    fn prose(line: Line<'static>) -> Self {
        Self { line, prose: true }
    }
}

struct View {
    title: String,
    panel: Panel,
    /// The matrix, styled, and the prose around it.
    summary: Vec<Row>,
    /// The text `show` or `compare` prints.
    full: Vec<Row>,
    scroll: usize,
    /// Columns cut off the left of every table line, so wide lines can be read to their end.
    hscroll: usize,
    /// Columns inside the pane when it was last drawn, which prose wraps to; the key handler has
    /// no other way to count the rows it scrolls over. Nothing wraps before the first draw.
    width: Cell<usize>,
}

const HSTEP: usize = 8;

fn plain_rows(text: &str, prose: bool) -> Vec<Row> {
    text.lines()
        .map(|l| {
            let line = Line::from(l.to_string());
            if prose {
                Row::prose(line)
            } else {
                Row::table(line)
            }
        })
        .collect()
}

fn columns(line: &Line) -> usize {
    line.spans.iter().map(|s| s.content.chars().count()).sum()
}

/// `line` broken into rows of at most `width` columns, each keeping the styles of its text. Breaks
/// fall on spaces; a word longer than a row is cut. A bullet (`  - text`) continues under its text.
fn wrap(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    type Cells = Vec<(char, Style)>;
    let width = width.max(1);
    let cells: Cells = line
        .spans
        .iter()
        .flat_map(|s| s.content.chars().map(|c| (c, s.style)))
        .collect();
    let lead = cells.iter().take_while(|(c, _)| *c == ' ').count();
    let bullet = matches!(cells.get(lead..lead + 2), Some([('-', _), (' ', _)]));
    let indent = lead + if bullet { 2 } else { 0 };
    // An indent that leaves no room for text is dropped.
    let (head, mut rest) = if indent < width {
        cells.split_at(indent)
    } else {
        cells[lead..].split_at(0)
    };
    let hang = if bullet && indent < width { indent } else { 0 };
    let fresh = || -> Cells { vec![(' ', Style::default()); hang] };
    let (mut rows, mut row, mut filled) = (Vec::<Cells>::new(), head.to_vec(), false);
    while let Some(&(first, _)) = rest.first() {
        let space = first == ' ';
        let len = rest
            .iter()
            .take_while(|(c, _)| (*c == ' ') == space)
            .count();
        let (token, tail) = rest.split_at(len);
        rest = tail;
        if space {
            // Spaces at the start of a row are indent, not text.
            if filled {
                row.extend_from_slice(token);
            }
            continue;
        }
        let mut word = token;
        while row.len() + word.len() > width {
            if !filled {
                // Nothing to move to the next row: the word itself is longer than a row.
                let (now, later) = word.split_at(width - row.len());
                row.extend_from_slice(now);
                word = later;
            }
            rows.push(std::mem::replace(&mut row, fresh()));
            filled = false;
        }
        row.extend_from_slice(word);
        filled = true;
    }
    rows.push(row);
    rows.into_iter()
        .map(|mut cells| {
            while cells.last().is_some_and(|(c, _)| *c == ' ') {
                cells.pop();
            }
            let mut runs: Vec<(String, Style)> = Vec::new();
            for (c, style) in cells {
                match runs.last_mut() {
                    Some((text, last)) if *last == style => text.push(c),
                    _ => runs.push((c.to_string(), style)),
                }
            }
            runs.into_iter()
                .map(|(text, style)| Span::styled(text, style))
                .collect()
        })
        .collect()
}

/// `line` without its first `n` columns; the rest keeps its styles.
fn skip_cols(line: &Line<'static>, mut n: usize) -> Line<'static> {
    line.spans
        .iter()
        .filter_map(|span| {
            let len = span.content.chars().count();
            let dropped = n.min(len);
            n -= dropped;
            (dropped < len).then(|| {
                Span::styled(
                    span.content.chars().skip(dropped).collect::<String>(),
                    span.style,
                )
            })
        })
        .collect()
}

impl View {
    /// Opens on the summary.
    fn new(title: String, summary: Vec<Row>, full: &str) -> Self {
        Self {
            title,
            panel: Panel::Summary,
            summary,
            full: plain_rows(full, false),
            scroll: 0,
            hscroll: 0,
            width: Cell::new(usize::MAX),
        }
    }

    fn lines(&self) -> &[Row] {
        match self.panel {
            Panel::Summary => &self.summary,
            Panel::Full => &self.full,
        }
    }

    fn title(&self) -> String {
        let panel = match self.panel {
            Panel::Summary => "summary",
            Panel::Full => "full report",
        };
        format!("{} · {panel}", self.title)
    }

    /// Shows `panel` from its top left; the panel already shown stays where it is.
    fn select(&mut self, panel: Panel) {
        if self.panel != panel {
            (self.panel, self.scroll, self.hscroll) = (panel, 0, 0);
        }
    }

    /// The rows shown at `width` columns, from the first: prose wrapped, table lines cut by `hscroll`.
    fn display(&self, width: usize) -> Vec<Line<'static>> {
        self.lines()
            .iter()
            .flat_map(|row| {
                if row.prose {
                    wrap(&row.line, width)
                } else {
                    vec![skip_cols(&row.line, self.hscroll)]
                }
            })
            .collect()
    }

    /// Moves the view: vertical keys scroll rows (wrapped prose counts every row it takes),
    /// Left/Right shift table lines by `HSTEP` columns, kept between 0 and the widest one.
    fn scroll(&mut self, nav: Nav) {
        let rows = self.display(self.width.get()).len();
        let widest = self
            .lines()
            .iter()
            .filter(|r| !r.prose)
            .map(|r| columns(&r.line))
            .max();
        match nav {
            Nav::Left => self.hscroll = self.hscroll.saturating_sub(HSTEP),
            Nav::Right => {
                self.hscroll = (self.hscroll + HSTEP).min(widest.unwrap_or(0).saturating_sub(1));
            }
            // A resize can leave `scroll` past the last row: start from the last one.
            _ => self.scroll = step(self.scroll.min(rows.saturating_sub(1)), rows, nav),
        }
    }
}

fn delta_style(kind: report::DeltaKind) -> Style {
    match kind {
        report::DeltaKind::Gained => theme::base().fg(theme::STATUS_OK),
        report::DeltaKind::Lost => theme::base().fg(theme::STATUS_FAIL),
        report::DeltaKind::Noise => theme::dim(),
    }
}

/// The matrix as a table laid out like `Display for Matrix` (left aligned, two spaces between
/// columns, no indent), each delta coloured by how it reads: the numbers stay in the text, so
/// colour is never the only sign. The pooled row, when shown, follows a dim rule.
fn matrix_lines(matrix: &report::Matrix, pooled: bool) -> Vec<Row> {
    if matrix.columns.is_empty() {
        return Vec::new();
    }
    let mut rows = matrix.table();
    let pooled_at = rows.len();
    let pooled = pooled.then(|| matrix.pooled_row()).flatten();
    let shown = pooled.is_some();
    rows.extend(pooled);
    let width = |cell: &report::CellText| cell.to_string().chars().count();
    let widths: Vec<usize> = (0..rows[0].len())
        .map(|c| rows.iter().map(|r| width(&r[c])).max().unwrap_or(0))
        .collect();
    let mut lines = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let is_pooled = shown && i == pooled_at;
        if is_pooled {
            let rule = widths.iter().sum::<usize>() + 2 * (widths.len() - 1);
            lines.push(Row::table(Line::from(Span::styled(
                "─".repeat(rule),
                theme::dim(),
            ))));
        }
        let mut spans = Vec::new();
        for (c, cell) in row.iter().enumerate() {
            let style = if i == 0 || (is_pooled && c < 2) {
                theme::dim()
            } else {
                theme::base()
            };
            spans.push(Span::styled(cell.counts.clone(), style));
            if let Some((delta, kind)) = &cell.delta {
                spans.push(Span::styled("  ", style));
                spans.push(Span::styled(delta.clone(), delta_style(*kind)));
            }
            if c + 1 < row.len() {
                spans.push(Span::styled(" ".repeat(widths[c] - width(cell) + 2), style));
            }
        }
        lines.push(Row::table(Line::from(spans)));
    }
    let notes = matrix.notes(shown);
    lines.extend(
        notes
            .into_iter()
            .map(|n| Row::prose(Line::from(Span::styled(n, theme::dim())))),
    );
    lines
}

/// What the summary panel shows for `runs`: for a comparison, first whether the Runs compare;
/// then the matrix. A single Run pools its rows only when it has several, as its report does.
fn summary(runs: &[RunFile]) -> Vec<Row> {
    let mut lines = Vec::new();
    if runs.len() > 1 {
        let (incomparable, head) = report::comparability(runs);
        let style = if incomparable {
            theme::base().fg(theme::STATUS_WARN)
        } else {
            theme::dim()
        };
        lines.extend(
            head.into_iter()
                .map(|l| Row::prose(Line::from(Span::styled(l, style)))),
        );
        lines.push(Row::prose(Line::default()));
    }
    let matrix = report::Matrix::from_runs(runs);
    lines.extend(matrix_lines(
        &matrix,
        runs.len() > 1 || matrix.rows.len() > 1,
    ));
    lines
}

#[derive(Default)]
pub struct State {
    entries: Vec<Entry>,
    selected: usize,
    /// File names marked for comparison, in the order they were marked: the first is the baseline.
    marked: Vec<String>,
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

    /// Marks `name` after the others, or unmarks it.
    fn toggle_mark(&mut self, name: &str) {
        match self.marked.iter().position(|m| m == name) {
            Some(i) => {
                self.marked.remove(i);
            }
            None => self.marked.push(name.to_string()),
        }
    }

    /// `[ ]` unmarked, `[1]` the first marked (the baseline), and so on; `[+]` from the tenth.
    fn mark_label(&self, name: &str) -> String {
        match self.marked.iter().position(|m| m == name) {
            None => "[ ]".into(),
            Some(i) if i < 9 => format!("[{}]", i + 1),
            Some(_) => "[+]".into(),
        }
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
        let title = format!("report {}", entry.name);
        self.view = Some(match load_run(&dir.join(&entry.name)) {
            Ok(run) => View::new(
                title,
                summary(std::slice::from_ref(&run)),
                &report::report(&run).to_string(),
            ),
            Err(e) => View::new(title, plain_rows(&e, true), &e),
        });
    }

    fn compare_marked(&mut self, dir: &Path) -> Result<(), String> {
        let runs: Vec<RunFile> = self
            .marked
            .iter()
            .map(|name| load_run(&dir.join(name)))
            .collect::<Result<_, _>>()?;
        if runs.len() < 2 {
            return Err("mark two or more readable runs with Space to compare".into());
        }
        self.view = Some(View::new(
            format!("compare {} runs", runs.len()),
            summary(&runs),
            &report::Comparison(&runs).to_string(),
        ));
        Ok(())
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let dir = app.out.clone();
    let state = &mut app.results;
    if let Some(view) = state.view.as_mut() {
        if key.code == KeyCode::Esc {
            state.view = None;
        } else if key.code == KeyCode::Char('[') {
            view.select(Panel::Summary);
        } else if key.code == KeyCode::Char(']') {
            view.select(Panel::Full);
        } else if let Some(n) = nav(key) {
            view.scroll(n);
        }
        return;
    }
    match key.code {
        KeyCode::Char(' ') => {
            if let Some(Entry { name, run: Ok(_) }) = state.entries.get(state.selected) {
                let name = name.clone();
                state.toggle_mark(&name);
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

fn state_status(run: &RunSummary) -> Span<'static> {
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
        let inner = area.width.saturating_sub(2) as usize;
        view.width.set(inner);
        let rows = view.display(inner);
        let top = view.scroll.min(rows.len().saturating_sub(1));
        let lines: Vec<Line> = rows.into_iter().skip(top).collect();
        frame.render_widget(Paragraph::new(lines).block(block(&view.title())), area);
        return;
    }
    let rows: Vec<Line> = state
        .entries
        .iter()
        .map(|e| {
            let mark = state.mark_label(&e.name);
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
                            run.trials,
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::evidence::{EndRecord, read_run};

    /// A Run of candidates c, d, e and prompts p (baseline) and q on target `sonnet`, label high.
    /// Trials are (candidate, prompt index, case, effort).
    fn run_of(id: &str, trials: &[(&str, usize, &str, &str)]) -> RunFile {
        let candidates = ["c", "d", "e"]
            .map(|c| json!({"name": c, "provider": "zen", "model": "m", "url": "u"}));
        let mut lines = vec![json!({
            "type": "run", "schema_version": crate::evidence::SCHEMA_VERSION, "id": id,
            "created_unix_ms": 1, "dataset": {"name": "d", "sha256": "d"},
            "instructions": [{"name": "p", "sha256": "a", "text": "t"},
                {"name": "q", "sha256": "b", "text": "t"}],
            "grading_revision": crate::evidence::GRADING_REVISION,
            "candidates": candidates,
            "cases": ["c1"], "targets": ["sonnet"],
            "settings": {"repetitions": 1, "concurrency": 1, "timeout_secs": 60,
                "retries": 0, "include_unscored": false},
            "planned_trials": trials.len(),
        })];
        for (seq, (candidate, ins, case, effort)) in trials.iter().enumerate() {
            let body = json!({"answers": {"effort_sonnet": {"type": "choice", "choice": effort}}});
            let attempt = json!({"started_unix_ms": 0, "latency_ms": 1, "disposition": "answered",
                "http_status": 200, "raw_response": body.to_string(), "error": null});
            let (name, sha) = [("p", "a"), ("q", "b")][*ins];
            lines.push(json!({
                "type": "trial", "seq": seq, "candidate": candidate,
                "instructions": {"name": name, "sha256": sha},
                "case_id": case, "targets": ["sonnet"], "repetition": 1,
                "labels": {"sonnet": "high"}, "request": {}, "attempts": [attempt],
                "started_unix_ms": 0, "elapsed_ms": 1,
            }));
        }
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        read_run(text.as_bytes()).unwrap()
    }

    /// c gains three cases under q, d loses three, e gains one (noise).
    fn moving_run() -> RunFile {
        let mut trials = Vec::new();
        for case in ["c1", "c2", "c3"] {
            trials.extend([
                ("c", 0, case, "low"),
                ("c", 1, case, "high"),
                ("d", 0, case, "high"),
                ("d", 1, case, "low"),
            ]);
        }
        trials.extend([("e", 0, "c1", "low"), ("e", 1, "c1", "high")]);
        run_of("X1", &trials)
    }

    fn complete(mut run: RunFile) -> RunFile {
        run.end = Some(EndRecord {
            state: RunState::Complete,
            finished_unix_ms: 9,
        });
        run
    }

    fn plain(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// The summary's lines, without whether each is prose.
    fn summary_lines(runs: &[RunFile]) -> Vec<Line<'static>> {
        summary(runs).into_iter().map(|r| r.line).collect()
    }

    #[test]
    fn the_summary_rows_read_as_the_matrix_the_report_prints() {
        let run = moving_run();
        let shown = report::Matrix::from_runs(std::slice::from_ref(&run)).to_string();
        // The engine's two lines before the table repeat the header row; its rows are indented.
        let expected: Vec<&str> = shown
            .lines()
            .skip(2)
            .map(|l| l.strip_prefix("  ").unwrap())
            .collect();
        let rows: Vec<String> = summary_lines(&[run]).iter().map(plain).collect();
        // The separator before the pooled row is the only line the text does not have.
        let rows: Vec<&str> = rows
            .iter()
            .map(String::as_str)
            .filter(|l| !l.starts_with('─'))
            .collect();
        assert_eq!(rows, expected);
    }

    #[test]
    fn deltas_are_green_red_or_dim_and_the_rest_keeps_its_style() {
        let lines = summary_lines(&[moving_run()]);
        let delta_of = |row: &str| {
            let line = lines.iter().find(|l| plain(l).starts_with(row)).unwrap();
            line.spans
                .iter()
                .find(|s| s.content.starts_with('+'))
                .unwrap_or_else(|| panic!("no delta in {}", plain(line)))
                .clone()
        };
        let (gain, loss, noise) = (delta_of("c "), delta_of("d "), delta_of("e "));
        assert_eq!(gain.content, "+3 -0");
        assert_eq!(gain.style.fg, Some(theme::STATUS_OK));
        assert_eq!(loss.content, "+0 -3");
        assert_eq!(loss.style.fg, Some(theme::STATUS_FAIL));
        assert_eq!(noise.content, "+1 -0 ~");
        assert_eq!(noise.style, theme::dim());
        // The header row, the separator and the pooled label are dim, the footnotes too.
        assert!(lines[0].spans.iter().all(|s| s.style == theme::dim()));
        assert!(plain(&lines[0]).contains("p (baseline)"));
        let pooled = lines
            .iter()
            .position(|l| plain(l).starts_with("pooled"))
            .unwrap();
        assert_eq!(lines[pooled - 1].spans[0].style, theme::dim());
        assert_eq!(lines[pooled].spans[0].style, theme::dim());
        assert!(
            lines[pooled + 1..]
                .iter()
                .all(|l| l.spans.iter().all(|s| s.style == theme::dim()))
        );
        // A counts text is the base style.
        let counts = lines[1]
            .spans
            .iter()
            .find(|s| s.content.contains('%'))
            .unwrap();
        assert_eq!(counts.style, theme::base());
    }

    #[test]
    fn a_compare_summary_opens_with_the_verdict_on_whether_the_runs_compare() {
        let (x, y) = (moving_run(), moving_run());
        // Neither Run has an end record: both are partial.
        let lines = summary_lines(&[x, y]);
        assert_eq!(plain(&lines[0]), "Not directly comparable:");
        assert!(
            lines[0]
                .spans
                .iter()
                .all(|s| s.style.fg == Some(theme::STATUS_WARN))
        );
        assert!(plain(&lines[1]).starts_with("  - X1: partial run"));
        let blank = lines.iter().position(|l| plain(l).is_empty()).unwrap();
        assert!(plain(&lines[blank + 1]).starts_with("candidate"));

        let lines = summary_lines(&[complete(moving_run()), complete(moving_run())]);
        assert!(plain(&lines[0]).starts_with("no differences found in: "));
        assert_eq!(lines[0].spans[0].style, theme::dim());
        assert_eq!(plain(&lines[1]), "");
    }

    #[test]
    fn a_run_with_no_prompts_has_an_empty_summary() {
        let mut run = moving_run();
        run.manifest.instructions.clear();
        run.trials.clear();
        assert!(summary(&[run]).is_empty());
    }

    #[test]
    fn panels_are_picked_not_cycled_and_each_starts_at_the_top() {
        let mut view = View::new(
            "report x".into(),
            plain_rows("a\nb\nc", false),
            "1\n2\n3\n4",
        );
        assert_eq!(view.title(), "report x · summary");
        view.scroll(Nav::Down);
        view.scroll(Nav::Right);
        assert_eq!(view.scroll, 1);
        view.select(Panel::Full);
        assert_eq!(
            (view.title(), view.scroll, view.hscroll),
            ("report x · full report".into(), 0, 0)
        );
        view.scroll(Nav::Bottom);
        assert_eq!(view.scroll, 3); // the full report has four lines
        view.select(Panel::Full);
        assert_eq!(view.scroll, 3); // picking the panel already shown changes nothing
        view.select(Panel::Summary);
        assert_eq!(
            (view.title(), view.scroll),
            ("report x · summary".into(), 0)
        );
        view.scroll(Nav::Bottom);
        assert_eq!(view.scroll, 2);
    }

    #[test]
    fn sideways_scroll_steps_by_columns_and_stays_inside_the_widest_line() {
        let mut view = View::new(
            "t".into(),
            Vec::new(),
            &format!("short\n{}", "x".repeat(20)),
        );
        view.select(Panel::Full);
        view.scroll(Nav::Left);
        assert_eq!(view.hscroll, 0);
        view.scroll(Nav::Right);
        assert_eq!(view.hscroll, HSTEP);
        for _ in 0..5 {
            view.scroll(Nav::Right);
        }
        assert_eq!(view.hscroll, 19); // the widest line is 20 columns
        view.scroll(Nav::Left);
        assert_eq!(view.hscroll, 11);
        view.scroll(Nav::Down);
        assert_eq!((view.scroll, view.hscroll), (1, 11));
        // An empty text has nothing to scroll to.
        let mut empty = View::new("t".into(), Vec::new(), "");
        empty.scroll(Nav::Right);
        assert_eq!(empty.hscroll, 0);
    }

    #[test]
    fn cutting_columns_keeps_the_style_of_what_is_left() {
        let line = Line::from(vec![
            Span::styled("abc", theme::dim()),
            Span::styled("def", theme::base()),
        ]);
        let cut = skip_cols(&line, 4);
        assert_eq!(cut.spans.len(), 1);
        assert_eq!(
            (cut.spans[0].content.as_ref(), cut.spans[0].style),
            ("ef", theme::base())
        );
        assert_eq!(plain(&skip_cols(&line, 1)), "bcdef");
        assert_eq!(plain(&skip_cols(&line, 9)), "");
    }

    fn wrapped(text: &str, width: usize) -> Vec<String> {
        wrap(&Line::from(text.to_string()), width)
            .iter()
            .map(plain)
            .collect()
    }

    #[test]
    fn wrapping_breaks_greedily_on_spaces() {
        assert_eq!(wrapped("aaa bbb ccc", 7), ["aaa bbb", "ccc"]);
        assert_eq!(wrapped("aaa bbb ccc", 6), ["aaa", "bbb", "ccc"]);
        assert_eq!(wrapped("aaa bbb ccc", 11), ["aaa bbb ccc"]);
        // The space a break falls on is dropped, not carried to the next row.
        assert_eq!(wrapped("aaa  bbb", 4), ["aaa", "bbb"]);
        assert_eq!(wrapped("", 5), [""]);
    }

    #[test]
    fn a_word_longer_than_the_width_is_cut() {
        assert_eq!(wrapped("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrapped("xx abcdefghij", 4), ["xx", "abcd", "efgh", "ij"]);
    }

    #[test]
    fn a_wrapped_bullet_continues_under_its_text() {
        assert_eq!(
            wrapped("  - one two three", 10),
            ["  - one", "    two", "    three"]
        );
        // Prose without a bullet has nothing to hang under.
        assert_eq!(wrapped("one two three", 7), ["one two", "three"]);
        // A bullet too deep for the width is wrapped plainly.
        assert_eq!(wrapped("  - abc", 3), ["-", "abc"]);
    }

    #[test]
    fn wrapped_pieces_keep_the_style_of_their_text() {
        let (dim, warn) = (theme::dim(), theme::base().fg(theme::STATUS_WARN));
        let line = Line::from(vec![
            Span::styled("aaa b", dim),
            Span::styled("bb ccc", warn),
        ]);
        let rows = wrap(&line, 5);
        let parts: Vec<Vec<(&str, Style)>> = rows
            .iter()
            .map(|r| {
                r.spans
                    .iter()
                    .map(|s| (s.content.as_ref(), s.style))
                    .collect()
            })
            .collect();
        assert_eq!(
            parts,
            [
                vec![("aaa", dim)],
                vec![("b", dim), ("bb", warn)],
                vec![("ccc", warn)]
            ]
        );
    }

    #[test]
    fn wrapping_never_panics_or_loses_text_at_tiny_widths() {
        let text = "  - not directly comparable: dataset differs";
        for width in 0..=6 {
            let rows = wrapped(text, width);
            assert!(
                rows.iter().all(|r| r.chars().count() <= width.max(1)),
                "{rows:?}"
            );
            let kept: String = rows.concat().split_whitespace().collect();
            assert_eq!(
                kept,
                text.split_whitespace().collect::<String>(),
                "width {width}"
            );
        }
    }

    /// A summary with one long prose line and a table row wider than any pane below.
    fn prose_and_table() -> View {
        let prose = "word ".repeat(30);
        let table = Line::from(format!("{}|", "x".repeat(100)));
        View::new(
            "t".into(),
            vec![Row::prose(Line::from(prose)), Row::table(table)],
            "",
        )
    }

    #[test]
    fn prose_wraps_and_table_lines_do_not() {
        let mut view = prose_and_table();
        let rows = view.display(20);
        // 30 five-letter words, four to a row of at most 20 columns, then the table row alone.
        assert_eq!(rows.len(), 8 + 1);
        assert!(rows[..8].iter().all(|r| columns(r) <= 20));
        assert_eq!(columns(&rows[8]), 101);
        // Sideways scroll cuts the table row and leaves the prose where it is.
        view.scroll(Nav::Right);
        let shifted = view.display(20);
        assert_eq!(columns(&shifted[8]), 101 - HSTEP);
        assert_eq!(shifted[..8], rows[..8]);
        // The widest line to scroll to is the table row, not the prose.
        for _ in 0..30 {
            view.scroll(Nav::Right);
        }
        assert_eq!(view.hscroll, 100);
    }

    #[test]
    fn vertical_scrolling_counts_wrapped_rows_and_survives_a_resize() {
        let mut view = prose_and_table();
        view.width.set(20);
        view.scroll(Nav::Bottom);
        assert_eq!(view.scroll, 8); // 8 wrapped rows and the table row: the last is index 8
        view.scroll(Nav::Down);
        assert_eq!(view.scroll, 8);
        view.scroll(Nav::Top);
        for _ in 0..30 {
            view.scroll(Nav::Down);
        }
        assert_eq!(view.scroll, 8);
        // Widen the pane: the prose takes fewer rows, and Up starts from the new last row.
        view.width.set(200);
        assert_eq!(view.display(200).len(), 2);
        view.scroll(Nav::Up);
        assert_eq!(view.scroll, 0);
        view.scroll = 8;
        view.scroll(Nav::Down);
        assert_eq!(view.scroll, 1);
        view.scroll = 8;
        view.scroll(Nav::PageDown);
        assert_eq!(view.scroll, 1);
    }

    #[test]
    fn marks_keep_their_order_and_show_it_as_a_number() {
        let mut state = State::default();
        for name in ["b", "a", "c"] {
            state.toggle_mark(name);
        }
        let labels: Vec<String> = ["a", "b", "c", "d"].map(|n| state.mark_label(n)).into();
        assert_eq!(labels, ["[2]", "[1]", "[3]", "[ ]"]);
        // Unmarking renumbers the rest; marking again goes to the end.
        state.toggle_mark("b");
        assert_eq!(state.marked, ["a", "c"]);
        assert_eq!(
            (state.mark_label("a"), state.mark_label("c")),
            ("[1]".into(), "[2]".into())
        );
        state.toggle_mark("b");
        assert_eq!(state.mark_label("b"), "[3]");
        // From the tenth the number no longer fits.
        for i in 0..7 {
            state.toggle_mark(&format!("n{i}"));
        }
        assert_eq!(state.mark_label("n5"), "[9]");
        assert_eq!(state.mark_label("n6"), "[+]");
    }
}
