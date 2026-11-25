//! The agents pane (E21), after Dango's: the mode, model and effort each agent's new
//! sessions start with; which agents new tabs offer, each with where its program is and the ways to install, update
//! or go back (`install`), and the ACP registry; and the rules Claude stored with "Immer erlauben". The app keeps no
//! credentials and has no agent of its own (E21), so Dango's own agent stays out.

use std::rc::Rc;

use objc2::rc::Retained;
use objc2_app_kit::NSView;
use splat_i18n::{t, tf};
use term_config::{ClaudeMode, CodexApproval, Config};
use term_ui::kit::ns;
use term_ui_agent::{AcpAgent, Agent};

use super::form::{Fields, Form, Items, Read, Shown, Write, secondary};
use super::panes::{fixed, popup_row, word};

mod install;
mod register;
#[cfg(feature = "selftest")]
pub(crate) use install::existing as installs_target;
#[cfg(feature = "selftest")]
pub(crate) use register::window_text as register_text;

/// The width of a model's field.
const MODEL_WIDTH: f64 = 220.0;

pub(super) fn pane(fields: &Fields, rules: &super::rules::Rules) -> Retained<NSView> {
    let mut form = Form::new(fields.mtm());
    claude_rows(fields, &mut form);
    codex_rows(fields, &mut form);
    form.heading("OpenCode");
    model_row(
        fields,
        &mut form,
        |c| c.agent.opencode.model.clone(),
        &["agent", "opencode", "model"],
        "anthropic/claude-sonnet",
    );
    form.hint(t(
        "Womit neue Sitzungen starten, bis du im Tab etwas anderes wählst. Leere Felder überlassen die Wahl den Einstellungen des Agenten.",
    ));
    offer_rows(fields, &mut form);
    form.heading(t("Gespeicherte Regeln von Claude"));
    form.wide(&rules.view());
    form.wide(&secondary(
        fields.mtm(),
        t(
            "In einem nicht vertrauten Arbeitsbereich laden Agenten weder Hooks noch MCP-Server noch Einstellungen des Projekts.",
        ),
    ));
    Retained::into_super(form.build())
}

fn claude_rows(fields: &Fields, form: &mut Form) {
    form.heading("Claude");
    let modes = fixed(
        ClaudeMode::ALL
            .iter()
            .map(|mode| (claude_title(*mode), mode.id()))
            .collect(),
    );
    popup_row(
        fields,
        form,
        t("Freigaben:"),
        modes,
        word(
            |c: &Config| c.agent.claude.permission_mode.id().to_string(),
            &["agent", "claude", "permission_mode"],
        ),
    );
    model_row(
        fields,
        form,
        |c| c.agent.claude.model.clone(),
        &["agent", "claude", "model"],
        "opus, sonnet …",
    );
    let efforts = efforts(&["low", "medium", "high", "xhigh", "max"]);
    popup_row(
        fields,
        form,
        t("Denkaufwand:"),
        efforts,
        optional(|c| c.agent.claude.effort.clone(), &["agent", "claude", "effort"]),
    );
}

fn codex_rows(fields: &Fields, form: &mut Form) {
    form.heading("Codex");
    let approvals = fixed(
        CodexApproval::ALL
            .iter()
            .map(|approval| (codex_title(*approval), approval.id()))
            .collect(),
    );
    popup_row(
        fields,
        form,
        t("Freigaben:"),
        approvals,
        word(
            |c: &Config| c.agent.codex.approval.id().to_string(),
            &["agent", "codex", "approval"],
        ),
    );
    model_row(
        fields,
        form,
        |c| c.agent.codex.model.clone(),
        &["agent", "codex", "model"],
        "gpt-5-codex",
    );
    popup_row(
        fields,
        form,
        t("Denkaufwand:"),
        efforts(&["minimal", "low", "medium", "high"]),
        optional(|c| c.agent.codex.effort.clone(), &["agent", "codex", "effort"]),
    );
}

/// A model's field, which leaves the model to the agent while it is empty.
fn model_row(
    fields: &Fields,
    form: &mut Form,
    get: fn(&Config) -> String,
    path: &'static [&'static str],
    example: &str,
) {
    let (field, _) = fields.text(
        MODEL_WIDTH,
        Box::new(move |c| Shown::Text(get(c))),
        optional_write(path),
    );
    field.setPlaceholderString(Some(&ns(t("Wie beim Agenten"))));
    field.setToolTip(Some(&ns(&tf("Etwa {}", &[example]))));
    form.row(t("Modell:"), &field);
}

/// A text the file holds at `path`, which goes from the file once it is empty.
fn optional(get: fn(&Config) -> String, path: &'static [&'static str]) -> (Read, Write) {
    (Box::new(move |c| Shown::Text(get(c))), optional_write(path))
}

fn optional_write(path: &'static [&'static str]) -> Write {
    Rc::new(move |doc, shown| {
        let text = shown.text().trim();
        if text.is_empty() {
            doc.remove(path);
        } else {
            doc.set(path, text);
        }
        Ok(true)
    })
}

/// The effort levels `levels`, after the choice that leaves it to the model.
fn efforts(levels: &'static [&'static str]) -> Items {
    Box::new(move |_| {
        std::iter::once((t("Wie das Modell").to_string(), String::new()))
            .chain(
                levels
                    .iter()
                    .map(|level| (effort_title(level).to_string(), (*level).to_string())),
            )
            .collect()
    })
}

fn effort_title(level: &str) -> &'static str {
    match level {
        "minimal" => t("Minimal"),
        "low" => t("Niedrig"),
        "medium" => t("Mittel"),
        "high" => t("Hoch"),
        "xhigh" => t("Extra hoch"),
        _ => t("Maximal"),
    }
}

/// A switch for each agent the app knows, whether new tabs offer it, with where its program is beside it.
fn offer_rows(fields: &Fields, form: &mut Form) {
    form.heading(t("Neue Tabs"));
    let agents: Vec<Agent> = [Agent::Claude, Agent::Codex, Agent::OpenCode]
        .into_iter()
        .chain(AcpAgent::known().into_iter().map(Agent::Acp))
        .collect();
    let target = install::target(fields.mtm());
    for (at, agent) in agents.into_iter().enumerate() {
        let id = agent.id().to_string();
        let default = agent.offered_by_default();
        let read_id = id.clone();
        let (check, _) = fields.check(
            agent.name(),
            Box::new(move |c| Shown::Bool(c.agent.offers(&read_id, default))),
            Rc::new(move |doc, shown| {
                if shown.on() == default {
                    doc.remove(&["agent", "offer", &id]);
                } else {
                    doc.set(&["agent", "offer", &id], shown.on());
                }
                Ok(true)
            }),
        );
        let line = target.line(agent, &check);
        form.row(if at == 0 { t("Anbieten:") } else { "" }, &line);
    }
    form.hint(t(
        "Ein Agent startet aus dem PATH der Login-Shell, nie aus dem Arbeitsbereich, sonst aus der Version, die die App auf deinen Wunsch installiert hat. Jeder nutzt seine eigene Anmeldung.",
    ));
    form.heading(t("ACP-Register"));
    form.row("", &target.register_line());
    form.hint(t(
        "Das Register lädt die App nur, wenn du es verlangst; installiert wird erst nach deiner Bestätigung, geprüft gegen die Prüfsumme der Version.",
    ));
}

/// Looks again for each agent's program, which the user may have installed since, and shows where it is.
pub(super) fn shown() {
    if let Some(target) = install::existing() {
        target.refresh();
    }
}

fn claude_title(mode: ClaudeMode) -> &'static str {
    match mode {
        ClaudeMode::Ask => t("Vor Änderungen fragen"),
        ClaudeMode::AcceptEdits => t("Änderungen annehmen"),
        ClaudeMode::Plan => t("Nur planen"),
    }
}

fn codex_title(approval: CodexApproval) -> &'static str {
    match approval {
        CodexApproval::ReadOnly => t("Nur lesen"),
        CodexApproval::Auto => t("Im Projekt arbeiten"),
        CodexApproval::FullAccess => t("Voller Zugriff"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_effort_menus_start_with_the_models_own() {
        let config = Config::default();
        let items = efforts(&["low", "max"])(&config);
        assert_eq!(items.first().map(|(_, value)| value.as_str()), Some(""));
        assert_eq!(
            items.iter().map(|(_, value)| value.as_str()).collect::<Vec<_>>(),
            ["", "low", "max"]
        );
    }
}
