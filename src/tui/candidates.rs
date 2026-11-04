//! Candidate management: the config's `[[candidate]]` tables, with their manual prices.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{App, block, cut, edit, list_lines, nav, step, theme};
use crate::cost::{Price, fmt_usd};
use crate::provider::{Candidate, Provider, candidates_toml};

pub const HINTS: &str = "a add · e/Enter edit · d delete · s save · j/k move · q quit";
const FORM_HINTS: &str =
    "Tab/Down next field · Up previous · Space/Left/Right provider · Enter apply · Esc cancel";

const LABELS: [&str; 6] = [
    "name",
    "provider",
    "model",
    "input $ per million tokens",
    "output $ per million tokens",
    "$ per request",
];
const PROVIDER: usize = 1;

pub struct Form {
    pub(super) fields: [String; 6],
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
                price(|p| p.per_request),
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
        let [name, provider, model, input, output, per_request] =
            &self.fields.each_ref().map(|f| f.trim());
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
        let price = if [input, output, per_request].iter().all(|f| f.is_empty()) {
            None
        } else if input.is_empty() {
            return Err(
                "a price needs input $ per million tokens (blank all three for none)".into(),
            );
        } else {
            let price = Price {
                input_per_million: number("input", input)?,
                output_per_million: number("output", output)?,
                per_request: number("per request", per_request)?,
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
    selected: usize,
    /// Edits not yet written to the config file.
    pub dirty: bool,
    pub form: Option<Form>,
}

impl State {
    pub fn new(items: Vec<Candidate>) -> Self {
        Self {
            items,
            selected: 0,
            dirty: false,
            form: None,
        }
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
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
                Ok(()) => format!(
                    "saved {} (comments in the file are not kept)",
                    app.config.display()
                ),
                Err(e) => e,
            };
        }
        _ => {
            if let Some(nav) = nav(key) {
                state.selected = step(state.selected, state.items.len(), nav);
            }
        }
    }
}

fn save(app: &mut App) -> Result<(), String> {
    let text = candidates_toml(&app.candidates.items)?;
    if let Some(dir) = app.config.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    // Write beside the config, then rename: a failed write never leaves a half-written config.
    let mut temp = app.config.clone().into_os_string();
    temp.push(".tmp");
    // The config may hold API keys: new files are private, an existing config keeps its mode.
    let permissions = std::fs::metadata(&app.config).map(|m| m.permissions());
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temp)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .and_then(|()| permissions.map_or(Ok(()), |p| std::fs::set_permissions(&temp, p)))
        .and_then(|()| std::fs::rename(&temp, &app.config))
        .map_err(|e| format!("cannot write {}: {e}", app.config.display()))?;
    app.candidates.dirty = false;
    Ok(())
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

/// Prices read as input / output per million tokens / per request.
fn price_text(price: Option<&Price>) -> Span<'static> {
    match price {
        None => theme::status(theme::Status::Unknown, "cost unknown"),
        Some(p) => Span::styled(
            format!(
                "{} / {} / {}",
                fmt_usd(p.input_per_million),
                fmt_usd(p.output_per_million),
                fmt_usd(p.per_request)
            ),
            theme::base(),
        ),
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
    // Name, provider and price keep their columns; the model takes what is left.
    let model_width = (area.width as usize)
        .saturating_sub(2 + 2 + 12 + 1 + 10 + 1 + 1 + 16)
        .max(4);
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
                        "{:<12} {:<10} {:<model_width$} ",
                        cut(&c.name, 12),
                        c.provider.name(),
                        cut(&c.model, model_width)
                    )),
                    price_text(c.price.as_ref()),
                ])
            })
            .collect()
    };
    frame.render_widget(
        Paragraph::new(list_lines(rows, state.selected, height)).block(block(&title)),
        area,
    );
}
