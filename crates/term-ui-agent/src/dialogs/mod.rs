//! The overlays of an agent tab for the agent's configuration: `/status`, `/export`, `/mcp` and the others the
//! CLI shows only as terminal dialogs. Each function has one overlay for every agent, built from the neutral data in
//! `model`; the agent's backend fetches and writes, and says what it can do (`backend`).

mod account;
mod aside;
mod backend;
mod commands;
mod config;
mod export;
mod feedback;
mod frame;
mod hooks;
mod mcp;
mod memory;
pub(crate) mod model;
mod permissions;
mod plugins;
mod resume;
mod rows;
#[cfg(feature = "selftest")]
mod selftest_probe;
mod skills;
mod status;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use crate::common::util::main_soon;
use dispatch2::MainThreadBound;
use objc2::MainThreadMarker;
use splat_i18n::t;

use crate::tab::AgentTab;
pub use backend::remove_claude_rule;

/// The MCP servers an ACP session in the trusted workspace `cwd` gets when it starts.
pub(crate) fn mcp_for_launch(cwd: &std::path::Path, env: &[(String, String)]) -> Vec<term_acp::mcp::McpServerSpec> {
    backend::acp_mcp::servers(cwd, env, true)
}
pub(crate) use commands::with_local;
use frame::Frame;

/// An overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Dialog {
    /// `/status`.
    Status,
    /// `/export`.
    Export,
    /// `/mcp`.
    Mcp,
    /// `/permissions`.
    Permissions,
    /// `/hooks`.
    Hooks,
    /// `/memory`.
    Memory,
    /// `/config`, `/settings` and `/output-style`.
    Config,
    /// `/skills`.
    Skills,
    /// `/plugins` and `/plugin`.
    Plugins,
    /// `/logout` and `/login`.
    Account,
    /// `/bug` and `/feedback`.
    Feedback,
    /// `/btw`, with the question typed after it.
    Aside(String),
    /// `/resume`.
    Resume,
}

impl Dialog {
    /// The overlay a command typed alone in the composer opens, such as `/status`; none for anything else, which
    /// goes to the agent as typed.
    pub(crate) fn from_typed(typed: &str) -> Option<Dialog> {
        match typed.trim() {
            "/status" => Some(Dialog::Status),
            "/export" => Some(Dialog::Export),
            "/mcp" => Some(Dialog::Mcp),
            "/permissions" => Some(Dialog::Permissions),
            "/hooks" => Some(Dialog::Hooks),
            "/memory" => Some(Dialog::Memory),
            "/config" | "/settings" | "/output-style" => Some(Dialog::Config),
            "/skills" => Some(Dialog::Skills),
            "/plugins" | "/plugin" => Some(Dialog::Plugins),
            "/logout" | "/login" => Some(Dialog::Account),
            "/bug" | "/feedback" => Some(Dialog::Feedback),
            "/resume" => Some(Dialog::Resume),
            typed => {
                let rest = typed.strip_prefix("/btw")?;
                (rest.is_empty() || rest.starts_with(char::is_whitespace))
                    .then(|| Dialog::Aside(rest.trim().to_string()))
            }
        }
    }

    /// The command that opens it, without the slash.
    fn command(&self) -> &'static str {
        match self {
            Dialog::Status => "status",
            Dialog::Export => "export",
            Dialog::Mcp => "mcp",
            Dialog::Permissions => "permissions",
            Dialog::Hooks => "hooks",
            Dialog::Memory => "memory",
            Dialog::Config => "config",
            Dialog::Skills => "skills",
            Dialog::Plugins => "plugins",
            Dialog::Account => "logout",
            Dialog::Feedback => "bug",
            Dialog::Aside(_) => "btw",
            Dialog::Resume => "resume",
        }
    }

    /// The overlay's title.
    fn title(&self) -> &'static str {
        match self {
            Dialog::Status => t("Status"),
            Dialog::Export => t("Unterhaltung exportieren"),
            Dialog::Mcp => t("MCP-Server"),
            Dialog::Permissions => t("Berechtigungen"),
            Dialog::Hooks => t("Hooks"),
            Dialog::Memory => t("Gedächtnis und Anweisungen"),
            Dialog::Config => t("Einstellungen des Agenten"),
            Dialog::Skills => t("Skills"),
            Dialog::Plugins => t("Plugins"),
            Dialog::Account => t("Konto"),
            Dialog::Feedback => t("Fehler melden"),
            Dialog::Aside(_) => t("Frage nebenbei"),
            Dialog::Resume => t("Sitzung fortsetzen"),
        }
    }

    /// The symbol before its title.
    fn symbol(&self) -> &'static str {
        match self {
            Dialog::Status => "info.circle",
            Dialog::Export => "square.and.arrow.up",
            Dialog::Mcp => "server.rack",
            Dialog::Permissions => "lock.shield",
            Dialog::Hooks => "link",
            Dialog::Memory => "brain",
            Dialog::Config => "gearshape",
            Dialog::Skills => "wand.and.stars",
            Dialog::Plugins => "puzzlepiece.extension",
            Dialog::Account => "person.crop.circle",
            Dialog::Feedback => "ladybug",
            Dialog::Aside(_) => "bubble.left.and.text.bubble.right",
            Dialog::Resume => "clock.arrow.circlepath",
        }
    }
}

/// Whether `tab`'s agent offers `dialog`, as its backend can fill it, and does not take the command itself.
pub(crate) fn offers(tab: &AgentTab, dialog: &Dialog) -> bool {
    let name = dialog.command();
    let (caps, codex) = {
        let state = tab.state.borrow();
        (state.acp_caps().cloned(), state.source().codex().is_some())
    };
    commands::local_commands(&tab.agent, caps.as_ref(), codex)
        .iter()
        .any(|(offered, _)| offered == name)
        && !tab.agent_commands().iter().any(|(own, _)| own == name)
}

/// What a button of an overlay does.
type Action = Rc<dyn Fn(&AgentTab)>;

/// An open overlay of a tab.
pub(crate) struct Open {
    pub(crate) dialog: Dialog,
    pub(crate) frame: Frame,
    /// Tells answers for this overlay from answers for one closed before.
    pub(crate) generation: u64,
    /// The tab it is open over.
    pub(crate) tab: Weak<AgentTab>,
    /// Whether it waits for the session to start, to fill itself then.
    waiting: Cell<bool>,
    /// What each button does, by its tag.
    actions: RefCell<Vec<Action>>,
    /// The form's fields, by their key.
    fields: RefCell<Vec<(&'static str, objc2::rc::Retained<objc2_app_kit::NSControl>)>>,
}

impl Open {
    /// Keeps `control` as the form's field `key`.
    pub(crate) fn keep_field(&self, key: &'static str, control: &objc2_app_kit::NSControl) {
        self.fields.borrow_mut().push((key, objc2::Message::retain(control)));
    }

    /// The text of the form's field `key`, trimmed; empty when there is none.
    pub(crate) fn text(&self, key: &str) -> String {
        self.fields
            .borrow()
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, control)| control.stringValue().to_string().trim().to_string())
            .unwrap_or_default()
    }

    /// The index chosen in the form's pop-up `key`.
    pub(crate) fn chosen(&self, key: &str) -> Option<usize> {
        let fields = self.fields.borrow();
        let (_, control) = fields.iter().find(|(k, _)| *k == key)?;
        let object: &objc2::runtime::AnyObject = control;
        let popup = object.downcast_ref::<objc2_app_kit::NSPopUpButton>()?;
        usize::try_from(popup.indexOfSelectedItem()).ok()
    }

    /// Forgets the form's fields, before the overlay shows other rows.
    pub(crate) fn clear_fields(&self) {
        self.fields.borrow_mut().clear();
    }

    /// Keeps `act` for a button; the tag to give it.
    pub(crate) fn action(&self, act: impl Fn(&AgentTab) + 'static) -> isize {
        let mut actions = self.actions.borrow_mut();
        actions.push(Rc::new(act));
        isize::try_from(actions.len() - 1).unwrap_or(-1)
    }
}

/// Runs the action of the button tagged `tag` in the open overlay of `tab`.
pub(crate) fn run_action(tab: &AgentTab, tag: isize) {
    let act = tab.dialog.borrow().as_ref().and_then(|open| {
        let index = usize::try_from(tag).ok()?;
        open.actions.borrow().get(index).cloned()
    });
    if let Some(act) = act {
        act(tab);
    }
}

/// Calls `then` on the main thread with the tab of `open` and `value`, when `open` is still the tab's overlay.
pub(crate) fn answer<T: Send + 'static>(
    mtm: MainThreadMarker,
    open: &Open,
    then: impl FnOnce(&AgentTab, &Open, T) + 'static,
) -> impl FnOnce(T) + Send + 'static {
    let generation = open.generation;
    let bound = MainThreadBound::new((open.tab.clone(), then), mtm);
    move |value| {
        main_soon(move || {
            let Some(mtm) = MainThreadMarker::new() else { return };
            let (tab, then) = bound.into_inner(mtm);
            let Some(tab) = tab.upgrade() else { return };
            let open = tab.dialog.borrow().clone();
            if let Some(open) = open.filter(|open| open.generation == generation) {
                then(&tab, &open, value);
            }
        });
    }
}

impl AgentTab {
    /// Opens `dialog` over the tab, in place of an overlay open before.
    pub(crate) fn open_dialog(&self, dialog: Dialog) {
        self.close_dialog();
        let weak = self.me.borrow().clone();
        let frame = Frame::new(self.mtm, weak.clone(), dialog.title(), dialog.symbol());
        let generation = self.dialogs_opened.get() + 1;
        self.dialogs_opened.set(generation);
        let open = Rc::new(Open {
            dialog: dialog.clone(),
            frame,
            generation,
            tab: weak,
            waiting: Cell::new(false),
            actions: RefCell::new(Vec::new()),
            fields: RefCell::new(Vec::new()),
        });
        open.frame.show(&self.root());
        open.frame
            .set_asking(!self.state.borrow().thread().pending_requests().is_empty());
        *self.dialog.borrow_mut() = Some(open.clone());
        let backend = backend::of(self);
        match dialog {
            Dialog::Status => status::fill(self, &open, &*backend),
            Dialog::Export => export::fill(self, &open, &*backend),
            Dialog::Mcp => mcp::fill(self, &open, &*backend),
            Dialog::Permissions => permissions::fill(self, &open, &*backend),
            Dialog::Hooks => hooks::fill(self, &open, &*backend),
            Dialog::Memory => memory::fill(self, &open, &*backend),
            Dialog::Config => config::fill(self, &open, &*backend),
            Dialog::Skills => skills::fill(self, &open, &*backend),
            Dialog::Plugins => plugins::fill(self, &open, &*backend),
            Dialog::Account => account::fill(self, &open, &*backend),
            Dialog::Feedback => feedback::fill(self, &open, &*backend),
            Dialog::Aside(question) => aside::fill(self, &open, &*backend, &question),
            Dialog::Resume => resume::fill(self, &open),
        }
    }

    /// Closes the tab's overlay, if one is open, and gives the keyboard focus to the composer, where its command
    /// was typed.
    pub(crate) fn close_dialog(&self) {
        let open = self.dialog.borrow_mut().take();
        if let Some(open) = open {
            open.frame.hide();
            self.focus_composer();
        }
    }

    /// Fills the open overlay again, such as after its session started or a change was written.
    pub(crate) fn refill_dialog(&self) {
        let dialog = self.dialog.borrow().as_ref().map(|open| open.dialog.clone());
        if let Some(dialog) = dialog {
            self.open_dialog(dialog);
        }
    }
}

/// Runs a change through the tab's backend; the overlay loads again once it is done, or says why not.
pub(crate) fn change(tab: &AgentTab, act: impl FnOnce(&dyn backend::Backend, backend::Done<()>)) {
    let Some(open) = tab.dialog.borrow().clone() else {
        return;
    };
    open.frame.set_notice(Some(t("Wird ausgeführt …")));
    let done = answer(
        tab.mtm,
        &open,
        |tab, open, result: Result<(), term_base::explain::Explained>| match result {
            Ok(()) => tab.refill_dialog(),
            Err(why) => open.frame.set_notice(Some(&why.0)),
        },
    );
    act(&*backend::of(tab), Box::new(done));
}

/// Whether the overlay `open` can ask `backend` now: with its session running. Otherwise it says so and offers to
/// start the session, after which it fills itself (`session_ready`).
pub(crate) fn needs_session(tab: &AgentTab, open: &Open, backend: &dyn backend::Backend) -> bool {
    if backend.live() {
        return true;
    }
    let mtm = tab.mtm;
    open.waiting.set(true);
    // A session that opens, or waits for a sign-in, is no session to start again: its process runs.
    let starting = {
        use crate::source::Source;
        match tab.state.borrow().source() {
            Source::Starting => true,
            Source::Session(driven) => {
                driven.acp().is_some_and(|session| !session.is_ended())
                    || driven.codex().is_some_and(|session| !session.is_ended())
                    || driven.opencode().is_some_and(|session| !session.is_ended())
            }
            _ => false,
        }
    };
    if starting {
        open.frame.set_rows(&[rows::waiting(mtm)]);
        open.frame.set_notice(Some(t("Startet die Sitzung …")));
        return false;
    }
    open.frame.set_rows(&[rows::text(
        mtm,
        t("Die Sitzung läuft nicht. Starte sie, um das hier zu sehen."),
    )]);
    let start = rows::button(mtm, open, t("Sitzung starten"), |tab| {
        let idle = matches!(tab.state.borrow().source(), crate::source::Source::Idle);
        if idle {
            tab.start();
        }
        tab.refill_dialog();
    });
    open.frame.set_buttons(&[crate::ui::view(&start)]);
    false
}

impl AgentTab {
    /// Ends the session and starts it again, continuing its conversation, so what applies from the next start
    /// applies now; not while a turn runs.
    pub(crate) fn restart_session(&self) {
        let running = self.state.borrow().thread().status() != term_thread::Status::Idle;
        if running {
            if let Some(open) = self.dialog.borrow().clone() {
                open.frame.set_notice(Some(t("Erst wenn der Agent fertig ist.")));
            }
            return;
        }
        let source = self.state.borrow_mut().end();
        source.end();
        self.start();
        self.refill_dialog();
    }
}

/// Says in the open overlay of `tab` whether the agent waits for an answer in the conversation.
pub(crate) fn requests_changed(tab: &AgentTab) {
    let Some(open) = tab.dialog.borrow().clone() else {
        return;
    };
    let waits = !tab.state.borrow().thread().pending_requests().is_empty();
    open.frame.set_asking(waits);
}

/// The session of `tab` is ready: an overlay that waited for it fills itself now.
pub(crate) fn session_ready(tab: &AgentTab) {
    let waiting = tab.dialog.borrow().as_ref().is_some_and(|open| open.waiting.get());
    if waiting {
        tab.refill_dialog();
    }
}

/// Opens a terminal in the tab's folder with `claude` on the clipboard, for what this version of the CLI shows only
/// there.
pub(crate) fn open_terminal(tab: &AgentTab) {
    let pasteboard = objc2_app_kit::NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    pasteboard.setString_forType(
        &crate::common::util::ns("claude"),
        crate::common::constant!(objc2_app_kit::NSPasteboardTypeString),
    );
    if let Some(controller) = tab.controller() {
        crate::host::host().open_terminal(controller.id(), &tab.cwd);
        term_ui::workspace_host::Notices::show_notice(&controller, t("„claude“ liegt in der Zwischenablage."));
    }
    tab.close_dialog();
}

#[cfg(test)]
mod tests {
    use super::Dialog;

    #[test]
    fn a_command_alone_opens_its_overlay_and_btw_takes_its_question() {
        assert_eq!(Dialog::from_typed(" /mcp "), Some(Dialog::Mcp));
        assert_eq!(Dialog::from_typed("/mcp list"), None);
        assert_eq!(Dialog::from_typed("/btw"), Some(Dialog::Aside(String::new())));
        assert_eq!(
            Dialog::from_typed("/btw  Warum rot? "),
            Some(Dialog::Aside("Warum rot?".into()))
        );
        assert_eq!(Dialog::from_typed("/btwx"), None);
        assert_eq!(Dialog::from_typed("/resume"), Some(Dialog::Resume));
    }
}
