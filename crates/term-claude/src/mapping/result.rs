//! The end of a turn: its outcome, cost and tokens.

use std::time::{Duration, SystemTime};

use term_thread::{ErrorKind, ModelCost, Notice, Status, ToolPatch, ToolStatus, TurnUsage, Update, UsagePatch};

use super::{Mapper, notice, tokens_of, tokens_since};
use crate::protocol::TurnResult;

impl Mapper {
    pub(super) fn result(&mut self, r: &TurnResult, now: SystemTime) -> Vec<Update> {
        let mut out = self.close_streams(now);
        let aborted = r.terminal_reason.as_deref().is_some_and(|t| t.starts_with("aborted"));
        if aborted {
            for call in self.running.drain() {
                out.push(Update::Tool {
                    key: call,
                    patch: ToolPatch {
                        status: Some(ToolStatus::Failed),
                        ended: Some(now),
                        ..ToolPatch::default()
                    },
                });
            }
        }
        let mut usage = self.result_usage(r);
        let rejected = rewind_rejected(r);
        if rejected.is_none() {
            usage.turn = Some(self.turn_usage(r));
        }
        out.push(Update::Usage(usage));
        if let Some(detail) = rejected {
            out.push(notice(now, Notice::RewindRejected { detail }));
        } else if r.is_error && !aborted {
            let text = r.result.clone().unwrap_or_default();
            let auth = text.contains("/login");
            if !(auth && self.auth_reported) {
                let message = match &r.errors {
                    Some(errors) if !errors.is_empty() => errors.join("\n"),
                    _ if !text.is_empty() => text,
                    _ => r.subtype.clone(),
                };
                let kind = if auth { ErrorKind::Auth } else { ErrorKind::Turn };
                out.push(notice(now, Notice::Error { kind, message }));
            }
        } else if aborted {
            out.push(notice(now, Notice::Interrupted));
        }
        out.push(Update::Activity(None));
        out.push(self.status(Status::Idle));
        self.turn += 1;
        self.auth_reported = false;
        out
    }

    /// What the turn that `r` ends took: the difference of the session's totals after it and before it.
    fn turn_usage(&mut self, r: &TurnResult) -> TurnUsage {
        let known = !self.totals_unknown;
        let tokens = tokens_of(r.model_usage.values());
        let turn = TurnUsage {
            message: None,
            cost_usd: r
                .total_cost_usd
                .filter(|_| known)
                .map(|cost| (cost - self.totals.cost_usd).max(0.0)),
            tokens: known.then(|| tokens_since(tokens, self.totals.tokens)),
            duration: r.duration_ms.map(Duration::from_millis),
        };
        if let Some(cost_usd) = r.total_cost_usd {
            self.totals.cost_usd = cost_usd;
            self.totals_unknown = false;
        }
        if !r.model_usage.is_empty() {
            self.totals.tokens = tokens;
        }
        turn
    }

    fn result_usage(&self, r: &TurnResult) -> UsagePatch {
        let mut patch = UsagePatch {
            cost_usd: r.total_cost_usd,
            models: (!r.model_usage.is_empty()).then(|| model_costs(r)),
            ..UsagePatch::default()
        };
        if !r.model_usage.is_empty() {
            let sum = |f: fn(&crate::protocol::ModelUsage) -> u64| Some(r.model_usage.values().map(f).sum());
            patch.input_tokens = sum(|m| m.input_tokens);
            patch.output_tokens = sum(|m| m.output_tokens);
            patch.cache_read_tokens = sum(|m| m.cache_read_input_tokens);
            patch.cache_write_tokens = sum(|m| m.cache_creation_input_tokens);
            patch.context_size = self
                .model
                .as_ref()
                .and_then(|m| r.model_usage.get(m))
                .and_then(|m| m.context_window)
                .or_else(|| r.model_usage.values().filter_map(|m| m.context_window).max());
        }
        patch
    }
}

/// The session's totals by model, the most expensive first.
fn model_costs(r: &TurnResult) -> Vec<ModelCost> {
    let mut models: Vec<ModelCost> = r
        .model_usage
        .iter()
        .map(|(model, usage)| ModelCost {
            model: model.clone(),
            cost_usd: usage.cost_usd,
            tokens: tokens_of(std::iter::once(usage)),
            context_window: usage.context_window,
        })
        .collect();
    models.sort_by(|a, b| b.cost_usd.total_cmp(&a.cost_usd).then_with(|| a.model.cmp(&b.model)));
    models
}

/// The start of the CLI's refusal of a guarded rewind; the refusal is deterministic, so the session is resumed
/// without the fork instead of trying again.
const REWIND_REJECTED: &str = "Resume rejected by --resume-drops-turn:";

/// The CLI's explanation when `r` refuses a guarded rewind.
pub(crate) fn rewind_rejected(r: &TurnResult) -> Option<String> {
    r.errors
        .iter()
        .flatten()
        .chain(r.result.iter())
        .find(|message| message.starts_with(REWIND_REJECTED))
        .cloned()
}
