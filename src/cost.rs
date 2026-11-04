//! Manual route prices and the observed-cost estimate (SPEC §6). USD, derived at read time.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::provider::{Attempt, Disposition};

/// A Candidate's manual price. Decision models usually charge nothing for output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Price {
    pub input_per_million: f64,
    #[serde(default)]
    pub output_per_million: f64,
    #[serde(default)]
    pub per_request: f64,
}

impl Price {
    /// Rejects negative and non-finite values.
    pub fn validate(&self) -> Result<(), String> {
        for (field, value) in [
            ("input_per_million", self.input_per_million),
            ("output_per_million", self.output_per_million),
            ("per_request", self.per_request),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(format!(
                    "{field} must be a non-negative number, got {value}"
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnknownReason {
    NoPrice,
    NoUsage,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AttemptCost {
    Known(f64),
    /// A non-2xx reply or transport error; counted as 0 on the assumption that it was not billed.
    FailedUnbilled,
    Unknown(UnknownReason),
}

pub fn attempt_cost(price: Option<&Price>, attempt: &Attempt) -> AttemptCost {
    let Some(price) = price else {
        return AttemptCost::Unknown(UnknownReason::NoPrice);
    };
    if attempt.disposition != Disposition::Answered {
        return AttemptCost::FailedUnbilled;
    }
    let usage: Value = attempt
        .raw_response
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|body| body.get("usage").cloned())
        .unwrap_or(Value::Null);
    // A count is needed only when its rate is nonzero, so an explicit zero price needs no usage.
    let part = |field: &str, rate: f64| -> Option<f64> {
        if rate == 0.0 {
            return Some(0.0);
        }
        let tokens = usage.get(field)?.as_f64().filter(|t| *t >= 0.0)?;
        Some(tokens * rate / 1e6)
    };
    match (
        part("input_tokens", price.input_per_million),
        part("output_tokens", price.output_per_million),
    ) {
        (Some(input), Some(output)) => AttemptCost::Known(input + output + price.per_request),
        _ => AttemptCost::Unknown(UnknownReason::NoUsage),
    }
}

/// Prints an amount with at least three significant digits and no trailing zeros; exact 0 prints `0`.
pub fn fmt_usd(x: f64) -> String {
    if x == 0.0 {
        return "0".to_string();
    }
    let decimals = (2 - x.abs().log10().floor() as i32).max(0) as usize;
    let text = format!("{x:.decimals$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[derive(Debug, Default)]
pub struct CostTally {
    known_usd: f64,
    known: usize,
    failed_unbilled: usize,
    no_price: usize,
    no_usage: usize,
}

impl CostTally {
    pub fn add(&mut self, cost: AttemptCost) {
        match cost {
            AttemptCost::Known(usd) => {
                self.known_usd += usd;
                self.known += 1;
            }
            AttemptCost::FailedUnbilled => self.failed_unbilled += 1,
            AttemptCost::Unknown(UnknownReason::NoPrice) => self.no_price += 1,
            AttemptCost::Unknown(UnknownReason::NoUsage) => self.no_usage += 1,
        }
    }
}

impl fmt::Display for CostTally {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.known + self.failed_unbilled + self.no_usage == 0 && self.no_price > 0 {
            let n = plural(self.no_price, "attempt", "attempts");
            return write!(f, "cost: unknown (no price for {n})");
        }
        let mut clauses = Vec::new();
        if self.known > 0 {
            clauses.push(format!(
                "{} USD over {}",
                fmt_usd(self.known_usd),
                plural(self.known, "answered attempt", "answered attempts")
            ));
        }
        if self.failed_unbilled > 0 {
            clauses.push(format!(
                "{} counted as 0 USD (assumed unbilled)",
                plural(self.failed_unbilled, "failed attempt", "failed attempts")
            ));
        }
        for (n, why) in [(self.no_usage, "no usage"), (self.no_price, "no price")] {
            if n > 0 {
                clauses.push(format!(
                    "{} unknown ({why})",
                    plural(n, "attempt", "attempts")
                ));
            }
        }
        if clauses.is_empty() {
            return f.write_str("cost: no attempts");
        }
        write!(f, "cost: {}", clauses.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn price(i: f64, o: f64, r: f64) -> Price {
        Price {
            input_per_million: i,
            output_per_million: o,
            per_request: r,
        }
    }

    fn attempt(disposition: Disposition, status: Option<u16>, raw: Option<Value>) -> Attempt {
        Attempt {
            started_unix_ms: 0,
            latency_ms: 0,
            disposition,
            http_status: status,
            raw_response: raw.map(|v| v.to_string()),
            error: None,
        }
    }

    fn answered(usage: Value) -> Attempt {
        attempt(
            Disposition::Answered,
            Some(200),
            Some(json!({"answers": {}, "usage": usage})),
        )
    }

    fn known(cost: AttemptCost) -> f64 {
        match cost {
            AttemptCost::Known(x) => x,
            other => panic!("expected Known, got {other:?}"),
        }
    }

    #[test]
    fn rate_formula() {
        // 488 * 0.04 / 1e6 = 0.00001952; 54 * 0.08 / 1e6 = 0.00000432
        let a = answered(json!({"input_tokens": 488, "output_tokens": 54, "cost": 99.0}));
        let x = known(attempt_cost(Some(&price(0.04, 0.08, 0.0)), &a));
        assert!((x - 0.00002384).abs() < 1e-12, "{x}");
    }

    #[test]
    fn per_request_fee_is_added() {
        let a = answered(json!({"input_tokens": 1_000_000, "output_tokens": 0}));
        let x = known(attempt_cost(Some(&price(2.0, 0.0, 0.003)), &a));
        assert!((x - 2.003).abs() < 1e-12, "{x}");
    }

    #[test]
    fn omitted_output_rate_counts_as_zero() {
        let p: Price = toml::from_str("input_per_million = 0.04").unwrap();
        assert_eq!(p, price(0.04, 0.0, 0.0));
        // No output_tokens needed when the output rate is 0.
        let a = answered(json!({"input_tokens": 500_000}));
        let x = known(attempt_cost(Some(&p), &a));
        assert!((x - 0.02).abs() < 1e-12, "{x}");
    }

    #[test]
    fn explicit_zero_price_is_known_zero_even_without_usage() {
        let a = attempt(
            Disposition::Answered,
            Some(200),
            Some(json!({"answers": {}})),
        );
        assert_eq!(
            attempt_cost(Some(&price(0.0, 0.0, 0.0)), &a),
            AttemptCost::Known(0.0)
        );
    }

    #[test]
    fn no_price_is_unknown_even_for_failures() {
        let ok = answered(json!({"input_tokens": 1, "output_tokens": 1}));
        let failed = attempt(Disposition::ProviderError, Some(503), Some(json!({})));
        for a in [ok, failed] {
            assert_eq!(
                attempt_cost(None, &a),
                AttemptCost::Unknown(UnknownReason::NoPrice)
            );
        }
    }

    #[test]
    fn missing_required_usage_is_unknown() {
        let p = price(0.04, 0.0, 0.0);
        for raw in [
            json!({"answers": {}}),
            json!({"usage": {"output_tokens": 5}}),
            json!({"usage": {"input_tokens": -3}}),
            json!({"usage": {"input_tokens": "12"}}),
        ] {
            let a = attempt(Disposition::Answered, Some(200), Some(raw.clone()));
            assert_eq!(
                attempt_cost(Some(&p), &a),
                AttemptCost::Unknown(UnknownReason::NoUsage),
                "{raw}"
            );
        }
        let not_json = Attempt {
            raw_response: Some("nope".into()),
            ..answered(json!({}))
        };
        assert_eq!(
            attempt_cost(Some(&p), &not_json),
            AttemptCost::Unknown(UnknownReason::NoUsage)
        );
        // A nonzero output rate makes output_tokens required.
        let a = answered(json!({"input_tokens": 5}));
        assert_eq!(
            attempt_cost(Some(&price(0.04, 0.08, 0.0)), &a),
            AttemptCost::Unknown(UnknownReason::NoUsage)
        );
    }

    #[test]
    fn failed_attempts_are_unbilled() {
        let p = price(0.04, 0.08, 0.01);
        let provider = attempt(Disposition::ProviderError, Some(429), Some(json!({})));
        let transport = Attempt {
            error: Some("timeout".into()),
            ..attempt(Disposition::TransportError, None, None)
        };
        for a in [provider, transport] {
            assert_eq!(attempt_cost(Some(&p), &a), AttemptCost::FailedUnbilled);
        }
    }

    #[test]
    fn rejects_negative_and_non_finite_prices() {
        assert!(price(0.04, 0.0, 0.0).validate().is_ok());
        assert!(price(0.0, 0.0, 0.0).validate().is_ok());
        for bad in [
            price(-0.01, 0.0, 0.0),
            price(0.04, -1.0, 0.0),
            price(0.04, 0.0, -0.001),
            price(f64::NAN, 0.0, 0.0),
            price(0.04, f64::INFINITY, 0.0),
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn formats_usd_with_three_significant_digits() {
        for (x, expected) in [
            (2.0496e-5, "0.0000205"),
            (0.0, "0"),
            (1.5, "1.5"),
            (12.345678, "12.3"),
            (0.5, "0.5"),
            (100.0, "100"),
        ] {
            assert_eq!(fmt_usd(x), expected, "{x}");
        }
    }

    #[test]
    fn tally_display() {
        let mut t = CostTally::default();
        t.add(AttemptCost::Known(2.0496e-5));
        t.add(AttemptCost::FailedUnbilled);
        t.add(AttemptCost::FailedUnbilled);
        t.add(AttemptCost::Unknown(UnknownReason::NoUsage));
        for _ in 0..3 {
            t.add(AttemptCost::Unknown(UnknownReason::NoPrice));
        }
        assert_eq!(
            t.to_string(),
            "cost: 0.0000205 USD over 1 answered attempt; 2 failed attempts counted as 0 USD (assumed unbilled); 1 attempt unknown (no usage); 3 attempts unknown (no price)"
        );
    }

    #[test]
    fn tally_display_with_nothing_known() {
        let mut t = CostTally::default();
        for _ in 0..3 {
            t.add(AttemptCost::Unknown(UnknownReason::NoPrice));
        }
        assert_eq!(t.to_string(), "cost: unknown (no price for 3 attempts)");
        let mut t = CostTally::default();
        t.add(AttemptCost::Unknown(UnknownReason::NoPrice));
        assert_eq!(t.to_string(), "cost: unknown (no price for 1 attempt)");
    }
}
