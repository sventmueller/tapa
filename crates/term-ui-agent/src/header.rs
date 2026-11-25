//! The head of the agent tab: the session's title, then Remote Control, the focus view, the session's subagents and
//! background processes, bookmark, history and a new session.

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSButton, NSClickGestureRecognizer, NSColor, NSImageSymbolConfiguration, NSLayoutConstraint, NSStackView,
    NSTextField,
};
use objc2_foundation::NSEdgeInsets;
use splat_i18n::{t, tf};

use crate::work::{Part, Tally};

use super::style;
use super::target::AgentTarget;
use super::ui;
use crate::common::util::{ns, symbol};
#[cfg(feature = "selftest")]
mod selftest_probe;

/// The symbol of the button for the session's subagents, the one the activity view shows before each of them.
const SUBAGENTS: &str = "person.2";

/// The symbol of the button for the session's background processes.
const BACKGROUND: &str = "terminal";

/// The button that opens the list of `part`, unavailable until the session has some.
fn work_button(mtm: MainThreadMarker, target: &AgentTarget, part: Part) -> Retained<NSButton> {
    let (name, action) = match part {
        Part::Subagents | Part::All => (SUBAGENTS, sel!(ideAgentSubagentList:)),
        Part::Background => (BACKGROUND, sel!(ideAgentBackgroundList:)),
    };
    let button = ui::symbol_button(
        mtm,
        name,
        part.heading(),
        crate::common::style::symbol::SMALL,
        target,
        action,
        0,
    );
    button.setEnabled(false);
    button
}

/// The session's title, which a double click renames.
fn title(mtm: MainThreadMarker, target: &AgentTarget) -> Retained<NSTextField> {
    let title = ui::label(
        mtm,
        t("Neue Sitzung"),
        &style::bold(style::BODY),
        &NSColor::labelColor(),
    );
    title.setToolTip(Some(&ns(t("Doppelklick zum Umbenennen"))));
    // SAFETY: The recognizer sends its action to the target, which answers it with a method that takes the sender;
    // the view keeps the recognizer, AppKit holds the target weakly.
    let rename = unsafe {
        NSClickGestureRecognizer::initWithTarget_action(
            NSClickGestureRecognizer::alloc(mtm),
            Some(target),
            Some(sel!(ideAgentRename:)),
        )
    };
    rename.setNumberOfClicksRequired(2);
    title.addGestureRecognizer(&rename);
    title
}

/// What the Remote Control button shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Remote {
    /// The agent offers none.
    Hidden,
    /// Off; a click turns it on.
    Off,
    /// It connects.
    Connecting,
    /// The phone reaches the session.
    Connected,
    /// It lost its connection or did not start, for this reason.
    Failed(String),
}

impl Remote {
    /// The button for Claude's `remote`.
    pub(crate) fn of_claude(remote: &term_claude::RemoteControl) -> Remote {
        match remote {
            term_claude::RemoteControl::Unavailable => Remote::Hidden,
            term_claude::RemoteControl::Off => Remote::Off,
            term_claude::RemoteControl::Connecting => Remote::Connecting,
            term_claude::RemoteControl::Connected(_) => Remote::Connected,
            term_claude::RemoteControl::Failed(why) => Remote::Failed(why.clone()),
        }
    }
}

/// Puts the title's text on the left edge of the window's tab bar, which AppKit insets 8 points from the content.
const TITLE_INSET: f64 = 8.0;
/// The head's height at the default text size.
const HEIGHT: f64 = 36.0;
/// The room between the title and the buttons at the default text size.
const SPACING: f64 = 8.0;

pub(crate) struct Header {
    pub(crate) view: Retained<NSStackView>,
    /// Holds the head at its height, which follows the text size.
    height: Retained<NSLayoutConstraint>,
    title: Retained<NSTextField>,
    bookmark: Retained<NSButton>,
    focus: Retained<NSButton>,
    /// Opens the list of the session's subagents.
    pub(crate) subagents: Retained<NSButton>,
    /// Opens the list of the session's background processes.
    pub(crate) background: Retained<NSButton>,
    remote: Retained<NSButton>,
    history: Retained<NSButton>,
    new: Retained<NSButton>,
}

impl Header {
    pub(crate) fn new(mtm: MainThreadMarker, target: &AgentTarget) -> Header {
        let title = title(mtm, target);
        let bookmark = ui::symbol_button(
            mtm,
            "bookmark",
            t("Lesezeichen"),
            crate::common::style::symbol::SMALL,
            target,
            sel!(ideAgentBookmark:),
            0,
        );
        let history = ui::symbol_button(
            mtm,
            "clock.arrow.circlepath",
            t("Frühere Sitzungen"),
            crate::common::style::symbol::SMALL,
            target,
            sel!(ideAgentHistory:),
            0,
        );
        let new = ui::symbol_button(
            mtm,
            "plus.bubble",
            t("Neue Sitzung"),
            crate::common::style::symbol::SMALL,
            target,
            sel!(ideAgentNewSession:),
            0,
        );
        let focus = ui::symbol_button(
            mtm,
            "text.bubble",
            t("Nur die Unterhaltung zeigen"),
            crate::common::style::symbol::SMALL,
            target,
            sel!(ideAgentFocusView:),
            0,
        );
        let subagents = work_button(mtm, target, Part::Subagents);
        let background = work_button(mtm, target, Part::Background);
        let remote = ui::symbol_button(
            mtm,
            "iphone",
            t("Remote Control: auf dem Handy oder auf claude.ai weiterarbeiten"),
            crate::common::style::symbol::SMALL,
            target,
            sel!(ideAgentRemoteControl:),
            0,
        );
        remote.setHidden(true);
        let spacer = ui::spacer(mtm);
        let view = ui::row(
            mtm,
            SPACING,
            &[
                &title,
                &spacer,
                &remote,
                &subagents,
                &background,
                &focus,
                &bookmark,
                &history,
                &new,
            ],
        );
        view.setEdgeInsets(NSEdgeInsets {
            top: 0.0,
            left: TITLE_INSET,
            bottom: 0.0,
            right: 12.0,
        });
        view.setTranslatesAutoresizingMaskIntoConstraints(false);
        let height = view.heightAnchor().constraintEqualToConstant(HEIGHT);
        height.setActive(true);
        let header = Header {
            view,
            height,
            title,
            bookmark,
            focus,
            subagents,
            background,
            remote,
            history,
            new,
        };
        header.text_size_changed();
        header
    }

    /// Shows the title and every button at the text size the user chose for the agent tab, with the room and
    /// height around them in proportion.
    pub(crate) fn text_size_changed(&self) {
        self.title.setFont(Some(&style::bold(style::chat(style::BODY))));
        let symbols = NSImageSymbolConfiguration::configurationWithPointSize_weight(
            style::chat(crate::common::style::symbol::SMALL),
            crate::common::constant!(objc2_app_kit::NSFontWeightRegular),
        );
        for button in [
            &self.remote,
            &self.subagents,
            &self.background,
            &self.focus,
            &self.bookmark,
            &self.history,
            &self.new,
        ] {
            button.setSymbolConfiguration(Some(&symbols));
        }
        self.view.setSpacing(style::chat(SPACING));
        self.height.setConstant(style::chat(HEIGHT));
    }

    /// Shows where the session stands with Claude's Remote Control; hidden where the agent does not offer it.
    pub(crate) fn set_remote(&self, remote: &Remote) {
        let interrupted;
        let (name, color, tip) = match remote {
            Remote::Hidden => {
                self.remote.setHidden(true);
                return;
            }
            Remote::Off => (
                "iphone",
                NSColor::secondaryLabelColor(),
                t("Remote Control: auf dem Handy oder auf claude.ai weiterarbeiten"),
            ),
            Remote::Connecting => (
                "iphone.radiowaves.left.and.right",
                NSColor::secondaryLabelColor(),
                t("Remote Control verbindet …"),
            ),
            Remote::Connected => (
                "iphone.radiowaves.left.and.right",
                NSColor::controlAccentColor(),
                t("Remote Control ist an: diese Sitzung läuft auch auf dem Handy und auf claude.ai"),
            ),
            Remote::Failed(why) => {
                interrupted = tf("Remote Control ist unterbrochen: {}", &[why]);
                ("iphone.slash", NSColor::systemOrangeColor(), interrupted.as_str())
            }
        };
        self.remote.setImage(symbol(name, tip).as_deref());
        self.remote.setContentTintColor(Some(&color));
        self.remote.setToolTip(Some(&ns(tip)));
        self.remote.setHidden(false);
    }

    /// Shows what the list of `part` holds: in the accent colour while something runs, the numbers in the tooltip and
    /// for VoiceOver, unavailable while it is empty; `silent` names an agent that reports no such work.
    pub(crate) fn set_work(&self, part: Part, tally: Tally, silent: Option<&str>) {
        let button = self.work_button(part);
        let (running, ended) = (tally.running.to_string(), tally.ended.to_string());
        let subagents = part == Part::Subagents;
        let tip = match silent {
            Some(agent) if subagents => tf("{} meldet keine Subagenten", &[agent]),
            Some(agent) => tf("{} meldet keine Hintergrundprozesse", &[agent]),
            None if tally.listed == 0 && subagents => {
                t("Diese Sitzung hat noch keine Subagenten gestartet").to_string()
            }
            None if tally.listed == 0 => t("Diese Sitzung hat noch keine Hintergrundprozesse gestartet").to_string(),
            None if subagents => tf("Subagenten: {} laufend, {} beendet", &[&running, &ended]),
            None => tf("Hintergrundprozesse: {} laufend, {} beendet", &[&running, &ended]),
        };
        button.setToolTip(Some(&ns(&tip)));
        crate::common::util::set_label(button, Some(&tip));
        button.setEnabled(silent.is_none() && tally.listed > 0);
        let color = if tally.running > 0 && silent.is_none() {
            NSColor::controlAccentColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        button.setContentTintColor(Some(&color));
    }

    /// The button that opens the list of `part`; the subagents' for the whole overview.
    pub(crate) fn work_button(&self, part: Part) -> &NSButton {
        match part {
            Part::Subagents | Part::All => &self.subagents,
            Part::Background => &self.background,
        }
    }

    pub(crate) fn set_title(&self, title: &str) {
        self.title.setStringValue(&ns(title));
    }

    /// Marks the focus view as on or off.
    pub(crate) fn set_focus_view(&self, on: bool) {
        let (name, color) = if on {
            ("text.bubble.fill", NSColor::controlAccentColor())
        } else {
            ("text.bubble", NSColor::secondaryLabelColor())
        };
        self.focus
            .setImage(symbol(name, t("Nur die Unterhaltung zeigen")).as_deref());
        self.focus.setContentTintColor(Some(&color));
        self.focus.setState(if on {
            objc2_app_kit::NSControlStateValueOn
        } else {
            objc2_app_kit::NSControlStateValueOff
        });
    }

    pub(crate) fn set_bookmarked(&self, on: bool) {
        let (name, color) = if on {
            ("bookmark.fill", NSColor::controlAccentColor())
        } else {
            ("bookmark", NSColor::secondaryLabelColor())
        };
        self.bookmark.setImage(symbol(name, t("Lesezeichen")).as_deref());
        self.bookmark.setContentTintColor(Some(&color));
    }
}
