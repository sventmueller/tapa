//! The keys pane (F167): every action with its chords, editable as text or by pressing the chord, and the chords
//! an action takes from another reported beside the warnings of the file's `[keys]`.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::sel;
use objc2_app_kit::{
    NSButton, NSEvent, NSEventMask, NSEventModifierFlags, NSScrollView, NSStackView, NSTextField, NSView,
};
use objc2_foundation::NSArray;
use splat_i18n::{t, tf};
use term_config::{Action, Chord, Config, chords_text, parse_chords};
use term_ui::kit::{ns, set_action, space};

use super::form::{Fields, Form, Shown};

/// Escape, which ends recording without a chord.
const ESCAPE: u16 = 0x35;

/// The title of `action` as the menus, the palette and the window show it; the file's name where none is given.
pub(crate) fn title(action: Action) -> String {
    let titles = [
        (Action::NewWindow, t("Neues Fenster")),
        (Action::NewWorkspace, t("Neuer Arbeitsbereich …")),
        (Action::NewTab, t("Neuer Tab")),
        (Action::RenameWorkspace, t("Arbeitsbereich umbenennen")),
        (Action::NewClaudeTab, t("Neuer Claude-Tab")),
        (Action::NewCodexTab, t("Neuer Codex-Tab")),
        (Action::NewOpenCodeTab, t("Neuer OpenCode-Tab")),
        (Action::NewBrowserTab, t("Neuer Browser-Tab")),
        (Action::NewFile, t("Neue Datei")),
        (Action::OpenFile, t("Öffnen …")),
        (Action::Save, t("Sichern")),
        (Action::SaveAs, t("Sichern unter …")),
        (Action::SaveAll, t("Alles sichern")),
        (Action::ToggleFiles, t("Dateien ein-/ausblenden")),
        (Action::ToggleGit, t("Git ein-/ausblenden")),
        (Action::ToggleDictation, t("Diktat starten")),
        (Action::ToggleTrust, t("Arbeitsbereich vertrauen")),
        (Action::Close, t("Schließen")),
        (Action::CloseTab, t("Tab schließen")),
        (Action::CloseWorkspace, t("Arbeitsbereich entfernen")),
        (Action::CloseWindow, t("Fenster schließen")),
        (Action::SplitRight, t("Rechts teilen")),
        (Action::SplitDown, t("Unten teilen")),
        (Action::ClosePane, t("Bereich schließen")),
        (Action::ZoomPane, t("Bereich maximieren")),
        (Action::EqualizePanes, t("Bereiche angleichen")),
        (Action::ResizeLeft, t("Trennlinie nach links")),
        (Action::ResizeRight, t("Trennlinie nach rechts")),
        (Action::ResizeUp, t("Trennlinie nach oben")),
        (Action::ResizeDown, t("Trennlinie nach unten")),
        (Action::NextPane, t("Nächsten Bereich auswählen")),
        (Action::PreviousPane, t("Vorherigen Bereich auswählen")),
        (Action::NextTab, t("Nächsten Tab zeigen")),
        (Action::PreviousTab, t("Vorherigen Tab zeigen")),
        (Action::NextWorkspace, t("Nächsten Arbeitsbereich zeigen")),
        (Action::PreviousWorkspace, t("Vorherigen Arbeitsbereich zeigen")),
        (Action::TabSwitcher, t("Schnell öffnen …")),
        (Action::Copy, t("Kopieren")),
        (Action::CopyFormatted, t("Mit Formatierung kopieren")),
        (Action::Paste, t("Einsetzen")),
        (Action::SelectAll, t("Alles auswählen")),
        (Action::Clear, t("Leeren")),
        (Action::Find, t("Suchen …")),
        (Action::FindNext, t("Weitersuchen")),
        (Action::FindPrevious, t("Rückwärts suchen")),
        (Action::PreviousPrompt, t("Voriger Prompt")),
        (Action::NextPrompt, t("Nächster Prompt")),
        (Action::SelectBlock, t("Block auswählen")),
        (Action::CopyOutput, t("Ausgabe kopieren")),
        (Action::SaveOutput, t("Ausgabe sichern …")),
        (Action::ToggleBlock, t("Block ein-/ausklappen")),
        (Action::ToggleBookmark, t("Lesezeichen setzen/entfernen")),
        (Action::PreviousBookmark, t("Voriges Lesezeichen")),
        (Action::NextBookmark, t("Nächstes Lesezeichen")),
        (Action::ScrollTop, t("Zum Anfang")),
        (Action::ScrollBottom, t("Zum Ende")),
        (Action::PageUp, t("Seite hoch")),
        (Action::PageDown, t("Seite runter")),
        (Action::FontBigger, t("Größer")),
        (Action::FontSmaller, t("Kleiner")),
        (Action::FontReset, t("Originalgröße")),
        (Action::ToggleInputField, t("Eingabefeld ein-/ausblenden")),
        (Action::ToggleSidebar, t("Seitenleiste ein-/ausblenden")),
        (Action::CopyMode, t("Auswahl mit der Tastatur")),
        (Action::CommandPalette, t("Befehlspalette …")),
        (Action::QuickTerminal, t("Schnelles Terminal ein-/ausblenden")),
        (Action::Settings, t("Einstellungen …")),
        (Action::ReloadConfig, t("Einstellungen neu laden")),
        (Action::SecureKeyboardEntry, t("Sichere Tastatureingabe")),
        (Action::LastTab, t("Letzter Tab")),
    ];
    if let Some((_, title)) = titles.iter().find(|(known, _)| *known == action) {
        return (*title).to_string();
    }
    match action.name().strip_prefix("tab_") {
        Some(number) => tf("Tab {}", &[number]),
        None => action.name().to_string(),
    }
}

/// The chords bound to `action` in `config`.
fn chords_of(config: &Config, action: Action) -> Vec<Chord> {
    let (bindings, _) = config.bindings();
    bindings
        .into_iter()
        .filter(|binding| binding.action == action)
        .map(|binding| binding.chord)
        .collect()
}

/// What the pane says about the bindings in force: the file's mistakes and chords bound twice.
pub(super) fn warnings(config: &Config) -> Vec<String> {
    let (bindings, warnings) = config.bindings();
    let mut out: Vec<String> = warnings.into_iter().map(|warning| warning.0).collect();
    for (chord, actions) in term_config::conflicts(&bindings) {
        let names: Vec<String> = actions.iter().map(|action| format!("„{}“", title(*action))).collect();
        out.push(tf(
            "{} ist mehrfach belegt: {}",
            &[&chord.to_string(), &names.join(", ")],
        ));
    }
    out
}

/// The notes for chords `action` took from other actions.
fn taken_notes(action: Action, taken: &[(Chord, Action)]) -> Vec<String> {
    taken
        .iter()
        .map(|(chord, from)| {
            tf(
                "{} gehört jetzt zu „{}“ statt zu „{}“.",
                &[&chord.to_string(), &title(action), &title(*from)],
            )
        })
        .collect()
}

/// Binds `action` to `chords` in the file, noting what other actions lose.
pub(super) fn bind(
    doc: &mut term_config::Document,
    action: Action,
    chords: &[Chord],
    notes: &RefCell<Vec<String>>,
) -> bool {
    let taken = doc.bind(action, chords);
    notes.borrow_mut().extend(taken_notes(action, &taken));
    true
}

/// The pane, and the chord field of each action in the order of `Action::ALL`.
pub(super) fn pane(
    fields: &Fields,
    notes: &Rc<RefCell<Vec<String>>>,
) -> (Retained<NSView>, Vec<Retained<NSTextField>>) {
    let mut form = Form::new(fields.mtm());
    let mut chord_fields = Vec::new();
    for (index, &action) in Action::ALL.iter().enumerate() {
        let notes = notes.clone();
        let (field, _) = fields.text(
            200.0,
            Box::new(move |config| Shown::Text(chords_text(&chords_of(config, action)))),
            Rc::new(move |doc, shown| {
                let chords =
                    parse_chords(shown.text()).map_err(|error| tf("„{}“: {}", &[shown.text(), &error.to_string()]))?;
                Ok(bind(doc, action, &chords, &notes))
            }),
        );
        field.setPlaceholderString(Some(&ns(t("Kein Kürzel"))));
        // SAFETY: A nil target and no action until `set_action` gives it both.
        let record = unsafe { NSButton::buttonWithTitle_target_action(&ns(t("Aufnehmen")), None, None, fields.mtm()) };
        record.setTag(isize::try_from(index).unwrap_or(0));
        record.setToolTip(Some(&ns(t(
            "Das nächste gedrückte Kürzel übernehmen; Escape bricht ab",
        ))));
        set_action(&record, fields.target(), sel!(termSettingsRecord:));
        let row = objc2_app_kit::NSStackView::stackViewWithViews(
            &NSArray::from_retained_slice(&[
                Retained::into_super(Retained::into_super(field.clone())),
                Retained::into_super(Retained::into_super(record)),
            ]),
            fields.mtm(),
        );
        form.row(&format!("{}:", title(action).trim_end_matches('…').trim_end()), &row);
        chord_fields.push(field);
    }
    let grid = form.build();
    let scroll = NSScrollView::new(fields.mtm());
    scroll.setHasVerticalScroller(true);
    scroll.setDrawsBackground(false);
    scroll.setBorderType(objc2_app_kit::NSBorderType::BezelBorder);
    // The grid sits inset in a view of its own, as NSGridView has no margins and the list has a border.
    let inset = NSView::new(fields.mtm());
    inset.addSubview(&grid);
    let margins = objc2_foundation::NSEdgeInsets {
        top: space::M,
        left: space::M,
        bottom: space::M,
        right: space::M,
    };
    term_ui::kit::pin_inset(&grid, &inset, margins);
    scroll.setDocumentView(Some(&inset));
    inset.setTranslatesAutoresizingMaskIntoConstraints(false);
    let clip = scroll.contentView();
    term_ui::kit::activate([
        inset.topAnchor().constraintEqualToAnchor(&clip.topAnchor()),
        inset.leadingAnchor().constraintEqualToAnchor(&clip.leadingAnchor()),
        inset.trailingAnchor().constraintEqualToAnchor(&clip.trailingAnchor()),
        scroll.heightAnchor().constraintEqualToConstant(420.0),
        scroll
            .widthAnchor()
            .constraintEqualToAnchor_constant(&inset.widthAnchor(), 20.0),
    ]);
    let hint = super::form::secondary(
        fields.mtm(),
        t(
            "Ein Kürzel wie cmd+shift+t eintippen oder „Aufnehmen“ klicken und es drücken; mehrere durch Leerzeichen trennen.",
        ),
    );
    let views: [Retained<NSView>; 2] = [
        Retained::into_super(Retained::into_super(hint)),
        Retained::into_super(scroll),
    ];
    let stack = NSStackView::stackViewWithViews(&NSArray::from_retained_slice(&views), fields.mtm());
    stack.setOrientation(objc2_app_kit::NSUserInterfaceLayoutOrientation::Vertical);
    stack.setAlignment(objc2_app_kit::NSLayoutAttribute::Leading);
    stack.setSpacing(space::M);
    (Retained::into_super(stack), chord_fields)
}

/// Listens for the next key press in the settings window, which it keeps from the window, and hands its chord to `done`, or `None` for Escape.
pub(super) fn record(done: impl Fn(Option<Chord>) + 'static) -> Option<Retained<AnyObject>> {
    let block = RcBlock::new(move |pointer: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a live event for the duration of the call.
        let event = unsafe { pointer.as_ref() };
        if !term_base::guarded(false, || super::owns_event(event)) {
            return pointer.as_ptr();
        }
        term_base::guarded((), || {
            let plain = !event.modifierFlags().intersects(
                NSEventModifierFlags::Command | NSEventModifierFlags::Control | NSEventModifierFlags::Option,
            );
            if plain && event.keyCode() == ESCAPE {
                done(None);
            } else if let Some(chord) = term_ui::chord_of(event) {
                done(Some(chord));
            }
        });
        std::ptr::null_mut()
    });
    // SAFETY: The block returns null, which swallows the key, as a local monitor may, and runs on the main thread.
    unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &block) }
}

/// Ends a recording `record` started.
pub(super) fn stop(monitor: &AnyObject) {
    // SAFETY: The monitor is the object `addLocalMonitorForEventsMatchingMask_handler` returned, removed once.
    unsafe { NSEvent::removeMonitor(monitor) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_a_title() {
        for &action in Action::ALL {
            assert!(!title(action).is_empty());
        }
        assert_eq!(title(Action::Tab3), "Tab 3");
    }

    #[test]
    fn a_taken_chord_is_named_with_both_actions() {
        let notes = taken_notes(Action::NewTab, &[("cmd+f".parse().unwrap(), Action::Find)]);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("cmd+f"), "{notes:?}");
    }
}
