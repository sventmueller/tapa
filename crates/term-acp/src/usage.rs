//! Usage is optional. Preserve the agent's units and the latest token report without guessing a turn/session
//! scope: ACP implementations currently use both. The `quota` extension in Codex and Gemini is token usage,
//! not a subscription's remaining allowance.

use agent_client_protocol::v1::schema as acp;
use serde_json::Value;
use term_thread::{ReportedCost, TokenReport, Update, UsagePatch};

pub(crate) fn context(usage: &acp::UsageUpdate) -> Update {
    let cost = usage
        .cost
        .as_ref()
        .filter(|c| c.amount.is_finite() && c.amount >= 0.0 && !c.currency.trim().is_empty());
    Update::Usage(UsagePatch {
        context_used: Some(usage.used),
        context_size: Some(usage.size),
        cost_usd: cost.filter(|c| c.currency == "USD").map(|c| c.amount),
        reported_cost: cost.map(|c| ReportedCost {
            amount: c.amount,
            currency: c.currency.clone(),
        }),
        ..UsagePatch::default()
    })
}

pub(crate) fn response(response: &acp::PromptResponse) -> Option<Update> {
    let report = response
        .usage
        .as_ref()
        .map(|usage| TokenReport {
            total: usage.total_tokens,
            input: usage.input_tokens,
            output: usage.output_tokens,
            cache_read: usage.cached_read_tokens,
            cache_write: usage.cached_write_tokens,
            thought: usage.thought_tokens,
        })
        .or_else(|| quota(response.meta.as_ref()?.get("quota")?.get("token_count")?))?;
    Some(Update::Usage(UsagePatch {
        reported_tokens: Some(report),
        ..UsagePatch::default()
    }))
}

fn quota(count: &Value) -> Option<TokenReport> {
    let number = |camel: &str, snake: &str| count.get(camel).or_else(|| count.get(snake)).and_then(Value::as_u64);
    let input = number("inputTokens", "input_tokens")?;
    let output = number("outputTokens", "output_tokens")?;
    Some(TokenReport {
        total: number("totalTokens", "total_tokens").or_else(|| input.checked_add(output))?,
        input,
        output,
        cache_read: number("cachedInputTokens", "cached_input_tokens"),
        cache_write: number("cacheWriteInputTokens", "cache_write_input_tokens"),
        thought: number("reasoningOutputTokens", "reasoning_output_tokens"),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use term_thread::{Thread, ThreadId};

    use super::*;

    fn reported(value: Value) -> Option<Update> {
        response(&serde_json::from_value(value).unwrap())
    }

    #[test]
    fn standard_report_wins_without_double_counting_cache_or_reasoning() {
        let update = reported(json!({"stopReason": "end_turn", "usage": {
            "totalTokens": 100, "inputTokens": 80, "outputTokens": 20,
            "cachedReadTokens": 50, "thoughtTokens": 10
        }, "_meta": {"quota": {"token_count": {"inputTokens": 900, "outputTokens": 900}}}}))
        .unwrap();
        let mut thread = Thread::new(ThreadId::from("usage"), "codex-acp", "/work");
        thread.apply(update.clone());
        thread.apply(update);
        let usage = thread.usage();
        let report = usage.reported_tokens.as_ref().unwrap();
        assert_eq!(report.total, 100);
        assert_eq!(report.cache_read, Some(50));
        assert_eq!(report.thought, Some(10));
        assert_eq!(usage.context_used, None);
        assert!(usage.turns.is_empty());
        assert_eq!(usage.input_tokens, 0, "no inferred session total");
    }

    #[test]
    fn codex_and_gemini_metadata_are_supported_when_standard_usage_is_absent() {
        for count in [
            json!({"inputTokens": 70, "outputTokens": 30, "totalTokens": 100,
                   "cachedInputTokens": 20, "reasoningOutputTokens": 10}),
            json!({"input_tokens": 70, "output_tokens": 30}),
        ] {
            let update = reported(json!({"stopReason": "cancelled", "_meta": {
                "quota": {"token_count": count}
            }}))
            .unwrap();
            let Update::Usage(patch) = update else {
                panic!("not usage")
            };
            assert_eq!(patch.reported_tokens.unwrap().total, 100);
            assert_eq!(patch.subscription, None);
        }
    }

    #[test]
    fn missing_null_and_invalid_reports_do_not_invent_zero_usage() {
        for count in [
            Value::Null,
            json!({}),
            json!({"inputTokens": -1, "outputTokens": 10}),
            json!({"input_tokens": u64::MAX, "output_tokens": 1}),
        ] {
            assert!(
                reported(json!({"stopReason": "end_turn", "_meta": {
                    "quota": {"token_count": count}
                }}))
                .is_none()
            );
        }
        assert!(reported(json!({"stopReason": "end_turn"})).is_none());
    }

    #[test]
    fn cost_keeps_its_currency_and_partial_updates_keep_known_values() {
        let mut thread = Thread::new(ThreadId::from("usage"), "test", "/work");
        for currency in ["USD", "EUR", "credits"] {
            let usage = serde_json::from_value(json!({"used": 12, "size": 200,
                "cost": {"amount": 2.5, "currency": currency}}))
            .unwrap();
            thread.apply(context(&usage));
            assert_eq!(thread.usage().reported_cost.as_ref().unwrap().currency, currency);
            assert_eq!(thread.usage().cost_usd, (currency == "USD").then_some(2.5));
        }
        thread.apply(context(&acp::UsageUpdate::new(20, 200)));
        assert_eq!(thread.usage().context_used, Some(20));
        assert_eq!(thread.usage().reported_cost.as_ref().unwrap().currency, "credits");
    }
}
