use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

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

/// Adds a Candidate through the form; `price` is the three price fields.
fn add_candidate(app: &mut App, name: &str, model: &str, price: [&str; 3]) {
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
    add_candidate(&mut app, "alpha", "free-model", ["", "", ""]);
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
    add_candidate(&mut app, "alpha", "m", ["1.5", "", ""]);
    let price = app.candidates.items[0].price.clone().unwrap();
    assert_eq!(
        (price.input_per_million, price.output_per_million),
        (1.5, 0.0)
    );
    // Duplicate name: the form stays open and says why.
    add_candidate(&mut app, "alpha", "m2", ["", "", ""]);
    assert!(
        app.message.contains("duplicate candidate name `alpha`"),
        "{}",
        app.message
    );
    assert!(app.candidates.form.is_some());
    press(&mut app, KeyCode::Esc);
    // Negative price, and a price without an input rate.
    add_candidate(&mut app, "beta", "m", ["-1", "", ""]);
    assert!(app.message.contains("non-negative"), "{}", app.message);
    press(&mut app, KeyCode::Esc);
    add_candidate(&mut app, "beta", "m", ["", "2", ""]);
    assert!(app.message.contains("needs input"), "{}", app.message);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.candidates.items.len(), 1);
}

#[test]
fn editing_replaces_and_deleting_removes_the_candidate_everywhere() {
    let (mut app, _) = app();
    add_candidate(&mut app, "alpha", "m", ["", "", ""]);
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
    add_candidate(&mut app, "alpha", "m", ["", "", ""]);
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
    add_candidate(&mut app, "alpha", "m", ["", "", ""]);
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
    add_candidate(&mut app, "alpha", "m", ["", "", ""]);
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
        add_candidate(&mut app, "alpha", "m", ["", "", ""]);
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
fn saving_writes_a_temp_file_then_renames_it_over_the_config() {
    let (mut app, dir) = app();
    add_candidate(&mut app, "alpha", "m", ["", "", ""]);
    press(&mut app, KeyCode::Char('s'));
    let config = dir.join("config.toml");
    let first = std::fs::read_to_string(&config).unwrap();
    assert!(!dir.join("config.toml.tmp").exists());
    // A blocked temp path makes the save fail and leaves the existing config untouched.
    std::fs::create_dir(dir.join("config.toml.tmp")).unwrap();
    add_candidate(&mut app, "beta", "m", ["", "", ""]);
    press(&mut app, KeyCode::Char('s'));
    assert!(app.message.contains("cannot write"), "{}", app.message);
    assert!(app.candidates.dirty);
    assert_eq!(std::fs::read_to_string(&config).unwrap(), first);
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
    add_candidate(&mut app, "beta", "n", ["", "", ""]);
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
    add_candidate(&mut app, "beta", "n", ["", "", ""]);
    press(&mut app, KeyCode::Char('s'));
    assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    let saved = std::fs::read_to_string(&target).unwrap();
    assert!(
        saved.contains("# mine") && saved.contains("beta"),
        "{saved}"
    );
    assert!(!dir.join("real/config.toml.tmp").exists());
}

#[test]
fn saving_through_a_dangling_symlink_creates_the_target_and_keeps_the_link() {
    use std::os::unix::fs::PermissionsExt;
    let (_, dir) = app();
    std::fs::create_dir_all(&dir).unwrap();
    let link = dir.join("config.toml");
    std::os::unix::fs::symlink("shared/new.toml", &link).unwrap();
    let mut app = App::new(Some(link.clone()), Some(dir.join("runs"))).unwrap();
    add_candidate(&mut app, "beta", "n", ["", "", ""]);
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
    add_candidate(&mut app, "beta", "n", ["", "", ""]);
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
    // A stale, world-readable temp file from an earlier failed save must not lend its mode.
    let stale = dir.join("config.toml.tmp");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&stale, "").unwrap();
    std::fs::set_permissions(&stale, std::fs::Permissions::from_mode(0o644)).unwrap();
    add_candidate(&mut app, "alpha", "m", ["", "", ""]);
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(mode(&config), 0o600);
    for existing in [0o600, 0o640] {
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(existing)).unwrap();
        add_candidate(&mut app, &format!("n{existing}"), "m", ["", "", ""]);
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
    add_candidate(&mut app, "alpha", "m", ["", "", ""]);
    press(&mut app, KeyCode::Char('s'));
    app.setup.chosen.insert("alpha".into());
    assert!(setup::args(&app).unwrap().cases.is_empty());
    press(&mut app, KeyCode::Char('2'));
    press(&mut app, KeyCode::Char(' '));
    let marked: Vec<String> = app.dataset.marked.iter().cloned().collect();
    assert_eq!(marked.len(), 1);
    assert_eq!(setup::args(&app).unwrap().cases, marked);
}
