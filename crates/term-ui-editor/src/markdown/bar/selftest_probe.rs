//! What the self-test reads from and does with the format bar, kept out of its production code.

use objc2::ClassType;

use super::*;

impl FormatBar {
    /// The bar's title while it shows.
    pub(crate) fn shown_title(&self) -> Option<String> {
        (!self.view.isHidden()).then(|| self.title.stringValue().to_string())
    }

    /// The button that runs `command`, as the bar shows it.
    pub(crate) fn button_of(&self, command: Command) -> Option<Retained<NSButton>> {
        if command == Command::Preview {
            return Some(self.preview.clone());
        }
        self.controls
            .iter()
            .filter_map(|control| control.downcast_ref::<NSButton>())
            .find(|button| button.tag() == command as isize && !button.isKindOfClass(NSPopUpButton::class()))
            .map(objc2::rc::Retained::from)
    }
}
