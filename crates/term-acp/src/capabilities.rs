//! What an agent offered when the connection started and what its session offers since, read from the protocol into
//! the thread model's `Capabilities`. Only what the agent said is there; extension fields the app does not read are
//! kept as text to be shown.

use agent_client_protocol::v1::schema as acp;
use serde_json::Value;
pub use term_thread::{
    AuthMethod, Capabilities, Category, ConfigOption, Mode, OptionChoice, OptionValue, SlashCommand, TerminalSignIn,
};

/// Fills `Capabilities` from what the agent sends.
pub(crate) trait Learn {
    /// What `initialize` answered.
    fn learn_initialize(&mut self, response: &acp::InitializeResponse);
    /// The session's modes, when it has any.
    fn learn_modes(&mut self, modes: Option<&acp::SessionModeState>);
    /// The session's settings.
    fn learn_options(&mut self, options: &[acp::SessionConfigOption]);
    /// The commands the agent offers.
    fn learn_commands(&mut self, commands: &[acp::AvailableCommand]);
}

fn meta_entries(prefix: &str, meta: Option<&acp::Meta>) -> Vec<(String, String)> {
    meta.into_iter()
        .flatten()
        .map(|(key, value)| (format!("{prefix}_meta.{key}"), compact(value)))
        .collect()
}

fn compact(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() > 200 {
        format!("{}…", text.chars().take(200).collect::<String>())
    } else {
        text
    }
}

fn auth_method(method: &acp::AuthMethod) -> Option<AuthMethod> {
    match method {
        acp::AuthMethod::Agent(m) => Some(AuthMethod {
            id: m.id.0.to_string(),
            name: m.name.clone(),
            description: m.description.clone(),
            terminal: None,
        }),
        acp::AuthMethod::Terminal(m) => {
            let mut env: Vec<(String, String)> = m.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            env.sort();
            Some(AuthMethod {
                id: m.id.0.to_string(),
                name: m.name.clone(),
                description: m.description.clone(),
                terminal: Some(TerminalSignIn {
                    args: m.args.clone(),
                    env,
                }),
            })
        }
        _ => None,
    }
}

fn category(category: Option<&acp::SessionConfigOptionCategory>) -> Category {
    match category {
        Some(acp::SessionConfigOptionCategory::Mode) => Category::Mode,
        Some(acp::SessionConfigOptionCategory::Model) => Category::Model,
        Some(acp::SessionConfigOptionCategory::ModelConfig) => Category::ModelConfig,
        Some(acp::SessionConfigOptionCategory::ThoughtLevel) => Category::ThoughtLevel,
        Some(acp::SessionConfigOptionCategory::Other(name)) => Category::Other(name.clone()),
        Some(_) => Category::Other(String::new()),
        None => Category::None,
    }
}

fn choice(option: &acp::SessionConfigSelectOption, group: Option<&str>) -> OptionChoice {
    OptionChoice {
        value: option.value.0.to_string(),
        name: option.name.clone(),
        description: option.description.clone(),
        group: group.map(str::to_string),
    }
}

pub(crate) fn config_option(option: &acp::SessionConfigOption) -> Option<ConfigOption> {
    let value = match &option.kind {
        acp::SessionConfigKind::Select(select) => {
            let choices = match &select.options {
                acp::SessionConfigSelectOptions::Ungrouped(options) => {
                    options.iter().map(|o| choice(o, None)).collect()
                }
                acp::SessionConfigSelectOptions::Grouped(groups) => groups
                    .iter()
                    .flat_map(|g| g.options.iter().map(|o| choice(o, Some(&g.name))))
                    .collect(),
                _ => Vec::new(),
            };
            OptionValue::Select {
                current: select.current_value.0.to_string(),
                choices,
            }
        }
        acp::SessionConfigKind::Boolean(b) => OptionValue::Bool(b.current_value),
        _ => return None,
    };
    Some(ConfigOption {
        id: option.id.0.to_string(),
        name: option.name.clone(),
        description: option.description.clone(),
        category: category(option.category.as_ref()),
        value,
    })
}

impl Learn for Capabilities {
    fn learn_initialize(&mut self, response: &acp::InitializeResponse) {
        let caps = &response.agent_capabilities;
        self.protocol_version = Some(response.protocol_version.as_u16());
        self.agent = response
            .agent_info
            .as_ref()
            .map(|i| (i.name.clone(), i.title.clone(), i.version.clone()));
        self.load_session = caps.load_session;
        self.list_sessions = caps.session_capabilities.list.is_some();
        self.resume_session = caps.session_capabilities.resume.is_some();
        self.images = caps.prompt_capabilities.image;
        self.audio = caps.prompt_capabilities.audio;
        self.embedded_context = caps.prompt_capabilities.embedded_context;
        self.mcp_http = caps.mcp_capabilities.http;
        self.mcp_sse = caps.mcp_capabilities.sse;
        self.logout = caps.auth.logout.is_some();
        self.fork = caps.session_capabilities.fork.is_some();
        self.close = caps.session_capabilities.close.is_some();
        self.auth_methods = response.auth_methods.iter().filter_map(auth_method).collect();
        self.unknown = [
            meta_entries("", response.meta.as_ref()),
            meta_entries("agentCapabilities.", caps.meta.as_ref()),
            meta_entries(
                "agentCapabilities.sessionCapabilities.",
                caps.session_capabilities.meta.as_ref(),
            ),
            meta_entries(
                "agentCapabilities.promptCapabilities.",
                caps.prompt_capabilities.meta.as_ref(),
            ),
        ]
        .concat();
    }

    fn learn_modes(&mut self, modes: Option<&acp::SessionModeState>) {
        let Some(modes) = modes else { return };
        self.modes = modes
            .available_modes
            .iter()
            .map(|m| Mode {
                id: m.id.0.to_string(),
                name: m.name.clone(),
                description: m.description.clone(),
            })
            .collect();
        self.mode = Some(modes.current_mode_id.0.to_string());
    }

    fn learn_options(&mut self, options: &[acp::SessionConfigOption]) {
        self.options = options.iter().filter_map(config_option).collect();
    }

    fn learn_commands(&mut self, commands: &[acp::AvailableCommand]) {
        self.commands = commands
            .iter()
            .map(|c| SlashCommand {
                name: c.name.clone(),
                description: c.description.clone(),
                hint: match &c.input {
                    Some(acp::AvailableCommandInput::Unstructured(input)) => Some(input.hint.clone()),
                    _ => None,
                },
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn what_the_agent_offers_is_read_and_unknown_fields_are_kept() {
        let response: acp::InitializeResponse = serde_json::from_value(json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": true,
                "promptCapabilities": {"image": true, "embeddedContext": true},
                "_meta": {"vendor.feature": {"on": true}}
            },
            "authMethods": [
                {"id": "browser", "name": "Sign in with the browser"},
                {"type": "terminal", "id": "tty", "name": "Sign in in a terminal", "args": ["login"], "env": {"A": "1"}}
            ],
            "agentInfo": {"name": "test", "version": "1.0.0"}
        }))
        .unwrap();
        let mut caps = Capabilities::default();
        caps.learn_initialize(&response);
        assert_eq!(caps.protocol_version, Some(1));
        assert!(caps.load_session && caps.images && caps.embedded_context && !caps.audio);
        assert_eq!(caps.auth_methods.len(), 2);
        assert_eq!(
            caps.auth_methods[1].terminal,
            Some(TerminalSignIn {
                args: vec!["login".into()],
                env: vec![("A".into(), "1".into())]
            })
        );
        assert_eq!(
            caps.unknown,
            [(
                "agentCapabilities._meta.vendor.feature".to_string(),
                r#"{"on":true}"#.to_string()
            )]
        );
    }

    #[test]
    fn a_grouped_model_option_keeps_its_groups() {
        let option: acp::SessionConfigOption = serde_json::from_value(json!({
            "id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "deep",
            "options": [{"group": "g", "name": "Fast ones", "options": [{"value": "fast", "name": "Fast"}]},
                        {"group": "h", "name": "Slow ones", "options": [{"value": "deep", "name": "Deep"}]}]
        }))
        .unwrap();
        let option = config_option(&option).unwrap();
        assert_eq!(option.category, Category::Model);
        assert_eq!(option.current_name(), "Deep");
        let OptionValue::Select { choices, .. } = &option.value else {
            panic!()
        };
        assert_eq!(choices[0].group.as_deref(), Some("Fast ones"));
    }
}
