//! What the self-test reads from a timeline: the rows built, their heights, the cards' texts and buttons.

use objc2::rc::Retained;
use objc2_app_kit::{NSAccessibility, NSView};

use super::timeline::{BOTTOM, TOP, Timeline};

impl Timeline {
    /// How many cards were built so far.
    pub fn built(&self) -> usize {
        self.built.get()
    }

    /// Checks the geometry of visible rows against the heights published to AppKit.
    pub fn height_discrepancy(&self) -> f64 {
        let range = self.table.rowsInRect(self.table.visibleRect());
        (range.location..range.location + range.length)
            .filter(|row| *row < self.count())
            .map(|row| {
                (self.table.rectOfRow(isize::try_from(row).unwrap_or(0)).size.height - self.row_height(row)).abs()
            })
            .fold(0.0, f64::max)
    }

    /// Includes deferred height correction and drawing in the streaming timing budget.
    pub fn settle(&self) {
        if let Some(window) = self.scroll.window() {
            window.layoutIfNeeded();
        }
        self.width_changed();
        for _ in 0..32 {
            self.flush_corrections();
            if let Some(window) = self.scroll.window() {
                window.displayIfNeeded();
            }
            if self.corrections.borrow().is_empty() {
                break;
            }
        }
    }

    /// The current answer's text view, for checking reuse and selection during streaming.
    pub fn streaming_text(&self) -> Option<Retained<objc2_app_kit::NSTextView>> {
        let row = isize::try_from(self.count().checked_sub(1)?).ok()?;
        let cell = self.table.viewAtColumn_row_makeIfNecessary(0, row, false)?;
        let text = cell
            .subviews()
            .firstObject()?
            .downcast::<super::text::CardText>()
            .ok()?;
        Some(text.into_super())
    }

    /// The rows in view whose height differs from what their card needs, as `row: table height / card height`.
    pub fn misfits(&self) -> Vec<String> {
        let range = self.table.rowsInRect(self.table.visibleRect());
        (range.location..range.location + range.length)
            .filter_map(|row| {
                let rect = self.table.rectOfRow(isize::try_from(row).ok()?);
                let view = self
                    .table
                    .viewAtColumn_row_makeIfNecessary(0, isize::try_from(row).ok()?, false)?;
                let card = view.subviews().firstObject()?.fittingSize().height + TOP + BOTTOM;
                ((rect.size.height - card).abs() > 1.5).then(|| format!("{row}: {} / {card}", rect.size.height))
            })
            .collect()
    }

    /// What the permission editors hold.
    pub fn editor_texts(&self) -> Vec<String> {
        let inputs = self.inputs.borrow();
        inputs.editors.keys().filter_map(|id| inputs.editor_text(*id)).collect()
    }

    /// The titles of the buttons on the card of the last row.
    pub fn last_card_buttons(&self) -> Vec<String> {
        fn collect(view: &NSView, out: &mut Vec<String>) {
            if let Some(button) = view.downcast_ref::<objc2_app_kit::NSButton>() {
                out.push(button.title().to_string());
            }
            for child in view.subviews().iter() {
                collect(&child, out);
            }
        }
        let last = self.table.numberOfRows() - 1;
        let Some(view) = (last >= 0)
            .then(|| self.table.viewAtColumn_row_makeIfNecessary(0, last, true))
            .flatten()
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        collect(&view, &mut out);
        out
    }

    /// Clicks the `nth` button whose title starts with `title` on the card of the last row, as the user would;
    /// whether there was one.
    pub fn press_last(&self, title: &str, nth: usize) -> bool {
        fn collect(view: &NSView, title: &str, out: &mut Vec<Retained<objc2_app_kit::NSButton>>) {
            if let Some(button) = view.downcast_ref::<objc2_app_kit::NSButton>()
                && button.title().to_string().starts_with(title)
            {
                out.push(objc2::Message::retain(button));
            }
            for child in view.subviews().iter() {
                collect(&child, title, out);
            }
        }
        let last = self.table.numberOfRows() - 1;
        let mut found = Vec::new();
        if let Some(view) = (last >= 0)
            .then(|| self.table.viewAtColumn_row_makeIfNecessary(0, last, true))
            .flatten()
        {
            collect(&view, title, &mut found);
        }
        let Some(button) = found.get(nth) else {
            return false;
        };
        // SAFETY: A click without a sender, on the main thread, sends the button's action as a real click does.
        unsafe { button.performClick(None) };
        true
    }

    /// Clicks the button of the card of entry `id` whose title starts with `label` or whose tooltip is `label`, as
    /// the user would; whether there was one.
    pub fn press_on(&self, id: u64, label: &str) -> bool {
        fn find(view: &NSView, label: &str) -> Option<Retained<objc2_app_kit::NSButton>> {
            if let Some(button) = view.downcast_ref::<objc2_app_kit::NSButton>()
                && (button.title().to_string().starts_with(label)
                    || button.toolTip().is_some_and(|tip| tip.to_string() == label))
            {
                return Some(objc2::Message::retain(button));
            }
            view.subviews().iter().find_map(|child| find(&child, label))
        }
        let row = self
            .positions
            .borrow()
            .iter()
            .find_map(|(entry, row)| (entry.get() == id).then_some(*row));
        let Some(row) = row.and_then(|r| isize::try_from(r).ok()) else {
            return false;
        };
        self.table.scrollRowToVisible(row);
        let button = self
            .table
            .viewAtColumn_row_makeIfNecessary(0, row, true)
            .and_then(|view| find(&view, label));
        if let Some(button) = &button {
            // SAFETY: A click without a sender, on the main thread, sends the button's action as a real click does.
            unsafe { button.performClick(None) };
        }
        button.is_some()
    }

    /// Selects the row of entry `id` and sends the table the key `key`, as typing it with the timeline focused does;
    /// whether the entry has a row.
    pub fn key_on(&self, id: u64, key: &str) -> bool {
        let row = self
            .positions
            .borrow()
            .iter()
            .find_map(|(entry, row)| (entry.get() == id).then_some(*row));
        let Some(row) = row else { return false };
        self.table.scrollRowToVisible(isize::try_from(row).unwrap_or(0));
        self.table
            .selectRowIndexes_byExtendingSelection(&objc2_foundation::NSIndexSet::indexSetWithIndex(row), false);
        let window = self.table.window().map_or(0, |w| w.windowNumber());
        let event = objc2_app_kit::NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            objc2_app_kit::NSEventType::KeyDown,
            objc2_foundation::NSPoint::new(0.0, 0.0),
            objc2_app_kit::NSEventModifierFlags::empty(),
            0.0,
            window,
            None,
            &crate::common::util::ns(key),
            &crate::common::util::ns(key),
            false,
            49,
        );
        if let Some(event) = event {
            self.table.keyDown(&event);
        }
        true
    }

    /// Whether `part` of entry `id` is open.
    pub fn is_open(&self, id: u64, part: super::cards::Part) -> bool {
        self.expanded.borrow().contains(&(id, part))
    }

    /// Replaces what the input editor of permission request `id` holds, as typing would; whether it has one.
    pub fn edit_input(&self, id: u64, text: &str) -> bool {
        let inputs = self.inputs.borrow();
        let view = inputs
            .editors
            .get(&id)
            .and_then(|scroll| scroll.documentView())
            .and_then(|view| view.downcast::<objc2_app_kit::NSTextView>().ok());
        if let Some(view) = &view {
            view.setString(&crate::common::util::ns(text));
        }
        view.is_some()
    }

    /// Types `text` into the free-text field `what` of request `id`; whether it has one.
    pub fn type_field(&self, id: u64, what: u8, text: &str) -> bool {
        let inputs = self.inputs.borrow();
        let field = inputs.fields.get(&(id, what));
        if let Some(field) = field {
            field.setStringValue(&crate::common::util::ns(text));
        }
        field.is_some()
    }

    /// Ticks the options titled `titles` of question `question` of request `id`, as clicks on them would.
    pub fn tick_options(&self, id: u64, question: u8, titles: &[&str]) -> usize {
        let inputs = self.inputs.borrow();
        let boxes = inputs.checks.get(&(id, question)).cloned().unwrap_or_default();
        drop(inputs);
        let mut ticked = 0;
        for button in boxes
            .iter()
            .filter(|b| titles.contains(&b.title().to_string().as_str()))
        {
            // SAFETY: A click without a sender, on the main thread, toggles the checkbox as a real click does.
            unsafe { button.performClick(None) };
            ticked += 1;
        }
        ticked
    }

    /// The cards in view, as their VoiceOver summaries.
    pub fn visible_summaries(&self) -> Vec<String> {
        let range = self.table.rowsInRect(self.table.visibleRect());
        (range.location..range.location + range.length)
            .filter_map(|row| {
                let view = self
                    .table
                    .viewAtColumn_row_makeIfNecessary(0, isize::try_from(row).ok()?, false)?;
                let card = view.subviews().firstObject()?;
                card.accessibilityLabel().map(|l| l.to_string())
            })
            .collect()
    }
}
