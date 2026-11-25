//! What the overlays show, the same for every agent: each backend turns its protocol's answers into these, and
//! the overlays build their rows from nothing else.

/// A section of `/status`: its title and rows, each a name and a value; a row without a name is a sentence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StatusSection {
    /// The title.
    pub(crate) title: String,
    /// The rows.
    pub(crate) rows: Vec<(Option<String>, String)>,
}

/// A question asked beside the conversation and its answer (`/btw`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Aside {
    /// The question.
    pub(crate) question: String,
    /// The answer.
    pub(crate) answer: String,
    /// What to know about the answer, such as that another model gave it.
    pub(crate) note: Option<String>,
}

/// The conversation as text, for `/export`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Exported {
    /// The conversation.
    pub(crate) text: String,
    /// The file name to offer when saving it.
    pub(crate) file_name: String,
}

/// How an MCP server stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum McpState {
    /// Connected; its tools are offered.
    Connected,
    /// Connecting.
    Pending,
    /// It could not connect.
    Failed,
    /// It waits for a sign-in.
    NeedsSignIn,
    /// Switched off.
    Disabled,
    /// The agent does not report it.
    Unknown,
}

/// How an MCP server is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transport {
    /// A program on this Mac, talked to over its standard input and output.
    Stdio,
    /// A server on the web, over HTTP.
    Http,
    /// A server on the web, over server-sent events.
    Sse,
}

impl Transport {
    pub(crate) const ALL: [Transport; 3] = [Transport::Stdio, Transport::Http, Transport::Sse];

    /// The name the agents' configurations use.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Transport::Stdio => "stdio",
            Transport::Http => "http",
            Transport::Sse => "sse",
        }
    }
}

/// Where an MCP server's definition lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// This project, for this user only.
    Local,
    /// This project, shared with everyone who has it (`.mcp.json`).
    Project,
    /// Every project of this user.
    User,
}

impl Scope {
    pub(crate) const ALL: [Scope; 3] = [Scope::Local, Scope::Project, Scope::User];

    /// The name the agents' configurations use.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Scope::Local => "local",
            Scope::Project => "project",
            Scope::User => "user",
        }
    }

    pub(crate) fn from_id(id: &str) -> Option<Scope> {
        Scope::ALL.into_iter().find(|scope| scope.id() == id)
    }
}

/// An MCP server as `/mcp` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct McpServer {
    /// Its name.
    pub(crate) name: String,
    /// How it stands.
    pub(crate) state: McpState,
    /// Why it failed, or what else the agent says about it.
    pub(crate) detail: Option<String>,
    /// How it is reached, as the agent names it.
    pub(crate) transport: Option<String>,
    /// Where its definition lives.
    pub(crate) scope: Option<Scope>,
    /// How many tools it offers, while connected.
    pub(crate) tools: Option<usize>,
    /// Whether it can be signed in to: a server on the web.
    pub(crate) remote: bool,
}

/// A new MCP server as the form describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct McpDraft {
    pub(crate) name: String,
    pub(crate) transport: Transport,
    /// The program and its arguments, for `Stdio`.
    pub(crate) command: Vec<String>,
    /// The address, for `Http` and `Sse`.
    pub(crate) url: String,
    /// Variables for the program, `NAME=value`.
    pub(crate) env: Vec<(String, String)>,
    /// Headers for the server, `Name: value`.
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) scope: Scope,
}

/// Why an agent cannot do an action of an overlay; none when it can. The overlay shows the action greyed with it.
pub(crate) type Blocked = Option<&'static str>;

/// What `/mcp` can do for an agent; an action it cannot do shows greyed with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct McpCaps {
    /// Why the states are not live, such as when they apply from the next session start.
    pub(crate) live: Blocked,
    pub(crate) toggle: Blocked,
    pub(crate) reconnect: Blocked,
    pub(crate) sign_in: Blocked,
    pub(crate) add: Blocked,
    pub(crate) remove: Blocked,
    /// Starting the session again, continuing its conversation, which servers that apply from the next start need.
    pub(crate) restart: Blocked,
    /// The transports a new server may use.
    pub(crate) transports: Vec<Transport>,
    /// The places a new server may go.
    pub(crate) scopes: Vec<Scope>,
}

/// What a permission rule does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Behavior {
    Allow,
    Ask,
    Deny,
}

impl Behavior {
    pub(crate) const ALL: [Behavior; 3] = [Behavior::Allow, Behavior::Ask, Behavior::Deny];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Behavior::Allow => "allow",
            Behavior::Ask => "ask",
            Behavior::Deny => "deny",
        }
    }

    pub(crate) fn from_id(id: &str) -> Option<Behavior> {
        Behavior::ALL.into_iter().find(|b| b.id() == id)
    }
}

/// A permission rule as `/permissions` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PermissionRule {
    pub(crate) behavior: Behavior,
    /// The rule, such as `Bash(git push *)`.
    pub(crate) rule: String,
    /// Where it lives, as the agent names it, which a removal takes back.
    pub(crate) source: String,
    /// Where it lives, for people.
    pub(crate) place: String,
    /// The agent's wording of what it allows.
    pub(crate) wording: Option<String>,
    /// Whether the user may remove it here.
    pub(crate) editable: bool,
}

/// The permission rules and the folders the agent may work in besides its own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Permissions {
    pub(crate) rules: Vec<PermissionRule>,
    pub(crate) folders: Vec<String>,
}

/// What `/permissions` can do for an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PermissionCaps {
    pub(crate) add: Blocked,
    pub(crate) remove: Blocked,
    /// The places a new rule may go.
    pub(crate) scopes: Vec<Scope>,
}

/// A hook event the agent knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HookEvent {
    pub(crate) name: String,
    pub(crate) summary: String,
    pub(crate) matcher: bool,
}

/// A hook as `/hooks` lists it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Hook {
    pub(crate) event: String,
    pub(crate) matcher: String,
    /// `command`, `prompt`, `http`, …
    pub(crate) kind: String,
    /// What runs.
    pub(crate) text: String,
    /// Where it lives, for people.
    pub(crate) place: String,
    /// Where it lives, as the agent names it.
    pub(crate) source: String,
    /// The definition as its file holds it, present for a hook the user may change.
    pub(crate) definition: Option<serde_json::Value>,
}

/// The hooks and the events they can run on.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Hooks {
    pub(crate) hooks: Vec<Hook>,
    pub(crate) events: Vec<HookEvent>,
}

/// A new or changed hook as the form describes it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HookDraft {
    pub(crate) event: String,
    pub(crate) matcher: String,
    pub(crate) command: String,
    pub(crate) timeout: Option<u64>,
    pub(crate) scope: Scope,
}

/// An instruction file the agent reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstructionFile {
    pub(crate) label: String,
    pub(crate) place: String,
    pub(crate) path: std::path::PathBuf,
    pub(crate) exists: bool,
}

/// The rows of `/memory`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Memory {
    pub(crate) files: Vec<InstructionFile>,
    /// The folders it keeps memory in, by their label.
    pub(crate) folders: Vec<(String, std::path::PathBuf)>,
    /// Automatic memory: whether it is on and whether the user may switch it; none when the agent has none.
    pub(crate) automatic: Option<(bool, bool)>,
}

/// A setting as `/config` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Setting {
    /// The agent's name for it.
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) value: String,
    /// Where its value comes from, for people.
    pub(crate) source: String,
    /// The values to choose from, by the agent's id and the name shown; none where it cannot be changed here.
    pub(crate) choices: Vec<(String, String)>,
    /// The id of the value in use, among `choices`.
    pub(crate) current: String,
}

/// The rows of `/config`: the settings in effect, the output style and the files they live in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Settings {
    pub(crate) settings: Vec<Setting>,
    /// The output style in effect and the ones to choose from; none when the agent has none.
    pub(crate) styles: Option<(String, Vec<String>)>,
    /// The settings files, by their label, to open in an editor.
    pub(crate) files: Vec<(String, std::path::PathBuf)>,
}

/// A skill as `/skills` lists it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Skill {
    pub(crate) name: String,
    pub(crate) shown: String,
    pub(crate) description: String,
    pub(crate) source: String,
    pub(crate) tokens: Option<u64>,
    pub(crate) enabled: bool,
    /// What the agent needs back to change it.
    pub(crate) handles: serde_json::Value,
}

/// A plugin, installed or offered by a marketplace (`/plugins`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Plugin {
    /// Its id, `name@marketplace`.
    pub(crate) id: String,
    /// Its name.
    pub(crate) name: String,
    /// The marketplace it comes from.
    pub(crate) marketplace: String,
    /// What it does.
    pub(crate) description: String,
    /// Its version, where known.
    pub(crate) version: Option<String>,
    /// Where it is installed; none for one only offered.
    pub(crate) scope: Option<Scope>,
    /// Whether it is switched on.
    pub(crate) enabled: bool,
    /// How many installed it, as the marketplace counts.
    pub(crate) installs: Option<u64>,
}

/// A marketplace plugins come from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Marketplace {
    /// Its name.
    pub(crate) name: String,
    /// Where it comes from, such as a GitHub repository.
    pub(crate) source: String,
}

/// What `/plugins` shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Plugins {
    /// The installed plugins.
    pub(crate) installed: Vec<Plugin>,
    /// The plugins the marketplaces offer.
    pub(crate) available: Vec<Plugin>,
    /// The marketplaces.
    pub(crate) marketplaces: Vec<Marketplace>,
}

/// How an install or update ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PluginStep {
    /// It is done.
    Done,
    /// It runs a command the marketplace declares, which the user has to see and accept first.
    Confirm {
        /// The command.
        command: String,
        /// What accepting exactly this command takes.
        sha256: String,
    },
}

/// A change to a plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PluginChange {
    /// Switches it on.
    Enable,
    /// Switches it off.
    Disable,
    /// Removes it.
    Uninstall,
}

/// A change to a marketplace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarketChange {
    /// Adds it from its source.
    Add,
    /// Fetches it again.
    Update,
    /// Removes it.
    Remove,
}

/// Who is signed in to the agent (`/logout`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Account {
    /// Whether anyone is signed in.
    pub(crate) signed_in: bool,
    /// How, in the user's words.
    pub(crate) method: String,
    /// The account's name or address, where the agent tells it.
    pub(crate) name: Option<String>,
}

/// What a report to the agent's maker is about (`/bug`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FeedbackKind {
    /// Something does not work.
    Bug,
    /// Something could be better.
    Idea,
}

impl FeedbackKind {
    pub(crate) const ALL: [FeedbackKind; 2] = [FeedbackKind::Bug, FeedbackKind::Idea];

    pub(crate) fn id(self) -> &'static str {
        match self {
            FeedbackKind::Bug => "bug",
            FeedbackKind::Idea => "idea",
        }
    }
}
