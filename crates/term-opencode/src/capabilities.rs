//! What OpenCode and its session offer: the release, the agents (which the UI shows as modes), the models with their
//! variants (which it shows as efforts) and the providers.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::version::Version;

fn text(value: &Value, name: &str) -> Option<String> {
    value.get(name).and_then(Value::as_str).map(str::to_string)
}

fn texts(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// A model as a session names it: the provider, the model and the variant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelRef {
    /// The provider, such as `anthropic`.
    pub provider: String,
    /// The provider's id of the model, which may contain `/`.
    pub model: String,
    /// The variant, such as `high`; OpenCode's choice when `None`.
    pub variant: Option<String>,
}

impl ModelRef {
    /// The model `provider/model`; the provider's id holds no `/`, so the first one splits.
    pub fn parse(id: &str) -> Option<ModelRef> {
        let (provider, model) = id.split_once('/')?;
        (!provider.is_empty() && !model.is_empty()).then(|| ModelRef {
            provider: provider.to_string(),
            model: model.to_string(),
            variant: None,
        })
    }

    /// `provider/model`, the value of the `model` option.
    pub fn id(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }

    /// The `Model.Ref` a response holds.
    pub fn from_json(value: &Value) -> Option<ModelRef> {
        Some(ModelRef {
            provider: text(value, "providerID")?,
            model: text(value, "id")?,
            variant: text(value, "variant"),
        })
    }

    /// The `Model.Ref` a request takes.
    pub fn json(&self) -> Value {
        let mut value = json!({"providerID": self.provider, "id": self.model});
        if let Some(variant) = &self.variant {
            value["variant"] = json!(variant);
        }
        value
    }
}

/// A model as `GET /api/model` lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    /// OpenCode's id of the entry.
    pub id: String,
    /// The provider's id of the model, which a `ModelRef` names.
    pub model_id: String,
    /// The provider.
    pub provider: String,
    /// What the picker reads.
    pub name: String,
    /// The ids of its variants, such as reasoning levels.
    pub variants: Vec<String>,
    /// What it takes in, such as `text` and `image`.
    pub input: Vec<String>,
    /// The tokens its context holds.
    pub context: Option<u64>,
    /// `alpha`, `beta`, `deprecated` or `active`.
    pub status: String,
    /// Whether it can be used.
    pub enabled: bool,
}

impl Model {
    /// The model in a `Model.Info` of the API.
    pub fn from_json(value: &Value) -> Option<Model> {
        let limit = value.get("limit");
        Some(Model {
            id: text(value, "id")?,
            model_id: text(value, "modelID").or_else(|| text(value, "id"))?,
            provider: text(value, "providerID")?,
            name: text(value, "name").unwrap_or_default(),
            variants: value
                .get("variants")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|v| text(v, "id"))
                .collect(),
            input: texts(value.get("capabilities").and_then(|c| c.get("input"))),
            context: limit
                .and_then(|l| l.get("context"))
                .and_then(Value::as_u64)
                .filter(|n| *n > 0),
            status: text(value, "status").unwrap_or_default(),
            enabled: value.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        })
    }

    /// `provider/model`, the value of the `model` option.
    pub fn reference(&self) -> String {
        format!("{}/{}", self.provider, self.model_id)
    }

    fn is(&self, model: &ModelRef) -> bool {
        self.provider == model.provider && (self.model_id == model.model || self.id == model.model)
    }
}

/// Where an agent can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMode {
    /// As the session's agent.
    Primary,
    /// Only as a subagent another one starts.
    Subagent,
    /// Both.
    All,
}

/// An agent as `GET /api/agent` lists it; the selectable ones are the session's modes.
#[derive(Debug, Clone, PartialEq)]
pub struct Agent {
    /// Its id, which `Session::set_mode` takes, such as `build`.
    pub id: String,
    /// What the picker reads.
    pub name: String,
    /// What OpenCode says about it.
    pub description: Option<String>,
    /// Where it can run.
    pub mode: AgentMode,
    /// Whether pickers leave it out.
    pub hidden: bool,
    /// The model it brings, if it has its own.
    pub model: Option<ModelRef>,
}

impl Agent {
    /// The agent in an `Agent.Info` of the API.
    pub fn from_json(value: &Value) -> Option<Agent> {
        Some(Agent {
            id: text(value, "id")?,
            name: text(value, "name").unwrap_or_default(),
            description: text(value, "description").filter(|d| !d.is_empty()),
            mode: match value.get("mode").and_then(Value::as_str) {
                Some("subagent") => AgentMode::Subagent,
                Some("all") => AgentMode::All,
                _ => AgentMode::Primary,
            },
            hidden: value.get("hidden").and_then(Value::as_bool).unwrap_or(false),
            model: value.get("model").and_then(ModelRef::from_json),
        })
    }

    /// Whether a session can switch to it.
    pub fn is_selectable(&self) -> bool {
        self.mode != AgentMode::Subagent && !self.hidden
    }
}

/// A provider as `GET /api/provider` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    /// Its id, such as `anthropic`.
    pub id: String,
    /// What the user reads.
    pub name: String,
    /// `auto`, `enabled` or `disabled`.
    pub activation: String,
}

impl Provider {
    /// The provider in a `Provider.Info` of the API.
    pub fn from_json(value: &Value) -> Option<Provider> {
        Some(Provider {
            id: text(value, "id")?,
            name: text(value, "name").unwrap_or_default(),
            activation: text(value, "activation").unwrap_or_default(),
        })
    }
}

/// The items of a list answer's `data`, read by `read`.
pub(crate) fn list<T>(result: &Value, read: fn(&Value) -> Option<T>) -> Vec<T> {
    result
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(read)
        .collect()
}

/// What the server and its session offer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Capabilities {
    /// The release, from `opencode --version`.
    pub version: Option<Version>,
    /// The release the server reports in `GET /api/info`.
    pub server_version: Option<String>,
    /// The agents.
    pub agents: Vec<Agent>,
    /// The models the configured providers offer.
    pub models: Vec<Model>,
    /// The providers.
    pub providers: Vec<Provider>,
    /// The model OpenCode uses when nothing else is chosen; `None` when no provider is connected.
    pub default_model: Option<ModelRef>,
    /// The session's agent.
    pub agent: Option<String>,
    /// The session's model.
    pub model: Option<ModelRef>,
    /// The folder the session works in.
    pub cwd: Option<PathBuf>,
}

impl Capabilities {
    /// The agents the session can switch to, as modes.
    pub fn modes(&self) -> Vec<&Agent> {
        self.agents.iter().filter(|a| a.is_selectable()).collect()
    }

    /// The session's agent, the mode the UI shows.
    pub fn mode(&self) -> Option<&str> {
        self.agent.as_deref()
    }

    /// The session's model, or OpenCode's default while the session names none.
    pub fn effective_model(&self) -> Option<&ModelRef> {
        self.model.as_ref().or(self.default_model.as_ref())
    }

    /// The session's model as `GET /api/model` describes it.
    pub fn current_model(&self) -> Option<&Model> {
        let model = self.effective_model()?;
        self.models.iter().find(|m| m.is(model))
    }

    /// The variants the session's model offers, which the UI shows as efforts.
    pub fn variants(&self) -> &[String] {
        self.current_model().map(|m| m.variants.as_slice()).unwrap_or_default()
    }

    /// The session's variant.
    pub fn variant(&self) -> Option<&str> {
        self.model.as_ref().and_then(|m| m.variant.as_deref())
    }

    /// Whether the session's model takes images; true while OpenCode has not said otherwise.
    pub fn images(&self) -> bool {
        self.current_model()
            .is_none_or(|m| m.input.is_empty() || m.input.iter().any(|i| i == "image"))
    }

    /// The tokens the session's model holds.
    pub fn context_size(&self) -> Option<u64> {
        self.current_model().and_then(|m| m.context)
    }

    /// Whether a turn can run: some provider offers a model.
    pub fn signed_in(&self) -> bool {
        self.default_model.is_some() || !self.models.is_empty()
    }

    /// Learns the agent, model and folder of a `Session.Info`.
    pub(crate) fn learn_session(&mut self, info: &Value) {
        if let Some(agent) = text(info, "agent") {
            self.agent = Some(agent);
        }
        if let Some(model) = info.get("model").and_then(ModelRef::from_json) {
            self.model = Some(model);
        }
        if let Some(dir) = info.get("location").and_then(|l| text(l, "directory")) {
            self.cwd = Some(PathBuf::from(dir));
        }
    }

    pub(crate) fn learn_info(&mut self, info: &Value) {
        self.server_version = text(info, "version");
    }

    pub(crate) fn learn_agents(&mut self, result: &Value) {
        self.agents = list(result, Agent::from_json);
    }

    pub(crate) fn learn_models(&mut self, result: &Value) {
        self.models = list(result, Model::from_json)
            .into_iter()
            .filter(|m| m.enabled)
            .collect();
    }

    pub(crate) fn learn_providers(&mut self, result: &Value) {
        self.providers = list(result, Provider::from_json);
    }

    pub(crate) fn learn_default(&mut self, result: &Value) {
        self.default_model = result.get("data").and_then(|m| {
            Some(ModelRef {
                provider: text(m, "providerID")?,
                model: text(m, "modelID").or_else(|| text(m, "id"))?,
                variant: None,
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps() -> Capabilities {
        let mut caps = Capabilities::default();
        caps.learn_models(&json!({"data": [
            {"id": "claude-sonnet-4", "modelID": "claude-sonnet-4", "providerID": "anthropic", "name": "Sonnet",
             "capabilities": {"tools": true, "input": ["text", "image"], "output": ["text"]},
             "variants": [{"id": "high"}, {"id": "max"}], "limit": {"context": 200000, "output": 64000},
             "status": "active", "enabled": true},
            {"id": "gpt-x", "modelID": "gpt-x", "providerID": "openai", "name": "X",
             "capabilities": {"tools": true, "input": ["text"], "output": ["text"]}, "variants": [],
             "limit": {"context": 0, "output": 0}, "status": "beta", "enabled": true},
            {"id": "off", "modelID": "off", "providerID": "openai", "name": "Off", "capabilities": {},
             "variants": [], "limit": {"context": 1, "output": 1}, "status": "active", "enabled": false}]}));
        caps
    }

    #[test]
    fn the_session_model_offers_its_variants_and_says_whether_it_takes_images() {
        let mut caps = caps();
        assert_eq!(caps.models.len(), 2, "a disabled model is left out");
        assert!(caps.signed_in());
        caps.learn_default(&json!({"data": {"id": "gpt-x", "modelID": "gpt-x", "providerID": "openai"}}));
        assert!(!caps.images());
        assert_eq!(caps.context_size(), None);
        caps.learn_session(
            &json!({"agent": "plan", "model": {"providerID": "anthropic", "id": "claude-sonnet-4",
            "variant": "high"}, "location": {"directory": "/w"}}),
        );
        assert_eq!(caps.variants(), ["high", "max"]);
        assert_eq!(caps.variant(), Some("high"));
        assert!(caps.images());
        assert_eq!(caps.context_size(), Some(200000));
        assert_eq!(caps.mode(), Some("plan"));
        assert_eq!(caps.current_model().unwrap().reference(), "anthropic/claude-sonnet-4");
    }

    #[test]
    fn subagents_and_hidden_agents_are_no_modes() {
        let mut caps = Capabilities::default();
        caps.learn_agents(&json!({"data": [
            {"id": "build", "name": "build", "mode": "primary", "hidden": false},
            {"id": "general", "name": "general", "mode": "subagent", "hidden": false},
            {"id": "title", "name": "title", "mode": "primary", "hidden": true},
            {"id": "docs", "name": "docs", "mode": "all", "hidden": false, "description": "Writes docs",
             "model": {"providerID": "openai", "id": "gpt-x"}}]}));
        let modes: Vec<&str> = caps.modes().iter().map(|a| a.id.as_str()).collect();
        assert_eq!(modes, ["build", "docs"]);
        assert_eq!(caps.agents[3].model.as_ref().unwrap().id(), "openai/gpt-x");
        assert!(!caps.signed_in());
    }

    #[test]
    fn a_model_reference_splits_at_its_first_slash() {
        let model = ModelRef::parse("openrouter/meta/llama").unwrap();
        assert_eq!(
            (model.provider.as_str(), model.model.as_str()),
            ("openrouter", "meta/llama")
        );
        assert_eq!(model.id(), "openrouter/meta/llama");
        assert_eq!(ModelRef::parse("nomodel"), None);
        assert_eq!(ModelRef::parse("/x"), None);
        let with = ModelRef {
            variant: Some("high".into()),
            ..model
        };
        assert_eq!(ModelRef::from_json(&with.json()), Some(with));
    }
}
