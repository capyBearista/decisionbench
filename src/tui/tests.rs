use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// An App over a fresh, empty temp directory.
fn app() -> (App, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "decisionbench-tui-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    // A recycled process id must not bring back an earlier run's files.
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::new(Some(dir.join("config.toml")), Some(dir.join("runs"))).unwrap();
    (app, dir)
}

fn press(app: &mut App, code: KeyCode) -> Effect {
    handle_key(app, KeyEvent::new(code, KeyModifiers::NONE))
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
}

/// Adds a Candidate through the form; `price` is the two price fields.
fn add_candidate(app: &mut App, name: &str, model: &str, price: [&str; 2]) {
    app.screen = Screen::Candidates;
    press(app, KeyCode::Char('a'));
    type_text(app, name);
    press(app, KeyCode::Tab);
    press(app, KeyCode::Char(' ')); // provider: openrouter -> zen
    press(app, KeyCode::Tab);
    type_text(app, model);
    for field in price {
        press(app, KeyCode::Tab);
        type_text(app, field);
    }
    press(app, KeyCode::Enter);
}

#[test]
fn number_keys_tab_and_backtab_switch_screens() {
    let (mut app, _) = app();
    assert_eq!(app.screen, Screen::Candidates);
    press(&mut app, KeyCode::Char('3'));
    assert_eq!(app.screen, Screen::Setup);
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.screen, Screen::Run);
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.screen, Screen::Candidates);
    press(&mut app, KeyCode::BackTab);
    assert_eq!(app.screen, Screen::Results);
}

#[test]
fn q_and_ctrl_c_quit_from_a_idle_app() {
    let (mut app, _) = app();
    assert_eq!(press(&mut app, KeyCode::Char('q')), Effect::Quit);
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(handle_key(&mut app, ctrl_c), Effect::Quit);
}

#[test]
fn typing_in_a_form_is_not_a_command() {
    let (mut app, _) = app();
    press(&mut app, KeyCode::Char('a'));
    assert!(app.editing());
    assert_eq!(press(&mut app, KeyCode::Char('q')), Effect::None);
    press(&mut app, KeyCode::Char('2'));
    assert_eq!(app.screen, Screen::Candidates);
    assert_eq!(app.candidates.form.as_ref().unwrap().fields[0], "q2");
    press(&mut app, KeyCode::Esc);
    assert!(!app.editing());
}

#[test]
fn a_candidate_without_prices_is_added_with_unknown_cost_and_saved() {
    let (mut app, dir) = app();
    add_candidate(&mut app, "alpha", "free-model", ["", ""]);
    let items = &app.candidates.items;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].provider.name(), "zen");
    assert!(items[0].price.is_none());
    assert!(app.candidates.dirty);
    press(&mut app, KeyCode::Char('s'));
    assert!(!app.candidates.dirty);
    let saved = std::fs::read_to_string(dir.join("config.toml")).unwrap();
    assert_eq!(
        crate::provider::parse_candidates(&saved).unwrap(),
        app.candidates.items
    );
    // A fresh App reads it back.
    let again = App::new(Some(dir.join("config.toml")), Some(dir.join("runs"))).unwrap();
    assert_eq!(again.candidates.items, app.candidates.items);
}

#[test]
fn candidate_validation_matches_the_config_loader() {
    let (mut app, _) = app();
    add_candidate(&mut app, "alpha", "m", ["1.5", ""]);
    let price = app.candidates.items[0].price.clone().unwrap();
    assert_eq!(
        (price.input_per_million, price.output_per_million),
        (1.5, 0.0)
    );
    // Duplicate name: the form stays open and says why.
    add_candidate(&mut app, "alpha", "m2", ["", ""]);
    assert!(
        app.message.contains("duplicate candidate name `alpha`"),
        "{}",
        app.message
    );
    assert!(app.candidates.form.is_some());
    press(&mut app, KeyCode::Esc);
    // Negative price, and a price without an input rate.
    add_candidate(&mut app, "beta", "m", ["-1", ""]);
    assert!(app.message.contains("non-negative"), "{}", app.message);
    press(&mut app, KeyCode::Esc);
    add_candidate(&mut app, "beta", "m", ["", "2"]);
    assert!(app.message.contains("needs input"), "{}", app.message);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.candidates.items.len(), 1);
}

#[test]
fn editing_replaces_and_deleting_removes_the_candidate_everywhere() {
    let (mut app, _) = app();
    add_candidate(&mut app, "alpha", "m", ["", ""]);
    app.setup.chosen.insert("alpha".into());
    press(&mut app, KeyCode::Enter); // edit the selected one
    for _ in 0..5 {
        press(&mut app, KeyCode::Backspace);
    }
    type_text(&mut app, "omega");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.candidates.items[0].name, "omega");
    assert!(app.setup.chosen.contains("omega") && !app.setup.chosen.contains("alpha"));
    press(&mut app, KeyCode::Char('d'));
    assert!(app.candidates.items.is_empty() && app.setup.chosen.is_empty());
}

#[test]
fn setup_toggles_and_validates_numbers() {
    let (mut app, _) = app();
    add_candidate(&mut app, "alpha", "m", ["", ""]);
    press(&mut app, KeyCode::Char('3'));
    press(&mut app, KeyCode::Char(' ')); // cursor starts on the first candidate
    assert!(app.setup.chosen.contains("alpha"));
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Char(' ')); // haiku off
    assert_eq!(app.setup.targets.len(), 3);
    // Move to repetitions (past 3 more targets and the instructions row) and type 0, then 5.
    for _ in 0..5 {
        press(&mut app, KeyCode::Char('j'));
    }
    press(&mut app, KeyCode::Enter);
    assert!(app.editing());
    press(&mut app, KeyCode::Backspace);
    type_text(&mut app, "0");
    press(&mut app, KeyCode::Enter);
    assert!(app.message.contains("at least 1"), "{}", app.message);
    assert!(app.editing());
    press(&mut app, KeyCode::Backspace);
    type_text(&mut app, "5");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.setup.repetitions, 5);
    press(&mut app, KeyCode::Char('l')); // + on a number row
    assert_eq!(app.setup.repetitions, 6);
}

#[test]
fn start_refuses_incomplete_setups_without_leaving_the_screen() {
    let (mut app, dir) = app();
    press(&mut app, KeyCode::Char('3'));
    press(&mut app, KeyCode::Char('G'));
    press(&mut app, KeyCode::Enter);
    assert!(
        app.message.contains("choose at least one candidate"),
        "{}",
        app.message
    );
    add_candidate(&mut app, "alpha", "m", ["", ""]);
    app.setup.chosen.insert("alpha".into());
    press(&mut app, KeyCode::Char('3'));
    press(&mut app, KeyCode::Char('G'));
    press(&mut app, KeyCode::Enter);
    assert!(
        app.message.contains("unsaved candidate edits"),
        "{}",
        app.message
    );
    assert_eq!(app.screen, Screen::Setup);
    assert!(!dir.join("runs").exists());
}

#[test]
fn quitting_with_unsaved_candidate_edits_needs_a_second_press() {
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    let (mut app, _) = app();
    add_candidate(&mut app, "alpha", "m", ["", ""]);
    assert_eq!(press(&mut app, KeyCode::Char('q')), Effect::None);
    assert_eq!(
        app.message,
        "unsaved candidate edits: press q again to quit without saving, or s to save"
    );
    assert_eq!(press(&mut app, KeyCode::Char('q')), Effect::Quit);
    // Any other key in between disarms it; Ctrl-C counts like q.
    assert_eq!(handle_key(&mut app, ctrl_c), Effect::None);
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(handle_key(&mut app, ctrl_c), Effect::None);
    assert_eq!(handle_key(&mut app, ctrl_c), Effect::Quit);
    // Saved edits quit at once.
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(press(&mut app, KeyCode::Char('q')), Effect::Quit);
}

#[test]
fn quitting_a_run_with_unsaved_candidate_edits_takes_two_presses() {
    use super::progress::Phase;
    for phase in [Phase::Running, Phase::Cancelling] {
        let (mut app, _) = app();
        add_candidate(&mut app, "alpha", "m", ["", ""]);
        app.run.phase = phase.clone();
        assert_eq!(press(&mut app, KeyCode::Char('q')), Effect::None);
        assert_eq!(
            app.message,
            "run in progress and unsaved candidate edits: press again to cancel the run and quit without saving"
        );
        assert_eq!(app.run.phase, phase);
        assert_eq!(press(&mut app, KeyCode::Char('q')), Effect::Quit);
    }
}

#[test]
fn saving_leaves_no_temp_file_behind() {
    let (mut app, dir) = app();
    add_candidate(&mut app, "alpha", "m", ["", ""]);
    press(&mut app, KeyCode::Char('s'));
    add_candidate(&mut app, "beta", "m", ["", ""]);
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(app.message, format!("saved {}", app.config.display()));
    assert!(
        std::fs::read_to_string(dir.join("config.toml"))
            .unwrap()
            .contains("beta")
    );
    assert_eq!(crate::batch::temp_files_in(&dir), Vec::<String>::new());
}

#[test]
fn saving_ignores_a_planted_symlink_at_the_old_temp_name() {
    let (mut app, dir) = app();
    std::fs::create_dir_all(&dir).unwrap();
    let (planted, victim) = (dir.join("config.toml.tmp"), dir.join("victim"));
    std::os::unix::fs::symlink(&victim, &planted).unwrap();
    add_candidate(&mut app, "alpha", "m", ["", ""]);
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(app.message, format!("saved {}", app.config.display()));
    assert!(
        std::fs::read_to_string(dir.join("config.toml"))
            .unwrap()
            .contains("alpha")
    );
    assert!(!victim.exists());
    assert!(std::fs::symlink_metadata(&planted).unwrap().is_symlink());
    assert_eq!(crate::batch::temp_files_in(&dir), ["config.toml.tmp"]);
}

#[test]
fn saving_keeps_the_configs_comments() {
    let (_, dir) = app();
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        "# mine\n[[candidate]]\nname = \"a\"\nprovider = \"zen\"\nmodel = \"m\" # pinned\n",
    )
    .unwrap();
    let mut app = App::new(Some(config.clone()), Some(dir.join("runs"))).unwrap();
    add_candidate(&mut app, "beta", "n", ["", ""]);
    press(&mut app, KeyCode::Char('s'));
    let saved = std::fs::read_to_string(&config).unwrap();
    assert!(
        saved.contains("# mine") && saved.contains("model = \"m\" # pinned"),
        "{saved}"
    );
    assert!(saved.contains("beta"), "{saved}");
    assert_eq!(app.message, format!("saved {}", config.display()));
}

#[test]
fn saving_through_a_symlinked_config_keeps_the_link() {
    let (_, dir) = app();
    std::fs::create_dir_all(dir.join("real")).unwrap();
    let target = dir.join("real/config.toml");
    let link = dir.join("config.toml");
    std::fs::write(
        &target,
        "# mine\n[[candidate]]\nname = \"a\"\nprovider = \"zen\"\nmodel = \"m\"\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let mut app = App::new(Some(link.clone()), Some(dir.join("runs"))).unwrap();
    add_candidate(&mut app, "beta", "n", ["", ""]);
    press(&mut app, KeyCode::Char('s'));
    assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    let saved = std::fs::read_to_string(&target).unwrap();
    assert!(
        saved.contains("# mine") && saved.contains("beta"),
        "{saved}"
    );
    assert_eq!(
        crate::batch::temp_files_in(&dir.join("real")),
        Vec::<String>::new()
    );
}

#[test]
fn saving_through_a_dangling_symlink_creates_the_target_and_keeps_the_link() {
    use std::os::unix::fs::PermissionsExt;
    let (_, dir) = app();
    std::fs::create_dir_all(&dir).unwrap();
    let link = dir.join("config.toml");
    std::os::unix::fs::symlink("shared/new.toml", &link).unwrap();
    let mut app = App::new(Some(link.clone()), Some(dir.join("runs"))).unwrap();
    add_candidate(&mut app, "beta", "n", ["", ""]);
    press(&mut app, KeyCode::Char('s'));
    assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    let target = dir.join("shared/new.toml");
    assert!(std::fs::read_to_string(&target).unwrap().contains("beta"));
    let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn saving_through_a_two_hop_symlink_chain_keeps_both_links() {
    let (_, dir) = app();
    std::fs::create_dir_all(dir.join("real")).unwrap();
    let (first, second, target) = (
        dir.join("config.toml"),
        dir.join("hop.toml"),
        dir.join("real/config.toml"),
    );
    std::os::unix::fs::symlink("hop.toml", &first).unwrap();
    std::os::unix::fs::symlink("real/config.toml", &second).unwrap();
    let mut app = App::new(Some(first.clone()), Some(dir.join("runs"))).unwrap();
    add_candidate(&mut app, "beta", "n", ["", ""]);
    press(&mut app, KeyCode::Char('s'));
    assert!(std::fs::symlink_metadata(&first).unwrap().is_symlink());
    assert!(std::fs::symlink_metadata(&second).unwrap().is_symlink());
    assert!(std::fs::read_to_string(&target).unwrap().contains("beta"));
}

#[test]
fn saving_keeps_the_configs_permissions_and_creates_new_ones_private() {
    use std::os::unix::fs::PermissionsExt;
    let mode =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    let (mut app, dir) = app();
    let config = dir.join("config.toml");
    // A stale, world-readable file at the old temp name is neither reused nor lends its mode.
    let stale = dir.join("config.toml.tmp");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&stale, "").unwrap();
    std::fs::set_permissions(&stale, std::fs::Permissions::from_mode(0o644)).unwrap();
    add_candidate(&mut app, "alpha", "m", ["", ""]);
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(mode(&config), 0o600);
    assert_eq!(std::fs::read_to_string(&stale).unwrap(), "");
    assert_eq!(mode(&stale), 0o644);
    for existing in [0o600, 0o640] {
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(existing)).unwrap();
        add_candidate(&mut app, &format!("n{existing}"), "m", ["", ""]);
        press(&mut app, KeyCode::Char('s'));
        assert_eq!(mode(&config), existing);
    }
}

#[test]
fn space_marks_cases_on_the_dataset_screen_and_x_clears_them() {
    let (mut app, _) = app();
    assert_eq!(app.dataset.choice(), "all eligible");
    press(&mut app, KeyCode::Char('2'));
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(app.dataset.marked.len(), 2);
    assert!(app.dataset.choice().starts_with("2 selected ("));
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(app.dataset.marked.len(), 1);
    press(&mut app, KeyCode::Char('x'));
    assert!(app.dataset.marked.is_empty());
    assert_eq!(app.dataset.choice(), "all eligible");
}

#[test]
fn run_setup_passes_the_marked_cases_as_the_case_selection() {
    let (mut app, _) = app();
    add_candidate(&mut app, "alpha", "m", ["", ""]);
    press(&mut app, KeyCode::Char('s'));
    app.setup.chosen.insert("alpha".into());
    assert!(setup::args(&app).unwrap().cases.is_empty());
    press(&mut app, KeyCode::Char('2'));
    press(&mut app, KeyCode::Char(' '));
    let marked: Vec<String> = app.dataset.marked.iter().cloned().collect();
    assert_eq!(marked.len(), 1);
    assert_eq!(setup::args(&app).unwrap().cases, marked);
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

const SECRET: &str = "sk-test-secret-9876";

/// The Zen candidate `add_candidate` makes, selected, and its (not yet written) credentials file.
fn app_with_candidate() -> (App, PathBuf) {
    let (mut app, dir) = app();
    add_candidate(&mut app, "alpha", "m", ["", ""]);
    (app, dir.join("credentials.toml"))
}

/// What the Zen key status reads once a key is saved: the host's own `OPENCODE_API_KEY` outranks it.
fn saved_status() -> &'static str {
    match std::env::var("OPENCODE_API_KEY") {
        Ok(key) if !key.is_empty() => "key: env",
        _ => "key: saved",
    }
}

#[test]
fn k_opens_a_prompt_that_takes_letters_and_shows_only_bullets() {
    let (mut app, _) = app_with_candidate();
    press(&mut app, KeyCode::Char('K'));
    assert!(app.editing());
    type_text(&mut app, "kK");
    type_text(&mut app, SECRET);
    // Letters and digits (k and K included) are typed, not commands.
    assert_eq!(app.screen, Screen::Candidates);
    assert_eq!(press(&mut app, KeyCode::Char('q')), Effect::None);
    assert_eq!(
        app.candidates.key_prompt.as_ref().unwrap().text.len(),
        SECRET.len() + 3
    );
    for (w, h) in [(80, 24), (60, 16)] {
        let rows = draw(&app, w, h).join("\n");
        assert!(rows.contains("zen API key"), "{rows}");
        assert!(
            rows.contains(&"•".repeat(SECRET.len() + 3)),
            "{w}x{h}:\n{rows}"
        );
        assert!(!rows.contains(SECRET) && !rows.contains("sk-"), "{rows}");
    }
    assert!(!app.message.contains("sk-"), "{}", app.message);
    press(&mut app, KeyCode::Backspace);
    assert!(
        draw(&app, 80, 24)
            .join("\n")
            .contains(&"•".repeat(SECRET.len() + 2))
    );
}

#[test]
fn enter_saves_a_trimmed_key_privately_without_touching_the_config() {
    use std::os::unix::fs::PermissionsExt;
    let (mut app, creds) = app_with_candidate();
    assert!(app.candidates.dirty);
    press(&mut app, KeyCode::Char('K'));
    type_text(&mut app, &format!("  {SECRET} "));
    press(&mut app, KeyCode::Enter);
    assert!(!app.editing());
    assert!(
        app.message
            .starts_with(&format!("saved zen key to {}", creds.display()))
    );
    assert!(!app.message.contains("sk-"), "{}", app.message);
    assert_eq!(
        crate::credentials::load(&creds).unwrap().zen.as_deref(),
        Some(SECRET)
    );
    assert_eq!(
        std::fs::metadata(&creds).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // Immediate: the unsaved candidate edit is still unsaved, and no config was written.
    assert!(app.candidates.dirty && !creds.with_file_name("config.toml").exists());
    // The row now reports a key, and nothing on screen shows it.
    for (w, h) in [(80, 24), (60, 16)] {
        let rows = draw(&app, w, h).join("\n");
        // Narrow rows drop the `key: ` prefix.
        let status = if w >= 80 {
            saved_status()
        } else {
            saved_status().trim_start_matches("key: ")
        };
        assert!(rows.contains(status), "{w}x{h}:\n{rows}");
        assert!(!rows.contains(SECRET));
    }
}

#[test]
fn a_saved_key_says_when_an_environment_key_overrides_it() {
    use crate::provider::Provider;
    let (_, dir) = app();
    let config = dir.join("config.toml");
    let store = |env: Option<&str>| {
        super::candidates::store_key(&config, Provider::Zen, "x", env.map(str::to_string))
    };
    let path = dir.join("credentials.toml").display().to_string();
    assert_eq!(store(None), format!("saved zen key to {path}"));
    assert_eq!(store(Some("")), format!("saved zen key to {path}"));
    assert_eq!(
        store(Some("from-env")),
        format!("saved zen key to {path} (OPENCODE_API_KEY is set and overrides it)")
    );
}

#[test]
fn an_unreadable_credentials_file_shows_as_a_file_error_on_every_row() {
    let (mut app, dir) = app();
    add_candidate(&mut app, "alpha", "m", ["", ""]);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("credentials.toml"), "typo = \"x\"\n").unwrap();
    for (w, h) in [(80, 24), (60, 16)] {
        let rows = draw(&app, w, h);
        assert!(
            rows.iter()
                .any(|r| r.contains("alpha") && r.contains("! ") && r.contains("file error")),
            "{w}x{h}:\n{}",
            rows.join("\n")
        );
    }
}

#[test]
fn esc_and_an_empty_key_save_nothing() {
    let (mut app, creds) = app_with_candidate();
    press(&mut app, KeyCode::Char('K'));
    type_text(&mut app, SECRET);
    press(&mut app, KeyCode::Esc);
    assert!(!app.editing() && !creds.exists());
    for blank in ["", "   "] {
        press(&mut app, KeyCode::Char('K'));
        type_text(&mut app, blank);
        press(&mut app, KeyCode::Enter);
        assert!(!app.editing() && !creds.exists());
        assert_eq!(app.message, "empty: nothing saved");
    }
}

#[test]
fn a_failed_key_save_names_the_path_not_the_key() {
    let (mut app, creds) = app_with_candidate();
    std::fs::create_dir_all(&creds).unwrap(); // unreadable as a credentials file
    press(&mut app, KeyCode::Char('K'));
    type_text(&mut app, SECRET);
    press(&mut app, KeyCode::Enter);
    assert!(
        app.message.contains(&creds.display().to_string()),
        "{}",
        app.message
    );
    assert!(!app.message.contains("sk-"), "{}", app.message);
    assert!(!app.editing());
}

#[test]
fn lowercase_k_still_moves_up_and_does_not_open_the_prompt() {
    let (mut app, _) = app_with_candidate();
    add_candidate(&mut app, "beta", "m", ["", ""]);
    assert_eq!(app.candidates.selected, 1);
    press(&mut app, KeyCode::Char('k'));
    assert!(!app.editing() && app.candidates.key_prompt.is_none());
    assert_eq!(app.candidates.selected, 0);
}

#[test]
fn shift_k_without_a_candidate_opens_nothing() {
    let (mut app, _) = app();
    press(&mut app, KeyCode::Char('K'));
    assert!(!app.editing());
    assert!(app.message.contains("no candidate"), "{}", app.message);
}

#[test]
fn each_row_ends_with_its_key_status_without_being_clipped() {
    use crate::provider::{Candidate, Provider};
    let (mut app, dir) = app();
    for (name, provider) in [("zen-one", Provider::Zen), ("or-one", Provider::OpenRouter)] {
        app.candidates.items.push(Candidate {
            name: name.into(),
            decision_model: None,
            provider,
            model: "a-rather-long-model-identifier".into(),
            price: None,
        });
    }
    let creds = dir.join("credentials.toml");
    for (w, h) in [(60, 16), (80, 24), (120, 40)] {
        let rows = draw(&app, w, h);
        for c in &app.candidates.items {
            let expected =
                super::candidates::key_text(c.provider, c.provider.key_source(&creds), w >= 80);
            let row = rows.iter().find(|r| r.contains(&c.name)).unwrap();
            assert!(
                row.contains("cost unknown")
                    && row.contains(expected.content.as_ref())
                    && row.contains("a-rather"),
                "{w}x{h}: {row:?} lacks {:?}",
                expected.content
            );
        }
    }
}

/// The row of the one Candidate `add_candidate` makes, drawn at `width`x`height`.
fn candidate_row(name: &str, width: u16, height: u16) -> String {
    let (mut app, _) = app();
    add_candidate(&mut app, name, "a-rather-long-model-identifier", ["", ""]);
    let rows = draw(&app, width, height);
    let head: String = name.chars().take(8).collect();
    rows.into_iter()
        .find(|r| r.contains(&head))
        .unwrap_or_else(|| panic!("no row for {name} at {width}x{height}"))
}

#[test]
fn a_long_candidate_name_is_not_cut_at_twelve_characters() {
    for (w, h) in [(120, 40), (80, 24)] {
        let row = candidate_row("decider-flash-v.1.2", w, h);
        assert!(row.contains("decider-flash-v.1.2 "), "{w}x{h}: {row:?}");
    }
}

#[test]
fn a_long_candidate_name_leaves_the_model_and_key_status_readable_at_60_columns() {
    use crate::provider::Provider;
    let row = candidate_row("decider-flash-v.1.2", 60, 16);
    // `add_candidate` makes a Zen candidate; its key status depends on the host's environment.
    let key = Provider::Zen.key_source(&std::env::temp_dir().join("no-such-credentials.toml"));
    let key = super::candidates::key_text(Provider::Zen, key, false);
    assert!(
        row.contains("a-rather")
            && row.contains("cost unknown")
            && row.contains(key.content.as_ref()),
        "{row:?}"
    );
}

#[test]
fn a_candidate_name_over_24_characters_is_cut_with_an_ellipsis() {
    let row = candidate_row("abcdefghijklmnopqrstuvwxyz0123", 120, 40);
    assert!(row.contains("abcdefghijklmnopqrstuvw… "), "{row:?}");
    assert!(!row.contains("abcdefghijklmnopqrstuvwx"), "{row:?}");
}

#[test]
fn key_status_wording() {
    use super::candidates::key_text;
    use crate::provider::{KeySource, Provider};
    let text = |p, s, wide| key_text(p, s, wide).content.to_string();
    assert_eq!(text(Provider::Zen, KeySource::Env, true), "key: env");
    assert_eq!(text(Provider::Zen, KeySource::Env, false), "env");
    assert_eq!(
        text(Provider::OpenRouter, KeySource::Saved, true),
        "key: saved"
    );
    assert_eq!(text(Provider::OpenRouter, KeySource::Saved, false), "saved");
    assert_eq!(
        text(Provider::Zen, KeySource::Missing, true),
        "key: none (free ok)"
    );
    assert_eq!(text(Provider::Zen, KeySource::Missing, false), "none");
    assert_eq!(
        text(Provider::OpenRouter, KeySource::Missing, true),
        "! key: missing"
    );
    assert_eq!(
        text(Provider::OpenRouter, KeySource::Missing, false),
        "! missing"
    );
    for provider in [Provider::Zen, Provider::OpenRouter] {
        assert_eq!(
            text(provider, KeySource::Invalid, true),
            "! key: file error"
        );
        assert_eq!(text(provider, KeySource::Invalid, false), "! file error");
    }
}
