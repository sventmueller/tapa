//! A workspace's trust in its agent tabs (E21): a note above the composer while the agent runs without the project's
//! configuration, and, once the trust changes, the session started again with what the trust now allows, continuing
//! its conversation.

use objc2::rc::Retained;
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{NSColor, NSStackView};
use splat_i18n::t;
use term_ui::workspace_host::Trust;

use super::source::{Problem, Source};
use super::tab::AgentTab;
use super::target::AgentTarget;
use super::{style, ui};

/// The note above the composer, hidden until the tab finds its workspace untrusted.
pub(super) fn note(mtm: MainThreadMarker, target: &AgentTarget) -> Retained<NSStackView> {
    let text = t(
        "Arbeitsbereich nicht vertraut: Einstellungen, Skills, Hooks und MCP-Server des Projekts sind nicht geladen.",
    );
    let color = NSColor::secondaryLabelColor();
    let icon = ui::icon(mtm, "lock.shield", text, 12.0, &color);
    let label = ui::wrapping(mtm, text, &style::chat_small(), &color, 560.0);
    let button = ui::button(mtm, t("Arbeitsbereich vertrauen"), target, sel!(ideAgentTrust:), 0);
    ui::hug(&button);
    let row = ui::row(mtm, 6.0, &[&icon, &label, &ui::spacer(mtm), &button]);
    row.setHidden(true);
    row
}

impl AgentTab {
    /// Shows the note while the agent runs without the project's configuration: in an untrusted workspace other than
    /// the home folder, which is no project, unless the overlay offers the trust already.
    pub(super) fn show_trust_note(&self, problem: Option<&Problem>) {
        let hidden = self.controller.is_trusted()
            || self.cwd == term_base::home()
            || matches!(problem, Some(Problem::Untrusted(_)));
        if self.trust_note.isHidden() != hidden {
            self.trust_note.setHidden(hidden);
        }
    }

    /// The workspace's trust changed: the session starts again with the configuration the trust now allows, continuing
    /// its conversation, a running turn first ending; a parked or waiting session takes the trust when it starts.
    pub(crate) fn trust_moved(&self) {
        self.refresh();
        if self.parking.deferred.get() || self.parking.process.get().is_some() {
            return;
        }
        let (running, again) = {
            let state = self.state.borrow();
            let again =
                state.source().is_live() || matches!(state.source(), Source::Starting) || state.problem().is_some();
            (state.running(), again)
        };
        if running {
            self.restart_for_trust.set(true);
        } else if again {
            self.start();
        }
    }

    /// Starts the session again once its turn ended, where a change of trust waited for that; whether it did.
    pub(super) fn restart_after_turn(&self) -> bool {
        if !self.restart_for_trust.take() {
            return false;
        }
        self.start();
        true
    }

    /// Whether the note about the untrusted workspace shows.
    #[cfg(feature = "selftest")]
    pub fn trust_note_shown(&self) -> bool {
        !self.trust_note.isHidden()
    }
}
