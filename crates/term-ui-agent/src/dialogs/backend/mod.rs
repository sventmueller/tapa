//! Where the overlays get their data and send their changes: one backend per kind of agent. A backend maps its
//! protocol onto the neutral model and has no views; an overlay never asks which agent it serves, only what the
//! backend can do. What a backend leaves out answers that this agent cannot do it.

mod acp;
pub(crate) mod acp_mcp;
mod claude;
mod claude_map;
mod claude_plugins;
mod codex;
mod opencode;

pub use claude::remove_claude_rule;

use term_base::explain::Explained;

use super::model::{
    Account, Aside, Behavior, Blocked, Exported, FeedbackKind, Hook, HookDraft, Hooks, MarketChange, McpCaps, McpDraft,
    McpServer, Memory, PermissionCaps, PermissionRule, Permissions, PluginChange, PluginStep, Plugins, Scope, Settings,
    Skill, StatusSection,
};
use crate::acp::Agent;
use crate::tab::AgentTab;

/// How an answer comes back: on a worker thread, with what went wrong in the user's words.
pub(crate) type Done<T> = Box<dyn FnOnce(Result<T, Explained>) + Send>;

/// What an agent's overlays can ask.
pub(crate) trait Backend {
    /// Whether its session runs, which the overlays that ask the session need.
    fn live(&self) -> bool;

    /// The rows of `/status`.
    fn status(&self, done: Done<Vec<StatusSection>>) {
        done(Err(unavailable()));
    }

    /// The conversation as text.
    fn export(&self, done: Done<Exported>) {
        done(Err(unavailable()));
    }

    /// The plugins and marketplaces; `available` also the plugins the marketplaces offer.
    fn plugins(&self, done: Done<Plugins>) {
        done(Err(unavailable()));
    }

    /// Installs the plugin `id` in `scope`, or updates it where `update`; `accept` is the hash of the marketplace's
    /// command the user saw and accepted.
    fn plugin_install(&self, _id: &str, _scope: Scope, _update: bool, _accept: Option<String>, done: Done<PluginStep>) {
        done(Err(unavailable()));
    }

    /// Changes the plugin `id` installed in `scope`.
    fn plugin_change(&self, _id: &str, _scope: Scope, _change: PluginChange, done: Done<()>) {
        done(Err(unavailable()));
    }

    /// Changes the marketplace `name`, or adds one from the source `name`.
    fn market_change(&self, _name: &str, _change: MarketChange, done: Done<()>) {
        done(Err(unavailable()));
    }

    /// Who is signed in.
    fn account(&self, done: Done<Account>) {
        done(Err(unavailable()));
    }

    /// Signs out of the agent.
    fn sign_out(&self, done: Done<()>) {
        done(Err(unavailable()));
    }

    /// What a report sends and to whom, which the user confirms first; none where the agent takes no reports.
    fn feedback_sends(&self) -> Option<&'static str> {
        None
    }

    /// Sends a report; what became of it, in the user's words.
    fn send_feedback(&self, _kind: FeedbackKind, _title: &str, _description: &str, done: Done<String>) {
        done(Err(unavailable()));
    }

    /// Sets the setting `key` to the value `value`, one of its choices.
    fn set_setting(&self, _key: &str, _value: &str, done: Done<()>) {
        done(Err(unavailable()));
    }

    /// Asks `question` beside the conversation, after the questions `before`.
    fn aside(&self, _question: &str, _before: &[Aside], done: Done<Aside>) {
        done(Err(unavailable()));
    }

    /// What `/mcp` can do.
    fn mcp_caps(&self) -> McpCaps {
        let no = cannot();
        McpCaps {
            live: no,
            toggle: no,
            reconnect: no,
            sign_in: no,
            add: no,
            remove: no,
            restart: no,
            transports: Vec::new(),
            scopes: Vec::new(),
        }
    }

    /// The MCP servers.
    fn mcp_servers(&self, done: Done<Vec<McpServer>>) {
        done(Err(unavailable()));
    }

    /// Switches the server `name` on or off.
    fn mcp_toggle(&self, _name: &str, _enabled: bool, done: Done<()>) {
        done(Err(unavailable()));
    }

    /// Connects the server `name` again.
    fn mcp_reconnect(&self, _name: &str, done: Done<()>) {
        done(Err(unavailable()));
    }

    /// Starts the sign-in to the server `name`; the address to open in the browser, if any.
    fn mcp_sign_in(&self, _name: &str, done: Done<Option<String>>) {
        done(Err(unavailable()));
    }

    /// Hands over the address the browser landed on after a sign-in to the server `name`.
    fn mcp_sign_in_answer(&self, _name: &str, _url: &str, done: Done<()>) {
        done(Err(unavailable()));
    }

    /// Forgets the sign-in to the server `name`.
    fn mcp_sign_out(&self, _name: &str, done: Done<()>) {
        done(Err(unavailable()));
    }

    /// Adds the server `draft` describes.
    fn mcp_add(&self, _draft: McpDraft, done: Done<()>) {
        done(Err(unavailable()));
    }

    /// Removes the server `name` from `scope`.
    fn mcp_remove(&self, _name: &str, _scope: Scope, done: Done<()>) {
        done(Err(unavailable()));
    }

    /// What `/permissions` can do.
    fn permission_caps(&self) -> PermissionCaps {
        PermissionCaps {
            add: cannot(),
            remove: cannot(),
            scopes: Vec::new(),
        }
    }

    /// The permission rules and extra folders.
    fn permissions(&self, done: Done<Permissions>) {
        done(Err(unavailable()));
    }

    /// Adds `rule` with `behavior` in `scope`.
    fn rule_add(&self, _rule: &str, _behavior: Behavior, _scope: Scope, done: Done<()>) {
        done(Err(unavailable()));
    }

    /// Removes `rule`.
    fn rule_remove(&self, _rule: &PermissionRule, done: Done<()>) {
        done(Err(unavailable()));
    }

    /// Why hooks cannot be added or changed here; none when they can.
    fn hooks_blocked(&self) -> Blocked {
        cannot()
    }

    /// The hooks.
    fn hooks(&self, done: Done<Hooks>) {
        done(Err(unavailable()));
    }

    /// Adds the hook `draft` describes.
    fn hook_add(&self, _draft: HookDraft, done: Done<()>) {
        done(Err(unavailable()));
    }

    /// Puts the hook `draft` describes in the place of `old`.
    fn hook_replace(&self, _old: &Hook, _draft: HookDraft, done: Done<()>) {
        done(Err(unavailable()));
    }

    /// Removes `hook`.
    fn hook_remove(&self, _hook: &Hook, done: Done<()>) {
        done(Err(unavailable()));
    }

    /// The instruction files and memory.
    fn memory(&self, done: Done<Memory>) {
        done(Err(unavailable()));
    }

    /// Switches automatic memory on or off.
    fn set_auto_memory(&self, _on: bool, done: Done<()>) {
        done(Err(unavailable()));
    }

    /// The settings in effect.
    fn settings(&self, done: Done<Settings>) {
        done(Err(unavailable()));
    }

    /// Sets the output style.
    fn set_style(&self, _style: &str, done: Done<()>) {
        done(Err(unavailable()));
    }

    /// The skills.
    fn skills(&self, done: Done<Vec<Skill>>) {
        done(Err(unavailable()));
    }

    /// Switches `skill` on or off.
    fn set_skill(&self, _skill: &Skill, _on: bool, done: Done<()>) {
        done(Err(unavailable()));
    }
}

/// The reason of an action the agent cannot do.
pub(crate) fn cannot() -> Blocked {
    Some(splat_i18n::t("Dieser Agent kann das hier nicht."))
}

/// The backend of `tab`'s agent.
pub(crate) fn of(tab: &AgentTab) -> Box<dyn Backend> {
    if matches!(tab.agent, Agent::Claude) {
        return Box::new(claude::Claude::new(tab));
    }
    let (codex, opencode) = {
        let state = tab.state.borrow();
        (state.source().codex(), state.source().opencode())
    };
    match (codex, opencode) {
        (Some(session), _) => Box::new(codex::Codex::new(tab, session)),
        (None, Some(session)) => Box::new(opencode::OpenCode::new(tab, session)),
        (None, None) => Box::new(acp::Acp::new(tab, tab.agent.name())),
    }
}

pub(crate) fn unavailable() -> Explained {
    Explained(splat_i18n::t("Dieser Agent kann das hier nicht.").to_string())
}

/// Runs `work` on the work lane and answers through `done`, also when the lane is full or `work` panics.
pub(super) fn on_work<T: Send + 'static>(work: impl FnOnce() -> Result<T, Explained> + Send + 'static, done: Done<T>) {
    let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(done)));
    let answering = slot.clone();
    let asked = term_base::lane::WORK.ask(work, move |answer| {
        if let Some(done) = term_base::LockIgnoringPoison::lock_ignoring_poison(&*answering).take() {
            done(answer.unwrap_or_else(|| Err(unavailable())));
        }
    });
    if asked.is_err()
        && let Some(done) = term_base::LockIgnoringPoison::lock_ignoring_poison(&*slot).take()
    {
        done(Err(Explained(
            splat_i18n::t("Die App ist gerade ausgelastet. Versuch es gleich noch einmal.").to_string(),
        )));
    }
}

/// The file name an exported conversation is offered under: its title without what a file name cannot hold, or a
/// plain word for a conversation without a title.
pub(super) fn export_name(thread: &term_thread::Thread) -> String {
    let title = thread.title().replace(['/', ':'], "-");
    let title = title.trim().trim_start_matches('.');
    if title.is_empty() {
        format!("{}.txt", splat_i18n::t("Unterhaltung"))
    } else {
        format!("{title}.txt")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, channel};
    use std::time::Duration;

    use serde_json::json;
    use term_thread::{Thread, ThreadId, Update};

    use super::*;
    use crate::dialogs::model::Transport;

    /// A backend that offers nothing beyond what every backend must answer.
    struct Nothing;

    impl Backend for Nothing {
        fn live(&self) -> bool {
            true
        }
    }

    /// `done` and where its answer arrives, reduced to the reason of a failure.
    fn answered<T: 'static>() -> (Done<T>, Receiver<Option<String>>) {
        let (tx, rx) = channel();
        let done: Done<T> = Box::new(move |result| {
            let _ = tx.send(result.err().map(|why| why.0));
        });
        (done, rx)
    }

    /// Whether `call` answers at once that the agent cannot do it.
    fn refuses<T: 'static>(call: impl FnOnce(Done<T>)) -> bool {
        let (done, rx) = answered();
        call(done);
        rx.try_recv() == Ok(Some(unavailable().0))
    }

    fn hook() -> Hook {
        Hook {
            event: "PreToolUse".into(),
            matcher: "Bash".into(),
            kind: "command".into(),
            text: "true".into(),
            place: "settings.json".into(),
            source: "user".into(),
            definition: None,
        }
    }

    fn hook_draft() -> HookDraft {
        HookDraft {
            event: "PreToolUse".into(),
            matcher: String::new(),
            command: "true".into(),
            timeout: None,
            scope: Scope::User,
        }
    }

    fn mcp_draft() -> McpDraft {
        McpDraft {
            name: "docs".into(),
            transport: Transport::Stdio,
            command: vec!["true".into()],
            url: String::new(),
            env: Vec::new(),
            headers: Vec::new(),
            scope: Scope::User,
        }
    }

    fn skill() -> Skill {
        Skill {
            name: "review".into(),
            shown: "review".into(),
            description: String::new(),
            source: "user".into(),
            tokens: None,
            enabled: true,
            handles: json!({}),
        }
    }

    fn rule() -> PermissionRule {
        PermissionRule {
            behavior: Behavior::Allow,
            rule: "Bash(ls)".into(),
            source: "user".into(),
            place: "settings.json".into(),
            wording: None,
            editable: true,
        }
    }

    #[test]
    fn a_backend_that_offers_nothing_refuses_every_question_at_once() {
        let b = Nothing;
        assert!(refuses(|done| b.status(done)));
        assert!(refuses(|done| b.export(done)));
        assert!(refuses(|done| b.plugins(done)));
        assert!(refuses(|done| b.plugin_install("p@m", Scope::User, false, None, done)));
        assert!(refuses(|done| b.plugin_change(
            "p@m",
            Scope::User,
            PluginChange::Disable,
            done
        )));
        assert!(refuses(|done| b.market_change("m", MarketChange::Update, done)));
        assert!(refuses(|done| b.account(done)));
        assert!(refuses(|done| b.sign_out(done)));
        assert!(refuses(|done| b.send_feedback(FeedbackKind::Bug, "t", "d", done)));
        assert!(refuses(|done| b.set_setting("mode", "plan", done)));
        assert!(refuses(|done| b.aside("why?", &[], done)));
        assert!(refuses(|done| b.permissions(done)));
        assert!(refuses(|done| b.rule_add(
            "Bash(ls)",
            Behavior::Ask,
            Scope::Project,
            done
        )));
        assert!(refuses(|done| b.rule_remove(&rule(), done)));
        assert!(refuses(|done| b.hooks(done)));
        assert!(refuses(|done| b.hook_add(hook_draft(), done)));
        assert!(refuses(|done| b.hook_replace(&hook(), hook_draft(), done)));
        assert!(refuses(|done| b.hook_remove(&hook(), done)));
        assert!(refuses(|done| b.memory(done)));
        assert!(refuses(|done| b.set_auto_memory(true, done)));
        assert!(refuses(|done| b.settings(done)));
        assert!(refuses(|done| b.set_style("default", done)));
        assert!(refuses(|done| b.skills(done)));
        assert!(refuses(|done| b.set_skill(&skill(), false, done)));
    }

    #[test]
    fn a_backend_that_offers_nothing_refuses_every_server_change_at_once() {
        let b = Nothing;
        assert!(refuses(|done| b.mcp_servers(done)));
        assert!(refuses(|done| b.mcp_toggle("docs", false, done)));
        assert!(refuses(|done| b.mcp_reconnect("docs", done)));
        assert!(refuses(|done| b.mcp_sign_in("docs", done)));
        assert!(refuses(|done| b.mcp_sign_in_answer(
            "docs",
            "http://localhost/cb",
            done
        )));
        assert!(refuses(|done| b.mcp_sign_out("docs", done)));
        assert!(refuses(|done| b.mcp_add(mcp_draft(), done)));
        assert!(refuses(|done| b.mcp_remove("docs", Scope::Local, done)));
    }

    #[test]
    fn what_a_backend_leaves_out_is_greyed_with_the_reason() {
        let b = Nothing;
        let mcp = b.mcp_caps();
        let blocked = [
            mcp.live,
            mcp.toggle,
            mcp.reconnect,
            mcp.sign_in,
            mcp.add,
            mcp.remove,
            mcp.restart,
        ];
        assert!(blocked.iter().all(|why| *why == cannot() && why.is_some()));
        assert!(mcp.transports.is_empty() && mcp.scopes.is_empty());
        let permissions = b.permission_caps();
        assert_eq!((permissions.add, permissions.remove), (cannot(), cannot()));
        assert!(permissions.scopes.is_empty());
        assert_eq!(b.hooks_blocked(), cannot());
        assert_eq!(b.feedback_sends(), None);
        assert_eq!(cannot(), Some(unavailable().0.as_str()));
    }

    #[test]
    fn work_answers_from_the_work_lane_and_a_panic_answers_that_it_failed() {
        let (tx, rx) = channel();
        on_work(
            || Ok(std::thread::current().name().map(str::to_string)),
            Box::new(move |result| {
                let _ = tx.send(result);
            }),
        );
        let ran = rx.recv_timeout(Duration::from_secs(10)).expect("an answer");
        assert!(ran.is_ok_and(|name| name.is_some_and(|n| n.starts_with("term-work"))));

        let (done, rx) = answered::<()>();
        on_work(|| panic!("the work fails"), done);
        let failed = rx.recv_timeout(Duration::from_secs(10)).expect("an answer");
        assert_eq!(failed, Some(unavailable().0));

        let (done, rx) = answered::<()>();
        on_work(|| Err(Explained("no".into())), done);
        assert_eq!(rx.recv_timeout(Duration::from_secs(10)), Ok(Some("no".into())));
    }

    #[test]
    fn a_conversation_without_a_title_is_exported_under_a_plain_name() {
        let thread = Thread::new(ThreadId::from("t"), "agent", "/p");
        assert!(!super::export_name(&thread).starts_with('.'));
        assert!(super::export_name(&thread).ends_with(".txt"));
    }

    #[test]
    fn an_exported_conversation_is_named_after_its_title_without_what_a_file_name_cannot_hold() {
        let mut thread = Thread::new(ThreadId::from("t"), "agent", "/p");
        thread.apply(Update::Title("  ..fix a/b: now ".into()));
        assert_eq!(super::export_name(&thread), "fix a-b- now.txt");
    }
}
