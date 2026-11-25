//! The app's own words for what the ACP agents it ships with name only in English: their modes and thought levels.
//! Whatever an agent names that is not here shows as the agent names it.

use splat_i18n::t;
use term_acp::{ConfigOption, OptionValue};

use super::model::effort_name;

/// The name and description of the mode `mode` of the agent with the id `agent`, where the app has words for it.
pub(crate) fn mode(agent: &str, mode: &str) -> Option<(&'static str, &'static str)> {
    Some(match (agent, mode) {
        ("codex-acp", "read-only") => (
            t("Nur lesen"),
            t("Fragt vor jeder Dateiänderung und jedem Netzzugriff."),
        ),
        ("codex-acp", "workspace-write") => (
            t("Projekt bearbeiten"),
            t("Ändert Dateien im Projekt und fragt vor Schreibzugriffen außerhalb und vor Netzzugriffen."),
        ),
        ("codex-acp", "agent") => (
            t("Automatische Prüfung"),
            t("Fragt nur bei Aktionen, die als möglicherweise unsicher erkannt werden."),
        ),
        ("codex-acp", "agent-full-access") => (
            t("Voller Zugriff"),
            t("Uneingeschränkter Zugriff auf das Internet und jede Datei des Mac."),
        ),
        ("antigravity-acp", "default") => (
            t("Manuell"),
            t("Fragt vor Änderungen und Befehlen, wie die Freigaben es vorgeben."),
        ),
        ("antigravity-acp", "accept-edits") => (
            t("Änderungen annehmen"),
            t("Nimmt Dateiänderungen im Projekt ohne Rückfrage an."),
        ),
        ("antigravity-acp", "auto") => (
            t("Auto"),
            t("Erlaubt Änderungen und sichere Befehle und fragt vor Unumkehrbarem."),
        ),
        ("antigravity-acp", "plan") => (t("Plan"), t("Plant die Aufgabe sorgfältig, bevor es sie ausführt.")),
        _ => return None,
    })
}

/// The name of a thought level by the agent's value for it, where the app has a word for it.
pub(crate) fn level(value: &str) -> Option<String> {
    let name = effort_name(value);
    (name != value).then_some(name)
}

/// The name of the choice `value` of `option`: the app's word for a thought level it knows, else the agent's name.
pub(crate) fn choice(option: &ConfigOption, value: &str, agents_name: &str) -> String {
    if option.category == term_acp::Category::ThoughtLevel
        && let Some(name) = level(value)
    {
        return name;
    }
    agents_name.to_string()
}

/// The name of the choice `option` stands at now.
pub(crate) fn current(option: &ConfigOption) -> String {
    match &option.value {
        OptionValue::Select { current, .. } => choice(option, current, &option.current_name()),
        OptionValue::Bool(_) => option.current_name(),
    }
}

/// The heading of `option` in the model pill's menu.
pub(crate) fn heading(option: &ConfigOption) -> String {
    match option.category {
        term_acp::Category::Model => t("Modell").to_string(),
        term_acp::Category::ThoughtLevel => t("Denktiefe").to_string(),
        _ => option.name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use term_acp::{Category, ConfigOption, OptionChoice, OptionValue};

    use super::*;

    fn option(category: Category, current: &str, choices: &[(&str, &str)]) -> ConfigOption {
        ConfigOption {
            id: "option".into(),
            name: "Reasoning effort".into(),
            description: None,
            category,
            value: OptionValue::Select {
                current: current.into(),
                choices: choices
                    .iter()
                    .map(|(value, name)| OptionChoice {
                        value: (*value).into(),
                        name: (*name).into(),
                        description: None,
                        group: None,
                    })
                    .collect(),
            },
        }
    }

    #[test]
    fn the_shipped_agents_modes_have_the_apps_words_and_others_keep_theirs() {
        assert_eq!(mode("codex-acp", "agent").map(|m| m.0), Some("Automatische Prüfung"));
        assert_eq!(mode("antigravity-acp", "default").map(|m| m.0), Some("Manuell"));
        assert_eq!(
            mode("opencode", "default"),
            None,
            "another agent's default may mean something else"
        );
        assert_eq!(mode("codex-acp", "something-new"), None);
    }

    #[test]
    fn known_thought_levels_are_named_by_the_app_and_unknown_ones_by_the_agent() {
        let levels = option(Category::ThoughtLevel, "high", &[("high", "High"), ("turbo", "Turbo")]);
        assert_eq!(current(&levels), "Hoch");
        assert_eq!(choice(&levels, "turbo", "Turbo"), "Turbo");
        assert_eq!(heading(&levels), "Denktiefe");
        let models = option(Category::Model, "high", &[("high", "High")]);
        assert_eq!(current(&models), "High", "a model named like a level keeps its name");
    }

    #[test]
    fn every_mode_the_app_words_has_a_name_and_a_description_of_its_own() {
        let modes = [
            ("codex-acp", "read-only"),
            ("codex-acp", "workspace-write"),
            ("codex-acp", "agent"),
            ("codex-acp", "agent-full-access"),
            ("antigravity-acp", "default"),
            ("antigravity-acp", "accept-edits"),
            ("antigravity-acp", "auto"),
            ("antigravity-acp", "plan"),
        ];
        let worded: Vec<(&str, &str)> = modes.iter().filter_map(|(agent, id)| mode(agent, id)).collect();
        assert_eq!(worded.len(), modes.len());
        let descriptions: std::collections::BTreeSet<&str> = worded.iter().map(|(_, d)| *d).collect();
        assert_eq!(descriptions.len(), modes.len(), "no two modes are described alike");
        assert_eq!(
            mode("codex-acp", "agent-full-access").map(|m| m.0),
            Some("Voller Zugriff")
        );
    }

    #[test]
    fn other_options_are_headed_by_their_own_name_and_a_switch_names_its_state() {
        let mut other = option(Category::Other("brain".into()), "x", &[("x", "X")]);
        assert_eq!(heading(&other), "Reasoning effort");
        other.value = OptionValue::Bool(true);
        assert_eq!(current(&other), other.current_name());
        assert_eq!(level("turbo"), None);
        assert_eq!(level("max").as_deref(), Some("Maximal"));
    }
}
