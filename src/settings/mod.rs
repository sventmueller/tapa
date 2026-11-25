//! The settings window (F159, E14): panes in a toolbar as macOS settings have them, each control writing its one
//! field into the settings file, which keeps the user's comments and unknown keys. The file stays the truth: a change
//! from outside reaches the window through the reload the file watch runs, and what the file gets wrong shows as the
//! warnings of reading it instead of being overwritten.

mod agents;
mod dictation;
mod editor;
mod fonts;
mod form;
mod keys;
mod panes;
mod profiles;
mod rules;
mod target;

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSEvent, NSEventModifierFlags, NSFont, NSStackView, NSTextField, NSTextView,
    NSToolbar, NSToolbarItem, NSUserInterfaceLayoutOrientation, NSView, NSWindow, NSWindowStyleMask,
    NSWindowToolbarStyle,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSUserDefaults};
use splat_i18n::{t, tf};
use term_config::{Action, Chord, Document, unique_profile_name};
use term_ui::kit::{ns, space, symbol};

use crate::state::app;
#[cfg(feature = "selftest")]
pub(crate) use agents::{installs_target, register_text};
use form::Fields;
pub(crate) use keys::title as action_title;
use target::Target;

/// The window's panes, in the order of the toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pane {
    Window,
    Font,
    Theme,
    Cursor,
    Terminal,
    Input,
    Editor,
    Agents,
    Dictation,
    Profiles,
    Keys,
}

impl Pane {
    pub(crate) const ALL: [Pane; 11] = [
        Pane::Window,
        Pane::Font,
        Pane::Theme,
        Pane::Cursor,
        Pane::Terminal,
        Pane::Input,
        Pane::Editor,
        Pane::Agents,
        Pane::Dictation,
        Pane::Profiles,
        Pane::Keys,
    ];

    /// The toolbar identifier, also the name the self-test opens the pane by.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Pane::Window => "window",
            Pane::Font => "font",
            Pane::Theme => "theme",
            Pane::Cursor => "cursor",
            Pane::Terminal => "terminal",
            Pane::Input => "input",
            Pane::Editor => "editor",
            Pane::Agents => "agents",
            Pane::Dictation => "dictation",
            Pane::Profiles => "profiles",
            Pane::Keys => "keys",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Pane::Window => t("Allgemein"),
            Pane::Font => t("Schrift"),
            Pane::Theme => t("Farben"),
            Pane::Cursor => t("Cursor"),
            Pane::Terminal => t("Terminal"),
            Pane::Input => t("Eingabe"),
            Pane::Editor => t("Editor"),
            Pane::Agents => t("Agenten"),
            Pane::Dictation => t("Diktat"),
            Pane::Profiles => t("Profile"),
            Pane::Keys => t("Tastenkürzel"),
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Pane::Window => "gearshape",
            Pane::Font => "textformat",
            Pane::Theme => "paintpalette",
            Pane::Cursor => "character.cursor.ibeam",
            Pane::Terminal => "terminal",
            Pane::Input => "character.textbox",
            Pane::Editor => "doc.text",
            Pane::Agents => "sparkles",
            Pane::Dictation => "mic",
            Pane::Profiles => "person.crop.circle",
            Pane::Keys => "keyboard",
        }
    }

    fn index(self) -> usize {
        Pane::ALL.iter().position(|pane| *pane == self).unwrap_or(0)
    }
}

/// The space around the panes, as macOS settings keep it.
const MARGIN: f64 = 20.0;
/// What a toolbar item takes beside its label, and the toolbar beside its items.
const ITEM_PADDING: f64 = 18.0;
const TOOLBAR_PADDING: f64 = 40.0;

/// The width the toolbar needs to show every pane's item, none of them in its overflow menu.
fn toolbar_width(mtm: MainThreadMarker) -> f64 {
    let label = NSTextField::labelWithString(&ns(""), mtm);
    label.setFont(Some(&NSFont::systemFontOfSize(NSFont::smallSystemFontSize())));
    let items: f64 = Pane::ALL
        .iter()
        .map(|pane| {
            label.setStringValue(&ns(pane.title()));
            label.fittingSize().width + ITEM_PADDING
        })
        .sum();
    (items + TOOLBAR_PADDING).ceil()
}

/// The open window and what its controls need.
pub(crate) struct Settings {
    mtm: MainThreadMarker,
    window: Retained<NSWindow>,
    _target: Retained<Target>,
    _toolbar: Retained<NSToolbar>,
    stack: Retained<NSStackView>,
    notice: Retained<NSTextField>,
    views: Vec<Retained<NSView>>,
    shown: Cell<Pane>,
    fields: Fields,
    notes: Rc<RefCell<Vec<String>>>,
    chosen: profiles::Chosen,
    chord_fields: Vec<Retained<NSTextField>>,
    rules: Retained<rules::Rules>,
    recording: RefCell<Option<Retained<AnyObject>>>,
}

thread_local! {
    static OPEN: RefCell<Option<Rc<Settings>>> = const { RefCell::new(None) };
    static CLOSED: RefCell<Option<Rc<Settings>>> = const { RefCell::new(None) };
}

/// The user default that keeps the pane shown last across launches, as macOS settings windows do.
const LAST_PANE: &str = "SettingsPane";
/// The name the window's frame is saved under.
const FRAME_NAME: &str = "Settings";
/// Escape's key code.
const ESCAPE: u16 = 0x35;

fn last_pane() -> Pane {
    let saved = NSUserDefaults::standardUserDefaults().stringForKey(&ns(LAST_PANE));
    saved
        .and_then(|id| Pane::ALL.into_iter().find(|pane| pane.id() == id.to_string()))
        .unwrap_or(Pane::Window)
}

fn remember_pane(pane: Pane) {
    // SAFETY: A string is a property-list object, which is what the defaults store.
    unsafe { NSUserDefaults::standardUserDefaults().setObject_forKey(Some(&ns(pane.id())), &ns(LAST_PANE)) };
}

/// Runs `f` on the open window, holding no borrow of it while `f` runs, so `f` may reload the settings.
fn with_open<R>(f: impl FnOnce(&Settings) -> R) -> Option<R> {
    let open = OPEN.with(|cell| cell.borrow().clone())?;
    Some(f(&open))
}

/// Brings the settings window forward, opening it on the pane shown last.
pub(crate) fn show() {
    if with_open(|settings| settings.window.makeKeyAndOrderFront(None)).is_some() {
        return;
    }
    let settings = Rc::new(Settings::new(app().mtm));
    OPEN.with(|cell| *cell.borrow_mut() = Some(settings.clone()));
    settings.show_pane(last_pane());
    if !settings.window.setFrameUsingName(&ns(FRAME_NAME)) {
        settings.window.center();
    }
    settings.window.setFrameAutosaveName(&ns(FRAME_NAME));
    settings.fit();
    settings.window.makeKeyAndOrderFront(None);
}

/// Opens the window on the pane `id`; whether there is such a pane.
#[cfg(feature = "selftest")]
pub(crate) fn show_pane(id: &str) -> bool {
    let Some(pane) = Pane::ALL.iter().find(|pane| pane.id() == id) else {
        return false;
    };
    show();
    with_open(|settings| settings.show_pane(*pane));
    true
}

/// Scrolls the pane shown to its end, for the self-test's pictures of a pane that scrolls.
#[cfg(feature = "selftest")]
pub(crate) fn scroll_to_end() {
    with_open(|settings| {
        let view = &settings.views[settings.shown.get().index()];
        if let Some(scroll) = view.downcast_ref::<objc2_app_kit::NSScrollView>()
            && let Some(document) = scroll.documentView()
        {
            let end = if document.isFlipped() {
                document.frame().size.height
            } else {
                0.0
            };
            document.scrollPoint(NSPoint::new(0.0, end));
        }
    });
}

/// The settings window while it is open, for the self-test's pictures.
#[cfg(feature = "selftest")]
pub(crate) fn ns_window() -> Option<Retained<NSWindow>> {
    with_open(|settings| settings.window.clone()).filter(|window| window.isVisible())
}

/// Shows the settings the app just read; the reload after every change of the file calls it.
pub(crate) fn config_changed() {
    with_open(|settings| settings.refresh(None));
}

/// Closes the settings window for the close actions while it is in front, instead of the terminal behind it;
/// whether it did.
pub(crate) fn handles(action: Action) -> bool {
    if !matches!(action, Action::Close | Action::CloseTab | Action::CloseWindow) {
        return false;
    }
    with_open(|settings| settings.window.clone())
        .filter(|window| window.isKeyWindow())
        .map(|window| window.performClose(None))
        .is_some()
}

/// Whether the settings window has the keyboard.
pub(crate) fn is_key() -> bool {
    with_open(|settings| settings.window.isKeyWindow()).unwrap_or(false)
}

/// Closes the settings window on Escape, as long as no text field is being edited and no shortcut recorded; whether
/// it did.
pub(crate) fn escape(event: &NSEvent) -> bool {
    let modifiers = NSEventModifierFlags::Command
        | NSEventModifierFlags::Control
        | NSEventModifierFlags::Option
        | NSEventModifierFlags::Shift;
    if event.keyCode() != ESCAPE || event.modifierFlags().intersects(modifiers) || !owns_event(event) {
        return false;
    }
    let window = with_open(|settings| {
        let editing = settings
            .window
            .firstResponder()
            .is_some_and(|responder| responder.downcast_ref::<NSTextView>().is_some());
        (settings.recording.borrow().is_none() && !editing).then(|| settings.window.clone())
    })
    .flatten();
    window.map(|window| window.performClose(None)).is_some()
}

/// Forgets the window, which closed.
fn closed() {
    with_open(|settings| {
        settings.window.makeFirstResponder(None);
        settings.stop_recording();
    });
    let open = OPEN.with(|cell| cell.borrow_mut().take());
    if let Some(open) = open {
        // The window's delegate is among what it holds, and AppKit is still inside that delegate's call.
        CLOSED.with(|cell| *cell.borrow_mut() = Some(open));
        term_base::main_soon(|| CLOSED.with(|cell| drop(cell.borrow_mut().take())));
    }
}

/// Whether `event` is for the settings window.
fn owns_event(event: &NSEvent) -> bool {
    with_open(|settings| {
        event
            .window(settings.mtm)
            .is_some_and(|window| std::ptr::eq(&*window, &*settings.window))
    })
    .unwrap_or(false)
}

/// The toolbar item of `pane`, which shows it.
fn toolbar_item(target: &Target, pane: Pane) -> Retained<NSToolbarItem> {
    let item = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(target.mtm()), &ns(pane.id()));
    item.setLabel(&ns(pane.title()));
    item.setImage(symbol(pane.symbol(), pane.title()).as_deref());
    item.setTag(isize::try_from(pane.index()).unwrap_or(0));
    // SAFETY: The target answers `termSettingsPane:`, which takes the sender; the item holds the target weakly and
    // the window keeps it alive while the toolbar lives.
    unsafe {
        item.setTarget(Some(target));
        item.setAction(Some(sel!(termSettingsPane:)));
    }
    item
}

/// The new text of the settings file after `change`, or `None` when it changed nothing; a file that is no valid TOML
/// is refused, so the window never overwrites what the user is in the middle of writing.
fn edited(text: &str, change: impl FnOnce(&mut Document) -> Result<bool, String>) -> Result<Option<String>, String> {
    let mut doc = Document::parse(text).map_err(|error| {
        tf(
            "Die Einstellungsdatei ist kein gültiges TOML und bleibt, wie sie ist: {}",
            &[&error],
        )
    })?;
    if !change(&mut doc)? {
        return Ok(None);
    }
    let new = doc.text();
    Ok((new != text).then_some(new))
}

/// Changes the settings file at `path`, through a link to where it really is, and reloads it.
fn apply_at(path: &Path, change: impl FnOnce(&mut Document) -> Result<bool, String>) -> Result<bool, String> {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(tf(
                "Die Einstellungsdatei lässt sich nicht lesen: {}",
                &[&error.to_string()],
            ));
        }
    };
    let Some(new) = edited(&text, change)? else {
        return Ok(false);
    };
    term_base::atomic_write::write(&path, new.as_bytes()).map_err(|error| {
        tf(
            "Die Einstellungsdatei lässt sich nicht schreiben: {}",
            &[&error.to_string()],
        )
    })?;
    Ok(true)
}

/// Changes the app's settings file and has the app take up the change at once.
pub(crate) fn apply(change: impl FnOnce(&mut Document) -> Result<bool, String>) -> Result<(), String> {
    if apply_at(&app().config_path, change)? {
        crate::state::reload();
    }
    Ok(())
}

impl Settings {
    fn new(mtm: MainThreadMarker) -> Settings {
        let target = Target::new(mtm);
        let fields = Fields::new(mtm, Retained::into_super(Retained::into_super(target.clone())));
        let notes = Rc::new(RefCell::new(Vec::new()));
        let chosen: profiles::Chosen = Rc::new(RefCell::new(app().config().default_profile.clone()));
        let (keys_view, chord_fields) = keys::pane(&fields, &notes);
        let rules = rules::Rules::new(mtm);
        let views = vec![
            panes::window(&fields),
            panes::font(&fields),
            panes::theme(&fields),
            panes::cursor(&fields),
            panes::terminal(&fields),
            panes::input(&fields),
            editor::pane(&fields),
            agents::pane(&fields, &rules),
            dictation::pane(&fields),
            profiles::pane(&fields, &chosen),
            keys_view,
        ];
        let window = new_window(mtm, &target);
        let toolbar = NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), &ns("settings"));
        toolbar.setDelegate(Some(ProtocolObject::from_ref(&*target)));
        window.setToolbar(Some(&toolbar));
        window.setToolbarStyle(NSWindowToolbarStyle::Preference);
        let notice = NSTextField::wrappingLabelWithString(&ns(""), mtm);
        notice.setTextColor(Some(&NSColor::systemOrangeColor()));
        notice.setFont(Some(&NSFont::systemFontOfSize(NSFont::smallSystemFontSize())));
        notice.setPreferredMaxLayoutWidth(520.0);
        let stack = NSStackView::new(mtm);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setAlignment(objc2_app_kit::NSLayoutAttribute::Leading);
        stack.setSpacing(space::SECTION);
        stack.addArrangedSubview(&notice);
        fields.refresh(&app().config(), None);
        let views: Vec<Retained<NSView>> = views.iter().map(|view| form::scrolling(view)).collect();
        // One block as wide as the widest pane, in which every pane starts at the same edge, so the column of titles
        // stays where it is from pane to pane.
        let widest = views.iter().map(|view| view.fittingSize().width).fold(0.0, f64::max);
        let content = NSView::new(mtm);
        content.addSubview(&stack);
        stack.setTranslatesAutoresizingMaskIntoConstraints(false);
        term_ui::kit::activate([
            stack
                .topAnchor()
                .constraintEqualToAnchor_constant(&content.topAnchor(), MARGIN),
            stack
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&content.bottomAnchor(), -MARGIN),
            stack.centerXAnchor().constraintEqualToAnchor(&content.centerXAnchor()),
            stack.widthAnchor().constraintEqualToConstant(widest.ceil()),
            content
                .widthAnchor()
                .constraintGreaterThanOrEqualToConstant((widest + 2.0 * MARGIN).ceil().max(toolbar_width(mtm))),
        ]);
        window.setContentView(Some(&content));
        Settings {
            mtm,
            window,
            _target: target,
            _toolbar: toolbar,
            stack,
            notice,
            views,
            shown: Cell::new(Pane::Window),
            fields,
            notes,
            chosen,
            chord_fields,
            rules,
            recording: RefCell::new(None),
        }
    }

    /// Shows `pane`, the window growing or shrinking to it from its top edge.
    fn show_pane(&self, pane: Pane) {
        self.stop_recording();
        for view in self.stack.arrangedSubviews().iter() {
            if !std::ptr::eq(&*view, &**self.notice as &NSView) {
                self.stack.removeView(&view);
            }
        }
        self.stack.addArrangedSubview(&self.views[pane.index()]);
        self.shown.set(pane);
        if let Some(toolbar) = self.window.toolbar() {
            toolbar.setSelectedItemIdentifier(Some(&ns(pane.id())));
        }
        self.window.setTitle(&ns(pane.title()));
        if pane == Pane::Agents {
            self.rules.reload();
            agents::shown();
        }
        remember_pane(pane);
        self.refresh(None);
        self.fit();
    }

    fn fit(&self) {
        let Some(content) = self.window.contentView() else {
            return;
        };
        content.layoutSubtreeIfNeeded();
        let size = content.fittingSize();
        let frame = self.window.frame();
        let wanted = self.window.frameRectForContentRect(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(size.width, size.height),
        ));
        let top = frame.origin.y + frame.size.height;
        let next = NSRect::new(NSPoint::new(frame.origin.x, top - wanted.size.height), wanted.size);
        self.window
            .setFrame_display_animate(next, true, self.window.isVisible());
    }

    /// Shows the settings in force and what is wrong with them; `force` is a field updated even while it is edited.
    fn refresh(&self, force: Option<isize>) {
        let app = app();
        let config = app.config();
        self.fields.refresh(&config, force);
        let mut lines: Vec<String> = app.warnings.borrow().iter().map(|warning| warning.0.clone()).collect();
        lines.extend(keys::warnings(&config));
        lines.extend(self.notes.borrow().iter().cloned());
        lines.dedup();
        let text = lines
            .iter()
            .map(|line| format!("⚠︎ {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        self.notice.setStringValue(&ns(&text));
        self.notice.setHidden(text.is_empty());
    }

    /// The user changed the control `tag`.
    fn changed(&self, tag: isize) {
        let Some((shown, write)) = self.fields.taken(tag, &app().config()) else {
            return;
        };
        self.notes.borrow_mut().clear();
        if let Err(error) = apply(|doc| write(doc, &shown)) {
            self.notes.borrow_mut().push(error);
        }
        self.refresh(Some(tag));
    }

    fn add_profile(&self) {
        let name = unique_profile_name(&app().config(), t("Neues Profil"));
        self.notes.borrow_mut().clear();
        let added = apply(|doc| {
            doc.add_profile(&name).map_err(|error| profiles::explain(&error))?;
            Ok(true)
        });
        match added {
            Ok(()) => *self.chosen.borrow_mut() = name,
            Err(error) => self.notes.borrow_mut().push(error),
        }
        self.refresh(None);
    }

    fn delete_profile(&self) {
        let name = self.chosen.borrow().clone();
        if !app().config().profiles.iter().any(|p| p.name == name) || !profiles::confirm_delete(self.mtm, &name) {
            return;
        }
        self.notes.borrow_mut().clear();
        let deleted = apply(|doc| {
            doc.delete_profile(&name).map_err(|error| profiles::explain(&error))?;
            Ok(true)
        });
        if let Err(error) = deleted {
            self.notes.borrow_mut().push(error);
        }
        self.refresh(None);
    }

    /// Waits for the chord of the action at `tag` in `Action::ALL`.
    fn record(&self, tag: isize) {
        let Some((&action, field)) = usize::try_from(tag)
            .ok()
            .and_then(|at| Some((Action::ALL.get(at)?, self.chord_fields.get(at)?)))
        else {
            return;
        };
        self.stop_recording();
        field.setStringValue(&ns(""));
        field.setPlaceholderString(Some(&ns(t("Kürzel drücken …"))));
        let monitor = keys::record(move |chord| {
            term_base::main_soon(move || {
                with_open(|settings| settings.recorded(action, chord));
            });
        });
        *self.recording.borrow_mut() = monitor;
    }

    fn recorded(&self, action: Action, chord: Option<Chord>) {
        self.stop_recording();
        if let Some(chord) = chord {
            self.notes.borrow_mut().clear();
            let notes = self.notes.clone();
            if let Err(error) = apply(|doc| Ok(keys::bind(doc, action, &[chord], &notes))) {
                self.notes.borrow_mut().push(error);
            }
        }
        self.refresh(None);
    }

    fn stop_recording(&self) {
        if let Some(monitor) = self.recording.borrow_mut().take() {
            keys::stop(&monitor);
        }
        for field in &self.chord_fields {
            field.setPlaceholderString(Some(&ns(t("Kein Kürzel"))));
        }
    }
}

fn new_window(mtm: MainThreadMarker, target: &Target) -> Retained<NSWindow> {
    let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
    // SAFETY: A new window of the main thread's AppKit; the settings keep their own reference.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(560.0, 300.0)),
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: The settings keep their own reference, so closing must not release the window under them.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setDelegate(Some(ProtocolObject::from_ref(target)));
    window
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_is_written_into_the_users_text() {
        let dir = term_base::testing::scratch("settings-apply");
        let path = dir.join("config.toml");
        std::fs::write(&path, "# mine\n[font]\nsize = 13 # small\n[odd]\nx = 1\n").unwrap();
        let written = apply_at(&path, |doc| {
            doc.set(&["font", "size"], term_config::number(14.0));
            Ok(true)
        });
        assert_eq!(written, Ok(true));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# mine\n[font]\nsize = 14 # small\n[odd]\nx = 1\n"
        );
        assert_eq!(apply_at(&path, |_| Ok(false)), Ok(false));
    }

    #[test]
    fn a_broken_file_is_left_alone() {
        let dir = term_base::testing::scratch("settings-broken");
        let path = dir.join("config.toml");
        std::fs::write(&path, "font = [").unwrap();
        let written = apply_at(&path, |doc| {
            doc.set(&["font", "size"], 14);
            Ok(true)
        });
        assert!(written.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "font = [");
    }

    #[test]
    fn a_missing_file_is_made() {
        let dir = term_base::testing::scratch("settings-new");
        let path = dir.join("config.toml");
        apply_at(&path, |doc| {
            doc.set(&["cursor", "blink"], false);
            Ok(true)
        })
        .unwrap();
        let (config, warnings) = term_config::Config::load(&path).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(!config.cursor.blink);
    }

    #[test]
    fn a_refused_change_writes_nothing() {
        assert_eq!(edited("version = 1\n", |_| Err("no".into())), Err("no".to_string()));
        assert_eq!(edited("version = 1\n", |_| Ok(true)), Ok(None));
    }

    #[test]
    fn every_pane_has_its_own_identifier() {
        let mut ids: Vec<&str> = Pane::ALL.iter().map(|pane| pane.id()).collect();
        ids.dedup();
        assert_eq!(ids.len(), Pane::ALL.len());
        assert!(Pane::ALL.iter().enumerate().all(|(at, pane)| pane.index() == at));
    }
}
