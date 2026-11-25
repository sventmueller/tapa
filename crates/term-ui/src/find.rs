//! The find bar above a session (F104), laid out as macOS's own find bar: a search field whose menu holds the switches
//! for case, regular expressions and whole words and the recent searches, the count of matches, a control for the
//! previous and next match and Done. Return goes to the next match, ⇧Return to the previous, Escape and Done close the
//! bar; the session searches and selects the match.

use std::cell::Cell;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSBezelStyle, NSBox, NSBoxType, NSButton, NSColor, NSControl, NSControlSize, NSControlStateValueOff,
    NSControlStateValueOn, NSControlTextEditingDelegate, NSEventModifierFlags, NSFont, NSLayoutAttribute,
    NSLayoutConstraintOrientation, NSMenu, NSMenuItem, NSSearchField, NSSearchFieldClearRecentsMenuItemTag,
    NSSearchFieldDelegate, NSSearchFieldNoRecentsMenuItemTag, NSSearchFieldRecentsMenuItemTag,
    NSSearchFieldRecentsTitleMenuItemTag, NSSegmentSwitchTracking, NSSegmentedControl, NSStackView, NSTextField,
    NSTextFieldDelegate, NSTextView, NSUserInterfaceLayoutOrientation, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectView,
};
use objc2_foundation::{NSArray, NSEdgeInsets, NSNotification, NSObject, NSObjectProtocol, NSString};
use splat_i18n::{t, tf};
use term_session::{FindOptions, Found};

use crate::kit::{guarded, label, ns, set_action, symbol};

/// How many recent searches the field's menu keeps.
const RECENTS: isize = 10;

/// The switches in the field's menu, by the tag of their item.
const CASE: isize = 0;
const REGEX: isize = 1;
const WORD: isize = 2;

/// What the bar's delegate keeps: the view it searches in, by id, the switches and the field whose menu shows them.
pub(crate) struct Ivars {
    view: u64,
    options: [Cell<bool>; 3],
    field: Weak<NSSearchField>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Delegate` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermFindDelegate"]
    #[ivars = Ivars]
    pub(crate) struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSControlTextEditingDelegate for Delegate {
        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command(&self, _control: &NSControl, _view: &NSTextView, command: Sel) -> bool {
            guarded(false, || self.command(command))
        }
    }

    unsafe impl NSTextFieldDelegate for Delegate {}

    unsafe impl NSSearchFieldDelegate for Delegate {}

    impl Delegate {
        #[unsafe(method(controlTextDidChange:))]
        fn text_changed(&self, _notification: &NSNotification) {
            guarded((), || self.with_view(|view| view.find_step(None)));
        }

        #[unsafe(method(termFindStep:))]
        fn step(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                let forward = sender
                    .and_then(|sender| sender.downcast_ref::<NSSegmentedControl>())
                    .is_none_or(|control| control.selectedSegment() != 0);
                self.with_view(|view| view.find_step(Some(forward)));
            });
        }

        #[unsafe(method(termFindAgain:))]
        fn again(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.with_view(|view| view.find_step(None)));
        }

        #[unsafe(method(termFindOption:))]
        fn option(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                let Some(item) = sender.and_then(|sender| sender.downcast_ref::<NSMenuItem>()) else {
                    return;
                };
                if let Some(option) = usize::try_from(item.tag()).ok().and_then(|tag| self.ivars().options.get(tag)) {
                    option.set(!option.get());
                }
                self.refresh_menu();
                self.with_view(|view| view.find_step(None));
            });
        }

        #[unsafe(method(termFindClose:))]
        fn close(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.with_view(|view| view.end_find()));
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker, view: u64, field: &NSSearchField) -> Retained<Delegate> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            view,
            options: Default::default(),
            field: Weak::from(field),
        });
        // SAFETY: NSObject's designated initializer, called once on the newly allocated object.
        unsafe { msg_send![super(this), init] }
    }

    fn with_view(&self, f: impl FnOnce(&crate::TerminalView)) {
        if let Some(view) = crate::find(self.ivars().view) {
            f(&view);
        }
    }

    /// Return and ⇧Return step through the matches and keep the search among the recent ones, Escape closes; whether
    /// the command was handled.
    fn command(&self, command: Sel) -> bool {
        if command == sel!(insertNewline:) {
            let shift = NSApplication::sharedApplication(self.mtm())
                .currentEvent()
                .is_some_and(|event| event.modifierFlags().contains(NSEventModifierFlags::Shift));
            self.remember();
            self.with_view(|view| view.find_step(Some(!shift)));
            return true;
        }
        if command == sel!(cancelOperation:) {
            self.with_view(|view| view.end_find());
            return true;
        }
        false
    }

    fn remember(&self) {
        let Some(field) = self.ivars().field.load() else { return };
        let pattern = field.stringValue();
        if pattern.length() == 0 {
            return;
        }
        let mut recents: Vec<Retained<NSString>> = field
            .recentSearches()
            .iter()
            .filter(|recent| **recent != *pattern)
            .collect();
        recents.insert(0, pattern);
        recents.truncate(RECENTS.unsigned_abs());
        field.setRecentSearches(&NSArray::from_retained_slice(&recents));
    }

    fn options(&self) -> FindOptions {
        let [case, regex, word] = &self.ivars().options;
        FindOptions {
            regex: regex.get(),
            case_sensitive: case.get(),
            whole_word: word.get(),
        }
    }

    /// Builds the field's menu anew with the switches as they are, and says in the empty field which are on.
    fn refresh_menu(&self) {
        let Some(field) = self.ivars().field.load() else { return };
        let mtm = self.mtm();
        let menu = NSMenu::new(mtm);
        let names = option_names();
        for (tag, name) in names.iter().enumerate() {
            let tag = isize::try_from(tag).unwrap_or_default();
            let item = crate::kit::tagged(mtm, name, sel!(termFindOption:), "", tag);
            crate::kit::set_target(&item, self);
            let on = self.ivars().options[tag.unsigned_abs()].get();
            item.setState(if on {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            menu.addItem(&item);
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        for (title, tag) in [
            (t("Letzte Suchen"), NSSearchFieldRecentsTitleMenuItemTag),
            ("", NSSearchFieldRecentsMenuItemTag),
            (t("Keine letzten Suchen"), NSSearchFieldNoRecentsMenuItemTag),
        ] {
            let item = crate::kit::item(mtm, title, None, "");
            item.setTag(tag);
            menu.addItem(&item);
        }
        let clear = crate::kit::item(mtm, t("Letzte Suchen löschen"), None, "");
        clear.setTag(NSSearchFieldClearRecentsMenuItemTag);
        let divider = NSMenuItem::separatorItem(mtm);
        divider.setTag(NSSearchFieldClearRecentsMenuItemTag);
        menu.addItem(&divider);
        menu.addItem(&clear);
        field.setSearchMenuTemplate(Some(&menu));
        let on: Vec<&str> = names
            .iter()
            .zip(&self.ivars().options)
            .filter(|(_, option)| option.get())
            .map(|(name, _)| *name)
            .collect();
        let placeholder = if on.is_empty() {
            t("Im Verlauf suchen").to_string()
        } else {
            on.join(" · ")
        };
        field.setPlaceholderString(Some(&ns(&placeholder)));
    }
}

fn option_names() -> [&'static str; 3] {
    let mut names = [""; 3];
    for (tag, name) in [
        (CASE, t("Groß- und Kleinschreibung beachten")),
        (REGEX, t("Regulärer Ausdruck")),
        (WORD, t("Nur ganze Wörter")),
    ] {
        names[tag.unsigned_abs()] = name;
    }
    names
}

/// The bar and its controls.
pub(crate) struct FindBar {
    root: Retained<NSView>,
    field: Retained<NSSearchField>,
    count: Retained<NSTextField>,
    delegate: Retained<Delegate>,
}

fn search_field(mtm: MainThreadMarker) -> Retained<NSSearchField> {
    let field = NSSearchField::new(mtm);
    field.setControlSize(NSControlSize::Small);
    field.setFont(Some(&NSFont::systemFontOfSize(NSFont::smallSystemFontSize())));
    field.setSendsWholeSearchString(true);
    field.setMaximumRecents(RECENTS);
    field.setRecentsAutosaveName(Some(&ns("TermFindRecents")));
    crate::kit::set_label(&field, Some(t("Im Verlauf suchen")));
    field.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    field
}

/// The control for the previous and the next match.
fn steps(mtm: MainThreadMarker, target: &AnyObject) -> Retained<NSSegmentedControl> {
    let images = NSArray::from_retained_slice(&[
        symbol("chevron.left", t("Vorheriger Treffer")).unwrap_or_default(),
        symbol("chevron.right", t("Nächster Treffer")).unwrap_or_default(),
    ]);
    // SAFETY: A nil target and no action until `set_action` gives it both.
    let control = unsafe {
        NSSegmentedControl::segmentedControlWithImages_trackingMode_target_action(
            &images,
            NSSegmentSwitchTracking::Momentary,
            None,
            None,
            mtm,
        )
    };
    control.setControlSize(NSControlSize::Small);
    control.setToolTip_forSegment(Some(&ns(t("Vorheriger Treffer"))), 0);
    control.setToolTip_forSegment(Some(&ns(t("Nächster Treffer"))), 1);
    set_action(&control, target, sel!(termFindStep:));
    control
}

/// The bar's ground, a material as the window's bars have, with a separator along its bottom.
fn ground(mtm: MainThreadMarker, row: &NSView) -> Retained<NSView> {
    let ground = NSVisualEffectView::new(mtm);
    ground.setMaterial(NSVisualEffectMaterial::HeaderView);
    ground.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
    let line = NSBox::new(mtm);
    line.setBoxType(NSBoxType::Separator);
    ground.addSubview(row);
    ground.addSubview(&line);
    crate::kit::pin_sides(row, &ground, 0.0);
    crate::kit::pin_sides(&line, &ground, 0.0);
    crate::kit::activate([
        row.topAnchor().constraintEqualToAnchor(&ground.topAnchor()),
        line.topAnchor().constraintEqualToAnchor(&row.bottomAnchor()),
        line.bottomAnchor().constraintEqualToAnchor(&ground.bottomAnchor()),
    ]);
    ground.into_super()
}

impl FindBar {
    /// A hidden bar for the view `view`.
    pub(crate) fn new(mtm: MainThreadMarker, view: u64) -> FindBar {
        let field = search_field(mtm);
        let delegate = Delegate::new(mtm, view, &field);
        let target: &AnyObject = &delegate;
        // SAFETY: The field holds its delegate weakly; the bar keeps the delegate as long as the field.
        unsafe { field.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*delegate))) };
        set_action(&field, target, sel!(termFindAgain:));
        delegate.refresh_menu();
        let count = label(mtm, "");
        count.setTextColor(Some(&NSColor::secondaryLabelColor()));
        count.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
            NSFont::smallSystemFontSize(),
            0.0,
        )));
        let previous_next = steps(mtm, target);
        // SAFETY: A nil target and no action until `set_action` gives it both.
        let done = unsafe { NSButton::buttonWithTitle_target_action(&ns(t("Fertig")), None, None, mtm) };
        done.setBezelStyle(NSBezelStyle::Push);
        done.setControlSize(NSControlSize::Small);
        done.setFont(Some(&NSFont::systemFontOfSize(NSFont::smallSystemFontSize())));
        done.setToolTip(Some(&ns(t("Suche schließen"))));
        set_action(&done, target, sel!(termFindClose:));
        let row = NSStackView::new(mtm);
        row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        row.setAlignment(NSLayoutAttribute::CenterY);
        row.setSpacing(crate::kit::space::M);
        row.setEdgeInsets(NSEdgeInsets {
            top: 5.0,
            left: 10.0,
            bottom: 5.0,
            right: 10.0,
        });
        let views: [&NSView; 4] = [&field, &count, &previous_next, &done];
        for view in views {
            row.addArrangedSubview(view);
        }
        let root = ground(mtm, &row);
        root.setHidden(true);
        FindBar {
            root,
            field,
            count,
            delegate,
        }
    }

    pub(crate) fn view(&self) -> &NSView {
        &self.root
    }

    /// Shows the bar and puts the keyboard into its field, with the field's text selected.
    pub(crate) fn open(&self) {
        self.root.setHidden(false);
        if let Some(window) = self.root.window() {
            window.makeFirstResponder(Some(&self.field));
        }
        // SAFETY: Selecting the field's text with no sender, as the menu command does.
        unsafe { self.field.selectText(None) };
    }

    pub(crate) fn close(&self) {
        self.root.setHidden(true);
        self.count.setStringValue(&ns(""));
    }

    pub(crate) fn is_open(&self) -> bool {
        !self.root.isHidden()
    }

    /// Puts `text` into the field, as typing it would.
    pub(crate) fn set_pattern(&self, text: &str) {
        self.field.setStringValue(&ns(text));
    }

    pub(crate) fn pattern(&self) -> String {
        self.field.stringValue().to_string()
    }

    pub(crate) fn options(&self) -> FindOptions {
        self.delegate.options()
    }

    /// Shows what the search found, or why it could not run.
    pub(crate) fn show(&self, result: &Result<Found, String>) {
        let text = match result {
            Ok(Found { count: 0, .. }) if self.pattern().is_empty() => String::new(),
            Ok(Found { count: 0, .. }) => t("Keine Treffer").to_string(),
            Ok(Found {
                count,
                current: Some(current),
            }) => tf("{} von {}", &[&(current + 1).to_string(), &count.to_string()]),
            Ok(Found { count, current: None }) => tf("{} Treffer", &[&count.to_string()]),
            Err(_) => t("Ungültiger Ausdruck").to_string(),
        };
        let colour = if result.is_err() {
            NSColor::systemRedColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        self.count.setTextColor(Some(&colour));
        self.count.setStringValue(&ns(&text));
    }
}
