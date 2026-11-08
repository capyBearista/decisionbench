//! Provider routes and Candidates: how one Decisions request is built, sent and read (SPEC §4a).

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, TomlError};

use crate::cost::Price;
use crate::credentials::{self, Credentials};
use crate::dataset::Case;
use crate::grade::Target;
use crate::instructions::JudgeInstructions;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    OpenRouter,
    Zen,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Self::OpenRouter => "openrouter",
            Self::Zen => "zen",
        }
    }

    pub fn url(self) -> &'static str {
        match self {
            Self::OpenRouter => "https://openrouter.ai/api/alpha/decisions",
            Self::Zen => "https://opencode.ai/inference/systemone/v1/systemone",
        }
    }

    pub fn key_env(self) -> &'static str {
        match self {
            Self::OpenRouter => "OPENROUTER_API_KEY",
            Self::Zen => "OPENCODE_API_KEY",
        }
    }

    /// The API key: a non-empty environment variable, else the one saved in the `credentials`
    /// file. Free Zen models need none.
    pub fn key(self, credentials: &Path) -> Result<Option<String>, String> {
        self.key_with(std::env::var(self.key_env()).ok(), credentials)
    }

    /// Where `key` would come from. A credentials file that cannot be read or parsed is `Invalid`
    /// whatever the environment holds, because `key` fails on it too.
    pub fn key_source(self, credentials: &Path) -> KeySource {
        let Ok(saved) = credentials::load(credentials) else {
            return KeySource::Invalid;
        };
        let env = std::env::var(self.key_env()).ok();
        self.pick(env, &saved)
            .map_or(KeySource::Missing, |(source, _)| source)
    }

    fn key_with(self, env: Option<String>, credentials: &Path) -> Result<Option<String>, String> {
        let saved = credentials::load(credentials)?;
        match self.pick(env, &saved) {
            Some((_, key)) => Ok(Some(key)),
            None if self == Self::OpenRouter => Err(format!(
                "{} is not set (and no {} key in {})",
                self.key_env(),
                self.name(),
                credentials.display()
            )),
            None => Ok(None),
        }
    }

    /// The environment value wins over the saved one; an empty value counts as unset.
    fn pick(self, env: Option<String>, saved: &Credentials) -> Option<(KeySource, String)> {
        let saved = saved.get(self).map(str::to_string);
        env.filter(|key| !key.is_empty())
            .map(|key| (KeySource::Env, key))
            .or_else(|| {
                saved
                    .filter(|key| !key.is_empty())
                    .map(|key| (KeySource::Saved, key))
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Env,
    Saved,
    Missing,
    /// The credentials file exists but cannot be read or parsed.
    Invalid,
}

pub fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

// Test hook only: lets the CLI tests reach a local stub. It also redirects the API key, so never set it outside tests.
pub fn request_url(provider: Provider) -> String {
    env_nonempty("DECISIONBENCH_TEST_BASE_URL").unwrap_or_else(|| provider.url().to_string())
}

/// A Decision Model reached through one Provider Route; configured as a `[[candidate]]` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    /// The user's label for this Candidate; unique in the config.
    pub name: String,
    /// The underlying model, when known.
    pub decision_model: Option<String>,
    pub provider: Provider,
    /// The model id sent on the wire.
    pub model: String,
    /// Manual price (`[candidate.price]`); without one the cost is unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<Price>,
}

/// The parsed config file.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    #[serde(default, rename = "candidate")]
    pub candidates: Vec<Candidate>,
    /// A folder of prompt `.md` files; relative to the config file's folder, `~` not expanded.
    pub prompts_dir: Option<String>,
}

/// Parses the TOML config; Candidate names must be unique.
pub fn parse_config(text: &str) -> Result<ConfigFile, String> {
    let config: ConfigFile = toml::from_str(text).map_err(|e| e.to_string())?;
    for (i, candidate) in config.candidates.iter().enumerate() {
        if config.candidates[..i]
            .iter()
            .any(|c| c.name == candidate.name)
        {
            return Err(format!("duplicate candidate name `{}`", candidate.name));
        }
        if let Some(price) = &candidate.price {
            price
                .validate()
                .map_err(|e| format!("candidate `{}` price: {e}", candidate.name))?;
        }
    }
    Ok(config)
}

/// The config file text for `candidates`. With the `existing` text, its `[[candidate]]` tables are
/// updated in place (matched by name) so comments and other content survive. The result parses
/// back through `parse_config` into exactly `candidates`, which also rejects what the loader
/// would (duplicate names, bad prices).
pub fn candidates_toml(existing: Option<&str>, candidates: &[Candidate]) -> Result<String, String> {
    #[derive(Serialize)]
    struct Out<'a> {
        candidate: &'a [Candidate],
    }
    let fresh = toml::to_string(&Out {
        candidate: candidates,
    })
    .map_err(|e| e.to_string())?;
    let text = match existing {
        None => fresh,
        Some(existing) => {
            let mut doc: DocumentMut = existing.parse().map_err(|e: TomlError| e.to_string())?;
            let fresh: DocumentMut = fresh.parse().map_err(|e: TomlError| e.to_string())?;
            match (doc.get_mut("candidate"), &fresh["candidate"]) {
                (Some(Item::ArrayOfTables(old)), Item::ArrayOfTables(new)) => {
                    let name = |t: &Table| t.get("name").and_then(Item::as_str).map(str::to_owned);
                    let first = old.iter().next();
                    let header = first
                        .and_then(|t| Some((name(t), t.decor().prefix()?.as_str()?.to_owned())));
                    // In `candidates` order: a matched old table keeps its comments.
                    let mut rebuilt = ArrayOfTables::new();
                    for table in new.iter() {
                        let mut table = match old.iter().find(|o| name(o) == name(table)) {
                            Some(o) => {
                                let mut o = o.clone();
                                sync(&mut o, table);
                                o
                            }
                            None => table.clone(),
                        };
                        // A new first table inherits the file header comment above the old first.
                        if rebuilt.is_empty()
                            && let Some((first, header)) = &header
                            && *first != name(&table)
                        {
                            let own = table
                                .decor()
                                .prefix()
                                .and_then(|p| p.as_str())
                                .unwrap_or("");
                            let prefix = format!("{header}{own}");
                            table.decor_mut().set_prefix(prefix);
                        }
                        rebuilt.push(table);
                    }
                    *old = rebuilt;
                }
                (_, item) => doc["candidate"] = item.clone(),
            }
            doc.to_string()
        }
    };
    if parse_config(&text)?.candidates != candidates {
        return Err("the updated config does not match the candidates".into());
    }
    Ok(text)
}

/// Makes `old` hold what `new` holds, changing only what differs so comments stay.
fn sync(old: &mut Table, new: &Table) {
    let gone: Vec<String> = old
        .iter()
        .map(|(k, _)| k)
        .filter(|k| !new.contains_key(k))
        .map(str::to_owned)
        .collect();
    for key in gone {
        old.remove(&key);
    }
    let bare = |v: &toml_edit::Value| v.clone().decorated("", "").to_string();
    for (key, item) in new {
        match (old.get_mut(key), item) {
            (Some(Item::Table(o)), Item::Table(n)) => sync(o, n),
            (Some(Item::Value(o)), Item::Value(n)) => {
                if bare(o) != bare(n) {
                    let decor = o.decor().clone();
                    *o = n.clone();
                    *o.decor_mut() = decor;
                }
            }
            (Some(slot), _) => *slot = item.clone(),
            (None, _) => {
                old.insert(key, item.clone());
            }
        }
    }
}

/// The Decisions request body: one `effort_<target>` question per selected Target Model, in
/// `Target::ALL` order. `current_effort` and `current_model` are omitted, not nulled (ADR-0002).
/// Every selected target must have a section in `instructions`; `run` checks that first.
pub fn request_body(
    model: &str,
    instructions: &JudgeInstructions,
    case: &Case,
    targets: &[Target],
) -> Value {
    let mut state = Map::new();
    state.insert("task".into(), instructions.task.clone().into());
    if let Some(context) = &case.context {
        state.insert("recent_conversation".into(), context.clone().into());
    }
    state.insert("next_message".into(), case.message.clone().into());
    let questions: Map<String, Value> = Target::ALL
        .into_iter()
        .filter(|t| targets.contains(t))
        .filter_map(|t| {
            let question = instructions.question(t)?;
            let criteria: Map<String, Value> = question
                .criteria
                .iter()
                .map(|(effort, text)| (effort.name().to_string(), text.clone().into()))
                .collect();
            let body = json!({
                "type": "choice",
                "instructions": question.instructions,
                "criteria": criteria,
            });
            Some((format!("effort_{t}"), body))
        })
        .collect();
    json!({"model": model, "state": state, "questions": questions})
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    /// Any 2xx reply. The effort may still be invalid; grading decides that.
    Answered,
    /// A non-2xx reply.
    ProviderError,
    /// Connect, timeout or read failure.
    TransportError,
}

impl Disposition {
    pub fn name(self) -> &'static str {
        match self {
            Self::Answered => "answered",
            Self::ProviderError => "provider_error",
            Self::TransportError => "transport_error",
        }
    }
}

/// One physical request and what came back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub started_unix_ms: u64,
    pub latency_ms: u64,
    pub disposition: Disposition,
    pub http_status: Option<u16>,
    /// The reply body exactly as received.
    pub raw_response: Option<String>,
    /// The transport failure text.
    pub error: Option<String>,
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// An HTTP agent whose requests give up after `timeout`; build once and share across workers.
pub fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .build()
        .into()
}

/// Whether a failed Attempt is worth another try: a transport error or a transient HTTP status.
/// Any answered reply is final, even one whose effort turns out invalid.
pub fn retryable(attempt: &Attempt) -> bool {
    match attempt.disposition {
        Disposition::Answered => false,
        Disposition::TransportError => true,
        Disposition::ProviderError => matches!(
            attempt.http_status,
            Some(408 | 429 | 500 | 502 | 503 | 504 | 524 | 529)
        ),
    }
}

/// Sends one POST. The key goes only in the Authorization header; any echo of it is redacted.
pub fn send(agent: &ureq::Agent, url: &str, key: Option<&str>, body: &Value) -> Attempt {
    let started_unix_ms = now_unix_ms();
    let clock = Instant::now();
    let mut request = agent.post(url).header("content-type", "application/json");
    if let Some(key) = key {
        request = request.header("authorization", format!("Bearer {key}"));
    }
    // Keep the status when the headers arrived but the body could not be read.
    let mut status = None;
    let result = request.send(body.to_string()).and_then(|mut response| {
        status = Some(response.status().as_u16());
        // shortcut: invalid UTF-8 bytes become U+FFFD; store raw bytes (e.g. base64) if a provider ever needs them
        response
            .body_mut()
            .read_to_vec()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    });
    let latency_ms = clock.elapsed().as_millis() as u64;
    let (disposition, http_status, raw_response, error) = match (result, status) {
        (Ok(body), Some(status)) if (200..300).contains(&status) => {
            (Disposition::Answered, Some(status), Some(body), None)
        }
        (Ok(body), status) => (Disposition::ProviderError, status, Some(body), None),
        (Err(e), status) => (
            Disposition::TransportError,
            status,
            None,
            Some(e.to_string()),
        ),
    };
    // A provider may echo the key back; it must not reach the evidence or the terminal.
    let redact = |text: String| match key {
        Some(key) => text.replace(key, "[redacted]"),
        None => text,
    };
    Attempt {
        started_unix_ms,
        latency_ms,
        disposition,
        http_status,
        raw_response: raw_response.map(redact),
        error: error.map(redact),
    }
}

/// What an answered reply carried. Values stay raw JSON; `effort` is graded later.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prediction {
    /// `answers.effort_<target>.choice` for every target, or `null` when absent or the body is
    /// not JSON.
    pub efforts: BTreeMap<Target, Value>,
    /// Present `answers.effort_<target>.confidence` values.
    pub confidences: BTreeMap<Target, Value>,
    /// Present `answers.effort_<target>.probabilities` values.
    pub probabilities: BTreeMap<Target, Value>,
    pub usage: Option<Value>,
    pub response_model: Option<Value>,
    pub response_provider: Option<Value>,
}

/// Reads a reply body. Only the `answers.effort_<target>` paths are known; Zen's success shape is not yet observed.
pub fn normalize(raw: &str) -> Prediction {
    let body: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let answer = |target: Target, field: &str| {
        body.pointer(&format!("/answers/effort_{target}/{field}"))
            .cloned()
    };
    let each = |field: &str| -> BTreeMap<Target, Value> {
        Target::ALL
            .into_iter()
            .filter_map(|t| Some((t, answer(t, field)?)))
            .collect()
    };
    let top = |field: &str| body.get(field).cloned();
    Prediction {
        efforts: Target::ALL
            .into_iter()
            .map(|t| (t, answer(t, "choice").unwrap_or(Value::Null)))
            .collect(),
        confidences: each("confidence"),
        probabilities: each("probabilities"),
        usage: top("usage"),
        response_model: top("model"),
        response_provider: top("provider"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::{Effort, parse_cases};
    use crate::instructions::parse_instructions;

    fn parse_candidates(text: &str) -> Result<Vec<Candidate>, String> {
        parse_config(text).map(|config| config.candidates)
    }

    const OPENROUTER_200: &str = r#"{"model":"typesafe/jev-1.13-20260917","answers":{"effort_sonnet":{"type":"choice","choice":"low","probabilities":{"low":0.83,"medium":0.17,"high":0,"xhigh":0,"max":0},"confidence":0.79},"effort_opus":{"type":"choice","choice":"high"}},"usage":{"input_tokens":488,"output_tokens":54,"cost":2.0496e-05},"id":"gen-dec-1793762009-oxb0hWCuUBlv6AKzC6ON","provider":"TypeSafe"}"#;

    /// A credentials file with the given text in a fresh temp directory.
    fn credentials_file(text: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "decisionbench-key-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("credentials.toml");
        std::fs::write(&path, text).unwrap();
        path
    }

    fn some(text: &str) -> Option<String> {
        Some(text.to_string())
    }

    #[test]
    fn a_nonempty_environment_key_wins_over_a_saved_one() {
        let path = credentials_file("openrouter = \"saved\"\n");
        let key = Provider::OpenRouter.key_with(some("from-env"), &path);
        assert_eq!(key, Ok(some("from-env")));
        let saved = crate::credentials::load(&path).unwrap();
        let (source, _) = Provider::OpenRouter.pick(some("from-env"), &saved).unwrap();
        assert_eq!(source, KeySource::Env);
    }

    #[test]
    fn an_empty_environment_key_falls_through_to_the_saved_one() {
        let path = credentials_file("zen = \"saved\"\n");
        assert_eq!(Provider::Zen.key_with(some(""), &path), Ok(some("saved")));
        let saved = crate::credentials::load(&path).unwrap();
        let (source, _) = Provider::Zen.pick(some(""), &saved).unwrap();
        assert_eq!(source, KeySource::Saved);
        // An empty saved key counts as unset too.
        let empty = credentials_file("zen = \"\"\n");
        assert_eq!(Provider::Zen.key_with(None, &empty), Ok(None));
    }

    #[test]
    fn the_other_providers_saved_key_is_not_used() {
        let path = credentials_file("zen = \"zen-key\"\n");
        assert!(Provider::OpenRouter.key_with(None, &path).is_err());
        let path = credentials_file("openrouter = \"or-key\"\n");
        assert_eq!(Provider::Zen.key_with(None, &path), Ok(None));
    }

    #[test]
    fn no_key_anywhere_is_an_error_for_openrouter_and_fine_for_zen() {
        let path = std::env::temp_dir().join("decisionbench-key-absent/credentials.toml");
        assert_eq!(
            Provider::OpenRouter.key_with(None, &path),
            Err(format!(
                "OPENROUTER_API_KEY is not set (and no openrouter key in {})",
                path.display()
            ))
        );
        assert_eq!(Provider::Zen.key_with(None, &path), Ok(None));
    }

    #[test]
    fn a_broken_credentials_file_is_an_error_even_when_the_environment_has_a_key() {
        let path = credentials_file("typo = \"x\"\n");
        let error = Provider::Zen.key_with(some("from-env"), &path).unwrap_err();
        assert!(error.contains(&path.display().to_string()), "{error}");
    }

    #[test]
    fn candidates_round_trip_through_the_config_text() {
        let text = "[[candidate]]\nname = \"a\"\ndecision_model = \"jev\"\nprovider = \"zen\"\nmodel = \"m\"\n\n[candidate.price]\ninput_per_million = 1.5\n\n[[candidate]]\nname = \"b\"\nprovider = \"openrouter\"\nmodel = \"n\"\n";
        let candidates = parse_candidates(text).unwrap();
        let saved = candidates_toml(None, &candidates).unwrap();
        assert_eq!(parse_candidates(&saved).unwrap(), candidates);
        assert!(candidates_toml(None, &[candidates[0].clone(), candidates[0].clone()]).is_err());
        assert_eq!(
            parse_candidates(&candidates_toml(None, &[]).unwrap()).unwrap(),
            []
        );
    }

    #[test]
    fn a_config_may_name_a_prompts_folder() {
        let candidate = "[[candidate]]\nname = \"a\"\nprovider = \"zen\"\nmodel = \"m\"\n";
        let without = parse_config(candidate).unwrap();
        assert_eq!(without.prompts_dir, None);
        let with = parse_config(&format!("prompts_dir = \"prompts\"\n{candidate}")).unwrap();
        assert_eq!(with.prompts_dir.as_deref(), Some("prompts"));
        assert_eq!(with.candidates, without.candidates);
        // Only the folder: a new config can hold it before any Candidate exists.
        assert_eq!(
            parse_config("prompts_dir = \"p\"\n").unwrap().candidates,
            []
        );
        // Any other top-level key is still an error, and the folder must be a string.
        assert!(parse_config(&format!("prompt_dir = \"p\"\n{candidate}")).is_err());
        assert!(parse_config(&format!("prompts_dir = 5\n{candidate}")).is_err());
    }

    #[test]
    fn updating_candidates_keeps_the_prompts_folder_and_its_comments() {
        let existing = "# where my prompts live\nprompts_dir = \"mine\" # relative\n\n[[candidate]]\nname = \"a\"\nprovider = \"zen\"\nmodel = \"m\"\n";
        let mut candidates = parse_candidates(existing).unwrap();
        candidates[0].model = "m2".into();
        let saved = candidates_toml(Some(existing), &candidates).unwrap();
        for kept in [
            "# where my prompts live\nprompts_dir = \"mine\" # relative\n",
            "model = \"m2\"",
        ] {
            assert!(saved.contains(kept), "{kept} missing from:\n{saved}");
        }
        let config = parse_config(&saved).unwrap();
        assert_eq!(config.prompts_dir.as_deref(), Some("mine"));
        assert_eq!(config.candidates, candidates);
        // A fresh config never gets a folder from here.
        assert!(
            !candidates_toml(None, &candidates)
                .unwrap()
                .contains("prompts_dir")
        );
        // Emptying the list keeps the folder too.
        let emptied = candidates_toml(Some(&saved), &[]).unwrap();
        assert_eq!(
            parse_config(&emptied).unwrap().prompts_dir.as_deref(),
            Some("mine")
        );
    }

    #[test]
    fn updating_a_config_keeps_its_comments() {
        let existing = "# my benchmark config\n\n[[candidate]]\nname = \"a\"\nprovider = \"zen\"\nmodel = \"m\" # pinned\n\n# prices checked 2026-10\n[candidate.price]\ninput_per_million = 1.5\n\n# the second one\n[[candidate]]\nname = \"b\"\nprovider = \"openrouter\"\nmodel = \"n\"\n";
        let mut candidates = parse_candidates(existing).unwrap();
        candidates[0].price.as_mut().unwrap().input_per_million = 2.5;
        candidates.push(Candidate {
            name: "c".into(),
            decision_model: None,
            provider: Provider::Zen,
            model: "o".into(),
            price: None,
        });
        let saved = candidates_toml(Some(existing), &candidates).unwrap();
        for kept in [
            "# my benchmark config",
            "model = \"m\" # pinned",
            "# prices checked 2026-10",
            "# the second one",
            "input_per_million = 2.5",
        ] {
            assert!(saved.contains(kept), "{kept} missing from:\n{saved}");
        }
        assert_eq!(parse_candidates(&saved).unwrap(), candidates);
        // Removing a Candidate (and its comment) and emptying the list both stay loadable.
        let rest = [candidates[1].clone()];
        let saved = candidates_toml(Some(&saved), &rest).unwrap();
        assert!(!saved.contains("pinned") && saved.contains("# the second one"));
        assert_eq!(parse_candidates(&saved).unwrap(), rest);
        assert_eq!(
            parse_candidates(&candidates_toml(Some(&saved), &[]).unwrap()).unwrap(),
            []
        );
    }

    #[test]
    fn renaming_or_deleting_the_first_candidate_keeps_order_and_header() {
        let existing = "# header\n[[candidate]]\nname = \"a\"\nprovider = \"zen\"\nmodel = \"m\"\n\n# second\n[[candidate]]\nname = \"b\"\nprovider = \"zen\"\nmodel = \"n\"\n";
        let mut candidates = parse_candidates(existing).unwrap();
        candidates[0].name = "a2".into();
        let saved = candidates_toml(Some(existing), &candidates).unwrap();
        assert_eq!(parse_candidates(&saved).unwrap(), candidates);
        assert!(
            saved.starts_with("# header") && saved.contains("# second"),
            "{saved}"
        );
        let rest = [candidates[1].clone()];
        let saved = candidates_toml(Some(existing), &rest).unwrap();
        assert!(saved.starts_with("# header"), "{saved}");
        assert_eq!(parse_candidates(&saved).unwrap(), rest);
    }

    #[test]
    fn normalizes_an_openrouter_reply() {
        let p = normalize(OPENROUTER_200);
        assert_eq!(p.efforts[&Target::Sonnet], json!("low"));
        assert_eq!(p.efforts[&Target::Opus], json!("high"));
        // Targets that were not asked read as null.
        assert_eq!(p.efforts[&Target::Haiku], Value::Null);
        assert_eq!(p.efforts[&Target::Fable], Value::Null);
        assert_eq!(
            p.confidences,
            BTreeMap::from([(Target::Sonnet, json!(0.79))])
        );
        assert_eq!(
            p.probabilities,
            BTreeMap::from([(
                Target::Sonnet,
                json!({"low":0.83,"medium":0.17,"high":0,"xhigh":0,"max":0})
            )])
        );
        assert_eq!(
            p.usage,
            Some(json!({"input_tokens":488,"output_tokens":54,"cost":2.0496e-05}))
        );
        assert_eq!(p.response_model, Some(json!("typesafe/jev-1.13-20260917")));
        assert_eq!(p.response_provider, Some(json!("TypeSafe")));
    }

    #[test]
    fn missing_answers_or_non_json_leave_every_effort_null() {
        for raw in [
            r#"{"model":"m","usage":{"input_tokens":1}}"#,
            "not json",
            "",
        ] {
            let efforts = normalize(raw).efforts;
            assert_eq!(efforts.len(), 4, "{raw:?}");
            assert!(efforts.values().all(Value::is_null), "{raw:?}");
        }
        assert!(normalize(r#"{"model":"m"}"#).confidences.is_empty());
        assert_eq!(normalize("not json").usage, None);
    }

    fn instructions() -> JudgeInstructions {
        let criteria = |n: &str| {
            Effort::ALL
                .iter()
                .map(|e| format!("- {}: {n} {}\n", e.name(), e.name()))
                .collect::<String>()
        };
        let text: String = Target::ALL
            .iter()
            .map(|t| format!("\n## {t}\nAsk {t}?\n\n{}", criteria(&t.to_string())))
            .collect();
        parse_instructions("t", &format!("The task.\n{text}")).unwrap()
    }

    fn case(context: Option<&str>) -> Case {
        let context = context.map_or(String::new(), |c| format!(r#","context":"{c}""#));
        parse_cases(
            format!(r#"[{{"id":"c1","message":"do it"{context},"ok":{{"opus":"low"}}}}]"#)
                .as_bytes(),
        )
        .unwrap()
        .remove(0)
    }

    #[test]
    fn request_has_one_question_per_selected_target_in_scale_order() {
        // Given out of order; the request still lists them in Target::ALL order.
        let body = request_body(
            "m",
            &instructions(),
            &case(Some("earlier")),
            &[Target::Fable, Target::Sonnet],
        );
        assert_eq!(body["model"], "m");
        assert_eq!(
            body["state"],
            json!({"task":"The task.","recent_conversation":"earlier","next_message":"do it"})
        );
        let questions = body["questions"].as_object().unwrap();
        let names: Vec<&String> = questions.keys().collect();
        assert_eq!(names, ["effort_sonnet", "effort_fable"]);
        assert_eq!(
            questions["effort_fable"],
            json!({
                "type": "choice",
                "instructions": "Ask fable?",
                "criteria": {"low":"fable low","medium":"fable medium","high":"fable high","xhigh":"fable xhigh","max":"fable max"},
            })
        );
        assert_eq!(questions["effort_sonnet"]["instructions"], "Ask sonnet?");
        let raw = body.to_string();
        assert!(
            !raw.contains("target_model") && !raw.contains("current_"),
            "{raw}"
        );
    }

    #[test]
    fn request_omits_recent_conversation_without_context() {
        let body = request_body("m", &instructions(), &case(None), &[Target::Opus]);
        assert_eq!(
            body["state"],
            json!({"task":"The task.","next_message":"do it"})
        );
    }

    #[test]
    fn rejects_duplicate_candidate_names() {
        let one = "[[candidate]]\nname = \"a\"\nprovider = \"zen\"\nmodel = \"m\"\n";
        let err = parse_candidates(&format!("{one}{one}")).unwrap_err();
        assert!(err.contains("duplicate candidate name `a`"), "{err}");
    }

    fn failed(disposition: Disposition, http_status: Option<u16>) -> Attempt {
        Attempt {
            started_unix_ms: 0,
            latency_ms: 0,
            disposition,
            http_status,
            raw_response: None,
            error: None,
        }
    }

    #[test]
    fn retries_transport_errors_and_transient_statuses_only() {
        assert!(retryable(&failed(Disposition::TransportError, None)));
        // A transport error may still carry a status when the body could not be read.
        assert!(retryable(&failed(Disposition::TransportError, Some(200))));
        for status in [408, 429, 500, 502, 503, 504, 524, 529] {
            assert!(
                retryable(&failed(Disposition::ProviderError, Some(status))),
                "{status}"
            );
        }
        for status in [400, 401, 402, 403, 404, 413, 501, 505] {
            assert!(
                !retryable(&failed(Disposition::ProviderError, Some(status))),
                "{status}"
            );
        }
        assert!(!retryable(&failed(Disposition::ProviderError, None)));
        assert!(!retryable(&failed(Disposition::Answered, Some(200))));
        // Never retried even when the answer is later graded invalid.
        assert!(!retryable(&failed(Disposition::Answered, Some(503))));
    }

    const PRICED: &str = "[[candidate]]\nname = \"a\"\nprovider = \"zen\"\nmodel = \"m\"\n";

    #[test]
    fn parses_a_candidate_price_with_and_without_optional_fields() {
        let full = parse_candidates(&format!(
            "{PRICED}[candidate.price]\ninput_per_million = 0.04\noutput_per_million = 0.08\n"
        ))
        .unwrap();
        assert_eq!(
            full[0].price,
            Some(Price {
                input_per_million: 0.04,
                output_per_million: 0.08
            })
        );
        let minimal = parse_candidates(&format!(
            "{PRICED}[candidate.price]\ninput_per_million = 0.04\n"
        ))
        .unwrap();
        assert_eq!(
            minimal[0].price,
            Some(Price {
                input_per_million: 0.04,
                output_per_million: 0.0
            })
        );
        assert_eq!(parse_candidates(PRICED).unwrap()[0].price, None);
    }

    #[test]
    fn rejects_the_removed_per_request_fee() {
        let text =
            format!("{PRICED}[candidate.price]\ninput_per_million = 1\nper_request = 0.001\n");
        let err = parse_candidates(&text).unwrap_err();
        assert!(err.contains("unknown field `per_request`"), "{err}");
    }

    #[test]
    fn rejects_bad_prices_naming_the_candidate() {
        let unknown =
            format!("{PRICED}[candidate.price]\ninput_per_million = 1\ncurrency = \"EUR\"\n");
        assert!(parse_candidates(&unknown).is_err());
        let missing_input = format!("{PRICED}[candidate.price]\noutput_per_million = 1\n");
        assert!(parse_candidates(&missing_input).is_err());
        let negative = format!("{PRICED}[candidate.price]\ninput_per_million = -1\n");
        let err = parse_candidates(&negative).unwrap_err();
        assert!(
            err.contains("candidate `a`") && err.contains("input_per_million"),
            "{err}"
        );
    }
}
