//! Claude Code's answers in the overlays' neutral model, and the settings files they name.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use term_base::explain::Explained;
use term_claude::control::{HooksListing, MemoryDialog, PermissionRules, SettingsReport, SkillsDialog};

use crate::dialogs::model::{
    Account, Aside, Behavior, Hook, HookDraft, HookEvent, Hooks, InstructionFile, Memory, PermissionRule, Permissions,
    Scope, Setting, Settings, Skill,
};

/// The settings file of `scope`, as the CLI's writers name it.
pub(super) fn settings_source(scope: Scope) -> &'static str {
    match scope {
        Scope::Local => "localSettings",
        Scope::Project => "projectSettings",
        Scope::User => "userSettings",
    }
}

/// Where the settings source `source` lives, for people.
fn place(source: &str) -> String {
    match source {
        "localSettings" => splat_i18n::t("Nur dieses Projekt, nur für mich").to_string(),
        "projectSettings" => splat_i18n::t("Projekt").to_string(),
        "userSettings" => splat_i18n::t("Alle meine Projekte").to_string(),
        "policySettings" => splat_i18n::t("Von der Organisation").to_string(),
        "session" => splat_i18n::t("Nur diese Sitzung").to_string(),
        other => other.to_string(),
    }
}

pub(super) fn permissions(rules: PermissionRules) -> Permissions {
    Permissions {
        rules: rules
            .state
            .rules
            .into_iter()
            .filter_map(|row| {
                Some(PermissionRule {
                    behavior: Behavior::from_id(&row.behavior)?,
                    wording: row.description.map(|d| format!("{}{}", d.prefix, d.emphasis)),
                    editable: row.editability == "persistent",
                    place: place(&row.source),
                    source: row.source,
                    rule: row.rule,
                })
            })
            .collect(),
        folders: rules.state.workspace_directories.into_iter().map(|f| f.path).collect(),
    }
}

pub(super) fn hooks(listing: HooksListing) -> Hooks {
    Hooks {
        hooks: listing
            .hooks
            .into_iter()
            .map(|row| Hook {
                matcher: row.editable.as_ref().map_or(row.matcher.clone(), |e| e.matcher.clone()),
                definition: row.editable.map(|e| e.config),
                event: row.event,
                kind: row.kind,
                text: row.command_text,
                place: row.source_label,
                source: row.source,
            })
            .collect(),
        events: listing
            .event_catalog
            .into_iter()
            .map(|e| HookEvent {
                name: e.name,
                summary: e.summary,
                matcher: e.supports_matcher,
            })
            .collect(),
    }
}

/// The definition the CLI keeps for the command hook `draft`.
pub(super) fn hook_definition(draft: &HookDraft) -> Value {
    let mut hook = json!({ "type": "command", "command": draft.command });
    if let Some(timeout) = draft.timeout {
        hook["timeout"] = Value::from(timeout);
    }
    hook
}

/// Who `claude auth status` says is signed in.
pub(super) fn account(status: term_claude::cli::AuthStatus) -> Account {
    use splat_i18n::t;
    let method = match status.auth_method.as_str() {
        "claude.ai" => t("Abo bei claude.ai"),
        "api_key" | "api_key_helper" => t("API-Schlüssel"),
        "oauth_token" => t("Zugangs-Token"),
        "third_party" => t("Anderer Anbieter"),
        _ => t("Nicht angemeldet"),
    };
    Account {
        signed_in: status.logged_in,
        method: method.to_string(),
        name: status.email,
    }
}

/// What became of a report, in the user's words; why not, where it was neither sent nor saved.
pub(super) fn feedback(result: term_claude::control::FeedbackResult) -> Result<String, Explained> {
    use splat_i18n::tf;
    match (result.feedback_id, result.bundle_path) {
        (Some(_), Some(path)) => Ok(tf(
            "Der Bericht liegt auf diesem Mac unter {}; gesendet wurde nichts.",
            &[&path],
        )),
        (Some(id), None) => Ok(tf("Gesendet. Nummer des Berichts: {}", &[&id])),
        (None, _) => Err(Explained(
            result
                .unavailable_reason
                .or(result.failure_reason)
                .unwrap_or_else(|| splat_i18n::t("Der Bericht ließ sich nicht senden.").to_string()),
        )),
    }
}

/// The answer to the side question `question`.
pub(super) fn aside(question: String, answer: term_claude::control::SideAnswer) -> Aside {
    let mut note = answer.refusal_fallback.map(|fallback| fallback.content);
    if answer.synthetic && note.is_none() {
        note = Some(splat_i18n::t("Diese Antwort kam von Claude Code, nicht vom Modell.").to_string());
    }
    Aside {
        question,
        answer: answer
            .response
            .unwrap_or_else(|| splat_i18n::t("Die Frage wurde abgebrochen.").to_string()),
        note,
    }
}

pub(super) fn memory(dialog: MemoryDialog) -> Memory {
    Memory {
        files: dialog
            .files
            .into_iter()
            .map(|f| InstructionFile {
                label: f.label,
                place: f.description,
                path: PathBuf::from(f.path),
                exists: f.exists,
            })
            .collect(),
        folders: dialog
            .folders
            .into_iter()
            .map(|f| (f.label, PathBuf::from(f.path)))
            .collect(),
        automatic: Some((dialog.auto_memory.enabled, dialog.auto_memory.toggleable)),
    }
}

/// The settings `/config` shows, by key, in order.
const SHOWN: [&str; 7] = [
    "model",
    "effortLevel",
    "outputStyle",
    "permissions.defaultMode",
    "language",
    "autoMemoryEnabled",
    "cleanupPeriodDays",
];

/// What `/config` calls the setting `key`.
fn label(key: &str) -> &'static str {
    use splat_i18n::t;
    match key {
        "model" => t("Modell"),
        "effortLevel" => t("Aufwand"),
        "outputStyle" => t("Ausgabestil"),
        "permissions.defaultMode" => t("Rechte-Modus beim Start"),
        "language" => t("Sprache"),
        "autoMemoryEnabled" => t("Automatisches Gedächtnis"),
        _ => t("Sitzungen aufbewahren (Tage)"),
    }
}

/// The value at the dotted `key` in `settings`.
fn at<'a>(settings: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a Value> {
    let mut parts = key.split('.');
    let mut value = settings.get(parts.next()?)?;
    for part in parts {
        value = value.get(part)?;
    }
    Some(value)
}

fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub(super) fn settings(
    report: &SettingsReport,
    styles: Option<(String, Vec<String>)>,
    config: &Path,
    project: &Path,
) -> Settings {
    let mut shown = Vec::new();
    for key in SHOWN {
        let applied = (key == "model" || key == "effortLevel")
            .then(|| report.applied.get(if key == "model" { "model" } else { "effort" }))
            .flatten();
        let Some(value) = at(&report.effective, key).or(applied) else {
            continue;
        };
        let source = report
            .sources
            .iter()
            .rev()
            .find(|s| at(&s.settings, key).is_some())
            .map_or_else(|| splat_i18n::t("Voreinstellung").to_string(), |s| place(&s.source));
        shown.push(Setting {
            key: key.to_string(),
            label: label(key).to_string(),
            value: text(value),
            source,
            choices: Vec::new(),
            current: String::new(),
        });
    }
    let in_effect = report.effective.get("outputStyle").and_then(Value::as_str);
    Settings {
        settings: shown,
        styles: styles.map(|(current, all)| (in_effect.map_or(current, str::to_string), all)),
        files: vec![
            (place("userSettings"), config.join("settings.json")),
            (place("projectSettings"), project.join(".claude/settings.json")),
            (place("localSettings"), project.join(".claude/settings.local.json")),
        ],
    }
}

pub(super) fn skills(dialog: SkillsDialog) -> Vec<Skill> {
    dialog
        .skills
        .into_iter()
        .map(|s| Skill {
            enabled: s.state != "off",
            tokens: (s.tokens > 0).then_some(s.tokens),
            shown: if s.display_name.is_empty() {
                s.name.clone()
            } else {
                s.display_name
            },
            name: s.name,
            description: s.description,
            source: s.source,
            handles: s.handles,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture<T: term_claude::control::Answer>(name: &str) -> T {
        let path = format!(
            "{}/../term-claude/tests/dialog_fixtures/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        T::read(serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()).unwrap()
    }

    #[test]
    fn the_recorded_answers_map_onto_the_neutral_model() {
        let rules = permissions(fixture("list_permission_rules"));
        assert_eq!(rules.rules.len(), 4);
        assert_eq!(rules.rules[2].behavior, Behavior::Ask);
        assert!(rules.rules.iter().all(|r| r.editable && r.source == "userSettings"));
        let hooks = hooks(fixture("get_hooks_listing"));
        assert_eq!(hooks.hooks[0].matcher, "Bash");
        assert!(hooks.hooks[0].definition.is_some());
        assert!(hooks.events.iter().any(|e| e.name == "Stop" && !e.matcher));
        let memory = memory(fixture("get_memory_dialog"));
        assert_eq!(memory.automatic, Some((true, true)));
        assert!(memory.files[0].exists);
        let settings = settings(&fixture("get_settings"), None, Path::new("/c"), Path::new("/p"));
        assert!(settings.settings.iter().any(|s| s.value == "Explanatory"));
        let skills = skills(fixture("get_skills_dialog"));
        assert!(skills[0].enabled && skills[0].name == "demo");
    }

    #[test]
    fn a_side_answer_says_when_another_model_or_the_cli_gave_it() {
        let fallback = term_claude::control::RefusalFallback {
            original_model: "a".into(),
            fallback_model: "b".into(),
            content: "b answered".into(),
        };
        let answered = aside(
            "Why?".into(),
            term_claude::control::SideAnswer {
                response: Some("Because.".into()),
                synthetic: false,
                refusal_fallback: Some(fallback),
            },
        );
        assert_eq!(
            (answered.answer.as_str(), answered.note.as_deref()),
            ("Because.", Some("b answered"))
        );
        let made_up = aside(
            "Why?".into(),
            term_claude::control::SideAnswer {
                response: None,
                synthetic: true,
                refusal_fallback: None,
            },
        );
        assert!(made_up.note.is_some() && !made_up.answer.is_empty());
    }
}
