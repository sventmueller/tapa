//! Status lines in the timeline: compaction, retries, errors, hooks, limits, interruptions.

use std::time::SystemTime;

use objc2::rc::Retained;
use objc2::sel;
use objc2_app_kit::{NSColor, NSView};
use splat_i18n::{t, tf};
use term_thread::{EntryId, HookOutcome, LimitStatus, Notice, RateLimit};

use super::super::format;
use super::super::style;
use super::super::ui;
use super::{Ctx, Part, disclosure, is_auth, mono_text, notice_color, pack};

/// The window of a plan limit as the user reads it.
#[cfg_attr(
    not(feature = "selftest"),
    expect(unreachable_pub, reason = "the self-test re-exports it")
)]
pub fn window_name(window: &str) -> String {
    match window {
        "five_hour" => t("5-Stundenlimit").to_string(),
        "seven_day" => t("Wochenlimit").to_string(),
        "seven_day_opus" => t("7 Tage Opus").to_string(),
        "seven_day_sonnet" => t("7 Tage Sonnet").to_string(),
        other => other.replace('_', " "),
    }
}

fn reset(at: Option<SystemTime>) -> String {
    at.map(|at| tf(" · zurückgesetzt {}", &[&format::time_of_day(at)]))
        .unwrap_or_default()
}

/// What a notice says, on one line.
#[cfg_attr(
    not(feature = "selftest"),
    expect(unreachable_pub, reason = "the self-test re-exports it")
)]
pub fn text(notice: &Notice) -> String {
    match notice {
        Notice::Compaction {
            manual, tokens_before, ..
        } => {
            let freed = notice.freed_tokens().map_or_else(
                || tf("{} Tokens vorher", &[&format::tokens(*tokens_before)]),
                |freed| tf("{} Tokens frei", &[&format::tokens(freed)]),
            );
            let how = if *manual { t("von Hand") } else { t("automatisch") };
            tf("Verdichtet · {} ({})", &[&freed, how])
        }
        Notice::Retrying {
            attempt,
            max_attempts,
            error,
            http_status,
            ..
        } => {
            let mut text = tf(
                "Wiederhole … (Versuch {} von {})",
                &[&(attempt + 1).to_string(), &max_attempts.to_string()],
            );
            text.push_str(&format!(" · {error}"));
            if let Some(status) = http_status {
                text.push_str(&format!(" ({status})"));
            }
            text
        }
        Notice::Error { message, .. } if message.is_empty() => t("Fehler").to_string(),
        Notice::Error { message, .. } => message.clone(),
        Notice::Hook {
            name,
            event,
            exit_code,
            outcome,
            ..
        } => {
            let state = match outcome {
                HookOutcome::Running => t("läuft"),
                HookOutcome::Success => t("fertig"),
                HookOutcome::Error => t("fehlgeschlagen"),
                HookOutcome::Cancelled => t("abgebrochen"),
            };
            let mut text = tf("Hook {} ({}) · {}", &[name, event, state]);
            if let Some(code) = exit_code.filter(|c| *c != 0) {
                text.push_str(&tf(" · Code {}", &[&code.to_string()]));
            }
            text
        }
        Notice::RateLimit(limit) => limit_text(limit),
        Notice::Interrupted => t("Unterbrochen").to_string(),
        Notice::RewindRejected { .. } => {
            t("Zurückspulen abgelehnt, die Sitzung geht ohne Rücksprung weiter").to_string()
        }
        Notice::Info { text, .. } => text.clone(),
    }
}

pub(super) fn limit_text(limit: &RateLimit) -> String {
    let used = limit
        .utilization
        .map(|u| format!(" {} %", (u * 100.0).round()))
        .unwrap_or_default();
    match limit.status {
        LimitStatus::Rejected => tf(
            "Nutzungslimit {} erreicht{}",
            &[&window_name(&limit.window), &reset(limit.resets_at)],
        ),
        _ => tf(
            "Nutzung {}:{}{}",
            &[&window_name(&limit.window), &used, &reset(limit.resets_at)],
        ),
    }
}

fn symbol(notice: &Notice) -> &'static str {
    match notice {
        Notice::Compaction { .. } => "arrow.down.right.and.arrow.up.left",
        Notice::Retrying { .. } => "arrow.clockwise",
        Notice::Error { .. } => "exclamationmark.triangle",
        Notice::Hook { .. } => "link",
        Notice::RateLimit(_) => "gauge.with.dots.needle.67percent",
        Notice::Interrupted => "stop.circle",
        Notice::RewindRejected { .. } => "arrow.uturn.backward.circle",
        Notice::Info { .. } => "info.circle",
    }
}

pub(super) fn card(ctx: &Ctx<'_>, id: EntryId, notice: &Notice) -> Retained<NSView> {
    let mtm = ctx.mtm;
    let color = notice_color(notice);
    let line_text = text(notice);
    let icon = ui::icon(mtm, symbol(notice), &line_text, style::chat(11.0), &color);
    let label = ui::wrapping(mtm, &line_text, &style::chat_small(), &color, ctx.width - 60.0);
    let mut views: Vec<Retained<NSView>> = vec![ui::view(&icon), ui::view(&label)];
    let output = match notice {
        Notice::Hook { output, .. } if !output.trim().is_empty() => Some(output.clone()),
        _ => None,
    };
    if output.is_some() {
        views.push(ui::view(&disclosure(ctx, id, Part::Detail)));
    }
    if is_auth(notice) {
        views.push(ui::view(&ui::link(
            mtm,
            t("claude im Terminal anmelden"),
            ctx.target,
            sel!(ideAgentSignIn:),
            pack(id, 0),
        )));
    }
    views.push(ui::spacer(mtm));
    let refs: Vec<&NSView> = views.iter().map(|v| &**v).collect();
    let line = ui::row(mtm, 5.0, &refs);
    line.setAlignment(objc2_app_kit::NSLayoutAttribute::FirstBaseline);
    match output.filter(|_| ctx.is_open(id, Part::Detail)) {
        Some(output) => {
            let text = mono_text(ctx, &output, ctx.width - 18.0, &NSColor::secondaryLabelColor());
            let frame = ui::framed(
                mtm,
                &text,
                &NSColor::separatorColor(),
                Some(&style::code_background()),
                6.0,
                ui::insets(6.0, 8.0, 6.0, 8.0),
            );
            ui::view(&ui::column(mtm, 4.0, &[&line, &frame]))
        }
        None => ui::view(&line),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_compaction_says_how_much_it_freed() {
        let notice = Notice::Compaction {
            manual: false,
            tokens_before: 180_000,
            tokens_after: Some(42_000),
        };
        assert_eq!(text(&notice), "Verdichtet · 138 k Tokens frei (automatisch)");
    }

    #[test]
    fn a_retry_counts_the_attempt_that_comes() {
        let notice = Notice::Retrying {
            attempt: 2,
            max_attempts: 10,
            delay: std::time::Duration::from_secs(2),
            error: "server_error".into(),
            http_status: Some(529),
        };
        assert_eq!(text(&notice), "Wiederhole … (Versuch 3 von 10) · server_error (529)");
    }

    fn hook(outcome: HookOutcome, exit_code: Option<i32>) -> Notice {
        Notice::Hook {
            id: "h1".into(),
            name: "format".into(),
            event: "PostToolUse".into(),
            output: String::new(),
            exit_code,
            outcome,
        }
    }

    #[test]
    fn a_hook_says_where_it_stands_and_a_failing_exit_code() {
        assert_eq!(
            text(&hook(HookOutcome::Running, None)),
            "Hook format (PostToolUse) · läuft"
        );
        assert_eq!(
            text(&hook(HookOutcome::Success, Some(0))),
            "Hook format (PostToolUse) · fertig"
        );
        assert_eq!(
            text(&hook(HookOutcome::Error, Some(2))),
            "Hook format (PostToolUse) · fehlgeschlagen · Code 2"
        );
        assert_eq!(
            text(&hook(HookOutcome::Cancelled, None)),
            "Hook format (PostToolUse) · abgebrochen"
        );
    }

    #[test]
    fn errors_stops_rewinds_and_notes_say_what_happened() {
        let error = |message: &str| Notice::Error {
            kind: term_thread::ErrorKind::Process,
            message: message.into(),
        };
        assert_eq!(text(&error("")), "Fehler");
        assert_eq!(text(&error("the model crashed")), "the model crashed");
        assert_eq!(text(&Notice::Interrupted), "Unterbrochen");
        assert_eq!(
            text(&Notice::RewindRejected {
                detail: "too far".into()
            }),
            "Zurückspulen abgelehnt, die Sitzung geht ohne Rücksprung weiter"
        );
        let info = Notice::Info {
            level: term_thread::InfoLevel::Info,
            text: "Model switched".into(),
        };
        assert_eq!(text(&info), "Model switched");
        let compacted = Notice::Compaction {
            manual: true,
            tokens_before: 90_000,
            tokens_after: None,
        };
        assert_eq!(text(&compacted), "Verdichtet · 90 k Tokens vorher (von Hand)");
        let symbols: std::collections::BTreeSet<&str> = [
            error(""),
            Notice::Interrupted,
            info,
            compacted,
            hook(HookOutcome::Success, None),
        ]
        .iter()
        .map(symbol)
        .collect();
        assert_eq!(symbols.len(), 5, "each kind of notice has a symbol of its own");
    }

    #[test]
    fn a_limit_names_its_window_and_how_much_is_used_or_that_it_is_reached() {
        let limit = |window: &str, status, utilization| RateLimit {
            window: window.into(),
            status,
            utilization,
            resets_at: None,
        };
        assert_eq!(
            limit_text(&limit("five_hour", LimitStatus::Warning, Some(0.8))),
            "Nutzung 5-Stundenlimit: 80 %"
        );
        assert_eq!(
            limit_text(&limit("seven_day", LimitStatus::Rejected, None)),
            "Nutzungslimit Wochenlimit erreicht"
        );
        assert_eq!(
            text(&Notice::RateLimit(limit("seven_day_opus", LimitStatus::Allowed, None))),
            "Nutzung 7 Tage Opus:"
        );
        assert_eq!(window_name("seven_day_sonnet"), "7 Tage Sonnet");
        assert_eq!(window_name("monthly_extra"), "monthly extra");
        let reset = RateLimit {
            resets_at: Some(SystemTime::now()),
            ..limit("five_hour", LimitStatus::Rejected, None)
        };
        assert!(limit_text(&reset).contains(" · zurückgesetzt "));
    }
}
