//! Tokens, the context window and the plan's usage windows.
//!
//! Codex counts as the Responses API does: cached input is part of the input, and reasoning is part of the output.
//! The thread model keeps cache reads apart from input, so they are taken out of it.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use term_thread::{EntryKind, LimitStatus, Notice, RateLimit, TokenReport, Tokens, TurnUsage, Update, UsagePatch};

use super::put;
use crate::capabilities::Capabilities;
use crate::protocol::number;

/// From how much of a window used on a limit counts as close.
const WARNING_SHARE: f64 = 0.8;

/// The session's totals as the thread model counts them.
fn totals(breakdown: &Value) -> Tokens {
    let field = |name| number(breakdown, name).unwrap_or_default();
    let cached = field("cachedInputTokens");
    Tokens {
        input: field("inputTokens").saturating_sub(cached),
        output: field("outputTokens"),
        cache_read: cached,
        cache_write: field("cacheWriteInputTokens"),
    }
}

fn report(breakdown: &Value) -> TokenReport {
    TokenReport {
        total: number(breakdown, "totalTokens").unwrap_or_default(),
        input: number(breakdown, "inputTokens").unwrap_or_default(),
        output: number(breakdown, "outputTokens").unwrap_or_default(),
        cache_read: number(breakdown, "cachedInputTokens"),
        cache_write: number(breakdown, "cacheWriteInputTokens"),
        thought: number(breakdown, "reasoningOutputTokens"),
    }
}

/// What the mapper counts across notifications.
#[derive(Debug, Default)]
pub(super) struct Tally {
    latest: Option<Tokens>,
    at_turn_start: Option<Tokens>,
    context_used: Option<u64>,
}

impl Tally {
    pub(super) fn context_used(&self) -> Option<u64> {
        self.context_used
    }

    pub(super) fn turn_started(&mut self) {
        self.at_turn_start = Some(self.latest.unwrap_or_default());
    }

    /// `thread/tokenUsage/updated`: `total` holds the thread's totals, `last` the latest request, whose size is what
    /// the context window holds.
    pub(super) fn token_usage(&mut self, params: &Value) -> Vec<Update> {
        let Some(usage) = params.get("tokenUsage") else {
            return Vec::new();
        };
        let total = usage.get("total").unwrap_or(&Value::Null);
        let tokens = totals(total);
        self.latest = Some(tokens);
        self.context_used = usage
            .get("last")
            .and_then(|last| number(last, "totalTokens"))
            .or(self.context_used);
        vec![Update::Usage(UsagePatch {
            input_tokens: Some(tokens.input),
            output_tokens: Some(tokens.output),
            cache_read_tokens: Some(tokens.cache_read),
            cache_write_tokens: Some(tokens.cache_write),
            reported_tokens: Some(report(total)),
            context_used: self.context_used,
            context_size: number(usage, "modelContextWindow"),
            ..UsagePatch::default()
        })]
    }

    /// The finished turn: what it took, when the totals before it are known.
    pub(super) fn turn_completed(&mut self, duration: Option<Duration>) -> Vec<Update> {
        let before = self.at_turn_start.take();
        let tokens = match (before, self.latest) {
            (Some(before), Some(after)) => Some(Tokens {
                input: after.input.saturating_sub(before.input),
                output: after.output.saturating_sub(before.output),
                cache_read: after.cache_read.saturating_sub(before.cache_read),
                cache_write: after.cache_write.saturating_sub(before.cache_write),
            }),
            _ => None,
        };
        vec![Update::Usage(UsagePatch {
            turn: Some(TurnUsage {
                message: None,
                cost_usd: None,
                tokens,
                duration,
            }),
            ..UsagePatch::default()
        })]
    }
}

/// Whether use counts against the plan, from the account.
pub(crate) fn subscription(caps: &Capabilities) -> Update {
    Update::Usage(UsagePatch {
        subscription: Some(caps.account.as_ref().is_some_and(|a| a.is_subscription())),
        ..UsagePatch::default()
    })
}

/// The thread model's name for a window of `minutes`, as the usage view knows Claude's.
fn window_name(minutes: Option<u64>, fallback: &str) -> String {
    match minutes {
        Some(300) => "five_hour".into(),
        Some(10080) => "seven_day".into(),
        Some(n) => format!("{n}_minutes"),
        None => fallback.into(),
    }
}

/// The windows of a `RateLimitSnapshot`; a notice when one is close to or at its limit.
pub(crate) fn rate_limits(snapshot: &Value, at: SystemTime) -> Vec<Update> {
    let reached = snapshot.get("rateLimitReachedType").is_some_and(|r| !r.is_null());
    let mut out = Vec::new();
    for slot in ["primary", "secondary"] {
        let Some(window) = snapshot.get(slot).filter(|w| w.is_object()) else {
            continue;
        };
        let used = number(window, "usedPercent").map(|p| p as f64 / 100.0);
        let status = match used {
            Some(share) if share >= 1.0 => LimitStatus::Rejected,
            _ if reached && slot == "primary" => LimitStatus::Rejected,
            Some(share) if share >= WARNING_SHARE => LimitStatus::Warning,
            _ => LimitStatus::Allowed,
        };
        let limit = RateLimit {
            window: window_name(number(window, "windowDurationMins"), slot),
            status,
            utilization: used,
            resets_at: number(window, "resetsAt").map(|s| UNIX_EPOCH + Duration::from_secs(s)),
        };
        out.push(Update::RateLimit(limit.clone()));
        if status != LimitStatus::Allowed {
            let key = format!("limit:{}:{status:?}", limit.window);
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
    fn cached_input_is_taken_out_of_the_input_and_a_turn_counts_what_it_added() {
        let mut tally = Tally::default();
        let usage = |input: u64, cached: u64, output: u64| {
            json!({"threadId": "t", "turnId": "u", "tokenUsage": {
                "total": {"inputTokens": input, "cachedInputTokens": cached, "outputTokens": output,
                          "reasoningOutputTokens": 5, "totalTokens": input + output},
                "last": {"inputTokens": 10, "cachedInputTokens": 0, "outputTokens": 2,
                         "reasoningOutputTokens": 0, "totalTokens": 900},
                "modelContextWindow": 272000}})
        };
        tally.token_usage(&usage(100, 40, 10));
        tally.turn_started();
        let [Update::Usage(patch)]: [Update; 1] = tally.token_usage(&usage(300, 140, 30)).try_into().unwrap() else {
            panic!()
        };
        assert_eq!((patch.input_tokens, patch.cache_read_tokens), (Some(160), Some(140)));
        assert_eq!((patch.context_used, patch.context_size), (Some(900), Some(272000)));
        assert_eq!(patch.reported_tokens.unwrap().thought, Some(5));
        let [Update::Usage(patch)]: [Update; 1] = tally.turn_completed(None).try_into().unwrap() else {
            panic!()
        };
        let tokens = patch.turn.unwrap().tokens.unwrap();
        assert_eq!((tokens.input, tokens.cache_read, tokens.output), (100, 100, 20));
    }

    #[test]
    fn windows_are_named_by_their_length_and_a_full_one_is_rejected() {
        let at = UNIX_EPOCH;
        let updates = rate_limits(
            &json!({"primary": {"usedPercent": 85, "windowDurationMins": 300, "resetsAt": 100},
                    "secondary": {"usedPercent": 10, "windowDurationMins": 10080}}),
            at,
        );
        let Update::RateLimit(five) = &updates[0] else {
            panic!("{updates:?}")
        };
        assert_eq!((five.window.as_str(), five.status), ("five_hour", LimitStatus::Warning));
        assert_eq!(five.resets_at, Some(UNIX_EPOCH + Duration::from_secs(100)));
        assert!(matches!(
            &updates[1],
            Update::Put {
                entry: EntryKind::Notice(Notice::RateLimit(_)),
                ..
            }
        ));
        assert!(matches!(&updates[2], Update::RateLimit(l) if l.window == "seven_day"));
        let full = rate_limits(
            &json!({"primary": {"usedPercent": 20}, "rateLimitReachedType": "rate_limit_reached"}),
            at,
        );
        assert!(matches!(&full[0], Update::RateLimit(l) if l.status == LimitStatus::Rejected && l.window == "primary"));
    }
}
