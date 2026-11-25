//! What Codex and its thread offer: the release, the account, the models with their efforts, and the approval and
//! sandbox settings the thread runs with.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

use crate::launch::{ApprovalPolicy, SandboxMode};
use crate::version::Version;

/// A preset of approval policy and sandbox, as Codex's own `/approvals` offers them; the UI shows them as modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    /// Its id, which `Session::set_mode` takes.
    pub id: &'static str,
    /// When Codex asks.
    pub approval: ApprovalPolicy,
    /// What its commands may touch.
    pub sandbox: SandboxMode,
}

/// The presets, with the ids `codex-acp` gives them, so a thread keeps its mode across both connections.
pub const MODES: [Mode; 3] = [
    Mode {
        id: "read-only",
        approval: ApprovalPolicy::OnRequest,
        sandbox: SandboxMode::ReadOnly,
    },
    Mode {
        id: "auto",
        approval: ApprovalPolicy::OnRequest,
        sandbox: SandboxMode::WorkspaceWrite,
    },
    Mode {
        id: "full-access",
        approval: ApprovalPolicy::Never,
        sandbox: SandboxMode::DangerFullAccess,
    },
];

/// One reasoning effort a model offers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Effort {
    /// Its id, such as `high`.
    pub reasoning_effort: String,
    /// What Codex says about it.
    #[serde(default)]
    pub description: String,
}

/// A model as `model/list` names it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    /// The id the picker uses.
    pub id: String,
    /// The model's name, which `turn/start` takes.
    pub model: String,
    /// What the picker reads.
    #[serde(default)]
    pub display_name: String,
    /// What Codex says about it.
    #[serde(default)]
    pub description: String,
    /// Whether the default picker leaves it out.
    #[serde(default)]
    pub hidden: bool,
    /// Whether it is Codex's default.
    #[serde(default)]
    pub is_default: bool,
    /// The effort it starts with.
    #[serde(default)]
    pub default_reasoning_effort: String,
    /// The efforts it offers.
    #[serde(default)]
    pub supported_reasoning_efforts: Vec<Effort>,
    /// What it takes in, such as `text` and `image`; empty when Codex does not say.
    #[serde(default)]
    pub input_modalities: Vec<String>,
}

/// How Codex is signed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Account {
    /// With an OpenAI API key.
    ApiKey,
    /// With a ChatGPT account, whose plan sets the usage limits.
    ChatGpt {
        /// The account's address.
        email: Option<String>,
        /// The plan, such as `plus` or `pro`.
        plan: String,
    },
    /// Through a cloud provider, such as `amazonBedrock` or `gcp`.
    Provider(String),
}

impl Account {
    /// The account an `account/read` answer's `account` stands for.
    pub(crate) fn from_json(value: &Value) -> Option<Account> {
        let text = |name: &str| value.get(name).and_then(Value::as_str).map(str::to_string);
        Some(match value.get("type").and_then(Value::as_str)? {
            "apiKey" => Account::ApiKey,
            "chatgpt" => Account::ChatGpt {
                email: text("email"),
                plan: text("planType").unwrap_or_default(),
            },
            other => Account::Provider(other.to_string()),
        })
    }

    /// Whether use counts against a plan's limits rather than being billed per token (E57).
    pub fn is_subscription(&self) -> bool {
        matches!(self, Account::ChatGpt { .. })
    }
}

/// What the server and its thread offer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Capabilities {
    /// The release, from `codex --version`.
    pub version: Option<Version>,
    /// The user agent the server reports.
    pub user_agent: Option<String>,
    /// The server's `$CODEX_HOME`.
    pub codex_home: Option<PathBuf>,
    /// The account, once `account/read` answered; `None` also when nobody is signed in.
    pub account: Option<Account>,
    /// Whether the configured provider needs an OpenAI sign-in.
    pub requires_openai_auth: bool,
    /// The models `model/list` offered.
    pub models: Vec<Model>,
    /// The thread's model.
    pub model: Option<String>,
    /// The provider of the model, such as `openai`.
    pub model_provider: Option<String>,
    /// The thread's reasoning effort.
    pub effort: Option<String>,
    /// When Codex asks.
    pub approval: Option<ApprovalPolicy>,
    /// What its commands may touch.
    pub sandbox: Option<SandboxMode>,
    /// The folder the thread works in.
    pub cwd: Option<PathBuf>,
}

impl Capabilities {
    /// The preset the thread's approval policy and sandbox match.
    pub fn mode(&self) -> Option<&'static str> {
        MODES
            .iter()
            .find(|m| Some(m.approval) == self.approval && Some(m.sandbox) == self.sandbox)
            .map(|m| m.id)
    }

    /// The thread's model as `model/list` describes it.
    pub fn current_model(&self) -> Option<&Model> {
        let name = self.model.as_deref()?;
        self.models.iter().find(|m| m.model == name || m.id == name)
    }

    /// The efforts the thread's model offers.
    pub fn efforts(&self) -> &[Effort] {
        self.current_model()
            .map(|m| m.supported_reasoning_efforts.as_slice())
            .unwrap_or_default()
    }

    /// Whether the thread's model takes images; true while Codex has not said otherwise.
    pub fn images(&self) -> bool {
        self.current_model()
            .is_none_or(|m| m.input_modalities.is_empty() || m.input_modalities.iter().any(|i| i == "image"))
    }

    /// Whether a turn can run: signed in, or with a provider that needs no OpenAI sign-in.
    pub fn signed_in(&self) -> bool {
        self.account.is_some() || !self.requires_openai_auth
    }

    pub(crate) fn learn_initialize(&mut self, result: &Value) {
        let text = |name: &str| result.get(name).and_then(Value::as_str).map(str::to_string);
        self.user_agent = text("userAgent");
        self.codex_home = text("codexHome").map(PathBuf::from);
    }

    /// Learns from the answer to `thread/start`, `thread/resume` or `thread/fork`.
    pub(crate) fn learn_thread(&mut self, result: &Value) {
        let text = |name: &str| result.get(name).and_then(Value::as_str).map(str::to_string);
        self.model = text("model").or(self.model.take());
        self.model_provider = text("modelProvider").or(self.model_provider.take());
        self.effort = text("reasoningEffort").or(self.effort.take());
        self.cwd = text("cwd").map(PathBuf::from).or(self.cwd.take());
        if let Some(approval) = result.get("approvalPolicy") {
            self.approval = approval.as_str().and_then(ApprovalPolicy::from_wire);
        }
        if let Some(sandbox) = result.get("sandbox") {
            self.sandbox = SandboxMode::from_policy(sandbox);
        }
    }

    pub(crate) fn learn_account(&mut self, result: &Value) {
        self.account = result.get("account").and_then(Account::from_json);
        self.requires_openai_auth = result
            .get("requiresOpenaiAuth")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    }

    pub(crate) fn learn_models(&mut self, result: &Value) {
        if let Some(Ok(models)) = result
            .get("data")
            .map(|d| serde_json::from_value::<Vec<Model>>(d.clone()))
        {
            self.models = models;
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_thread_answer_sets_model_effort_and_the_mode_it_matches() {
        let mut caps = Capabilities::default();
        caps.learn_thread(
            &json!({"model": "gpt-5.5", "modelProvider": "openai", "reasoningEffort": "medium",
            "cwd": "/work", "approvalPolicy": "on-request", "sandbox": {"type": "workspaceWrite"}}),
        );
        assert_eq!(caps.mode(), Some("auto"));
        assert_eq!(caps.effort.as_deref(), Some("medium"));
        caps.learn_thread(&json!({"approvalPolicy": {"granular": {}}, "sandbox": {"type": "readOnly"}}));
        assert_eq!((caps.approval, caps.mode()), (None, None));
        assert_eq!(caps.model.as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn models_offer_their_efforts_and_say_whether_they_take_images() {
        let mut caps = Capabilities {
            model: Some("gpt-5.5".into()),
            ..Capabilities::default()
        };
        assert!(caps.images());
        caps.learn_models(
            &json!({"data": [{"id": "gpt-5.5", "model": "gpt-5.5", "displayName": "GPT-5.5",
            "defaultReasoningEffort": "medium", "inputModalities": ["text"],
            "supportedReasoningEfforts": [{"reasoningEffort": "low", "description": "Fast"},
                                          {"reasoningEffort": "high", "description": "Deep"}]}]}),
        );
        assert_eq!(caps.efforts().len(), 2);
        assert!(!caps.images());
    }

    #[test]
    fn an_answer_without_an_account_means_signing_in_when_openai_needs_it() {
        let mut caps = Capabilities::default();
        caps.learn_account(&json!({"account": null, "requiresOpenaiAuth": true}));
        assert!(!caps.signed_in());
        caps.learn_account(
            &json!({"account": {"type": "chatgpt", "email": "a@b.c", "planType": "plus"},
            "requiresOpenaiAuth": true}),
        );
        assert!(caps.signed_in());
        assert!(caps.account.as_ref().is_some_and(Account::is_subscription));
        caps.learn_account(&json!({"account": null, "requiresOpenaiAuth": false}));
        assert!(caps.signed_in());
    }
}
