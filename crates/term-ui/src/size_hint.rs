//! The grid's size in the middle of a session view while its window is resized (F218).

use objc2::{DefinedClass, MainThreadOnly};
use objc2_app_kit::NSColor;
use term_render::constant;
use term_session::Size;

use crate::TerminalView;
use crate::kit::{main_after, ns};

impl TerminalView {
    /// Shows the grid's columns and rows in the middle of the view for a moment, as the window is resized (F218).
    pub(crate) fn show_grid_size(&self, size: Size) {
        let mtm = self.mtm();
        let label = self
            .ivars()
            .size_label
            .borrow_mut()
            .get_or_insert_with(|| {
                let label = crate::kit::label(mtm, "");
                label.setFont(Some(&objc2_app_kit::NSFont::monospacedDigitSystemFontOfSize_weight(
                    15.0,
                    constant!(objc2_app_kit::NSFontWeightMedium),
                )));
                label.setDrawsBackground(true);
                label.setBackgroundColor(Some(&NSColor::windowBackgroundColor().colorWithAlphaComponent(0.85)));
                label.setBezeled(false);
                label.setAlignment(objc2_app_kit::NSTextAlignment::Center);
                crate::kit::hide_from_voiceover(&label);
                self.addSubview(&label);
                label
            })
            .clone();
        label.setStringValue(&ns(&format!(" {} × {} ", size.columns, size.lines)));
        label.sizeToFit();
        let bounds = self.bounds();
        let fitted = label.frame().size;
        label.setFrameOrigin(objc2_foundation::NSPoint::new(
            ((bounds.size.width - fitted.width) / 2.0).round(),
            ((bounds.size.height - fitted.height) / 2.0).round(),
        ));
        label.setHidden(false);
        let shown = self.ivars().size_shown.get() + 1;
        self.ivars().size_shown.set(shown);
        let id = self.id();
        main_after(std::time::Duration::from_millis(900), move || {
            if let Some(view) = super::find(id)
                && view.ivars().size_shown.get() == shown
                && let Some(label) = view.ivars().size_label.borrow().as_ref()
            {
                label.setHidden(true);
            }
        });
    }
}
