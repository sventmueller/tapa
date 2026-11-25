//! What an agent offered when the connection started and what its session offers since: sign-in methods, prompt
//! content, modes, configuration options such as the model, and commands. Only what the agent said is here; nothing
//! is assumed, and extension fields the app does not read are kept as text to be shown. Each connection fills it from
//! its own protocol.

/// How the agent signs in in a terminal: its arguments and variables for its environment.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(missing_docs, reason = "the names say it")]
pub struct TerminalSignIn {
    /// The arguments the agent runs with.
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// A way to sign in to the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthMethod {
    /// The agent's id for it.
    pub id: String,
    /// The name shown.
    pub name: String,
    /// What it does.
    pub description: Option<String>,
    /// For a sign-in in a terminal, how to run it; `None` when the agent signs in by itself, such as through the
    /// browser.
    pub terminal: Option<TerminalSignIn>,
}

/// A mode of the session, such as one asking before each edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mode {
    /// The agent's id for it.
    pub id: String,
    /// The name shown.
    pub name: String,
    /// What it does.
    pub description: Option<String>,
}

/// What a configuration option is about, as the agent classes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Category {
    /// The session mode.
    Mode,
    /// The model.
    Model,
    /// A setting of the model.
    ModelConfig,
    /// How much the model thinks.
    ThoughtLevel,
    /// A class this build does not know, by the agent's name.
    Other(String),
    /// No class given.
    None,
}

/// One value a select option can take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionChoice {
    /// The agent's id for it.
    pub value: String,
    /// The name shown.
    pub name: String,
    /// What it means.
    pub description: Option<String>,
    /// The group it is listed under, when the agent groups the choices.
    pub group: Option<String>,
}

/// The value of a configuration option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionValue {
    /// One of a list.
    Select {
        /// The value in use.
        current: String,
        /// The values to choose from.
        choices: Vec<OptionChoice>,
    },
    /// On or off.
    Bool(bool),
}

/// A setting of the session the agent lets the user change.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(missing_docs, reason = "the names say it")]
pub struct ConfigOption {
    /// The agent's id for it.
    pub id: String,
    /// The name shown.
    pub name: String,
    /// What it does.
    pub description: Option<String>,
    /// What it is about.
    pub category: Category,
    pub value: OptionValue,
}

impl ConfigOption {
    /// The agent's id of the value in use.
    pub fn current_value(&self) -> String {
        match &self.value {
            OptionValue::Select { current, .. } => current.clone(),
            OptionValue::Bool(on) => on.to_string(),
        }
    }

    /// The name of the value in use.
    pub fn current_name(&self) -> String {
        match &self.value {
            OptionValue::Select { current, choices } => choices
                .iter()
                .find(|c| c.value == *current)
                .map_or_else(|| current.clone(), |c| c.name.clone()),
            OptionValue::Bool(on) => on.to_string(),
        }
    }
}

/// A command the agent offers, typed as `/name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommand {
    /// Its name without the slash.
    pub name: String,
    /// What it does.
    pub description: String,
    /// What the input after the name should be, when it takes one.
    pub hint: Option<String>,
}

/// What the agent and its session offer.
#[derive(Debug, Clone, PartialEq, Default)]
#[expect(missing_docs, reason = "the names say it")]
pub struct Capabilities {
    /// The protocol version agreed on.
    pub protocol_version: Option<u16>,
    /// The agent's name, title and version, as it reports them.
    pub agent: Option<(String, Option<String>, String)>,
    /// Whether past sessions can be loaded.
    pub load_session: bool,
    /// Whether past sessions can be listed.
    pub list_sessions: bool,
    /// Whether a session can be resumed without replaying it.
    pub resume_session: bool,
    /// Whether prompts may hold images.
    pub images: bool,
    /// Whether prompts may hold audio.
    pub audio: bool,
    /// Whether prompts may embed the content of files.
    pub embedded_context: bool,
    /// Whether MCP servers over HTTP can be handed over.
    pub mcp_http: bool,
    /// Whether MCP servers over server-sent events can be handed over.
    pub mcp_sse: bool,
    /// Whether the agent signs out on request (`logout`).
    pub logout: bool,
    /// Whether a session can be forked (`session/fork`, unstable).
    pub fork: bool,
    /// Whether a session can be closed without ending the agent (`session/close`).
    pub close: bool,
    /// The ways to sign in.
    pub auth_methods: Vec<AuthMethod>,
    /// The modes of the session.
    pub modes: Vec<Mode>,
    /// The mode in use.
    pub mode: Option<String>,
    /// The settings of the session.
    pub options: Vec<ConfigOption>,
    pub commands: Vec<SlashCommand>,
    /// Extension fields the app does not read, as name and JSON text.
    pub unknown: Vec<(String, String)>,
}

impl Capabilities {
    /// The setting that chooses the model, when the agent offers one.
    pub fn model(&self) -> Option<&ConfigOption> {
        self.options.iter().find(|o| o.category == Category::Model)
    }

    /// The setting that chooses how much the model thinks, when the agent offers one.
    pub fn thought_level(&self) -> Option<&ConfigOption> {
        self.options.iter().find(|o| o.category == Category::ThoughtLevel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(id: &str, category: Category, current: &str) -> ConfigOption {
        ConfigOption {
            id: id.into(),
            name: id.into(),
            description: None,
            category,
            value: OptionValue::Select {
                current: current.into(),
                choices: vec![OptionChoice {
                    value: "deep".into(),
                    name: "Deep".into(),
                    description: None,
                    group: Some("Slow ones".into()),
                }],
            },
        }
    }

    #[test]
    fn the_model_and_thought_level_are_found_by_their_category() {
        let caps = Capabilities {
            options: vec![
                option("effort", Category::ThoughtLevel, "high"),
                option("model", Category::Model, "deep"),
            ],
            ..Capabilities::default()
        };
        let model = caps.model().unwrap();
        assert_eq!(
            (model.current_value(), model.current_name()),
            ("deep".into(), "Deep".into())
        );
        let effort = caps.thought_level().unwrap();
        assert_eq!(effort.current_name(), "high", "a value without a choice shows its id");
    }

    #[test]
    fn a_switch_reads_as_on_or_off() {
        let switch = ConfigOption {
            value: OptionValue::Bool(true),
            ..option("fast", Category::ModelConfig, "")
        };
        assert_eq!(
            (switch.current_value(), switch.current_name()),
            ("true".into(), "true".into())
        );
    }
}
