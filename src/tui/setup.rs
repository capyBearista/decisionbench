//! Run setup: which Candidates, Targets and settings, then Start.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{App, Nav, Screen, block, cut, edit, list_lines, nav, step, theme};
use crate::batch::{self, RunArgs, load_instructions, prepare};
use crate::cost::plural;
use crate::grade::Target;
use crate::instructions::BUNDLED_NAME;
use crate::run::{self, Settings};

/// A prompt file over this is listed but not read, so a stray large file cannot stall the screen.
const MAX_PROMPT_BYTES: u64 = 256 * 1024;
/// Folder files listed; the rest are counted in a note.
const MAX_FOLDER_FILES: usize = 200;

pub const HINTS: &str =
    "Enter edit/start · Space toggle · a add · d del · r rescan · h/l -/+ · Esc cancel · q quit";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Candidate(usize),
    Target(Target),
    Prompt(usize),
    PromptDir,
    Repetitions,
    Concurrency,
    Timeout,
    Retries,
    Unscored,
    Start,
}

/// Where a prompt (Judge Instructions) comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The built-in `neutral`.
    Bundled,
    /// A file the user added by path.
    Typed(PathBuf),
    /// A file found in the prompt folder.
    Folder(PathBuf),
}

pub struct Prompt {
    pub source: Source,
    /// What reports call it: the file stem.
    pub name: String,
    pub chosen: bool,
    /// Why it cannot be chosen.
    pub problem: Option<String>,
}

impl Prompt {
    fn path(&self) -> Option<&Path> {
        match &self.source {
            Source::Bundled => None,
            Source::Typed(path) | Source::Folder(path) => Some(path),
        }
    }
}

pub struct State {
    /// Chosen Candidate names.
    pub chosen: BTreeSet<String>,
    pub targets: BTreeSet<Target>,
    /// The bundled prompt, then the typed ones, then the prompt folder's.
    pub prompts: Vec<Prompt>,
    pub repetitions: u32,
    pub concurrency: u32,
    pub timeout_secs: u64,
    pub retries: u32,
    pub include_unscored: bool,
    cursor: usize,
    /// The text being typed into the row under the cursor.
    pub editing: Option<String>,
    /// `editing` is the path of a prompt to add, not the row's own value.
    adding: bool,
    /// Why the prompt folder lists nothing.
    folder_note: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            chosen: BTreeSet::new(),
            targets: Target::ALL.into_iter().collect(),
            prompts: vec![Prompt {
                source: Source::Bundled,
                name: BUNDLED_NAME.into(),
                chosen: true,
                problem: None,
            }],
            repetitions: 3,
            concurrency: 1,
            timeout_secs: 60,
            retries: 1,
            include_unscored: false,
            cursor: 0,
            editing: None,
            adding: false,
            folder_note: None,
        }
    }
}

fn rows(candidates: usize, prompts: usize) -> Vec<Row> {
    (0..candidates)
        .map(Row::Candidate)
        .chain(Target::ALL.map(Row::Target))
        .chain((0..prompts).map(Row::Prompt))
        .chain([
            Row::PromptDir,
            Row::Repetitions,
            Row::Concurrency,
            Row::Timeout,
            Row::Retries,
            Row::Unscored,
            Row::Start,
        ])
        .collect()
}

impl State {
    fn text(&self, row: Row) -> Option<String> {
        Some(match row {
            Row::Repetitions => self.repetitions.to_string(),
            Row::Concurrency => self.concurrency.to_string(),
            Row::Timeout => self.timeout_secs.to_string(),
            Row::Retries => self.retries.to_string(),
            _ => return None,
        })
    }

    /// Stores `text` into the row's field. Counts must be whole numbers, and all but retries at least 1.
    fn set(&mut self, row: Row, text: &str) -> Result<(), String> {
        let text = text.trim();
        let n: u64 = text
            .parse()
            .map_err(|_| format!("`{text}` is not a whole number"))?;
        if n == 0 && row != Row::Retries {
            return Err("must be at least 1".into());
        }
        match row {
            Row::Repetitions => self.repetitions = u32::try_from(n).map_err(|e| e.to_string())?,
            Row::Concurrency => self.concurrency = u32::try_from(n).map_err(|e| e.to_string())?,
            Row::Timeout => self.timeout_secs = n,
            Row::Retries => self.retries = u32::try_from(n).map_err(|e| e.to_string())?,
            _ => {}
        }
        Ok(())
    }
}

pub(super) fn args(app: &App) -> Result<RunArgs, String> {
    let state = &app.setup;
    if app.candidates.dirty {
        return Err("unsaved candidate edits: press s on the Candidates screen first".into());
    }
    // Config order, so the evidence lists Candidates as the config does.
    let candidates: Vec<String> = app
        .candidates
        .items
        .iter()
        .filter(|c| state.chosen.contains(&c.name))
        .map(|c| c.name.clone())
        .collect();
    if candidates.is_empty() {
        return Err("choose at least one candidate".into());
    }
    if state.targets.is_empty() {
        return Err("choose at least one target".into());
    }
    let instructions: Vec<Option<PathBuf>> = state
        .prompts
        .iter()
        .filter(|p| p.chosen)
        .map(|p| p.path().map(Path::to_path_buf))
        .collect();
    if instructions.is_empty() {
        return Err("choose at least one prompt".into());
    }
    Ok(RunArgs {
        candidates,
        instructions,
        cases: app.dataset.marked.iter().cloned().collect(),
        targets: state.targets.iter().copied().collect(),
        settings: Settings {
            repetitions: state.repetitions,
            concurrency: state.concurrency,
            timeout_secs: state.timeout_secs,
            retries: state.retries,
            include_unscored: state.include_unscored,
        },
        config: Some(app.config.clone()),
        dataset: None,
        out: Some(app.out.clone()),
    })
}

/// Appends the prompt file at `text`, chosen, after the bundled and typed prompts.
fn add_prompt(app: &mut App, text: &str) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("enter the path of a prompt file".into());
    }
    let path = PathBuf::from(text);
    let prompts = &mut app.setup.prompts;
    if prompts.iter().any(|p| p.path() == Some(path.as_path())) {
        return Err(format!("`{text}` is already added"));
    }
    let name = load_instructions(Some(&path))?.name;
    // A listed file with a problem can never be chosen, so it does not hold its name.
    if prompts
        .iter()
        .any(|p| p.problem.is_none() && p.name == name)
    {
        return Err(format!("a prompt named `{name}` is already listed"));
    }
    let at = prompts
        .iter()
        .position(|p| matches!(p.source, Source::Folder(_)))
        .unwrap_or(prompts.len());
    prompts.insert(
        at,
        Prompt {
            source: Source::Typed(path),
            name,
            chosen: true,
            problem: None,
        },
    );
    Ok(())
}

/// Saves the prompt folder to the config (blank clears it) and lists its files.
fn set_folder(app: &mut App, text: &str) -> Result<(), String> {
    batch::set_prompts_dir(&app.config, Some(text))?;
    app.prompts_dir = Some(text.trim().to_string()).filter(|dir| !dir.is_empty());
    app.message = format!("saved {}", app.config.display());
    rescan(app);
    Ok(())
}

/// The `*.md` files directly in `dir`, by file name.
fn markdown_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let fail = |e: std::io::Error| format!("cannot read prompt folder {}: {e}", dir.display());
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(fail)? {
        let path = entry.map_err(fail)?.path();
        if path.extension().is_some_and(|ext| ext == "md") && path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// What the cursor is on, in terms that survive the prompt rows changing.
enum Anchor {
    Row(Row),
    Prompt(Source),
}

fn anchor(app: &App) -> Anchor {
    let rows = rows(app.candidates.items.len(), app.setup.prompts.len());
    match rows[app.setup.cursor.min(rows.len() - 1)] {
        Row::Prompt(i) => Anchor::Prompt(app.setup.prompts[i].source.clone()),
        row => Anchor::Row(row),
    }
}

/// Puts the cursor back on `anchor`'s row; a prompt that is gone leaves it on the prompt folder row.
fn restore(app: &mut App, anchor: Anchor) {
    let rows = rows(app.candidates.items.len(), app.setup.prompts.len());
    let row = match anchor {
        Anchor::Row(row) => row,
        Anchor::Prompt(source) => (app.setup.prompts.iter())
            .position(|p| p.source == source)
            .map_or(Row::PromptDir, Row::Prompt),
    };
    if let Some(at) = rows.iter().position(|r| *r == row) {
        app.setup.cursor = at;
    }
}

/// Re-reads the prompt folder, keeping the cursor on its row. Reads files, so call it on a key,
/// never while drawing.
pub(super) fn rescan(app: &mut App) {
    let at = anchor(app);
    scan(app);
    restore(app, at);
}

/// Replaces the prompt folder's entries with what the folder holds now. Files already chosen stay
/// chosen; new ones start unchosen.
fn scan(app: &mut App) {
    let prompts = &mut app.setup.prompts;
    let chosen: HashSet<PathBuf> = prompts
        .iter()
        .filter(|p| p.chosen)
        .filter_map(|p| match &p.source {
            Source::Folder(path) => Some(path.clone()),
            _ => None,
        })
        .collect();
    prompts.retain(|p| !matches!(p.source, Source::Folder(_)));
    app.setup.folder_note = None;
    let Some(dir) = &app.prompts_dir else {
        return;
    };
    let dir = batch::prompts_dir_path(&app.config, dir);
    let mut files = match markdown_files(&dir) {
        Ok(files) => files,
        Err(e) => {
            app.setup.folder_note = Some(e.clone());
            app.message = e;
            return;
        }
    };
    if files.len() > MAX_FOLDER_FILES {
        app.setup.folder_note = Some(format!(
            "showing the first {MAX_FOLDER_FILES} of {} .md files",
            files.len()
        ));
        files.truncate(MAX_FOLDER_FILES);
    }
    let mut taken: HashSet<String> = prompts.iter().map(|p| p.name.clone()).collect();
    for path in files {
        let name = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let too_large = std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_PROMPT_BYTES);
        let problem = if too_large {
            Some(format!("too large (over {} KiB)", MAX_PROMPT_BYTES / 1024))
        } else {
            match load_instructions(Some(&path)) {
                // The row names the file already, so only the reason is kept.
                Err(e) => Some(
                    e.split_once(&format!("{}: ", path.display()))
                        .map_or(e.clone(), |(_, reason)| reason.to_string()),
                ),
                Ok(_) if !taken.insert(name.clone()) => Some("duplicate name".into()),
                Ok(_) => None,
            }
        };
        prompts.push(Prompt {
            chosen: problem.is_none() && chosen.contains(&path),
            source: Source::Folder(path),
            name,
            problem,
        });
    }
}

/// Validates everything, writes the manifest and starts the engine thread.
fn start(app: &mut App) {
    if app.run.active() {
        app.message = "a run is already in progress".into();
        return;
    }
    match args(app).and_then(|a| prepare(&a)) {
        Ok(prepared) => {
            app.message = format!("started {}", prepared.path.display());
            app.run.begin(prepared);
            app.screen = Screen::Run;
        }
        Err(e) => app.message = e,
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let rows = rows(app.candidates.items.len(), app.setup.prompts.len());
    let cursor = app.setup.cursor.min(rows.len() - 1);
    let row = rows[cursor];
    if let Some(buffer) = app.setup.editing.as_mut() {
        match key.code {
            KeyCode::Esc => {
                app.setup.editing = None;
                app.setup.adding = false;
            }
            KeyCode::Enter => {
                let text = std::mem::take(buffer);
                let done = if app.setup.adding {
                    add_prompt(app, &text)
                } else if row == Row::PromptDir {
                    set_folder(app, &text)
                } else {
                    app.setup.set(row, &text)
                };
                match done {
                    Ok(()) => {
                        app.setup.editing = None;
                        app.setup.adding = false;
                    }
                    Err(e) => {
                        app.setup.editing = Some(text);
                        app.message = e;
                    }
                }
            }
            _ => edit(buffer, key),
        }
        return;
    }
    let state = &mut app.setup;
    match key.code {
        KeyCode::Char(' ') => match row {
            Row::Candidate(i) => {
                let name = app.candidates.items[i].name.clone();
                if !state.chosen.remove(&name) {
                    state.chosen.insert(name);
                }
            }
            Row::Target(t) => {
                if !state.targets.remove(&t) {
                    state.targets.insert(t);
                }
            }
            Row::Prompt(i) => {
                let prompt = &mut state.prompts[i];
                match &prompt.problem {
                    Some(problem) => {
                        app.message = format!("{} cannot be chosen: {problem}", prompt.name);
                    }
                    None => prompt.chosen = !prompt.chosen,
                }
            }
            Row::Unscored => state.include_unscored = !state.include_unscored,
            _ => {}
        },
        KeyCode::Char('a') if matches!(row, Row::Prompt(_) | Row::PromptDir) => {
            state.editing = Some(String::new());
            state.adding = true;
        }
        KeyCode::Char('d') => {
            if let Row::Prompt(i) = row {
                match &state.prompts[i].source {
                    Source::Typed(_) => {
                        let at = anchor(app);
                        let gone = app.setup.prompts.remove(i);
                        // A folder file that shared its name may be choosable now.
                        scan(app);
                        restore(app, at);
                        app.message = format!("removed prompt {}", gone.name);
                    }
                    Source::Bundled => app.message = "neutral is built in".into(),
                    Source::Folder(_) => {
                        app.message =
                            "folder prompts follow the folder: clear the prompt folder to drop them"
                                .into();
                    }
                }
            }
        }
        KeyCode::Char('r') => {
            app.message = if app.prompts_dir.is_some() {
                "rescanned the prompt folder".into()
            } else {
                "no prompt folder set".into()
            };
            rescan(app);
        }
        KeyCode::Enter => match row {
            Row::Start => start(app),
            Row::Unscored => state.include_unscored = !state.include_unscored,
            Row::PromptDir => state.editing = Some(app.prompts_dir.clone().unwrap_or_default()),
            _ => state.editing = state.text(row),
        },
        _ => match nav(key) {
            Some(n @ (Nav::Left | Nav::Right)) => {
                if let Some(value) = state.text(row) {
                    let n_now: u64 = value.parse().unwrap_or(1);
                    let next = if n == Nav::Right {
                        n_now + 1
                    } else {
                        n_now.saturating_sub(1)
                    };
                    // Out-of-range steps (below the minimum) are ignored.
                    let _ = state.set(row, &next.to_string());
                }
            }
            Some(n) => state.cursor = step(cursor, rows.len(), n),
            None => {}
        },
    }
}

/// The dim line above Start: what Start would run, from the setup alone (nothing is prepared).
fn plan_text(app: &App) -> String {
    let state = &app.setup;
    let candidates = (app.candidates.items.iter())
        .filter(|c| state.chosen.contains(&c.name))
        .count();
    let prompts = state.prompts.iter().filter(|p| p.chosen).count();
    let targets: Vec<Target> = state.targets.iter().copied().collect();
    let cases = app.dataset.planned(&targets, state.include_unscored);
    let trials = run::trial_count(candidates, prompts, cases, state.repetitions);
    format!(
        "plan: {} × {} × {} × {} = {}, up to {}",
        plural(candidates, "candidate", "candidates"),
        plural(prompts, "prompt", "prompts"),
        plural(cases, "case", "cases"),
        plural(state.repetitions as usize, "repetition", "repetitions"),
        plural(trials, "trial", "trials"),
        plural(
            run::request_count(trials, state.retries),
            "request",
            "requests"
        ),
    )
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let state = &app.setup;
    let rows_all = rows(app.candidates.items.len(), state.prompts.len());
    let cursor = state.cursor.min(rows_all.len() - 1);
    let check = |on: bool| if on { "[x]" } else { "[ ]" };
    // Corners and the `▸ ` marker take four columns.
    let inner = area.width.saturating_sub(4) as usize;
    let mut lines: Vec<Line> = Vec::new();
    // Extra dim lines sit between rows, so the cursor's line is not its row's index.
    let mut focus = 0;
    for (i, row) in rows_all.iter().enumerate() {
        let typing = state.editing.as_ref().filter(|_| i == cursor);
        let field = |label: &str, value: String| {
            let value = typing.map_or(value, |t| format!("{t}▏"));
            Line::from(format!("{label:<22} {value}"))
        };
        if *row == Row::Start {
            lines.push(Line::from(Span::styled(
                cut(&plan_text(app), inner),
                theme::dim(),
            )));
        }
        if i == cursor {
            focus = lines.len();
        }
        lines.push(match *row {
            Row::Candidate(c) => {
                let name = &app.candidates.items[c].name;
                Line::from(format!(
                    "{} candidate {name}",
                    check(state.chosen.contains(name))
                ))
            }
            Row::Target(t) => {
                Line::from(format!("{} target {t}", check(state.targets.contains(&t))))
            }
            Row::Prompt(_) if state.adding && typing.is_some() => {
                field("add prompt file", String::new())
            }
            Row::Prompt(p) => {
                let prompt = &state.prompts[p];
                let mark = check(prompt.chosen);
                let bundled = if prompt.source == Source::Bundled {
                    " (bundled)"
                } else {
                    ""
                };
                match &prompt.problem {
                    None => Line::from(format!("{mark} prompt {}{bundled}", prompt.name)),
                    Some(problem) => {
                        let text =
                            cut(&format!("{mark} prompt {} · {problem}", prompt.name), inner);
                        // The cursor row keeps the focus colour.
                        if i == cursor {
                            Line::from(text)
                        } else {
                            Line::from(Span::styled(text, theme::dim()))
                        }
                    }
                }
            }
            Row::PromptDir if state.adding && typing.is_some() => {
                field("add prompt file", String::new())
            }
            Row::PromptDir => field(
                "prompt folder",
                // 22 for the label and a space.
                cut(
                    app.prompts_dir.as_deref().unwrap_or("(none)"),
                    inner.saturating_sub(23),
                ),
            ),
            Row::Repetitions => field("repetitions", state.repetitions.to_string()),
            Row::Concurrency => field("concurrency", state.concurrency.to_string()),
            Row::Timeout => field("timeout (seconds)", state.timeout_secs.to_string()),
            Row::Retries => field("retries", state.retries.to_string()),
            Row::Unscored => Line::from(format!(
                "{} include unscored cases",
                check(state.include_unscored)
            )),
            Row::Start => Line::from(Span::raw("▶ start run")),
        });
        if *row == Row::PromptDir
            && let Some(note) = &state.folder_note
        {
            lines.push(Line::from(Span::styled(cut(note, inner), theme::dim())));
        }
    }
    let height = area.height.saturating_sub(2) as usize;
    frame.render_widget(
        Paragraph::new(list_lines(lines, focus, height)).block(block(&cut(
            &format!(
                "run setup · cases: {} (chosen on Dataset)",
                app.dataset.choice()
            ),
            // Corners and the title's padding take four columns.
            area.width.saturating_sub(4) as usize,
        ))),
        area,
    );
}
