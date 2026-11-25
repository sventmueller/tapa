//! The input field's switch (E7, F209) as the window's toolbar shows it for the pane in front: whether the field of a
//! session is on, off, impossible while the shell reports no integration, or out of the way of a full-screen program.
//! The session tells the app when that changes, and the app updates its toolbar.

use std::cell::Cell;

use objc2::DefinedClass;
use splat_i18n::t;

use crate::TerminalView;

/// What the switch shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// The field is on and the shell reports its prompts.
    On,
    /// The user turned the field off.
    Off,
    /// The shell reports no prompts, so the field cannot show (OSC 133).
    NoIntegration,
    /// A full-screen program runs: the switch steps aside.
    Hidden,
}

impl Look {
    /// The look for a field that is `enabled`, in a shell that has `integration`, behind a program that may be
    /// `full_screen`.
    pub(crate) fn of(enabled: bool, integration: bool, full_screen: bool) -> Look {
        match (full_screen, integration, enabled) {
            (true, _, _) => Look::Hidden,
            (false, false, _) => Look::NoIntegration,
            (false, true, true) => Look::On,
            (false, true, false) => Look::Off,
        }
    }

    /// The switch's name, as its label and its symbol's description.
    pub fn name() -> &'static str {
        t("Eingabefeld")
    }

    /// The switch's tool tip, which also names it for VoiceOver.
    pub fn tool_tip(self) -> &'static str {
        match self {
            Look::On => t("Eingabefeld an"),
            Look::Off | Look::Hidden => t("Eingabefeld aus"),
            Look::NoIntegration => t("Eingabefeld aus: Die Shell meldet keine Integration (OSC 133)"),
        }
    }
}

/// The look a session's switch had when the app last heard of it.
pub(crate) struct FieldToggle {
    view: u64,
    shown: Cell<Option<Look>>,
}

impl FieldToggle {
    /// The switch of the session view `view`.
    pub(crate) fn new(view: u64) -> FieldToggle {
        FieldToggle {
            view,
            shown: Cell::new(None),
        }
    }

    /// Shows `look`: the app hears of it once the field's state is no longer borrowed.
    pub(crate) fn show(&self, look: Look) {
        if self.shown.replace(Some(look)) != Some(look) {
            let view = self.view;
            crate::kit::main_soon(move || crate::host().changed(view, crate::Change::Field));
        }
    }
}

impl TerminalView {
    /// What the input field's switch shows for this session.
    pub fn field_look(&self) -> Look {
        let Ok(field) = self.ivars().field.try_borrow() else {
            return Look::NoIntegration;
        };
        field
            .as_ref()
            .and_then(|field| field.toggle.get())
            .and_then(|toggle| toggle.shown.get())
            .unwrap_or(Look::NoIntegration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_look_follows_the_setting_the_integration_and_the_screen() {
        assert_eq!(Look::of(true, true, false), Look::On);
        assert_eq!(Look::of(false, true, false), Look::Off);
        assert_eq!(Look::of(true, false, false), Look::NoIntegration);
        assert_eq!(Look::of(false, false, false), Look::NoIntegration);
        assert_eq!(Look::of(true, true, true), Look::Hidden);
    }
}
