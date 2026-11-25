//! What the timeline's place shows when the session cannot start: what keeps it from starting.

use objc2::rc::Retained;
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{NSColor, NSLayoutAttribute, NSStackView, NSTextAlignment, NSView};
use splat_i18n::{t, tf};

use super::source::Problem;
use super::style;
use super::target::AgentTarget;
use super::ui;
use crate::common::util::ns;

fn centered(mtm: MainThreadMarker, views: &[&NSView]) -> Retained<NSStackView> {
    let stack = ui::row(mtm, 10.0, views);
    stack.setOrientation(objc2_app_kit::NSUserInterfaceLayoutOrientation::Vertical);
    stack.setAlignment(NSLayoutAttribute::CenterX);
    stack
}

/// The states only an agent beside Claude has: not there, being installed, waiting for a sign-in, ended on its own.
fn acp_problem(
    mtm: MainThreadMarker,
    target: &AgentTarget,
    problem: &Problem,
    agent: &str,
    methods: &[term_acp::AuthMethod],
) -> Option<Retained<NSStackView>> {
    let mut buttons: Vec<Retained<NSView>> = Vec::new();
    let (symbol, title, detail) = match problem {
        Problem::Missing(plan) => {
            let detail = match plan {
                Some(plan) => {
                    buttons.push(ui::view(&ui::button(
                        mtm,
                        t("Installieren …"),
                        target,
                        sel!(ideAgentInstall:),
                        0,
                    )));
                    tf(
                        "Weder im PATH der Login-Shell noch von dieser App installiert. Installierbar ist Version {}.",
                        &[&plan.version],
                    )
                }
                None => t("Weder im PATH der Login-Shell noch von dieser App installiert.").to_string(),
            };
            ("shippingbox", tf("{} ist nicht installiert", &[agent]), detail)
        }
        Problem::Installing(progress) => (
            "arrow.down.circle",
            tf("{} wird installiert", &[agent]),
            progress.clone(),
        ),
        Problem::AcpSignIn => {
            for (n, method) in methods.iter().enumerate() {
                let tag = isize::try_from(n).unwrap_or(0);
                let button = ui::button(mtm, &method.name, target, sel!(ideAgentSignIn:), tag);
                if let Some(description) = &method.description {
                    button.setToolTip(Some(&ns(description)));
                }
                buttons.push(ui::view(&button));
            }
            let detail = if methods.iter().any(|m| m.terminal.is_some()) {
                tf(
                    "{} möchte eine Anmeldung, bevor es eine Sitzung öffnet. Bei der Anmeldung im Terminal liegt der Befehl in der Zwischenablage.",
                    &[agent],
                )
            } else {
                tf("{} möchte eine Anmeldung, bevor es eine Sitzung öffnet.", &[agent])
            };
            (
                "person.crop.circle.badge.exclamationmark",
                t("Anmeldung nötig").to_string(),
                detail,
            )
        }
        Problem::Exited(why) => (
            "stop.circle",
            tf("{} wurde beendet", &[agent]),
            why.as_ref().map_or_else(
                || t("Das Programm hat sich ohne Meldung beendet.").to_string(),
                |why| tf("Letzte Meldung: {}", &[why]),
            ),
        ),
        _ => return None,
    };
    if !matches!(problem, Problem::Installing(_)) {
        buttons.push(ui::view(&ui::button(
            mtm,
            t("Erneut versuchen"),
            target,
            sel!(ideAgentRetry:),
            0,
        )));
    }
    let icon = ui::icon(mtm, symbol, &title, 30.0, &NSColor::tertiaryLabelColor());
    let heading = ui::label(mtm, &title, &style::bold(15.0), &NSColor::labelColor());
    let text = ui::wrapping(mtm, &detail, &style::body(), &NSColor::secondaryLabelColor(), 360.0);
    text.setAlignment(NSTextAlignment::Center);
    let refs: Vec<&NSView> = buttons.iter().map(|v| &**v).collect();
    let row = ui::row(mtm, 8.0, &refs);
    Some(centered(mtm, &[&icon, &heading, &text, &row]))
}

pub(crate) fn problem(
    mtm: MainThreadMarker,
    target: &AgentTarget,
    problem: &Problem,
    agent: &str,
    methods: &[term_acp::AuthMethod],
) -> Retained<NSStackView> {
    if let Some(view) = acp_problem(mtm, target, problem, agent, methods) {
        return view;
    }
    let (symbol, title, detail) = match problem {
        Problem::Untrusted(why) => (
            "lock.shield",
            t("Arbeitsbereich nicht vertraut").to_string(),
            why.clone(),
        ),
        Problem::NotFound => (
            "exclamationmark.triangle",
            t("Claude Code nicht gefunden").to_string(),
            t(
                "Installiere die Claude-Code-CLI, etwa mit „curl -fsSL https://claude.ai/install.sh | bash“, und versuche es dann erneut.",
            )
            .to_string(),
        ),
        Problem::SignIn(_) => (
            "person.crop.circle.badge.exclamationmark",
            t("Anmeldung nötig").to_string(),
            tf(
                "Melde dich im Terminal mit „claude“ an; {} liest und speichert keine Zugangsdaten.",
                &[&term_base::identity().name],
            ),
        ),
        Problem::Start(why) => (
            "exclamationmark.octagon",
            tf("{} ist nicht gestartet", &[agent]),
            tf("Fehler: {}", &[why]),
        ),
        Problem::Missing(_) | Problem::Installing(_) | Problem::AcpSignIn | Problem::Exited(_) => {
            return centered(mtm, &[]);
        }
    };
    let icon = ui::icon(mtm, symbol, &title, 30.0, &NSColor::tertiaryLabelColor());
    let heading = ui::label(mtm, &title, &style::bold(15.0), &NSColor::labelColor());
    let text = ui::wrapping(mtm, &detail, &style::body(), &NSColor::secondaryLabelColor(), 360.0);
    text.setAlignment(NSTextAlignment::Center);
    let buttons = buttons(mtm, target, problem);
    let refs: Vec<&NSView> = buttons.iter().map(|v| &**v).collect();
    let row = ui::row(mtm, 8.0, &refs);
    centered(mtm, &[&icon, &heading, &text, &row])
}

/// What the user can do about `problem`: trust the workspace, sign in, or try again.
fn buttons(mtm: MainThreadMarker, target: &AgentTarget, problem: &Problem) -> Vec<Retained<NSView>> {
    let mut buttons: Vec<Retained<NSView>> = Vec::new();
    match problem {
        Problem::Untrusted(_) => {
            buttons.push(ui::view(&ui::button(
                mtm,
                t("Arbeitsbereich vertrauen"),
                target,
                sel!(ideAgentTrust:),
                0,
            )));
        }
        Problem::SignIn(_) => {
            buttons.push(ui::view(&ui::button(
                mtm,
                t("claude im Terminal anmelden"),
                target,
                sel!(ideAgentSignIn:),
                0,
            )));
            buttons.push(ui::view(&ui::button(
                mtm,
                t("Erneut versuchen"),
                target,
                sel!(ideAgentRetry:),
                0,
            )));
        }
        Problem::NotFound
        | Problem::Start(_)
        | Problem::Missing(_)
        | Problem::Installing(_)
        | Problem::AcpSignIn
        | Problem::Exited(_) => {
            buttons.push(ui::view(&ui::button(
                mtm,
                t("Erneut versuchen"),
                target,
                sel!(ideAgentRetry:),
                0,
            )));
        }
    }
    buttons
}
