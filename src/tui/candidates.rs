//! Candidate management: the config's `[[candidate]]` tables, with their manual prices.

use std::os::unix::fs::PermissionsExt;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{App, block, cut, edit, list_lines, nav, step, theme};
use crate::batch::{resolve_links, write_atomic};
use crate::cost::{Price, fmt_usd};
use crate::credentials;
use crate::provider::{Candidate, KeySource, Provider, candidates_toml};

pub const HINTS: &str = "a add · e/Enter edit · d delete · s save · K key · q quit";
const FORM_HINTS: &str =
    "Tab/Down next field · Up previous · Space/Left/Right provider · Enter apply · Esc cancel";

const LABELS: [&str; 5] = [
    "name",
    "provider",
    "model",
    "input $ per million tokens",
    "output $ per million tokens",
];
const PROVIDER: usize = 1;

pub struct Form {
    pub(super) fields: [String; 5],
    focus: usize,
    /// Index being edited; `None` for a new Candidate.
    editing: Option<usize>,
}

impl Form {
    fn new(editing: Option<usize>, from: Option<&Candidate>) -> Self {
        let price = |f: fn(&Price) -> f64| {
            from.and_then(|c| c.price.as_ref())
                .map_or_else(String::new, |p| f(p).to_string())
        };
        Self {
            fields: [
                from.map_or_else(String::new, |c| c.name.clone()),
                from.map_or(Provider::OpenRouter, |c| c.provider)
                    .name()
                    .into(),
                from.map_or_else(String::new, |c| c.model.clone()),
                price(|p| p.input_per_million),
                price(|p| p.output_per_million),
            ],
            focus: 0,
            editing,
        }
    }

    fn cycle_provider(&mut self) {
        self.fields[PROVIDER] = match self.fields[PROVIDER].as_str() {
            "openrouter" => "zen",
            _ => "openrouter",
        }
        .into();
    }

    /// Validates like the config loader: a unique name, a model, and a non-negative price.
    /// Blank price fields mean no price, so the cost stays unknown.
    fn candidate(&self, items: &[Candidate]) -> Result<Candidate, String> {
        let [name, provider, model, input, output] = &self.fields.each_ref().map(|f| f.trim());
        if name.is_empty() || model.is_empty() {
            return Err("name and model are required".into());
        }
        if items
            .iter()
            .enumerate()
            .any(|(i, c)| c.name == *name && Some(i) != self.editing)
        {
            return Err(format!("duplicate candidate name `{name}`"));
        }
        let number = |label: &str, text: &str| {
            if text.is_empty() {
                Ok(0.0)
            } else {
                text.parse::<f64>()
                    .map_err(|_| format!("{label} must be a number, got `{text}`"))
            }
        };
        let price = if [input, output].iter().all(|f| f.is_empty()) {
            None
        } else if input.is_empty() {
            return Err("a price needs input $ per million tokens (blank both for none)".into());
        } else {
            let price = Price {
                input_per_million: number("input", input)?,
                output_per_million: number("output", output)?,
            };
            price.validate()?;
            Some(price)
        };
        Ok(Candidate {
            name: name.to_string(),
            decision_model: self.editing.and_then(|i| items[i].decision_model.clone()),
            provider: if *provider == "zen" {
                Provider::Zen
            } else {
                Provider::OpenRouter
            },
            model: model.to_string(),
            price,
        })
    }
}

pub struct State {
    pub items: Vec<Candidate>,
    pub(super) selected: usize,
    /// Edits not yet written to the config file.
    pub dirty: bool,
    pub form: Option<Form>,
    pub key_prompt: Option<KeyPrompt>,
}

/// The one-field prompt for a provider's API key. The text is only ever drawn as bullets, and the
/// type has no `Debug`, so the key cannot reach a log or a panic message.
pub struct KeyPrompt {
    provider: Provider,
    pub(super) text: String,
}

impl State {
    pub fn new(items: Vec<Candidate>) -> Self {
        Self {
            items,
            selected: 0,
            dirty: false,
            form: None,
            key_prompt: None,
        }
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    if app.candidates.key_prompt.is_some() {
        key_prompt_key(app, key);
        return;
    }
    if app.candidates.form.is_some() {
        form_key(app, key);
        return;
    }
    let state = &mut app.candidates;
    match key.code {
        KeyCode::Char('a') => state.form = Some(Form::new(None, None)),
        KeyCode::Char('e') | KeyCode::Enter => {
            if let Some(c) = state.items.get(state.selected) {
                state.form = Some(Form::new(Some(state.selected), Some(c)));
            }
        }
        KeyCode::Char('d') if !state.items.is_empty() => {
            let gone = state.items.remove(state.selected);
            app.setup.chosen.remove(&gone.name);
            state.selected = state.selected.min(state.items.len().saturating_sub(1));
            state.dirty = true;
            app.message = format!("deleted `{}` (unsaved; s writes the config)", gone.name);
        }
        KeyCode::Char('s') => {
            app.message = match save(app) {
                Ok(()) => format!("saved {}", app.config.display()),
                Err(e) => e,
            };
        }
        KeyCode::Char('K') => match state.items.get(state.selected) {
            Some(c) => {
                state.key_prompt = Some(KeyPrompt {
                    provider: c.provider,
                    text: String::new(),
                });
            }
            None => app.message = "no candidate: press a to add one".into(),
        },
        _ => {
            if let Some(nav) = nav(key) {
                state.selected = step(state.selected, state.items.len(), nav);
            }
        }
    }
}

fn save(app: &mut App) -> Result<(), String> {
    // A symlinked config is written through, so the link survives the rename.
    let config = resolve_links(&app.config)?;
    let existing = match std::fs::read_to_string(&config) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("cannot read {}: {e}", config.display())),
    };
    let text = candidates_toml(existing.as_deref(), &app.candidates.items)?;
    if let Some(dir) = config.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    // An existing config keeps its mode; a new one is private.
    let permissions = std::fs::metadata(&config)
        .map(|m| m.permissions())
        .unwrap_or_else(|_| std::fs::Permissions::from_mode(0o600));
    write_atomic(&config, &text, permissions)
        .map_err(|e| format!("cannot write {}: {e}", app.config.display()))?;
    app.candidates.dirty = false;
    Ok(())
}

/// Typing goes into the prompt; Enter saves a non-empty key at once (it is not a config edit, so
/// it ignores `dirty` and `s`), Esc cancels.
fn key_prompt_key(app: &mut App, key: KeyEvent) {
    let Some(prompt) = app.candidates.key_prompt.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Esc => app.candidates.key_prompt = None,
        KeyCode::Enter => {
            let (provider, secret) = (prompt.provider, prompt.text.trim().to_string());
            app.candidates.key_prompt = None;
            app.message = if secret.is_empty() {
                "empty: nothing saved".into()
            } else {
                store_key(
                    &app.config,
                    provider,
                    &secret,
                    std::env::var(provider.key_env()).ok(),
                )
            };
        }
        _ => edit(&mut prompt.text, key),
    }
}

/// Saves the key and returns the message to show: the path, never the key. `env` is the
/// provider's environment value, which outranks the saved key when it is non-empty.
pub(super) fn store_key(
    config: &std::path::Path,
    provider: Provider,
    secret: &str,
    env: Option<String>,
) -> String {
    let path = credentials::path_for(config);
    if let Err(e) = credentials::save_key(&path, provider, secret) {
        return e;
    }
    let mut message = format!("saved {} key to {}", provider.name(), path.display());
    if env.is_some_and(|value| !value.is_empty()) {
        message.push_str(&format!(
            " ({} is set and overrides it)",
            provider.key_env()
        ));
    }
    message
}

fn form_key(app: &mut App, key: KeyEvent) {
    let state = &mut app.candidates;
    let Some(form) = state.form.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Esc => state.form = None,
        KeyCode::Tab | KeyCode::Down => form.focus = (form.focus + 1) % LABELS.len(),
        KeyCode::BackTab | KeyCode::Up => {
            form.focus = (form.focus + LABELS.len() - 1) % LABELS.len()
        }
        KeyCode::Enter => match form.candidate(&state.items) {
            Ok(candidate) => {
                match form.editing {
                    Some(i) => {
                        let old = std::mem::replace(&mut state.items[i], candidate.clone());
                        if app.setup.chosen.remove(&old.name) {
                            app.setup.chosen.insert(candidate.name);
                        }
                    }
                    None => {
                        state.items.push(candidate);
                        state.selected = state.items.len() - 1;
                    }
                }
                state.dirty = true;
                state.form = None;
                app.message = "applied (unsaved; s writes the config)".into();
            }
            Err(e) => app.message = e,
        },
        _ if form.focus == PROVIDER => {
            if matches!(
                key.code,
                KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right
            ) {
                form.cycle_provider();
            }
        }
        _ => edit(&mut form.fields[form.focus], key),
    }
}

/// Prices read as input / output per million tokens.
fn price_text(price: Option<&Price>) -> Span<'static> {
    match price {
        None => theme::status(theme::Status::Unknown, "cost unknown"),
        Some(p) => Span::styled(
            format!(
                "{} / {}",
                fmt_usd(p.input_per_million),
                fmt_usd(p.output_per_million)
            ),
            theme::base(),
        ),
    }
}

/// Where the provider's key comes from. Zen's is optional, so no key is not a warning. `wide`
/// picks the long wording with the `key: ` prefix; narrow rows drop it to leave room for the model.
pub(super) fn key_text(provider: Provider, source: KeySource, wide: bool) -> Span<'static> {
    let prefix = if wide { "key: " } else { "" };
    let warn = |word: &str| theme::status(theme::Status::Warn, &format!("{prefix}{word}"));
    match (source, provider) {
        (KeySource::Invalid, _) => warn("file error"),
        (KeySource::Env, _) => Span::styled(format!("{prefix}env"), theme::base()),
        (KeySource::Saved, _) => Span::styled(format!("{prefix}saved"), theme::base()),
        (KeySource::Missing, Provider::Zen) => Span::styled(
            format!("{prefix}none{}", if wide { " (free ok)" } else { "" }),
            theme::dim(),
        ),
        (KeySource::Missing, Provider::OpenRouter) => warn("missing"),
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let state = &app.candidates;
    let title = format!(
        "candidates · {}{}",
        app.config.display(),
        if state.dirty {
            " · unsaved changes"
        } else {
            ""
        }
    );
    if let Some(prompt) = &state.key_prompt {
        let lines = vec![
            Line::from(Span::styled("Enter save · Esc cancel", theme::dim())),
            Line::from(""),
            Line::from(format!(
                "{} API key  {}▏",
                prompt.provider.name(),
                "•".repeat(prompt.text.chars().count())
            ))
            .style(theme::focus()),
            Line::from(""),
            Line::from(Span::styled(
                format!(
                    "saved to {} (mode 0600); the key is never shown",
                    credentials::path_for(&app.config).display()
                ),
                theme::dim(),
            )),
        ];
        frame.render_widget(Paragraph::new(lines).block(block("api key")), area);
        return;
    }
    if let Some(form) = &state.form {
        let mut lines = vec![
            Line::from(Span::styled(FORM_HINTS, theme::dim())),
            Line::from(""),
        ];
        for (i, (label, value)) in LABELS.iter().zip(&form.fields).enumerate() {
            let focused = i == form.focus;
            let cursor = if focused && i != PROVIDER { "▏" } else { "" };
            let line = Line::from(format!("{label:<28} {value}{cursor}"));
            lines.push(if focused {
                line.style(theme::focus())
            } else {
                line
            });
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "blank prices mean no price: the cost stays unknown",
            theme::dim(),
        )));
        let name = if form.editing.is_some() {
            "edit candidate"
        } else {
            "new candidate"
        };
        frame.render_widget(Paragraph::new(lines).block(block(name)), area);
        return;
    }
    let height = area.height.saturating_sub(2) as usize;
    // Name, provider, price and key status keep their widths (the longest price text is 14, the
    // key status 19 in the long wording, 12 in the short); the model takes what is left. Below 80
    // columns the provider column is dropped (the form still shows it) so the model stays readable.
    let wide = area.width >= 80;
    let (provider_width, key_width) = if wide { (11, 19) } else { (0, 12) };
    let model_width = (area.width as usize)
        .saturating_sub(2 + 2 + 12 + 1 + provider_width + 1 + 14 + 2 + key_width)
        .max(1);
    let credentials = credentials::path_for(&app.config);
    let rows: Vec<Line> = if state.items.is_empty() {
        vec![Line::from(Span::styled(
            "no candidates: press a to add one",
            theme::dim(),
        ))]
    } else {
        state
            .items
            .iter()
            .map(|c| {
                Line::from(vec![
                    Span::raw(format!(
                        "{:<12} {}{:<model_width$} ",
                        cut(&c.name, 12),
                        if wide {
                            format!("{:<10} ", c.provider.name())
                        } else {
                            String::new()
                        },
                        cut(&c.model, model_width)
                    )),
                    price_text(c.price.as_ref()),
                    Span::raw("  "),
                    key_text(c.provider, c.provider.key_source(&credentials), wide),
                ])
            })
            .collect()
    };
    frame.render_widget(
        Paragraph::new(list_lines(rows, state.selected, height)).block(block(&title)),
        area,
    );
}
