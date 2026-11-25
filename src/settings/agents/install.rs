//! The Agents pane's ways to install, update and go back, as Dango's settings offer them (E21): each agent's line says
//! where its program is, with a button to install the version the app pins where it is missing, to update to that
//! version or go back to the one before where the app installed it; and the ACP registry, fetched only when the user
//! asks, whose entries install from a window of their own (`register`). Claude has no installer here: it comes with
//! its own (`claude` in the terminal), as in Dango.

use std::cell::{OnceCell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{NSBezelStyle, NSButton, NSControlSize, NSLayoutAttribute, NSStackView, NSTextField, NSView};
use objc2_foundation::{NSArray, NSObjectProtocol};
use splat_i18n::{t, tf};
use term_acp::register::{self, Preset};
use term_acp::{Installs, Plan};
use term_base::guarded;
use term_ui::kit::alert::Alert;
use term_ui::kit::{ns, set_action, set_label, space};
use term_ui_agent::{AcpAgent, Agent};

use super::super::form::secondary;

/// How long the pane waits for the login shell's environment, whose PATH tells where the agents are.
const ENVIRONMENT: Duration = Duration::from_secs(15);

/// What an agent's line shows: where its program is, and the titles of its buttons, none where it has none.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct LineState {
    pub(crate) text: String,
    pub(crate) install: Option<String>,
    pub(crate) rollback: Option<String>,
}

/// An agent's line of the pane.
struct Line {
    agent: Agent,
    state: Retained<NSTextField>,
    install: Retained<NSButton>,
    rollback: Retained<NSButton>,
}

/// The registry's line: what it holds, its buttons, and the agents installed from it.
struct RegisterLine {
    state: Retained<NSTextField>,
    fetch: Retained<NSButton>,
    show: Retained<NSButton>,
    installed: Retained<NSStackView>,
}

#[derive(Default)]
pub(crate) struct Ivars {
    lines: RefCell<Vec<Line>>,
    register: OnceCell<RegisterLine>,
    /// What each installation running now has got to, by the agent's id; the registry's fetch under the empty id.
    progress: RefCell<BTreeMap<String, String>>,
    /// The ids of the agents installed from the registry, in the order their lines show them.
    entries: RefCell<Vec<String>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Installs` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermSettingsAgentInstalls"]
    #[ivars = Ivars]
    pub(crate) struct Target;

    unsafe impl NSObjectProtocol for Target {}

    impl Target {
        #[unsafe(method(termAgentsInstall:))]
        fn install_action(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                let agent = self.line_agent(sender);
                if let Some(preset) = agent.as_ref().and_then(preset_of) {
                    self.install(preset.id, preset.name, Plan::of_preset(preset), true);
                }
            });
        }

        #[unsafe(method(termAgentsRollback:))]
        fn rollback_action(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                if let Some(agent) = self.line_agent(sender) {
                    self.rollback(agent.id(), agent.name());
                }
            });
        }

        #[unsafe(method(termAgentsRollbackEntry:))]
        fn rollback_entry_action(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                let id = self.ivars().entries.borrow().get(index(sender)).cloned();
                if let Some(agent) = id.and_then(|id| AcpAgent::find(&id)) {
                    self.rollback(agent.id(), agent.name());
                }
            });
        }

        #[unsafe(method(termAgentsFetch:))]
        fn fetch_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.fetch());
        }

        #[unsafe(method(termAgentsShowRegister:))]
        fn show_register_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || super::register::show(self));
        }

        #[unsafe(method(termAgentsInstallEntry:))]
        fn install_entry_action(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                let entry = super::register::shown_entry(index(sender));
                if let Some(entry) = entry
                    && let Ok(plan) = Plan::of_entry(&entry)
                {
                    self.install(&entry.id, &entry.name, plan, true);
                }
            });
        }
    }
);

fn index(sender: Option<&AnyObject>) -> usize {
    // SAFETY: Every sender of these actions is a control, which answers `tag`.
    let tag: isize = sender.map_or(-1, |s| unsafe { msg_send![s, tag] });
    usize::try_from(tag).unwrap_or(usize::MAX)
}

/// The preset whose version the app installs for `agent`: Codex's and OpenCode's adapters, an ACP agent's own.
fn preset_of(agent: &Agent) -> Option<&'static Preset> {
    match agent {
        Agent::Claude => None,
        other => register::preset(other.id()),
    }
}

thread_local! {
    static TARGET: OnceCell<Retained<Target>> = const { OnceCell::new() };
}

/// The pane's target of these buttons, made once.
pub(crate) fn target(mtm: MainThreadMarker) -> Retained<Target> {
    TARGET.with(|cell| {
        cell.get_or_init(|| {
            let this = Target::alloc(mtm).set_ivars(Ivars::default());
            // SAFETY: NSObject's designated initializer, called once on the newly allocated object.
            unsafe { msg_send![super(this), init] }
        })
        .clone()
    })
}

/// The target while the pane exists, for the self-test and the registry window.
pub(crate) fn existing() -> Option<Retained<Target>> {
    TARGET.with(|cell| cell.get().cloned())
}

fn button(target: &Target, title: &str, action: Sel, tag: usize) -> Retained<NSButton> {
    let mtm = target.mtm();
    // SAFETY: Without a target and an action the button sends nothing until `set_action` gives it both.
    let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, mtm) };
    set_action(&button, target, action);
    button.setTag(isize::try_from(tag).unwrap_or(-1));
    button.setBezelStyle(NSBezelStyle::Push);
    button.setControlSize(NSControlSize::Small);
    button
}

fn line_of(mtm: MainThreadMarker, views: &[Retained<NSView>]) -> Retained<NSStackView> {
    let line = NSStackView::stackViewWithViews(&NSArray::from_retained_slice(views), mtm);
    line.setAlignment(NSLayoutAttribute::FirstBaseline);
    line.setSpacing(space::S);
    line
}

fn view(control: &NSView) -> Retained<NSView> {
    control.retain()
}

impl Target {
    /// The line of `agent`: its switch, where its program is, and its buttons, which `shown` fills.
    pub(crate) fn line(&self, agent: Agent, check: &NSView) -> Retained<NSStackView> {
        let mtm = self.mtm();
        let at = self.ivars().lines.borrow().len();
        let state = secondary(mtm, t("Wird gesucht …"));
        let install = button(self, "", sel!(termAgentsInstall:), at);
        let rollback = button(self, "", sel!(termAgentsRollback:), at);
        install.setHidden(true);
        rollback.setHidden(true);
        let views = [view(check), view(&state), view(&install), view(&rollback)];
        self.ivars().lines.borrow_mut().push(Line {
            agent,
            state,
            install,
            rollback,
        });
        line_of(mtm, &views)
    }

    /// The registry's line, with the agents installed from it below; once per pane.
    pub(crate) fn register_line(&self) -> Retained<NSStackView> {
        let mtm = self.mtm();
        let state = secondary(mtm, "");
        let fetch = button(self, t("Register laden"), sel!(termAgentsFetch:), 0);
        let show = button(self, t("Anzeigen …"), sel!(termAgentsShowRegister:), 0);
        let head = line_of(mtm, &[view(&state), view(&show), view(&fetch)]);
        let installed = NSStackView::new(mtm);
        installed.setOrientation(objc2_app_kit::NSUserInterfaceLayoutOrientation::Vertical);
        installed.setAlignment(NSLayoutAttribute::Leading);
        installed.setSpacing(space::S);
        let column =
            NSStackView::stackViewWithViews(&NSArray::from_retained_slice(&[view(&head), view(&installed)]), mtm);
        column.setOrientation(objc2_app_kit::NSUserInterfaceLayoutOrientation::Vertical);
        column.setAlignment(NSLayoutAttribute::Leading);
        column.setSpacing(space::S);
        let _ = self.ivars().register.set(RegisterLine {
            state,
            fetch,
            show,
            installed,
        });
        column
    }

    fn line_agent(&self, sender: Option<&AnyObject>) -> Option<Agent> {
        self.ivars()
            .lines
            .borrow()
            .get(index(sender))
            .map(|line| line.agent.clone())
    }

    /// Installs `plan` of agent `id` after the user agreed where `ask`, showing how far it got.
    pub(crate) fn install(&self, id: &str, name: &str, plan: Plan, ask: bool) {
        let mtm = self.mtm();
        if self.ivars().progress.borrow().contains_key(id) || (ask && !term_ui_agent::confirm_install(mtm, name, &plan))
        {
            return;
        }
        let key = id.to_string();
        self.set_progress(&key, Some(tf("{} wird installiert …", &[name])));
        let heard = key.clone();
        let progress = move |text: String| {
            if let Some(target) = existing() {
                target.set_progress(&heard, Some(text));
            }
        };
        let shown = name.to_string();
        term_ui_agent::install_plan(mtm, plan, name, progress, move |result| {
            let Some(target) = existing() else { return };
            target.set_progress(&key, None);
            match result {
                Ok(installed) => println_for_selftest(&format!("installed {key} {}", installed.version)),
                Err(why) => {
                    println_for_selftest(&format!("not installed {key}: {why}"));
                    alert(target.mtm(), &tf("{} wurde nicht installiert", &[&shown]), &why);
                }
            }
            target.refresh();
        });
    }

    /// Goes back to the version of agent `id` before its last update.
    pub(crate) fn rollback(&self, id: &str, name: &str) {
        if let Err(error) = term_ui_agent::installs().rollback(id) {
            alert(
                self.mtm(),
                &tf("{} konnte nicht zurückgesetzt werden", &[name]),
                &error.to_string(),
            );
        }
        self.refresh();
    }

    /// Fetches the registry on the user's request and keeps it.
    pub(crate) fn fetch(&self) {
        self.set_progress("", Some(t("Register wird geladen …").to_string()));
        term_ui_agent::fetch_register(self.mtm(), |result| {
            let Some(target) = existing() else { return };
            target.set_progress("", None);
            if let Err(why) = result {
                alert(target.mtm(), t("Das Register wurde nicht geladen"), &why);
            }
            target.refresh();
        });
    }

    fn set_progress(&self, id: &str, text: Option<String>) {
        {
            let mut progress = self.ivars().progress.borrow_mut();
            match text {
                Some(text) => progress.insert(id.to_string(), text),
                None => progress.remove(id),
            };
        }
        self.refresh();
    }

    /// Looks again for each agent's program, which the user may have installed since, and shows where it is and what
    /// the line offers; the registry's line and window follow.
    pub(crate) fn refresh(&self) {
        let agents: Vec<Agent> = self
            .ivars()
            .lines
            .borrow()
            .iter()
            .map(|line| line.agent.clone())
            .collect();
        let progress = self.ivars().progress.borrow().clone();
        term_base::thread::detach_or_run("agent programs", move || {
            let env = term_ui::workspace_host::environment(ENVIRONMENT);
            let installs = term_ui_agent::installs();
            let states: Vec<LineState> = agents
                .iter()
                .map(|agent| match progress.get(agent.id()) {
                    Some(busy) => LineState {
                        text: busy.clone(),
                        ..LineState::default()
                    },
                    None => line_state(agent, &env, &installs),
                })
                .collect();
            term_base::main_soon(move || {
                if let Some(target) = existing() {
                    target.show_states(&states);
                }
            });
        });
        self.show_register();
        super::register::refresh(self);
    }

    fn show_states(&self, states: &[LineState]) {
        for (line, state) in self.ivars().lines.borrow().iter().zip(states) {
            line.state.setStringValue(&ns(&state.text));
            for (button, title) in [(&line.install, &state.install), (&line.rollback, &state.rollback)] {
                button.setHidden(title.is_none());
                if let Some(title) = title {
                    button.setTitle(&ns(title));
                    set_label(button, Some(&format!("{title}: {}", line.agent.name())));
                }
            }
        }
    }

    /// The registry's line: what it holds, and a line for each agent installed from it.
    fn show_register(&self) {
        let Some(register) = self.ivars().register.get() else {
            return;
        };
        let busy = self.ivars().progress.borrow().get("").cloned();
        let cached = term_ui_agent::cached_register();
        let text = match (&busy, &cached) {
            (Some(busy), _) => busy.clone(),
            (None, Some((register, at))) => tf(
                "{} Agenten, Stand {}",
                &[&register.entries.len().to_string(), &term_ui_agent::age(*at)],
            ),
            (None, None) => t("Noch nicht geladen").to_string(),
        };
        register.state.setStringValue(&ns(&text));
        register.fetch.setTitle(&ns(if cached.is_some() {
            t("Neu laden")
        } else {
            t("Register laden")
        }));
        register.fetch.setEnabled(busy.is_none());
        register.show.setHidden(cached.is_none());
        for old in register.installed.arrangedSubviews().iter() {
            register.installed.removeArrangedSubview(&old);
            old.removeFromSuperview();
        }
        let installs = term_ui_agent::installs();
        let mut ids = Vec::new();
        for agent in AcpAgent::installed_entries() {
            let Some((installed, _)) = installs.current(agent.id()) else {
                continue;
            };
            let name = term_ui::kit::label(self.mtm(), agent.name());
            let state = secondary(self.mtm(), &tf("Bereit · Version {}", &[&installed.version]));
            let mut views = vec![view(&name), view(&state)];
            if let Some(previous) = installs.previous(agent.id()) {
                let title = tf("Zurück zu {}", &[&previous.version]);
                let back = button(self, &title, sel!(termAgentsRollbackEntry:), ids.len());
                set_label(&back, Some(&format!("{title}: {}", agent.name())));
                views.push(view(&back));
            }
            register.installed.addArrangedSubview(&line_of(self.mtm(), &views));
            ids.push(agent.id().to_string());
        }
        *self.ivars().entries.borrow_mut() = ids;
    }

    /// Opens the registry's window as its button does, for the self-test.
    #[cfg(feature = "selftest")]
    pub(crate) fn show_register_window(&self) {
        self.refresh();
        super::register::show(self);
    }

    /// Whether an installation of `id` runs now.
    pub(crate) fn busy(&self, id: &str) -> bool {
        self.ivars().progress.borrow().contains_key(id)
    }

    /// What the lines show, for the self-test: each agent's name, state and the titles of its buttons.
    #[cfg(feature = "selftest")]
    pub(crate) fn lines_text(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .ivars()
            .lines
            .borrow()
            .iter()
            .map(|line| {
                let mut text = format!("{}: {}", line.agent.name(), line.state.stringValue());
                for button in [&line.install, &line.rollback] {
                    if !button.isHidden() {
                        text.push_str(&format!(" [{}]", button.title()));
                    }
                }
                text
            })
            .collect();
        if let Some(register) = self.ivars().register.get() {
            lines.push(format!("{}: {}", t("ACP-Register"), register.state.stringValue()));
            for line in register.installed.arrangedSubviews().iter() {
                lines.push(format!("  {}", texts(&line).join(" ")));
            }
        }
        lines
    }
}

/// The texts and button titles in `view`, in order, for the self-test.
#[cfg(feature = "selftest")]
fn texts(view: &NSView) -> Vec<String> {
    if let Some(label) = view.downcast_ref::<NSTextField>() {
        return vec![label.stringValue().to_string()];
    }
    if let Some(button) = view.downcast_ref::<NSButton>() {
        return vec![format!("[{}]", button.title())];
    }
    let children = match view.downcast_ref::<NSStackView>() {
        Some(stack) => stack.arrangedSubviews(),
        None => view.subviews(),
    };
    children.iter().flat_map(|child| texts(&child)).collect()
}

/// What `agent`'s line says with the login shell's `env`: where its program is on the PATH, or the version the app
/// installed with the update to the version it pins and the way back, or that it is missing with its installation.
pub(crate) fn line_state(agent: &Agent, env: &[(String, String)], installs: &Installs) -> LineState {
    // A folder that holds no program, so that every agent is looked for on the PATH alone (E16).
    let nowhere = Path::new("/var/empty");
    let path = term_process::tool::var(env, "PATH");
    let on_path: Option<PathBuf> = match agent {
        Agent::Claude => agent.program(env, nowhere),
        Agent::Codex => term_process::tool::which_outside("codex", path, nowhere)
            .or_else(|| preset_of(agent).and_then(|p| p.command(env, nowhere))),
        _ => preset_of(agent).and_then(|p| p.command(env, nowhere)),
    };
    let ready = |path: &Path| tf("Bereit · {}", &[&term_ui::kit::panel::display_path(path)]);
    if let Some(program) = on_path {
        return LineState {
            text: ready(&program),
            ..LineState::default()
        };
    }
    let preset = preset_of(agent);
    let Some(preset) = preset else {
        let text = match agent {
            Agent::Claude => t("Nicht gefunden: claude im Terminal installieren und dort anmelden").to_string(),
            _ => t("Nicht gefunden").to_string(),
        };
        return LineState {
            text,
            ..LineState::default()
        };
    };
    match installs.current(preset.id) {
        Some((installed, _)) => {
            let pinned = preset.pinned.version();
            LineState {
                text: tf("Bereit · Version {}", &[&installed.version]),
                install: (installed.version != pinned).then(|| tf("Auf {} aktualisieren", &[pinned])),
                rollback: installs
                    .previous(preset.id)
                    .map(|previous| tf("Zurück zu {}", &[&previous.version])),
            }
        }
        None => LineState {
            text: t("Nicht vorhanden").to_string(),
            install: Some(t("Installieren …").to_string()),
            rollback: None,
        },
    }
}

/// Tells the user what went wrong; in the self-test, where nobody answers, it prints it instead.
fn alert(mtm: MainThreadMarker, title: &str, detail: &str) {
    if term_base::unattended::is() {
        println!("agentinstall alert {title}: {detail}");
        return;
    }
    Alert::new(mtm, title).informative(detail).run_modal();
}

fn println_for_selftest(line: &str) {
    if cfg!(feature = "selftest") {
        println!("agentinstall {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(dir: &Path, id: &str, current: &str, previous: Option<&str>) {
        let entry = |version: &str| {
            let folder = dir.join(id).join(version);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("agent"), "#!/bin/sh\n").unwrap();
            serde_json::json!({"version": version, "program": "agent", "args": [], "env": [], "verified": false})
        };
        let state = serde_json::json!({"current": entry(current), "previous": previous.map(entry)});
        std::fs::write(dir.join(id).join("state.json"), state.to_string()).unwrap();
    }

    #[test]
    fn a_missing_agent_offers_its_installation_and_claude_its_own_installer() {
        let dir = term_base::testing::scratch("settings-installs");
        let installs = Installs::new(dir.join("agents"));
        let env = [("PATH".to_string(), dir.display().to_string())];
        let missing = line_state(&Agent::OpenCode, &env, &installs);
        assert_eq!(missing.text, "Nicht vorhanden");
        assert_eq!(missing.install.as_deref(), Some("Installieren …"));
        let claude = line_state(&Agent::Claude, &env, &installs);
        assert!(
            claude.install.is_none(),
            "Claude comes with its own installer: {claude:?}"
        );
    }

    #[test]
    fn an_installed_agent_offers_the_pinned_version_and_the_one_before() {
        let dir = term_base::testing::scratch("settings-installed");
        let root = dir.join("agents");
        installed(&root, "opencode", "2.0.0", Some("1.9.0"));
        let installs = Installs::new(&root);
        let env = [("PATH".to_string(), dir.display().to_string())];
        let state = line_state(&Agent::OpenCode, &env, &installs);
        let pinned = register::preset("opencode").unwrap().pinned.version();
        assert_eq!(state.text, "Bereit · Version 2.0.0");
        assert_eq!(state.install, Some(format!("Auf {pinned} aktualisieren")));
        assert_eq!(state.rollback.as_deref(), Some("Zurück zu 1.9.0"));
        installed(&root, "opencode", pinned, None);
        let current = line_state(&Agent::OpenCode, &env, &installs);
        assert!(current.install.is_none() && current.rollback.is_none(), "{current:?}");
    }
}
