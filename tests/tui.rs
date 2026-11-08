//! TUI rendering and CLI/TUI agreement, offline over a hand-built fixture run.

use std::path::PathBuf;
use std::process::Command;

use decisionbench::tui::{App, Effect, Screen, handle_key, render};
use decisionbench::{evidence, report};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

fn fresh_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn reply(choice: &str) -> String {
    let answers: Vec<String> = ["haiku", "sonnet", "opus", "fable"]
        .iter()
        .map(|t| format!(r#""effort_{t}":{{"type":"choice","choice":"{choice}"}}"#))
        .collect();
    format!(r#"{{"answers":{{{}}}}}"#, answers.join(","))
}

/// A complete one-candidate run of one prompt: two Trials that hit and miss (the first answers
/// `first`), one failed Trial, no price.
fn run_lines(id: &str, created: u64, prompt: &str, first: &str) -> Vec<Value> {
    let sha = prompt[..1].repeat(64);
    let attempt = |disposition: &str, status: Value, body: String| {
        json!({"started_unix_ms": 0, "latency_ms": 120, "disposition": disposition,
               "http_status": status, "raw_response": body, "error": null})
    };
    let trial = |seq: usize, case: &str, attempts: Vec<Value>| {
        json!({"type": "trial", "seq": seq, "candidate": "alpha",
               "instructions": {"name": prompt, "sha256": sha},
               "case_id": case, "targets": ["sonnet", "opus"], "repetition": 1,
               "labels": {"sonnet": "low", "opus": "low-medium"},
               "request": {}, "attempts": attempts, "started_unix_ms": 0, "elapsed_ms": 150})
    };
    vec![
        json!({"type": "run", "schema_version": 4, "id": id, "created_unix_ms": created,
               "dataset": {"name": "ds.json", "sha256": "b".repeat(64)},
               "instructions": [{"name": prompt, "sha256": sha, "text": "t"}],
               "grading_revision": 2,
               "candidates": [{"name": "alpha", "provider": "zen", "model": "m", "url": "http://stub"}],
               "cases": ["c1", "c2", "c3"], "targets": ["sonnet", "opus"],
               "settings": {"repetitions": 1, "concurrency": 1, "timeout_secs": 60, "retries": 1, "include_unscored": false},
               "planned_trials": 3}),
        trial(0, "c1", vec![attempt("answered", json!(200), reply(first))]),
        trial(1, "c2", vec![attempt("answered", json!(200), reply("max"))]),
        trial(
            2,
            "c3",
            vec![attempt("provider_error", json!(503), "busy".into())],
        ),
        json!({"type": "end", "state": "complete", "finished_unix_ms": 9}),
    ]
}

fn fixture_lines() -> Vec<Value> {
    run_lines("fixture-1", 1000, "neutral", "low")
}

/// A newer run of another prompt that misses the case the fixture run hits.
fn second_run(file: &std::path::Path) {
    let text: String = run_lines("fixture-2", 2000, "terse", "max")
        .iter()
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(file.with_file_name("fixture-2.jsonl"), text).unwrap();
}

/// An evidence directory holding the fixture run, and an App over it.
fn app_with_fixture(test: &str) -> (App, PathBuf) {
    let dir = fresh_dir(test);
    let runs = dir.join("runs");
    std::fs::create_dir_all(&runs).unwrap();
    let text: String = fixture_lines().iter().map(|l| format!("{l}\n")).collect();
    let file = runs.join("fixture-1.jsonl");
    std::fs::write(&file, text).unwrap();
    let app = App::new(Some(dir.join("config.toml")), Some(runs)).unwrap();
    (app, file)
}

fn press(app: &mut App, code: KeyCode) -> Effect {
    handle_key(app, KeyEvent::new(code, KeyModifiers::NONE))
}

/// The screen as one string per row.
fn draw(app: &App, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| render(frame, app)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect()
}

fn contains(rows: &[String], text: &str) -> bool {
    rows.iter().any(|r| r.contains(text))
}

#[test]
fn every_screen_renders_at_80x24_and_60x16() {
    let (mut app, _) = app_with_fixture("tui-render");
    app.candidates
        .items
        .push(decisionbench::provider::Candidate {
            name: "alpha".into(),
            decision_model: None,
            provider: decisionbench::provider::Provider::Zen,
            model: "m".into(),
            price: None,
        });
    let expected = [
        (Screen::Candidates, "cost unknown"),
        (Screen::Dataset, "cases ·"),
        (Screen::Setup, "repetitions"),
        (Screen::Run, "no run yet"),
        (Screen::Results, "complete"),
    ];
    for (screen, text) in expected {
        app.screen = screen;
        for (w, h) in [(80, 24), (60, 16), (120, 40)] {
            let rows = draw(&app, w, h);
            assert!(
                contains(&rows, "Candidates") && contains(&rows, "Results"),
                "{screen:?} {w}x{h}"
            );
            assert!(
                contains(&rows, text),
                "{screen:?} {w}x{h} lacks {text:?}:\n{}",
                rows.join("\n")
            );
        }
    }
}

#[test]
fn dataset_lists_each_target_with_its_label_status() {
    let (mut app, _) = app_with_fixture("tui-dataset");
    app.screen = Screen::Dataset;
    let rows = draw(&app, 100, 30);
    for target in ["haiku", "sonnet", "opus", "fable"] {
        assert!(contains(&rows, &format!("{target} ")), "{target}");
    }
    // The first bundled case labels only some targets: both states show as symbol plus word.
    assert!(
        contains(&rows, "✓ ") && contains(&rows, "· "),
        "{}",
        rows.join("\n")
    );
}

#[test]
fn dataset_title_with_label_counts_fits_in_the_list_pane() {
    let (mut app, _) = app_with_fixture("tui-dataset-title");
    app.screen = Screen::Dataset;
    let (_, _, cases) = decisionbench::batch::load_dataset(None).unwrap();
    let summary = decisionbench::dataset::summarize(&cases);
    let [h, s, o, f] = summary.labels;
    let full = format!("{} cases · h{h} s{s} o{o} f{f}", summary.cases);
    let narrow = format!("{} cases · {h}/{s}/{o}/{f}", summary.cases);
    // Every width from the minimum up: each title is whole, and once the full one fits it stays.
    let mut shown = Vec::new();
    for width in 60..=100 {
        let rows = draw(&app, width, 30);
        let row = rows
            .iter()
            .find(|r| r.contains(" cases · "))
            .unwrap_or_else(|| panic!("no title at {width}:\n{}", rows.join("\n")));
        let start = row.find(&format!("{} cases · ", summary.cases)).unwrap();
        let title: String = row[start..]
            .chars()
            .take_while(|c| !matches!(c, '─' | '┐'))
            .collect();
        let title = title.trim_end();
        assert!(
            title == full || title == narrow,
            "title cut at {width}: {title:?}"
        );
        shown.push(title == full);
    }
    let first_full = shown.iter().position(|f| *f).expect("full title at 100");
    assert!(first_full > 0, "the narrow title is never used");
    assert!(shown[first_full..].iter().all(|f| *f), "{shown:?}");
}

#[test]
fn a_too_small_terminal_gets_a_message_not_a_panic() {
    let (app, _) = app_with_fixture("tui-small");
    for (w, h) in [(59, 16), (60, 15), (10, 3), (1, 1)] {
        let rows = draw(&app, w, h);
        if w >= 20 && h >= 2 {
            assert!(contains(&rows, "too small") || w < 30, "{w}x{h}");
        }
    }
    assert!(contains(&draw(&app, 59, 16), "terminal too small"));
}

/// The rows of a matrix's text without the `baseline:` and `columns:` lines and the indent.
fn matrix_rows(text: &str) -> Vec<&str> {
    text.lines().skip(2).map(str::trim).collect()
}

/// `text` with every run of whitespace made one space.
fn squashed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The rows of the matrix with its targets pooled (header included), whitespace squashed: padding
/// depends on the widest cell, which differs between the pooled and the expanded table.
fn collapsed_rows(matrix: &report::Matrix) -> Vec<String> {
    matrix
        .grouped_table()
        .iter()
        .map(|r| squashed(&r.iter().map(|c| format!("{c} ")).collect::<String>()))
        .collect()
}

/// `contains` for text whose whitespace does not matter.
fn contains_squashed(rows: &[String], text: &str) -> bool {
    rows.iter().any(|r| squashed(r).contains(text))
}

#[test]
fn the_results_pane_opens_on_the_summary_and_the_full_report_is_what_show_prints() {
    let (mut app, file) = app_with_fixture("tui-agreement");
    // The History list shows the run, then Enter opens its report.
    app.screen = Screen::Results;
    assert!(contains(&draw(&app, 100, 30), "fixture-1.jsonl"));
    press(&mut app, KeyCode::Enter);

    let run = evidence::read_run(&std::fs::read(&file).unwrap()).unwrap();
    let matrix =
        report::Matrix::from_runs(std::slice::from_ref(&run), report::Axis::Auto).to_string();
    let rows = draw(&app, 200, 30);
    assert!(contains(&rows, "report fixture-1.jsonl · summary"));
    assert!(!contains(&rows, "== Run =="), "{}", rows.join("\n"));
    // It opens with the targets pooled; `t` spreads them into the rows the matrix prints.
    let grouped = report::Matrix::from_runs(std::slice::from_ref(&run), report::Axis::Auto);
    for line in collapsed_rows(&grouped) {
        assert!(
            contains_squashed(&rows, &line),
            "summary lacks {line:?}:\n{}",
            rows.join("\n")
        );
    }
    press(&mut app, KeyCode::Char('t'));
    let rows = draw(&app, 200, 30);
    for line in matrix_rows(&matrix) {
        assert!(
            contains(&rows, line),
            "summary lacks {line:?}:\n{}",
            rows.join("\n")
        );
    }

    press(&mut app, KeyCode::Char(']'));
    let expected = report::report(&run, report::Axis::Auto).to_string();
    let shown = String::from_utf8(
        Command::new(env!("CARGO_BIN_EXE_decisionbench"))
            .args(["show", file.to_str().unwrap()])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(shown, expected, "`show` and the library report differ");

    let height = expected.lines().count() as u16 + 6;
    let width = expected.lines().map(|l| l.chars().count()).max().unwrap() as u16 + 4;
    let rows = draw(&app, width.max(80), height.max(24));
    assert!(contains(&rows, "report fixture-1.jsonl · full report"));
    for line in expected.lines().filter(|l| !l.is_empty()) {
        assert!(
            contains(&rows, line),
            "missing report line {line:?} in:\n{}",
            rows.join("\n")
        );
    }

    // `[` goes back to the summary.
    press(&mut app, KeyCode::Char('['));
    assert!(contains(&draw(&app, 200, 30), "· summary"));
}

#[test]
fn right_and_left_scroll_the_full_report_sideways_without_changing_its_text() {
    let (mut app, _) = app_with_fixture("tui-hscroll");
    app.screen = Screen::Results;
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char(']'));
    let before = draw(&app, 100, 40);
    let line = before
        .iter()
        .map(|r| r.trim_matches(|c: char| !c.is_alphanumeric()))
        .find(|r| r.starts_with("state: "))
        .expect("report has a state line")
        .to_string();
    press(&mut app, KeyCode::Right);
    let shifted = draw(&app, 100, 40);
    assert!(
        contains(&shifted, &line[8..]) && !contains(&shifted, &line),
        "{line}"
    );
    press(&mut app, KeyCode::Char('h'));
    assert_eq!(draw(&app, 100, 40), before);
}

#[test]
fn marked_runs_are_numbered_in_mark_order_and_the_first_is_the_baseline() {
    let (mut app, file) = app_with_fixture("tui-compare");
    second_run(&file);
    press(&mut app, KeyCode::Char('5'));
    press(&mut app, KeyCode::Char('r'));
    // The list is newest first (fixture-2, fixture-1); mark it the other way round.
    press(&mut app, KeyCode::Char('g'));
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Char('k'));
    press(&mut app, KeyCode::Char(' '));
    let rows = draw(&app, 100, 30);
    assert!(
        contains(&rows, "[1] fixture-1.jsonl"),
        "{}",
        rows.join("\n")
    );
    assert!(contains(&rows, "[2] fixture-2.jsonl"));
    press(&mut app, KeyCode::Char('m'));

    let runs: Vec<_> = ["fixture-1.jsonl", "fixture-2.jsonl"] // mark order
        .iter()
        .map(|n| evidence::read_run(&std::fs::read(file.with_file_name(n)).unwrap()).unwrap())
        .collect();
    let matrix = report::Matrix::from_runs(&runs, report::Axis::Auto).to_string();
    let height = 40;
    let rows = draw(&app, 200, height);
    assert!(
        contains(&rows, "compare 2 runs · summary"),
        "{}",
        app.message
    );
    assert!(contains(&rows, "neutral (baseline)") && !contains(&rows, "terse (baseline)"));
    assert!(contains(&rows, "no differences found in:"));
    let grouped = report::Matrix::from_runs(&runs, report::Axis::Auto);
    for line in collapsed_rows(&grouped) {
        assert!(contains_squashed(&rows, &line), "summary lacks {line:?}");
    }
    press(&mut app, KeyCode::Char('t'));
    let rows = draw(&app, 200, height);
    for line in matrix_rows(&matrix) {
        assert!(
            contains(&rows, line),
            "summary lacks {line:?}:\n{}",
            rows.join("\n")
        );
    }

    press(&mut app, KeyCode::Char(']'));
    let expected = report::Comparison(&runs, report::Axis::Auto).to_string();
    let rows = draw(&app, 200, height);
    assert!(contains(&rows, "compare 2 runs · full report"));
    for line in expected.lines().filter(|l| !l.is_empty()) {
        assert!(contains(&rows, line), "missing compare line {line:?}");
    }

    // Esc closes the view; unmarking the first run makes the second the one left.
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Char(' '));
    let rows = draw(&app, 100, 30);
    assert!(
        contains(&rows, "[1] fixture-2.jsonl"),
        "{}",
        rows.join("\n")
    );
}

/// The two fixture runs compared (the Results screen on the summary), and the runs.
fn comparing(test: &str) -> (App, Vec<evidence::RunFile>) {
    let (mut app, file) = app_with_fixture(test);
    second_run(&file);
    app.screen = Screen::Results;
    press(&mut app, KeyCode::Char('r'));
    // Newest first (fixture-2, fixture-1); mark fixture-1 first so it is the baseline.
    for key in ['g', 'j', ' ', 'k', ' ', 'm'] {
        press(&mut app, KeyCode::Char(key));
    }
    let runs = ["fixture-1.jsonl", "fixture-2.jsonl"]
        .iter()
        .map(|n| evidence::read_run(&std::fs::read(file.with_file_name(n)).unwrap()).unwrap())
        .collect();
    (app, runs)
}

/// What is inside the borders of `rows`, in one line with single spaces.
fn inner_text(rows: &[String]) -> String {
    let inner: Vec<&str> = rows.iter().map(|r| r.trim_matches('│')).collect();
    inner
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn summary_prose_wraps_to_a_narrow_pane_and_stays_whole() {
    let (mut app, runs) = comparing("tui-wrap");
    // The pooled note belongs to the pooled row, which the collapsed table of one candidate omits.
    press(&mut app, KeyCode::Char('t'));
    let mut prose: Vec<String> = report::comparability(&runs, report::Axis::Auto).1;
    prose.extend(
        report::Matrix::from_runs(&runs, report::Axis::Auto)
            .notes(true)
            .iter()
            .map(|n| n.to_string()),
    );
    assert!(prose.len() >= 3, "{prose:?}");
    for width in [60, 80] {
        let rows = draw(&app, width, 40);
        assert!(contains(&rows, "compare 2 runs · summary"));
        let text = inner_text(&rows);
        for line in &prose {
            let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                text.contains(&line),
                "{width} wide cuts {line:?}:\n{}",
                rows.join("\n")
            );
        }
    }
}

#[test]
fn table_rows_are_not_wrapped_and_still_scroll_sideways() {
    let (mut app, runs) = comparing("tui-table");
    press(&mut app, KeyCode::Char('t')); // the expanded table is the one `matrix_rows` pads
    let header = report::Matrix::from_runs(&runs, report::Axis::Auto).to_string();
    let header = matrix_rows(&header)[0].to_string();
    let before = draw(&app, 60, 40);
    assert!(contains(&before, &header), "{}", before.join("\n"));
    press(&mut app, KeyCode::Right);
    let shifted = draw(&app, 60, 40);
    assert!(contains(&shifted, &header[8..]) && !contains(&shifted, &header));
    // The prose does not move with the table.
    let notes = report::Matrix::from_runs(&runs, report::Axis::Auto)
        .notes(true)
        .join(" ");
    let notes = notes.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(inner_text(&shifted).contains(&notes));
}

#[test]
fn scrolling_to_the_end_reaches_the_last_wrapped_row_also_after_a_resize() {
    let (mut app, runs) = comparing("tui-wrap-scroll");
    press(&mut app, KeyCode::Char('t'));
    let notes = report::Matrix::from_runs(&runs, report::Axis::Auto).notes(true);
    let last = notes.last().unwrap().split_whitespace().last().unwrap();
    let first = report::comparability(&runs, report::Axis::Auto).1[0].clone();
    let first = first.split_whitespace().next().unwrap();
    // The notes take several rows at 60 columns, so the last row is far below the first.
    assert!(!contains(&draw(&app, 60, 16)[..4], last));
    for _ in 0..3 {
        press(&mut app, KeyCode::Char('G'));
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::PageDown);
    }
    let rows = draw(&app, 60, 16);
    assert!(contains(&rows, last), "{}", rows.join("\n"));
    assert!(!contains(&rows, first), "{}", rows.join("\n"));
    // Widening the pane makes fewer rows: the view still ends on the last, and k steps back one.
    assert!(contains(&draw(&app, 200, 16), last));
    press(&mut app, KeyCode::Char('j'));
    assert!(contains(&draw(&app, 200, 16), last));
    press(&mut app, KeyCode::Char('k'));
    let rows = draw(&app, 200, 16);
    assert!(
        contains(&rows, last) && contains(&rows, "pooled ="),
        "{}",
        rows.join("\n")
    );
}

#[test]
fn the_setup_title_fits_a_narrow_terminal_with_many_cases_marked() {
    let (mut app, _) = app_with_fixture("tui-setup-title");
    app.dataset
        .marked
        .extend((0..40).map(|i| format!("long-case-identifier-{i:03}")));
    app.screen = Screen::Setup;
    let rows = draw(&app, 60, 16);
    assert!(
        rows[1].starts_with("┌ run setup · cases: 40 selected") && rows[1].ends_with("… ┐"),
        "{}",
        rows[1]
    );
}

/// Opens the fixture run's report on the Results screen.
fn opened(test: &str) -> (App, PathBuf, evidence::RunFile) {
    let (mut app, file) = app_with_fixture(test);
    app.screen = Screen::Results;
    press(&mut app, KeyCode::Enter);
    let run = evidence::read_run(&std::fs::read(&file).unwrap()).unwrap();
    (app, file, run)
}

fn show_columns(file: &std::path::Path, axis: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_decisionbench"))
        .args(["show", file.to_str().unwrap(), "--columns", axis])
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn c_toggles_the_columns_of_both_panels_and_the_full_report_is_what_show_prints() {
    let (mut app, file, run) = opened("tui-columns");
    // One prompt and one candidate: Auto reads as prompts.
    assert!(contains(
        &draw(&app, 200, 30),
        "report fixture-1.jsonl · summary (columns: prompts, targets pooled)"
    ));
    press(&mut app, KeyCode::Char('t'));
    assert!(contains(
        &draw(&app, 200, 30),
        "report fixture-1.jsonl · summary (columns: prompts)"
    ));
    press(&mut app, KeyCode::Char('c'));
    let rows = draw(&app, 200, 30);
    assert!(
        contains(&rows, "· summary (columns: candidates)"),
        "{}",
        rows.join("\n")
    );
    let matrix =
        report::Matrix::from_runs(std::slice::from_ref(&run), report::Axis::Candidate).to_string();
    let header = matrix_rows(&matrix)[0]
        .split_whitespace()
        .collect::<Vec<_>>();
    assert_eq!(header.join(" "), "prompt target alpha (baseline)");
    for line in matrix_rows(&matrix) {
        assert!(
            contains(&rows, line),
            "lacks {line:?}:\n{}",
            rows.join("\n")
        );
    }

    press(&mut app, KeyCode::Char(']'));
    let expected = show_columns(&file, "candidate");
    assert_eq!(
        expected,
        report::report(&run, report::Axis::Candidate).to_string()
    );
    let rows = draw(&app, 200, 60);
    assert!(contains(&rows, "report fixture-1.jsonl · full report"));
    for line in expected.lines().filter(|l| !l.is_empty()) {
        assert!(
            contains(&rows, line),
            "missing {line:?}:\n{}",
            rows.join("\n")
        );
    }

    // c again goes back, on the panel shown; the other panel followed.
    press(&mut app, KeyCode::Char('c'));
    let rows = draw(&app, 200, 60);
    assert!(inner_text(&rows).contains("candidate target neutral (baseline)"));
    press(&mut app, KeyCode::Char('['));
    assert!(contains(
        &draw(&app, 200, 60),
        "· summary (columns: prompts)"
    ));
}

#[test]
fn a_new_view_starts_on_auto_and_pooled_whatever_the_last_one_toggled_to() {
    let (mut app, _, _) = opened("tui-columns-reset");
    press(&mut app, KeyCode::Char('c'));
    press(&mut app, KeyCode::Char('t'));
    assert!(contains(&draw(&app, 200, 30), "(columns: candidates)"));
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Enter);
    // The columns are back on Auto and the targets pooled.
    assert!(contains(
        &draw(&app, 200, 30),
        "(columns: prompts, targets pooled)"
    ));
}

#[test]
fn c_toggles_a_comparison_and_its_full_text_is_what_compare_prints() {
    let (mut app, runs) = comparing("tui-columns-compare");
    assert!(contains(
        &draw(&app, 200, 40),
        "compare 2 runs · summary (columns: prompts, targets pooled)"
    ));
    press(&mut app, KeyCode::Char('t'));
    assert!(contains(
        &draw(&app, 200, 40),
        "compare 2 runs · summary (columns: prompts)"
    ));
    press(&mut app, KeyCode::Char('c'));
    let rows = draw(&app, 200, 40);
    assert!(
        contains(&rows, "compare 2 runs · summary (columns: candidates)"),
        "{}",
        rows.join("\n")
    );
    let matrix = report::Matrix::from_runs(&runs, report::Axis::Candidate).to_string();
    for line in matrix_rows(&matrix) {
        assert!(
            contains(&rows, line),
            "lacks {line:?}:\n{}",
            rows.join("\n")
        );
    }
    assert!(contains(&rows, "alpha in fixture-1 (baseline)"));

    press(&mut app, KeyCode::Char(']'));
    let expected = report::Comparison(&runs, report::Axis::Candidate).to_string();
    let rows = draw(&app, 200, 40);
    for line in expected.lines().filter(|l| !l.is_empty()) {
        assert!(
            contains(&rows, line),
            "missing {line:?}:\n{}",
            rows.join("\n")
        );
    }
}

#[test]
fn c_on_a_comparison_of_candidates_toggles_the_comparability_lines_with_the_axis() {
    let (mut app, file) = app_with_fixture("tui-columns-candidates");
    // The same prompt under another candidate: Auto puts the candidates in the columns.
    let text: String = run_lines("fixture-3", 3000, "neutral", "max")
        .iter()
        .map(|l| format!("{}\n", l.to_string().replace("alpha", "beta")))
        .collect();
    std::fs::write(file.with_file_name("fixture-3.jsonl"), text).unwrap();
    app.screen = Screen::Results;
    press(&mut app, KeyCode::Char('r'));
    // Newest first (fixture-3, fixture-1); mark fixture-1 first so it is the baseline.
    for key in ['g', 'j', ' ', 'k', ' ', 'm'] {
        press(&mut app, KeyCode::Char(key));
    }
    let rows = draw(&app, 200, 40);
    assert!(
        contains(
            &rows,
            "compare 2 runs · summary (columns: candidates, targets pooled)"
        ),
        "{}",
        app.message
    );
    assert!(contains(&rows, "no differences found in:"));
    assert!(!contains(&rows, "is not in"), "{}", rows.join("\n"));

    press(&mut app, KeyCode::Char('c'));
    let rows = draw(&app, 200, 40);
    assert!(contains(&rows, "(columns: prompts, targets pooled)"));
    assert!(contains(&rows, "candidate `beta` is not in fixture-1"));

    press(&mut app, KeyCode::Char('c'));
    let rows = draw(&app, 200, 40);
    assert!(contains(&rows, "(columns: candidates, targets pooled)"));
    assert!(!contains(&rows, "is not in"), "{}", rows.join("\n"));
}

/// Screen rows whose first words are `label target`.
fn has_row(rows: &[String], label: &str, target: &str) -> bool {
    contains_squashed(rows, &format!("{label} {target} "))
}

#[test]
fn the_summary_opens_with_targets_pooled_and_t_spreads_them_out_and_back() {
    let (mut app, _, run) = opened("tui-targets");
    let rows = draw(&app, 200, 30);
    assert!(contains(
        &rows,
        "summary (columns: prompts, targets pooled)"
    ));
    // One row for the candidate, none per target.
    assert!(has_row(&rows, "alpha", "all"), "{}", rows.join("\n"));
    assert!(!has_row(&rows, "alpha", "sonnet") && !has_row(&rows, "alpha", "opus"));
    // The header keeps its target column.
    assert!(contains_squashed(
        &rows,
        "candidate target neutral (baseline)"
    ));

    press(&mut app, KeyCode::Char('t'));
    let rows = draw(&app, 200, 30);
    assert!(contains(&rows, "summary (columns: prompts)"));
    let matrix =
        report::Matrix::from_runs(std::slice::from_ref(&run), report::Axis::Auto).to_string();
    for line in matrix_rows(&matrix) {
        assert!(
            contains(&rows, line),
            "lacks {line:?}:\n{}",
            rows.join("\n")
        );
    }
    assert!(has_row(&rows, "alpha", "sonnet") && has_row(&rows, "alpha", "opus"));
    assert!(!has_row(&rows, "alpha", "all"));

    press(&mut app, KeyCode::Char('t'));
    let rows = draw(&app, 200, 30);
    assert!(contains(&rows, "(columns: prompts, targets pooled)"));
    assert!(has_row(&rows, "alpha", "all") && !has_row(&rows, "alpha", "sonnet"));
}

#[test]
fn c_keeps_whether_targets_are_pooled_and_t_keeps_the_columns() {
    let (mut app, _) = comparing("tui-targets-columns");
    press(&mut app, KeyCode::Char('c'));
    let rows = draw(&app, 200, 40);
    assert!(contains(&rows, "(columns: candidates, targets pooled)"));
    assert!(has_row(&rows, "neutral", "all") && has_row(&rows, "terse", "all"));
    press(&mut app, KeyCode::Char('t'));
    let rows = draw(&app, 200, 40);
    assert!(contains(&rows, "(columns: candidates)") && !contains(&rows, "targets pooled"));
    assert!(has_row(&rows, "neutral", "sonnet") && has_row(&rows, "neutral", "opus"));
    press(&mut app, KeyCode::Char('c'));
    let rows = draw(&app, 200, 40);
    assert!(contains(&rows, "(columns: prompts)") && !contains(&rows, "targets pooled"));
    assert!(has_row(&rows, "alpha", "sonnet") && has_row(&rows, "alpha", "opus"));
}

#[test]
fn t_does_not_change_the_full_report() {
    let (mut app, runs) = comparing("tui-targets-full");
    press(&mut app, KeyCode::Char(']'));
    let before = draw(&app, 200, 60);
    assert!(contains(&before, "compare 2 runs · full report"));
    press(&mut app, KeyCode::Char('t'));
    assert_eq!(draw(&app, 200, 60), before, "`t` moved the full report");
    // It took effect on the summary behind it, and the text is still what `compare` prints.
    let expected = report::Comparison(&runs, report::Axis::Auto).to_string();
    for line in expected.lines().filter(|l| !l.is_empty()) {
        assert!(contains(&before, line), "missing {line:?}");
    }
    press(&mut app, KeyCode::Char('['));
    let rows = draw(&app, 200, 60);
    assert!(contains(&rows, "summary (columns: prompts)") && !contains(&rows, "targets pooled"));
}

#[test]
fn t_does_nothing_on_a_run_that_cannot_be_read() {
    let (mut app, file) = app_with_fixture("tui-targets-error");
    std::fs::write(file.with_file_name("bad.jsonl"), "not a run\n").unwrap();
    app.screen = Screen::Results;
    press(&mut app, KeyCode::Char('r'));
    // Unreadable files sort last.
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Enter);
    let before = draw(&app, 100, 30);
    assert!(
        contains(&before, "report bad.jsonl · summary"),
        "{}",
        before.join("\n")
    );
    press(&mut app, KeyCode::Char('t'));
    assert_eq!(draw(&app, 100, 30), before);
}

#[test]
fn t_at_the_bottom_of_a_narrow_pane_keeps_the_last_row_in_view() {
    let (mut app, runs) = comparing("tui-targets-scroll");
    let notes = report::Matrix::from_runs(&runs, report::Axis::Auto).notes(true);
    let last = notes.last().unwrap().split_whitespace().last().unwrap();
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Char('G'));
    assert!(contains(&draw(&app, 60, 16), last));
    // One row fewer: the scroll is clamped to it, not left past the end.
    press(&mut app, KeyCode::Char('t'));
    assert!(contains(&draw(&app, 60, 16), last));
    press(&mut app, KeyCode::Char('j'));
    let at_end = draw(&app, 60, 16);
    assert!(contains(&at_end, last));
    // The collapsed table of one candidate has no pooled row, so only the delta note is left to
    // scroll over. `k` moves one row up, so the pane shows one more row of it.
    press(&mut app, KeyCode::Char('k'));
    let rows = draw(&app, 60, 16);
    let filled = |rows: &[String]| {
        rows.iter()
            .filter(|r| !r.trim_matches('│').trim().is_empty())
            .count()
    };
    assert!(
        contains(&rows, last) && filled(&rows) == filled(&at_end) + 1,
        "{}",
        rows.join("\n")
    );
}
