//! Remote Control for a native, local Codex tab. The phone reaches Codex's shared daemon.

use objc2::rc::Retained;
use objc2::sel;
use objc2_app_kit::{NSMenu, NSView};
use splat_i18n::{t, tf};
use term_base::explain::Explained;
use term_base::main_thread::main_on_run_loop;
use term_codex::{RemoteControl, Session};
use term_process::Supervisor;
use term_ui::kit::alert::Alert;
use term_ui::workspace_host::{Notices, Trust};

use crate::common::util::{main_soon, spawn};
use crate::header::Remote;
use crate::tab::AgentTab;

const PAIR: isize = 10;
const END: isize = 11;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Turning {
    On,
    Moving,
    Failed(String),
}

/// Prepares a trusted local launch to join the daemon when Codex's saved setting enables Remote Control.
pub(crate) fn share(launch: &mut term_codex::Launch) {
    if term_codex::remote::wanted(&launch.env) {
        match term_codex::remote::ensure_daemon(Supervisor::global(), &launch.program, &launch.env) {
            Ok(()) => launch.shared = true,
            Err(error) => term_base::diag::report("starting Codex's shared server", error),
        }
    }
}

impl AgentTab {
    /// A native Codex session which can run on this Mac's daemon.
    pub(crate) fn codex_session(&self) -> Option<Session> {
        if crate::remote_transcript::is_remote(&self.cwd) {
            return None;
        }
        self.state.borrow().source().codex().filter(Session::is_ready)
    }

    pub(crate) fn codex_remote_shown(&self, session: &Session) -> Remote {
        match self.codex_remote.borrow().as_ref() {
            Some(Turning::On) => Remote::Connecting,
            Some(Turning::Moving) => Remote::Moving,
            Some(Turning::Failed(why)) => Remote::Failed(why.clone()),
            None => match session.remote_control() {
                RemoteControl::Off => Remote::Off,
                RemoteControl::Connecting => Remote::Connecting,
                RemoteControl::Connected(_) => Remote::Connected,
                RemoteControl::Failed => Remote::Failed(t("Codex hat keine Verbindung zu OpenAI").to_string()),
            },
        }
    }

    pub(crate) fn codex_remote_click(&self, session: &Session, view: &NSView) {
        match self.codex_remote_shown(session) {
            Remote::Off | Remote::Failed(_) if !self.controller.is_trusted() => self.trust_workspace(),
            Remote::Off | Remote::Failed(_) => self.codex_remote_on(session),
            Remote::Connected => crate::actions::pop_up(&self.codex_remote_items(), view),
            Remote::Hidden | Remote::Connecting | Remote::Moving => {}
        }
    }

    pub(crate) fn codex_remote_items(&self) -> Retained<NSMenu> {
        let menu = NSMenu::new(self.mtm);
        menu.setAutoenablesItems(false);
        let action = sel!(ideAgentRemoteControlAction:);
        self.menu_item(&menu, t("Handy koppeln …"), action, PAIR);
        menu.addItem(&objc2_app_kit::NSMenuItem::separatorItem(self.mtm));
        self.menu_item(&menu, t("Remote Control beenden"), action, END);
        menu
    }

    pub(crate) fn codex_remote_action(&self, session: &Session, action: isize) {
        match action {
            PAIR => self.codex_pair(session),
            END => self.codex_remote_off(session),
            _ => {}
        }
    }

    fn codex_remote_on(&self, session: &Session) {
        *self.codex_remote.borrow_mut() = Some(Turning::On);
        self.refresh_remote();
        let launch = session.launch().clone();
        let key = self.key;
        if let Err(why) = spawn("codex-remote", move || {
            let started = term_codex::remote::start(Supervisor::global(), &launch.program, &launch.env)
                .map(drop)
                .map_err(|error| Explained(super::explained(&error)));
            main_soon(move || {
                if let Some(tab) = crate::get(key) {
                    tab.codex_remote_started(started);
                }
            });
        }) {
            self.codex_remote_started(Err(why));
        }
    }

    fn codex_remote_started(&self, started: Result<(), Explained>) {
        let moving =
            self.controller.is_trusted() && self.codex_session().is_some_and(|session| !session.launch().shared);
        *self.codex_remote.borrow_mut() = match started {
            Err(why) => Some(Turning::Failed(why.0)),
            Ok(()) if moving => Some(Turning::Moving),
            Ok(()) => None,
        };
        self.refresh_remote();
        self.codex_move_if_idle();
    }

    /// Changes transport once the current turn and its approvals have ended, preserving the Codex thread.
    pub(crate) fn codex_move_if_idle(&self) {
        if self.codex_remote.borrow().as_ref() != Some(&Turning::Moving) || self.state.borrow().running() {
            return;
        }
        self.codex_remote.borrow_mut().take();
        if self.controller.is_trusted() && self.codex_session().is_some() {
            self.start();
        } else {
            self.refresh_remote();
        }
    }

    pub(crate) fn codex_remote_heard(&self) {
        if matches!(self.codex_remote.borrow().as_ref(), Some(Turning::Failed(_))) {
            self.codex_remote.borrow_mut().take();
        }
    }

    fn codex_remote_off(&self, session: &Session) {
        let Ok(reply) = session.disable_remote_control() else {
            return;
        };
        let key = self.key;
        reply.then(move |result| {
            if let Err(error) = result {
                main_soon(move || {
                    if let Some(tab) = crate::get(key) {
                        *tab.codex_remote.borrow_mut() = Some(Turning::Failed(super::explained(&error)));
                        tab.refresh_remote();
                    }
                });
            }
        });
    }

    fn codex_pair(&self, session: &Session) {
        let launch = session.launch().clone();
        let key = self.key;
        if let Err(why) = spawn("codex-pair", move || {
            let paired = term_codex::remote::pair(Supervisor::global(), &launch.program, &launch.env);
            main_on_run_loop(move || {
                if let Some(tab) = crate::get(key) {
                    let alert = match paired {
                        Ok(pairing) => Alert::new(tab.mtm, t("Handy koppeln")).informative(&tf(
                            "Gib den Code {} in Codex auf dem Handy ein. Er gilt bis {}.",
                            &[&pairing.code, &crate::format::time_of_day(pairing.expires)],
                        )),
                        Err(error) => {
                            Alert::new(tab.mtm, t("Koppeln fehlgeschlagen")).informative(&super::explained(&error))
                        }
                    };
                    alert.run_modal();
                }
            });
        }) {
            self.controller.show_notice(&why.0);
        }
    }
}
