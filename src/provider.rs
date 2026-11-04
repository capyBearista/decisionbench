//! Provider routes and Candidates: how one Decisions request is built, sent and read (SPEC §4a).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

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

    fn key_env(self) -> &'static str {
        match self {
            Self::OpenRouter => "OPENROUTER_API_KEY",
            Self::Zen => "OPENCODE_API_KEY",
        }
    }

    /// The API key from the environment; an empty value counts as unset. Free Zen models need none.
    pub fn key(self) -> Result<Option<String>, String> {
        let key = env_nonempty(self.key_env());
        if key.is_none() && self == Self::OpenRouter {
            return Err(format!("{} is not set", self.key_env()));
        }
        Ok(key)
    }
}

pub fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// A Decision Model reached through one Provider Route; configured as a `[[candidate]]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    /// The user's label for this Candidate; unique in the config.
    pub name: String,
    /// The underlying model, when known.
    pub decision_model: Option<String>,
    pub provider: Provider,
    /// The model id sent on the wire.
    pub model: String,
    /// Overrides the provider's default URL (proxies, tests).
    pub url: Option<String>,
}

impl Candidate {
    pub fn effective_url(&self) -> &str {
        self.url.as_deref().unwrap_or(self.provider.url())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    candidate: Vec<Candidate>,
}

/// Parses the TOML config; Candidate names must be unique.
pub fn parse_candidates(text: &str) -> Result<Vec<Candidate>, String> {
    let config: Config = toml::from_str(text).map_err(|e| e.to_string())?;
    for (i, candidate) in config.candidate.iter().enumerate() {
        if config.candidate[..i]
            .iter()
            .any(|c| c.name == candidate.name)
        {
            return Err(format!("duplicate candidate name `{}`", candidate.name));
        }
    }
    Ok(config.candidate)
}

/// The Decisions request body. `current_effort` and `current_model` are omitted, not nulled (ADR-0002).
pub fn request_body(
    model: &str,
    instructions: &JudgeInstructions,
    case: &Case,
    target: Target,
) -> Value {
    let mut state = Map::new();
    state.insert(
        "target_model".into(),
        match target {
            Target::Sonnet => "Claude Sonnet",
            Target::Opus => "Claude Opus",
        }
        .into(),
    );
    if let Some(context) = &case.context {
        state.insert("recent_conversation".into(), context.clone().into());
    }
    state.insert("next_message".into(), case.message.clone().into());
    let criteria: Map<String, Value> = instructions
        .criteria
        .iter()
        .map(|(effort, text)| (effort.name().to_string(), text.clone().into()))
        .collect();
    json!({
        "model": model,
        "state": state,
        "questions": {"effort": {
            "type": "choice",
            "instructions": instructions.instructions,
            "criteria": criteria,
        }},
    })
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

/// Sends one POST. The key goes only in the Authorization header and is never returned.
pub fn send(url: &str, key: Option<&str>, body: &Value) -> Attempt {
    // shortcut: fixed 60 s timeout, configurable in #8
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into();
    let started_unix_ms = now_unix_ms();
    let clock = Instant::now();
    let mut request = agent.post(url).header("content-type", "application/json");
    if let Some(key) = key {
        request = request.header("authorization", format!("Bearer {key}"));
    }
    let result = request.send(body.to_string()).and_then(|mut response| {
        let status = response.status().as_u16();
        response
            .body_mut()
            .read_to_string()
            .map(|body| (status, body))
    });
    let latency_ms = clock.elapsed().as_millis() as u64;
    let (disposition, http_status, raw_response, error) = match result {
        Ok((status, body)) if (200..300).contains(&status) => {
            (Disposition::Answered, Some(status), Some(body), None)
        }
        Ok((status, body)) => (Disposition::ProviderError, Some(status), Some(body), None),
        Err(e) => (Disposition::TransportError, None, None, Some(e.to_string())),
    };
    Attempt {
        started_unix_ms,
        latency_ms,
        disposition,
        http_status,
        raw_response,
        error,
    }
}

/// What an answered reply carried. Values stay raw JSON; `effort` is graded later.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prediction {
    /// `answers.effort.choice`, or `null` when absent or the body is not JSON.
    pub effort: Value,
    pub confidence: Option<Value>,
    pub probabilities: Option<Value>,
    pub usage: Option<Value>,
    pub response_model: Option<Value>,
    pub response_provider: Option<Value>,
}

/// Reads a reply body. Only the `answers.effort` path is known; Zen's success shape is not yet observed.
pub fn normalize(raw: &str) -> Prediction {
    let body: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let answer = |field: &str| body.pointer(&format!("/answers/effort/{field}")).cloned();
    let top = |field: &str| body.get(field).cloned();
    Prediction {
        effort: answer("choice").unwrap_or(Value::Null),
        confidence: answer("confidence"),
        probabilities: answer("probabilities"),
        usage: top("usage"),
        response_model: top("model"),
        response_provider: top("provider"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPENROUTER_200: &str = r#"{"model":"typesafe/jev-1.13-20260917","answers":{"effort":{"type":"choice","choice":"low","probabilities":{"low":0.83,"medium":0.17,"high":0,"xhigh":0,"max":0},"confidence":0.79}},"usage":{"input_tokens":488,"output_tokens":54,"cost":2.0496e-05},"id":"gen-dec-1793762009-oxb0hWCuUBlv6AKzC6ON","provider":"TypeSafe"}"#;

    #[test]
    fn normalizes_an_openrouter_reply() {
        let p = normalize(OPENROUTER_200);
        assert_eq!(p.effort, json!("low"));
        assert_eq!(p.confidence, Some(json!(0.79)));
        assert_eq!(
            p.probabilities,
            Some(json!({"low":0.83,"medium":0.17,"high":0,"xhigh":0,"max":0}))
        );
        assert_eq!(
            p.usage,
            Some(json!({"input_tokens":488,"output_tokens":54,"cost":2.0496e-05}))
        );
        assert_eq!(p.response_model, Some(json!("typesafe/jev-1.13-20260917")));
        assert_eq!(p.response_provider, Some(json!("TypeSafe")));
    }

    #[test]
    fn missing_answers_or_non_json_leave_effort_null() {
        for raw in [
            r#"{"model":"m","usage":{"input_tokens":1}}"#,
            "not json",
            "",
        ] {
            assert_eq!(normalize(raw).effort, Value::Null, "{raw:?}");
        }
        assert_eq!(normalize(r#"{"model":"m"}"#).confidence, None);
        assert_eq!(normalize("not json").usage, None);
    }

    #[test]
    fn rejects_duplicate_candidate_names() {
        let one = "[[candidate]]\nname = \"a\"\nprovider = \"zen\"\nmodel = \"m\"\n";
        let err = parse_candidates(&format!("{one}{one}")).unwrap_err();
        assert!(err.contains("duplicate candidate name `a`"), "{err}");
        let found = parse_candidates(one).unwrap();
        assert_eq!(found[0].effective_url(), Provider::Zen.url());
    }
}
