//! Agent tabs with Codex, OpenCode and agents over ACP (E21): which agent a tab runs, whether its program is on the
//! login shell's PATH outside the workspace (E16), starting it, and the composer's menus filled from what the agent
//! offers instead of Claude's fixed lists. Codex and OpenCode speak their own protocols where the installed release
//! does, else their ACP adapter. An agent missing from the PATH runs from the version the app installed on the user's
//! word (`acp_install`).

use std::path::Path;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::sel;
use objc2_app_kit::{NSColor, NSMenu};
use splat_i18n::{t, tf};
use term_acp::register::{self, PRESETS, Preset};
use term_acp::{Effect, Launch, OptionValue, Sandbox};
use term_agents::{LaunchError, PathKind};
use term_base::LockIgnoringPoison;
use term_thread::{EntryKind, PermissionOption, RequestState, Thread};
use term_ui::workspace_host::Trust;

use super::acp_install::Availability;
use super::acp_words;
use super::cards::request::{ALLOW_ALWAYS, ALLOW_ONCE};
use super::source::{Problem, Source};
use super::tab::AgentTab;
use crate::common::util::{main_soon, ns};
#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub use selftest_probe::*;

/// An agent spoken to over ACP, as `term_acp`'s register describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpAgent {
    /// The register's id, which threads name as their agent.
    pub(crate) id: String,
    /// The name shown.
    pub(crate) name: String,
    /// Its colour as `#RRGGBB`, where it has one.
    pub(crate) colour: Option<String>,
    /// How its sandbox is switched on.
    pub(crate) sandbox: Sandbox,
    /// What its sandbox limits.
    pub(crate) effect: Effect,
}

impl AcpAgent {
    /// The agent `preset` describes.
    pub fn of_preset(preset: &Preset) -> AcpAgent {
        AcpAgent {
            id: preset.id.to_string(),
            name: preset.name.to_string(),
            colour: preset.colour.map(str::to_string),
            sandbox: preset.sandbox.clone(),
            effect: preset.effect,
        }
    }

    /// The agent with the register id `id`: one this build knows, else an entry of the registry last fetched.
    pub fn find(id: &str) -> Option<AcpAgent> {
        if let Some(preset) = register::preset(id) {
            return Some(AcpAgent::of_preset(preset));
        }
        let (register, _) = crate::acp_install::cached_register()?;
        let entry = register.entry(id)?;
        Some(AcpAgent::of_entry(entry))
    }

    /// The agent a registry entry describes, without a sandbox the app knows how to switch on.
    pub fn of_entry(entry: &term_acp::Entry) -> AcpAgent {
        AcpAgent {
            id: entry.id.clone(),
            name: entry.name.clone(),
            colour: None,
            sandbox: Sandbox::Unavailable,
            effect: Effect::None,
        }
    }

    /// The agents of the registry last fetched that the app installed, beside the presets.
    pub fn installed_entries() -> Vec<AcpAgent> {
        let Some((register, _)) = crate::acp_install::cached_register() else {
            return Vec::new();
        };
        let installs = crate::acp_install::installs();
        register
            .entries
            .iter()
            .filter(|e| register::preset(&e.id).is_none() && installs.current(&e.id).is_some())
            .map(AcpAgent::of_entry)
            .collect()
    }

    /// The name shown.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The agents of the menu that run only over ACP, in its order, those offered for new sessions: not Claude,
    /// Codex and OpenCode, which are agents of their own.
    pub fn presets() -> Vec<AcpAgent> {
        AcpAgent::known()
            .into_iter()
            .filter(|agent| Agent::Acp(agent.clone()).offered())
            .collect()
    }

    /// Every agent that runs only over ACP, offered for new sessions or not, in the menu's order.
    pub fn known() -> Vec<AcpAgent> {
        PRESETS
            .iter()
            .filter(|p| ![CLAUDE_ACP, crate::codex::PRESET, crate::opencode::PRESET].contains(&p.id))
            .map(AcpAgent::of_preset)
            .collect()
    }

    /// The register's id.
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// The preset of Claude over ACP, which no tab runs: Claude runs through its own CLI.
const CLAUDE_ACP: &str = "claude-acp";

/// Which agent a tab runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Agent {
    /// Claude through its own CLI (E21).
    Claude,
    /// Codex over its app server, else its ACP adapter `codex-acp`.
    Codex,
    /// OpenCode over its HTTP API, else `opencode acp`.
    OpenCode,
    /// Another agent over ACP, such as Antigravity.
    Acp(AcpAgent),
}

impl Agent {
    /// The agent a thread names: Claude, Codex, OpenCode, an agent the register knows, else Claude.
    pub fn of_thread(agent: &str) -> Agent {
        match agent {
            term_claude::AGENT => Agent::Claude,
            crate::codex::PRESET | term_codex::AGENT => Agent::Codex,
            crate::opencode::PRESET => Agent::OpenCode,
            other => AcpAgent::find(other).map_or(Agent::Claude, Agent::Acp),
        }
    }

    /// The id threads name.
    pub fn id(&self) -> &str {
        match self {
            Agent::Claude => term_claude::AGENT,
            Agent::Codex => crate::codex::PRESET,
            Agent::OpenCode => crate::opencode::PRESET,
            Agent::Acp(agent) => &agent.id,
        }
    }

    /// The name shown.
    pub fn name(&self) -> &str {
        match self {
            Agent::Claude => "Claude",
            Agent::Codex => "Codex",
            Agent::OpenCode => "OpenCode",
            Agent::Acp(agent) => &agent.name,
        }
    }

    /// Its colour, which tells its tabs apart in the tab bar, with enough contrast in every appearance; the system's
    /// accent colour for an agent without one.
    pub fn colour(&self) -> Retained<NSColor> {
        match (self, self.rgb()) {
            (Agent::Claude, _) => super::style::terracotta(),
            (_, Some(rgb)) => super::style::accent(rgb),
            (_, None) => NSColor::controlAccentColor(),
        }
    }

    /// Its own colour, if it has one.
    fn rgb(&self) -> Option<super::style::Rgb> {
        brand(self.id()).or_else(|| match self {
            Agent::Acp(agent) => agent.colour.as_deref().and_then(rgb_of),
            Agent::Claude | Agent::Codex | Agent::OpenCode => None,
        })
    }

    /// The colours of its mark's left, top, right and bottom point in the tab bar, for an agent that shows four.
    pub(crate) fn four_colours(&self) -> Option<[Retained<NSColor>; 4]> {
        four_colours_of(self.id())
    }

    /// Whether the composer shows what the agent offers, as for an ACP agent, instead of Claude's fixed lists.
    pub(crate) fn is_acp(&self) -> bool {
        !matches!(self, Agent::Claude)
    }

    /// The register's entry of the agent, for Codex and OpenCode their ACP adapter; none for Claude.
    pub(crate) fn preset(&self) -> Option<&'static Preset> {
        match self {
            Agent::Claude => None,
            other => register::preset(other.id()),
        }
    }

    /// The agent as an ACP agent, for its sandbox and the words of its modes; none for Claude.
    pub(crate) fn acp_agent(&self) -> Option<AcpAgent> {
        match self {
            Agent::Acp(agent) => Some(agent.clone()),
            Agent::Claude => None,
            other => other.preset().map(AcpAgent::of_preset),
        }
    }

    /// Whether the agent's program is on the login shell's PATH in `env` outside the workspace at `base`: Claude's
    /// own CLI, `codex` or its adapter, `opencode`, an ACP agent's command (E16).
    pub fn installed(&self, env: &[(String, String)], base: &Path) -> bool {
        self.program(env, base).is_some()
    }

    /// Where the agent's program is, as `installed` finds it: on the PATH, else the version the app installed.
    pub fn program(&self, env: &[(String, String)], base: &Path) -> Option<std::path::PathBuf> {
        let path = term_process::tool::var(env, "PATH");
        // A remote workspace runs only the programs there, which its stand-ins on the PATH stand for (E25).
        if let Some(bin) = term_remote::registry::bin_for(base) {
            let name = match self {
                Agent::Claude => "claude",
                Agent::Codex => "codex",
                Agent::OpenCode => "opencode",
                Agent::Acp(_) => self.preset()?.on_path.0,
            };
            return Some(bin.join(name)).filter(|program| term_process::tool::is_executable(program));
        }
        let installs = crate::acp_install::installs();
        match self {
            Agent::Claude => crate::claude_program(env).filter(|program| !term_process::tool::lies_in(program, base)),
            Agent::Codex => term_process::tool::which_outside("codex", path, base)
                .or_else(|| self.preset().and_then(|p| p.program(env, base, &installs))),
            Agent::OpenCode => self.preset().and_then(|p| p.program(env, base, &installs)),
            Agent::Acp(agent) => match self.preset() {
                Some(preset) => preset.program(env, base, &installs),
                None => installs
                    .current(&agent.id)
                    .map(|(_, program)| program)
                    .filter(|p| !term_process::tool::lies_in(p, base)),
            },
        }
    }

    /// Whether new tabs offer the agent unless the settings say otherwise; an agent from the registry always.
    pub fn offered_by_default(&self) -> bool {
        match self {
            Agent::Acp(agent) => register::preset(&agent.id).is_none_or(|p| p.offered),
            Agent::Claude | Agent::Codex | Agent::OpenCode => true,
        }
    }

    /// Whether new tabs offer the agent, as the settings have it (`set_offer`).
    pub fn offered(&self) -> bool {
        OFFER
            .lock_ignoring_poison()
            .get(self.id())
            .copied()
            .unwrap_or_else(|| self.offered_by_default())
    }
}

/// Which agents new tabs offer, by id, where the settings depart from what the app gives them.
static OFFER: std::sync::Mutex<std::collections::BTreeMap<String, bool>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Has new tabs offer the agents as `offer` says, by id; an agent it leaves out keeps what the app gives it.
pub fn set_offer(offer: std::collections::BTreeMap<String, bool>) {
    *OFFER.lock_ignoring_poison() = offer;
}

/// The colour the owner gave an agent the app ships with, by the agent's or its CLI's id.
fn brand(id: &str) -> Option<super::style::Rgb> {
    use super::style::Rgb;
    match id {
        "claude" | "claude-acp" => Some(super::style::TERRACOTTA),
        "codex-acp" | "codex" => Some(Rgb(0x1F, 0x6F, 0xEB)),
        "opencode" => Some(Rgb(0x3A, 0x3A, 0x3C)),
        "gemini" | "antigravity-acp" | "agy" => Some(GOOGLE[0]),
        _ => None,
    }
}

/// Google's blue, yellow, red and green, which its agents' marks show as Gemini's star does: blue at the right, yellow
/// at the left, red at the top, green at the bottom.
const GOOGLE: [super::style::Rgb; 4] = [
    super::style::Rgb(0x42, 0x85, 0xF4),
    super::style::Rgb(0xFB, 0xBC, 0x04),
    super::style::Rgb(0xEA, 0x43, 0x35),
    super::style::Rgb(0x34, 0xA8, 0x53),
];

fn four_colours_of(id: &str) -> Option<[Retained<NSColor>; 4]> {
    let [blue, yellow, red, green] = GOOGLE.map(super::style::accent);
    matches!(id, "gemini" | "antigravity-acp" | "agy").then(|| [yellow, red, blue, green])
}

/// The name shown for the agent a thread names, as the session list reads it.
pub fn agent_name(agent: &str) -> String {
    Agent::of_thread(agent).name().to_string()
}

fn rgb_of(hex: &str) -> Option<super::style::Rgb> {
    let hex = hex.strip_prefix('#')?;
    let value = u32::from_str_radix(hex, 16).ok().filter(|_| hex.len() == 6)?;
    let part = |shift: u32| u8::try_from((value >> shift) & 0xff).unwrap_or(0);
    Some(super::style::Rgb(part(16), part(8), part(0)))
}

/// How `agent` starts with the login shell's environment `env` in the workspace `cwd`: its own command on the PATH
/// outside `cwd` (E16), else the version the app installed, or in the self-test the stand-in it set.
pub(crate) fn launch_of(agent: &AcpAgent, env: &[(String, String)], cwd: &Path, trusted: bool) -> Option<Launch> {
    let preset = register::preset(&agent.id);
    #[cfg(feature = "selftest")]
    if let Some(test) = TEST_LAUNCH.lock_ignoring_poison().clone() {
        // The self-test's installs live in its own folder, so an entry it installed there may start.
        if test.is_none() && preset.is_none() && test_installs().is_some() {
            return Launch::installed(
                &agent.id,
                &agent.name,
                &crate::acp_install::installs(),
                env,
                cwd,
                trusted,
            );
        }
        return test.map(|mut launch| {
            launch.agent.clone_from(&agent.id);
            launch.name.clone_from(&agent.name);
            launch.cwd = cwd.to_path_buf();
            launch.trusted = trusted;
            launch.untrusted = preset.map_or(term_acp::Untrusted::Refused, |p| p.untrusted);
            launch
        });
    }
    let installs = crate::acp_install::installs();
    match preset {
        Some(preset) => Launch::of_preset(preset, &installs, env, cwd, trusted),
        None => Launch::installed(&agent.id, &agent.name, &installs, env, cwd, trusted),
    }
}

impl AgentTab {
    /// Finds the agent's program on a thread of its own, then starts it: Codex and OpenCode over their own protocols
    /// where the installed release speaks them, else over ACP.
    pub(super) fn start_acp(&self, thread: Thread) {
        let key = self.key;
        let cwd = self.cwd.clone();
        let agent = self.agent.clone();
        let trusted = self.controller.is_trusted();
        if let Err(why) = crate::common::util::spawn("acp-start", move || {
            let env = term_ui::workspace_host::environment_for(&cwd, Duration::from_secs(15));
            let mut codex = matches!(agent, Agent::Codex)
                .then(|| crate::codex::launch_for(&env, &cwd))
                .flatten();
            if let Some(launch) = &mut codex
                && trusted
                && !crate::remote_transcript::is_remote(&cwd)
            {
                crate::codex::remote::share(launch);
            }
            let opencode = matches!(agent, Agent::OpenCode)
                .then(|| crate::opencode::launch_for(&env, &cwd))
                .flatten();
            let sdk = codex.is_some() || opencode.is_some();
            let acp = agent.acp_agent();
            let launch = (!sdk)
                .then(|| acp.as_ref().and_then(|acp| launch_of(acp, &env, &cwd, trusted)))
                .flatten()
                .map(|mut launch| {
                    if trusted {
                        launch.mcp_servers = crate::dialogs::mcp_for_launch(&cwd, &env);
                    }
                    launch
                });
            let missing = (!sdk && launch.is_none())
                .then(|| acp.map(|acp| crate::acp_install::availability(&acp, &env, &cwd)))
                .flatten();
            main_soon(move || {
                if let Some(tab) = super::get(key) {
                    match (codex, opencode) {
                        (Some(codex), _) => tab.launch_codex(codex, thread),
                        (None, Some(opencode)) => tab.launch_opencode(opencode, thread),
                        (None, None) => tab.launch_acp(launch, missing, thread),
                    }
                }
            });
        }) {
            self.start_failed(Problem::Start(why.0));
        }
    }

    /// The agent's id of the session Codex or OpenCode continue for `thread`: the thread's, or the one the tab was
    /// opened to resume.
    pub(super) fn sdk_resume(&self, thread: &Thread) -> Option<String> {
        let state = self.state.borrow();
        thread
            .session()
            .map(str::to_string)
            .or_else(|| state.resume().map(|r| r.session.clone()))
    }

    /// Starts the agent as `launch` says for `thread` in the app's registry, or shows what it takes when there is no
    /// launch.
    fn launch_acp(&self, launch: Option<Launch>, missing: Option<Availability>, thread: Thread) {
        let Some(agent) = self.agent.acp_agent() else { return };
        let Some(mut launch) = launch else {
            let problem = match missing {
                Some(Availability::Installable(plan)) => Problem::Missing(Some(Box::new(plan))),
                Some(Availability::Unavailable(why)) => Problem::Start(why),
                _ => Problem::Missing(None),
            };
            self.start_failed(problem);
            return;
        };
        {
            let state = self.state.borrow();
            if state.sandbox() {
                launch = launch.sandboxed(&agent.sandbox);
            }
            launch.resume = thread
                .session()
                .map(str::to_string)
                .or_else(|| state.resume().map(|r| r.session.clone()));
        }
        let Some(sessions) = self.sessions.clone() else {
            return self.start_failed(Problem::Start(start_problem(LaunchError::Start(String::new()))));
        };
        let (cwd, env, trusted) = (launch.cwd.clone(), launch.env.clone(), launch.trusted);
        let path = term_agents::AgentPath::Acp(Box::new(launch));
        let mut start = term_agents::Launch::new(path, cwd, env, term_agents::UserChoices::default());
        start.thread = Some(thread);
        start.trusted = trusted;
        match crate::driven::Driven::start(&sessions, start) {
            Ok((driven, first)) => {
                self.run_session(driven);
                if let Some(first) = first {
                    self.absorb(first.thread, &[term_thread::Change::Cleared]);
                }
            }
            Err(error) => self.start_failed(launch_problem(error, start_problem)),
        }
    }

    /// The agent's program ended without the app stopping it: a session that had opened says the agent was ended,
    /// one that had not says it did not start, each with the last line the agent wrote; either offers to start again.
    pub(super) fn acp_ended(&self, stderr: &str) {
        let source = {
            let mut state = self.state.borrow_mut();
            let path = state.source().tab_session().map(crate::driven::Driven::kind);
            if !matches!(path, Some(PathKind::Acp | PathKind::Codex | PathKind::OpenCode)) {
                return;
            }
            state.end()
        };
        let opened = source.acp().is_some_and(|s| s.session_id().is_some())
            || source.codex().is_some_and(|s| s.session_id().is_some())
            || source.opencode().is_some_and(|s| s.session_id().is_some());
        source.end();
        self.set_problem(Some(ended_problem(opened, stderr)));
    }

    /// Signs in with method `index` of the agent: one the agent runs itself goes through the session; one in a
    /// terminal opens a terminal with the command on the pasteboard.
    pub(crate) fn acp_sign_in(&self, index: usize) {
        let caps = self.state.borrow().acp_caps().cloned().unwrap_or_default();
        let Some(method) = caps.auth_methods.get(index) else {
            return;
        };
        match &method.terminal {
            None => {
                let result = self.state.borrow().source().authenticate(&method.id);
                if let Err(error) = result {
                    crate::common::diag::report("signing in to an agent", &error);
                }
            }
            Some(terminal) => {
                let program = match self.state.borrow().source() {
                    Source::Session(driven) if driven.kind() == PathKind::Acp => self.launch_program(),
                    Source::Idle | Source::Starting => self.launch_program(),
                    Source::Session(_) => None,
                };
                let Some(program) = program else { return };
                let mut parts: Vec<String> = terminal.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
                parts.push(program);
                parts.extend(terminal.args.iter().cloned());
                super::actions::copy_text(&parts.join(" "));
                if let Some(controller) = self.controller() {
                    crate::host::host().open_terminal(controller.id(), &self.cwd);
                }
            }
        }
    }

    fn launch_program(&self) -> Option<String> {
        let agent = self.agent.acp_agent()?;
        let env = &term_ui::workspace_host::environment_for(&self.cwd, std::time::Duration::ZERO)[..];
        launch_of(&agent, env, &self.cwd, self.controller.is_trusted()).map(|l| l.program.display().to_string())
    }

    /// The model and mode pills, the sandbox switch and its explanation, from what the agent offers.
    pub(super) fn refresh_acp_pills(&self, agent: &AcpAgent) {
        let state = self.state.borrow();
        let caps = state.acp_caps().cloned().unwrap_or_default();
        let model = caps
            .model()
            .map_or_else(|| t("Modell des Agenten").to_string(), |m| m.current_name());
        let pill = match caps.thought_level() {
            Some(level) => tf("{} · {}", &[&model, &acp_words::current(level)]),
            None => model,
        };
        self.composer.model.setTitle(&ns(&pill));
        self.composer
            .model
            .setEnabled(caps.model().is_some() || caps.thought_level().is_some());
        let mode = caps
            .mode
            .as_ref()
            .and_then(|id| caps.modes.iter().find(|m| m.id == *id))
            .map_or_else(
                || t("Standard").to_string(),
                |m| acp_words::mode(&agent.id, &m.id).map_or_else(|| m.name.clone(), |(name, _)| name.to_string()),
            );
        self.composer.mode.setTitle(&ns(&mode));
        self.composer.mode.setEnabled(!caps.modes.is_empty());
        self.composer
            .mode
            .setImage(crate::common::util::symbol("slider.horizontal.3", &mode).as_deref());
        let available = agent.sandbox.is_available();
        let (symbol, tint, tip) = match (available, state.sandbox()) {
            (false, _) => (
                "shield.slash",
                NSColor::tertiaryLabelColor(),
                sandbox_unavailable(&agent.name),
            ),
            (true, true) => (
                "checkmark.shield.fill",
                NSColor::systemGreenColor(),
                tf("Sandbox an: {}", &[&effect_text(agent.effect)]),
            ),
            (true, false) => (
                "shield.slash",
                NSColor::secondaryLabelColor(),
                tf("Sandbox aus; eingeschaltet gilt: {}", &[&effect_text(agent.effect)]),
            ),
        };
        self.composer.sandbox.setEnabled(available);
        self.composer
            .sandbox
            .setImage(crate::common::util::symbol(symbol, &tip).as_deref());
        self.composer.sandbox.setContentTintColor(Some(&tint));
        self.composer.sandbox.setToolTip(Some(&ns(&tip)));
    }

    /// The menu of the model pill: the agent's models, grouped as it groups them, and how much it thinks.
    pub(super) fn acp_model_items(&self) -> Retained<NSMenu> {
        let menu = NSMenu::new(self.mtm);
        menu.setAutoenablesItems(false);
        let caps = self.state.borrow().acp_caps().cloned().unwrap_or_default();
        let mut offered = Vec::new();
        for option in [caps.model(), caps.thought_level()].into_iter().flatten() {
            if !offered.is_empty() {
                menu.addItem(&objc2_app_kit::NSMenuItem::separatorItem(self.mtm));
            }
            let heading = self.menu_item(&menu, &acp_words::heading(option), sel!(ideAgentPickModel:), -1);
            heading.setEnabled(false);
            let OptionValue::Select { current, choices } = &option.value else {
                continue;
            };
            let mut group = None;
            for choice in choices {
                if choice.group != group
                    && let Some(name) = &choice.group
                {
                    let label = self.menu_item(&menu, name, sel!(ideAgentPickModel:), -1);
                    label.setEnabled(false);
                }
                group.clone_from(&choice.group);
                let tag = isize::try_from(offered.len()).unwrap_or(-1);
                let name = acp_words::choice(option, &choice.value, &choice.name);
                let entry = self.menu_item(&menu, &name, sel!(ideAgentPickModel:), tag);
                if let Some(description) = &choice.description {
                    entry.setToolTip(Some(&ns(description)));
                }
                if choice.value == *current {
                    entry.setState(objc2_app_kit::NSControlStateValueOn);
                }
                offered.push(format!("{}\u{1f}{}", option.id, choice.value));
            }
        }
        self.state.borrow_mut().offered_mut().models = offered;
        menu
    }

    pub(super) fn acp_pick_model(&self, index: usize) {
        let Some(picked) = self.state.borrow().offered().models.get(index).cloned() else {
            return;
        };
        let Some((option, value)) = picked.split_once('\u{1f}') else {
            return;
        };
        self.acp_set_option(option, value);
    }

    /// Sets the agent's option `option`, its model or how much it thinks, to `value`, and remembers it for the
    /// agent's next sessions.
    fn acp_set_option(&self, option: &str, value: &str) {
        let result = self.state.borrow().source().set_option(option, value);
        if let Err(error) = result {
            return crate::common::diag::report("choosing a model", &error);
        }
        let caps = self.state.borrow().acp_caps().cloned().unwrap_or_default();
        let effort = caps.thought_level().is_some_and(|o| o.id == option);
        super::choices::remember(self.agent.id(), |choice| {
            let slot = if effort { &mut choice.effort } else { &mut choice.model };
            *slot = Some(value.to_string());
        });
    }

    /// Puts a session that just told its options on the model, thought level and mode the user picked last for the
    /// agent, those it offers; once per session.
    pub(super) fn acp_apply_choice(&self) {
        let Some(caps) = self.state.borrow_mut().apply_picks() else {
            return;
        };
        let choice = super::choices::of(self.agent.id());
        for (option, wanted) in [(caps.model(), choice.model), (caps.thought_level(), choice.effort)] {
            let (Some(option), Some(wanted)) = (option, wanted) else {
                continue;
            };
            let OptionValue::Select { current, choices } = &option.value else {
                continue;
            };
            if *current != wanted && choices.iter().any(|c| c.value == wanted) {
                let result = self.state.borrow().source().set_option(&option.id, &wanted);
                if let Err(error) = result {
                    crate::common::diag::report("choosing the model picked last", &error);
                }
            }
        }
        if let Some(mode) = choice.mode
            && caps.mode.as_ref() != Some(&mode)
            && caps.modes.iter().any(|m| m.id == mode)
        {
            let result = self.state.borrow().source().set_acp_mode(&mode);
            if let Err(error) = result {
                crate::common::diag::report("choosing the mode picked last", &error);
            }
        }
    }

    /// The menu of the mode pill: the agent's modes.
    pub(super) fn acp_mode_items(&self) -> Retained<NSMenu> {
        let menu = NSMenu::new(self.mtm);
        menu.setAutoenablesItems(false);
        let caps = self.state.borrow().acp_caps().cloned().unwrap_or_default();
        let agent = self.agent.id().to_string();
        for (n, mode) in caps.modes.iter().enumerate() {
            let own = acp_words::mode(&agent, &mode.id);
            let name = own.map_or_else(|| mode.name.clone(), |(name, _)| name.to_string());
            let entry = self.menu_item(&menu, &name, sel!(ideAgentPickMode:), isize::try_from(n).unwrap_or(-1));
            if let Some(description) = own
                .map(|(_, words)| words.to_string())
                .or_else(|| mode.description.clone())
            {
                entry.setToolTip(Some(&ns(&description)));
            }
            if caps.mode.as_ref() == Some(&mode.id) {
                entry.setState(objc2_app_kit::NSControlStateValueOn);
            }
        }
        self.state.borrow_mut().offered_mut().modes = caps.modes.iter().map(|m| m.id.clone()).collect();
        menu
    }

    pub(super) fn acp_pick_mode(&self, index: usize) {
        let Some(mode) = self.state.borrow().offered().modes.get(index).cloned() else {
            return;
        };
        self.acp_set_mode(mode);
    }

    /// Switches the agent to the mode `mode` and remembers it for the agent's next sessions.
    fn acp_set_mode(&self, mode: String) {
        let result = self.state.borrow().source().set_acp_mode(&mode);
        if let Err(error) = result {
            return crate::common::diag::report("choosing a mode", &error);
        }
        super::choices::remember(self.agent.id(), |choice| choice.mode = Some(mode));
    }

    /// The commands the agent offers, for the slash menu.
    pub(super) fn acp_commands(&self) -> Vec<(String, String)> {
        self.state
            .borrow()
            .acp_caps()
            .map(|caps| {
                caps.commands
                    .iter()
                    .map(|c| (c.name.clone(), c.description.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Answers permission request `id` with the option the user's choice stands for: allowing once, allowing
    /// always (the rule `rule`, when given), refusing.
    pub(super) fn acp_permission(&self, id: u64, what: u8, rule: Option<usize>) {
        let Some(entry) = self.entry(id) else { return };
        let EntryKind::PermissionRequest(request) = &entry.kind else {
            return;
        };
        if request.state != RequestState::Pending {
            return;
        }
        let index = match (what, rule) {
            (_, Some(rule)) => Some(rule),
            (ALLOW_ONCE, None) => request.options.iter().position(|o| *o == PermissionOption::AllowOnce),
            (ALLOW_ALWAYS, None) => request
                .options
                .iter()
                .position(|o| matches!(o, PermissionOption::AllowAlways { .. })),
            _ => request.options.iter().position(|o| *o == PermissionOption::Deny),
        };
        let result = self.state.borrow().source().answer_option(&request.request_id, index);
        if let Err(error) = result {
            crate::common::diag::report("answering a permission request", &error);
        }
    }
}

/// What a session that did not start shows: an agent that refuses an untrusted workspace offers to trust it, every
/// other error says what went wrong in the words `words` finds for it.
pub(crate) fn launch_problem(error: LaunchError, words: fn(LaunchError) -> String) -> Problem {
    match error {
        LaunchError::Untrusted(why) => Problem::Untrusted(why),
        other => Problem::Start(words(other)),
    }
}

/// Why the agent's session did not start, as the ACP session's own error words it.
fn start_problem(error: LaunchError) -> String {
    use term_base::explain::Explain;
    match error {
        LaunchError::NotFound(program) => term_acp::SessionError::NotFound(program).explain(),
        LaunchError::Start(why) => term_acp::SessionError::Start(why).explain(),
        other => other.to_string(),
    }
}

/// What an agent whose program ended on its own shows, from the end of its stderr: an exit once its session had
/// `opened`, else a start that failed.
fn ended_problem(opened: bool, stderr: &str) -> Problem {
    let last = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string);
    if opened {
        return Problem::Exited(last);
    }
    Problem::Start(last.map_or_else(
        || t("Das Programm hat sich ohne Meldung beendet.").to_string(),
        |why| tf("Das Programm hat sich beim Start beendet: {}", &[&why]),
    ))
}

/// What switching the sandbox on limits for an agent with `effect`.
pub(crate) fn effect_text(effect: Effect) -> String {
    match effect {
        Effect::ShellCommands => t("Shell-Befehle laufen abgeschirmt; Datei- und Webwerkzeuge nicht").to_string(),
        Effect::WorkspaceWrite => t("Befehle und Änderungen schreiben nur im Projekt, ohne Netzwerk").to_string(),
        Effect::ProjectWritesOnly => {
            t("Schreiben nur im Projekt; Lesen überall und Netzwerk bleiben offen").to_string()
        }
        Effect::None => t("Keine Sandbox").to_string(),
    }
}

fn sandbox_unavailable(name: &str) -> String {
    tf("{} bietet keine Sandbox, die sich hier einschalten lässt", &[name])
}

#[cfg(test)]
mod colour_tests {
    use super::*;

    #[test]
    fn every_shipped_agent_has_the_colour_the_owner_gave_it() {
        assert_eq!(brand("claude"), Some(super::super::style::TERRACOTTA));
        assert_eq!(brand("codex-acp"), brand("codex"), "the agent and its CLI look alike");
        assert!(brand("opencode").is_some());
        assert_eq!(brand("someone-else"), None);
    }

    #[test]
    fn only_googles_agents_show_four_colours() {
        assert!(four_colours_of("antigravity-acp").is_some());
        assert!(four_colours_of("agy").is_some());
        assert!(four_colours_of("codex-acp").is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_names_its_agent_by_the_registers_id() {
        assert_eq!(Agent::of_thread(term_claude::AGENT), Agent::Claude);
        let codex = Agent::of_thread("codex-acp");
        assert_eq!(codex, Agent::Codex);
        assert_eq!(
            Agent::of_thread(term_codex::AGENT),
            Agent::Codex,
            "a thread of Codex's own server"
        );
        assert_eq!(Agent::of_thread("opencode"), Agent::OpenCode);
        assert!(codex.is_acp() && !Agent::Claude.is_acp());
        assert_eq!((codex.id(), codex.name()), ("codex-acp", "Codex"));
        assert_eq!(agent_name("gemini"), "Gemini");
        assert_eq!(
            Agent::of_thread("antigravity-acp").preset().map(|p| p.on_path.0),
            Some("agy")
        );
        assert!(
            AcpAgent::presets()
                .iter()
                .all(|a| !["claude-acp", "codex-acp", "opencode", "gemini"].contains(&a.id())),
            "Claude, Codex and OpenCode are agents of their own, and Gemini is not offered"
        );
    }

    #[test]
    fn colours_of_the_register_are_read_as_six_hex_digits() {
        assert_eq!(rgb_of("#1F6FEB"), Some(crate::style::Rgb(0x1F, 0x6F, 0xEB)));
        assert_eq!(rgb_of("1F6FEB"), None);
        assert_eq!(rgb_of("#FFF"), None);
        assert_eq!(rgb_of("#GGGGGG"), None);
    }

    #[test]
    fn an_agent_that_ends_on_its_own_says_so_with_its_last_words() {
        assert_eq!(
            ended_problem(true, "loading\nthe model crashed\n\n"),
            Problem::Exited(Some("the model crashed".into()))
        );
        assert_eq!(ended_problem(true, "  \n"), Problem::Exited(None));
        assert_eq!(
            ended_problem(false, "error: no API key\n"),
            Problem::Start("Das Programm hat sich beim Start beendet: error: no API key".into())
        );
        assert_eq!(
            ended_problem(false, ""),
            Problem::Start("Das Programm hat sich ohne Meldung beendet.".into())
        );
    }

    #[test]
    fn the_sandbox_says_what_it_limits() {
        let effects = [
            Effect::ShellCommands,
            Effect::WorkspaceWrite,
            Effect::ProjectWritesOnly,
            Effect::None,
        ]
        .map(effect_text);
        assert_eq!(effects[3], "Keine Sandbox");
        assert_eq!(
            effects.iter().collect::<std::collections::BTreeSet<_>>().len(),
            4,
            "each effect has words of its own"
        );
        assert_eq!(
            sandbox_unavailable("OpenCode"),
            "OpenCode bietet keine Sandbox, die sich hier einschalten lässt"
        );
    }
}
