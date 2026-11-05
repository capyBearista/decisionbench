//! Offline workflow: drive the TUI state machine against a local stub of the Decisions endpoint.
//! One test function, because it sets the process-wide test base URL.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use decisionbench::tui::{App, Screen, handle_key, render};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const ALL_LOW: &str = r#"{"answers":{"effort_haiku":{"type":"choice","choice":"low"},"effort_sonnet":{"type":"choice","choice":"low"},"effort_opus":{"type":"choice","choice":"low"},"effort_fable":{"type":"choice","choice":"low"}}}"#;

/// Answers every request with `ALL_LOW` after `delay`; returns its URL.
fn stub(delay: Duration) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/decisions", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut raw = Vec::new();
                let mut chunk = [0; 4096];
                loop {
                    let Ok(n) = stream.read(&mut chunk) else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&raw).to_lowercase();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length: usize = text[..end]
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .map_or(0, |v| v.trim().parse().unwrap());
                        if raw.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                std::thread::sleep(delay);
                let response = format!(
                    "HTTP/1.1 200 Stub\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{ALL_LOW}",
                    ALL_LOW.len()
                );
                let _ = stream.write_all(response.as_bytes());
            });
        }
    });
    url
}

fn press(app: &mut App, code: KeyCode) {
    handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
}

fn until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(app) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        app.tick();
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn screen_text(app: &App) -> String {
    let (width, height) = (200, 120);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| render(frame, app)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .flat_map(|y| {
            let row: String = (0..width).map(|x| buffer[(x, y)].symbol()).collect();
            [row, "\n".into()]
        })
        .collect()
}

fn show(file: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_decisionbench"))
        .args(["show", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

/// An App whose config already holds one keyless Zen candidate, set up to ask only about opus
/// (66 labelled cases) once.
fn new_app(name: &str) -> (App, PathBuf) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        "[[candidate]]\nname = \"alpha\"\nprovider = \"zen\"\nmodel = \"free\"\n",
    )
    .unwrap();
    let out = dir.join("runs");
    let mut app = App::new(Some(config), Some(out.clone())).unwrap();
    app.setup.targets = [decisionbench::grade::Target::Opus].into();
    app.setup.repetitions = 1;
    (app, out)
}

/// Keys only: Setup screen, pick the first candidate, jump to Start and press Enter.
fn start(app: &mut App) {
    press(app, KeyCode::Char('3'));
    press(app, KeyCode::Char(' '));
    press(app, KeyCode::Char('G'));
    press(app, KeyCode::Enter);
    assert_eq!(app.screen, Screen::Run, "{}", app.message);
}

fn only_file(dir: &Path) -> PathBuf {
    let files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    files.into_iter().next().unwrap()
}

#[test]
fn a_run_completes_and_a_cancelled_run_leaves_partial_evidence_that_show_reads() {
    // SAFETY: the only test in this binary, so no other test thread reads the environment.
    unsafe { std::env::set_var("DECISIONBENCH_TEST_BASE_URL", stub(Duration::ZERO)) };
    let (mut app, out) = new_app("tui-workflow-complete");
    start(&mut app);
    until(&mut app, "the run to complete", |a| {
        a.run.done == a.run.planned && !a.run.active()
    });
    assert_eq!((app.run.planned, app.run.errors), (66, 0));
    let file = only_file(&out);
    assert!(show(&file).contains("state: complete"));

    // Enter on the Run screen opens the report in Results, on its summary; the full report panel
    // is the same text `show` prints.
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen, Screen::Results);
    press(&mut app, KeyCode::Char(']'));
    let screen = screen_text(&app);
    for line in show(&file).lines().filter(|l| !l.is_empty()) {
        assert!(screen.contains(line), "missing {line:?}");
    }

    // Cancel after the first trial, then wait for the in-flight one to end.
    unsafe {
        std::env::set_var(
            "DECISIONBENCH_TEST_BASE_URL",
            stub(Duration::from_millis(300)),
        )
    };
    let (mut app, out) = new_app("tui-workflow-cancel");
    start(&mut app);
    until(&mut app, "the first trial", |a| a.run.done >= 1);
    press(&mut app, KeyCode::Char('c'));
    assert!(screen_text(&app).contains("cancelling"));
    until(&mut app, "the run to stop", |a| !a.run.active());
    assert!(
        app.run.done >= 1 && app.run.done < app.run.planned,
        "{}",
        app.run.done
    );
    assert!(screen_text(&app).contains("cancelled"));
    let file = only_file(&out);
    let text = show(&file);
    assert!(text.contains("partial (cancelled"), "{text}");
    assert!(
        text.contains(&format!("trials: {} recorded of 66 planned", app.run.done)),
        "{text}"
    );
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char(']'));
    let screen = screen_text(&app);
    for line in text.lines().filter(|l| !l.is_empty()) {
        assert!(screen.contains(line), "missing {line:?}");
    }

    // One marked case on the Dataset screen: the run plans exactly that case.
    let (mut app, out) = new_app("tui-workflow-marked");
    press(&mut app, KeyCode::Char('2'));
    press(&mut app, KeyCode::Char('j')); // c002, labelled for opus
    press(&mut app, KeyCode::Char(' '));
    assert!(screen_text(&app).contains("[x]"));
    press(&mut app, KeyCode::Char('3'));
    assert!(screen_text(&app).contains("cases: 1 selected (c002)"));
    start(&mut app);
    until(&mut app, "the marked run to end", |a| !a.run.active());
    assert_eq!(app.run.planned, 1);
    assert!(show(&only_file(&out)).contains("state: complete"));

    // The bundled prompt plus a second one added on the Setup screen: two trials for the one case.
    let (mut app, out) = new_app("tui-workflow-prompts");
    let second = out.with_file_name("second.md");
    std::fs::copy("prompts/neutral.md", &second).unwrap();
    press(&mut app, KeyCode::Char('2'));
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Char('3'));
    press(&mut app, KeyCode::Char(' ')); // the candidate
    for _ in 0..5 {
        press(&mut app, KeyCode::Char('j')); // past the targets, onto the bundled prompt
    }
    press(&mut app, KeyCode::Char('a'));
    for c in second.display().to_string().chars() {
        press(&mut app, KeyCode::Char(c));
    }
    press(&mut app, KeyCode::Enter);
    assert!(
        screen_text(&app).contains("[x] prompt second"),
        "{}",
        app.message
    );
    assert!(screen_text(&app).contains("plan: 1 candidate × 2 prompts × 1 case"));
    press(&mut app, KeyCode::Char('G'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen, Screen::Run, "{}", app.message);
    until(&mut app, "the two-prompt run to end", |a| !a.run.active());
    assert_eq!(app.run.planned, 2);
    let run = decisionbench::batch::load_run(&only_file(&out)).unwrap();
    let names: Vec<_> = run
        .manifest
        .instructions
        .iter()
        .map(|i| i.name.as_str())
        .collect();
    assert_eq!(names, ["neutral", "second"]);
    assert_eq!(run.trials.len(), 2);
}
