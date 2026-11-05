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

/// A complete one-candidate run: two Trials that hit and miss, one failed Trial, no price.
fn fixture_lines() -> Vec<Value> {
    let sha = "a".repeat(64);
    let attempt = |disposition: &str, status: Value, body: String| {
        json!({"started_unix_ms": 0, "latency_ms": 120, "disposition": disposition,
               "http_status": status, "raw_response": body, "error": null})
    };
    let trial = |seq: usize, case: &str, attempts: Vec<Value>| {
        json!({"type": "trial", "seq": seq, "candidate": "alpha",
               "instructions": {"name": "neutral", "sha256": sha},
               "case_id": case, "targets": ["sonnet", "opus"], "repetition": 1,
               "labels": {"sonnet": "low", "opus": "low-medium"},
               "request": {}, "attempts": attempts, "started_unix_ms": 0, "elapsed_ms": 150})
    };
    vec![
        json!({"type": "run", "schema_version": 3, "id": "fixture-1", "created_unix_ms": 1000,
               "dataset": {"name": "ds.json", "sha256": "b".repeat(64)},
               "instructions": [{"name": "neutral", "sha256": sha, "text": "t"}],
               "grading_revision": 2,
               "candidates": [{"name": "alpha", "provider": "zen", "model": "m", "url": "http://stub"}],
               "cases": ["c1", "c2", "c3"], "targets": ["sonnet", "opus"],
               "settings": {"repetitions": 1, "concurrency": 1, "timeout_secs": 60, "retries": 1, "include_unscored": false},
               "planned_trials": 3}),
        trial(0, "c1", vec![attempt("answered", json!(200), reply("low"))]),
        trial(1, "c2", vec![attempt("answered", json!(200), reply("max"))]),
        trial(
            2,
            "c3",
            vec![attempt("provider_error", json!(503), "busy".into())],
        ),
        json!({"type": "end", "state": "complete", "finished_unix_ms": 9}),
    ]
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
    for width in [60, 80, 100] {
        let rows = draw(&app, width, 30);
        let title = rows
            .iter()
            .find(|r| r.contains(" cases · "))
            .unwrap_or_else(|| panic!("no title at {width}:\n{}", rows.join("\n")));
        let counts = title.split(" cases · ").nth(1).unwrap();
        let words: Vec<&str> = counts.split_whitespace().collect();
        for (word, initial) in words.iter().zip(['h', 's', 'o', 'f']) {
            assert!(
                word.starts_with(initial) && word[1..].parse::<usize>().is_ok(),
                "title cut at {width}: {title:?}"
            );
        }
    }
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

#[test]
fn the_results_pane_shows_exactly_what_show_prints() {
    let (mut app, file) = app_with_fixture("tui-agreement");
    // The History list shows the run, then Enter opens its report.
    app.screen = Screen::Results;
    assert!(contains(&draw(&app, 100, 30), "fixture-1.jsonl"));
    press(&mut app, KeyCode::Enter);

    let run = evidence::read_run(&std::fs::read(&file).unwrap()).unwrap();
    let expected = report::report(&run).to_string();
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
    for line in expected.lines().filter(|l| !l.is_empty()) {
        assert!(
            contains(&rows, line),
            "missing report line {line:?} in:\n{}",
            rows.join("\n")
        );
    }
}

#[test]
fn right_and_left_scroll_the_report_sideways_without_changing_its_text() {
    let (mut app, _) = app_with_fixture("tui-hscroll");
    app.screen = Screen::Results;
    press(&mut app, KeyCode::Enter);
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
fn marking_two_runs_shows_the_compare_text() {
    let (mut app, file) = app_with_fixture("tui-compare");
    // A second run: the same file under another name and id.
    let second = std::fs::read_to_string(&file)
        .unwrap()
        .replace("fixture-1", "fixture-2")
        .replace("1000", "2000");
    std::fs::write(file.with_file_name("fixture-2.jsonl"), second).unwrap();
    press(&mut app, KeyCode::Char('5'));
    press(&mut app, KeyCode::Char('r'));
    press(&mut app, KeyCode::Char('g'));
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Char('m'));

    let runs: Vec<_> = ["fixture-2.jsonl", "fixture-1.jsonl"] // newest first, like the list
        .iter()
        .map(|n| evidence::read_run(&std::fs::read(file.with_file_name(n)).unwrap()).unwrap())
        .collect();
    let expected = report::Comparison(&runs).to_string();
    let rows = draw(&app, 200, (expected.lines().count() as u16 + 6).max(24));
    assert!(contains(&rows, "compare 2 runs"), "{}", app.message);
    for line in expected.lines().filter(|l| !l.is_empty()) {
        assert!(contains(&rows, line), "missing compare line {line:?}");
    }
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
