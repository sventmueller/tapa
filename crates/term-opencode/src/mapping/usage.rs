//! Tokens, cost and the context window, from `TokenUsage.Info`.
//!
//! OpenCode counts cache reads and writes apart from the input, as the thread model does, and reasoning apart from
//! the output; reasoning is billed as output, so it is added to it. A step's tokens are what one request to the model
//! took, so the latest step's sum is what the context holds.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use term_thread::{EntryKind, LimitStatus, Notice, RateLimit, TokenReport, Tokens, TurnUsage, Update, UsagePatch};

use super::put;

/// From how much of a window used on a limit counts as close.
const WARNING_SHARE: f64 = 0.8;

fn number(value: &Value, path: &[&str]) -> u64 {
    let mut at = value;
    for name in path {
        match at.get(*name) {
            Some(next) => at = next,
            None => return 0,
        }
    }
    super::whole(at).unwrap_or_default()
}

/// The tokens of a `TokenUsage.Info` as the thread model counts them.
pub(crate) fn tokens(usage: &Value) -> Tokens {
    Tokens {
        input: number(usage, &["input"]),
        output: number(usage, &["output"]) + number(usage, &["reasoning"]),
        cache_read: number(usage, &["cache", "read"]),
        cache_write: number(usage, &["cache", "write"]),
    }
}

fn report(usage: &Value) -> TokenReport {
    let tokens = tokens(usage);
    TokenReport {
        total: tokens.total(),
        input: tokens.input,
        output: number(usage, &["output"]),
        cache_read: Some(tokens.cache_read),
        cache_write: Some(tokens.cache_write),
        thought: Some(number(usage, &["reasoning"])),
    }
}

fn add(a: Tokens, b: Tokens) -> Tokens {
    Tokens {
        input: a.input + b.input,
        output: a.output + b.output,
        cache_read: a.cache_read + b.cache_read,
        cache_write: a.cache_write + b.cache_write,
    }
}

/// What the mapper counts across events.
#[derive(Debug, Default)]
pub(super) struct Tally {
    turn: Option<(Tokens, f64)>,
    context_used: Option<u64>,
}

impl Tally {
    pub(super) fn context_used(&self) -> Option<u64> {
        self.context_used
    }

    pub(super) fn turn_started(&mut self) {
        self.turn = Some((Tokens::default(), 0.0));
    }

    /// `session.step.ended` or a failed step that reports its tokens: one request to the model.
    pub(super) fn step(&mut self, data: &Value, context_size: Option<u64>) -> Vec<Update> {
        let Some(usage) = data.get("tokens") else {
            return Vec::new();
        };
        let step = tokens(usage);
        let cost = data.get("cost").and_then(Value::as_f64).unwrap_or_default();
        if let Some((sum, spent)) = &mut self.turn {
            *sum = add(*sum, step);
            *spent += cost;
        }
        self.context_used = Some(step.total()).filter(|n| *n > 0).or(self.context_used);
        vec![Update::Usage(UsagePatch {
            context_used: self.context_used,
            context_size,
            ..UsagePatch::default()
        })]
    }

    /// `session.usage.updated`: the session's totals.
    pub(super) fn totals(&mut self, data: &Value) -> Vec<Update> {
        let Some(usage) = data.get("tokens") else {
            return Vec::new();
        };
        let totals = tokens(usage);
        vec![Update::Usage(UsagePatch {
            input_tokens: Some(totals.input),
            output_tokens: Some(totals.output),
            cache_read_tokens: Some(totals.cache_read),
            cache_write_tokens: Some(totals.cache_write),
            cost_usd: data.get("cost").and_then(Value::as_f64),
            reported_tokens: Some(report(usage)),
            ..UsagePatch::default()
        })]
    }

    /// The finished turn: what its steps took and cost.
    pub(super) fn turn_completed(&mut self, duration: Option<Duration>) -> Vec<Update> {
        let Some((tokens, cost)) = self.turn.take() else {
            return Vec::new();
        };
        vec![Update::Usage(UsagePatch {
            turn: Some(TurnUsage {
                message: None,
                cost_usd: Some(cost),
                tokens: Some(tokens),
                duration,
            }),
            ..UsagePatch::default()
        })]
    }
}

/// The meters of an OpenCode Go subscription in `UsageLimits.Info`, named by their window; a notice when one is close
/// to or at its limit.
pub(crate) fn rate_limits(info: &Value, at: SystemTime) -> Vec<Update> {
    let mut out = Vec::new();
    let Some(go) = info.get("go").filter(|g| g.is_object()) else {
        return out;
    };
    for window in ["rolling", "weekly", "monthly"] {
        let Some(meter) = go.get(window).filter(|m| m.is_object()) else {
            continue;
        };
        let share = meter.get("percent").and_then(Value::as_f64).map(|p| p / 100.0);
        let status = match share {
            _ if meter.get("limited").and_then(Value::as_bool) == Some(true) => LimitStatus::Rejected,
            Some(share) if share >= 1.0 => LimitStatus::Rejected,
            Some(share) if share >= WARNING_SHARE => LimitStatus::Warning,
            _ => LimitStatus::Allowed,
        };
        let limit = RateLimit {
            window: window.to_string(),
            status,
            utilization: share,
            resets_at: meter
                .get("resetsAt")
                .and_then(super::duration_ms)
                .map(|d| UNIX_EPOCH + d),
        };
        out.push(Update::RateLimit(limit.clone()));
        if status != LimitStatus::Allowed {
            let key = format!("limit:{window}:{status:?}");
            out.push(put(key, at, EntryKind::Notice(Notice::RateLimit(limit))));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn steps_add_up_to_the_turn_and_the_last_one_fills_the_context() {
        let mut tally = Tally::default();
        tally.turn_started();
        let step = |input: u64, read: u64| {
            json!({"cost": 0.01, "tokens": {"input": input, "output": 10, "reasoning": 5,
                                            "cache": {"read": read, "write": 0}}})
        };
        tally.step(&step(100, 1000), Some(200000));
        let [Update::Usage(patch)]: [Update; 1] = tally.step(&step(50, 1200), Some(200000)).try_into().unwrap() else {
            panic!()
        };
        assert_eq!((patch.context_used, patch.context_size), (Some(1265), Some(200000)));
        let [Update::Usage(patch)]: [Update; 1] = tally.turn_completed(None).try_into().unwrap() else {
            panic!()
        };
        let turn = patch.turn.unwrap();
        assert_eq!(turn.tokens.unwrap().input, 150);
        assert_eq!(turn.tokens.unwrap().output, 30);
        assert!((turn.cost_usd.unwrap() - 0.02).abs() < 1e-9);
        assert!(tally.turn_completed(None).is_empty(), "a turn ends once");
        let [Update::Usage(totals)]: [Update; 1] = tally
            .totals(&json!({"cost": 1.5, "tokens": {"input": 7, "output": 3, "reasoning": 2,
                                                   "cache": {"read": 40, "write": 9}}}))
            .try_into()
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(
            (totals.input_tokens, totals.output_tokens, totals.cache_write_tokens),
            (Some(7), Some(5), Some(9))
        );
        assert_eq!(totals.cost_usd, Some(1.5));
        assert_eq!(totals.reported_tokens.unwrap().thought, Some(2));
    }

    #[test]
    fn go_meters_are_windows_and_a_limited_one_is_rejected() {
        let updates = rate_limits(
            &json!({"workspace": {"id": "w", "usageURL": "u"}, "budgets": [], "go": {
                "rolling": {"percent": 85, "limited": false, "resetsAt": 1000},
                "weekly": {"percent": 10, "limited": false, "resetsAt": 2000},
                "monthly": {"percent": 30, "limited": true, "resetsAt": 3000}}}),
            UNIX_EPOCH,
        );
        let limits: Vec<(String, LimitStatus)> = updates
            .iter()
            .filter_map(|u| match u {
                Update::RateLimit(l) => Some((l.window.clone(), l.status)),
                _ => None,
            })
            .collect();
        assert_eq!(
            limits,
            [
                ("rolling".to_string(), LimitStatus::Warning),
                ("weekly".to_string(), LimitStatus::Allowed),
                ("monthly".to_string(), LimitStatus::Rejected)
            ]
        );
        assert_eq!(updates.len(), 5, "two notices");
        assert!(rate_limits(&json!({"budgets": []}), UNIX_EPOCH).is_empty());
    }
}
