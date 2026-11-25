//! The dictation pane of a build with dictation. The pop-ups, the vocabulary and fn are fields of the settings file
//! like any other pane's; the offer, the download, the keys and fn's permission are buttons of their own, which a
//! note of the form brings up to date with every refresh of the window.

use std::rc::Rc;

use lowspeak::dictation::Choices;
use lowspeak::i18n::Texts;
use lowspeak::models::{ApiProvider, Language, LocalModel, OnlineModel, PostProcessingMode, TranscriptionMode};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSButton, NSControlSize, NSGridView, NSProgressIndicator, NSProgressIndicatorStyle, NSStackView,
    NSTextField, NSView,
};
use objc2_foundation::{NSNumber, NSNumberFormatter, NSNumberFormatterStyle, NSObjectProtocol};
use splat_i18n::{t, tf};
use term_base::guarded;
use term_config::{Config, Document, strings};
use term_ui::kit::{ns, set_action, space};
use term_ui_dictation::{self as dictation, FnState, PROVIDERS, choices};

use super::super::form::{Fields, Form, Items, Read, Shown, Write, secondary};

/// The width of the vocabulary's field.
const VOCABULARY_WIDTH: f64 = 300.0;

/// What the buttons of the pane change, the tag of each.
const TAKE: isize = 1;
const IGNORE: isize = 0;

struct Views {
    /// The target of the buttons, which hold it only weakly.
    _actions: Retained<DictationActions>,
    offer: Retained<NSStackView>,
    download_row: Retained<NSStackView>,
    model: Retained<NSTextField>,
    progress: Retained<NSProgressIndicator>,
    download: Retained<NSButton>,
    keys: Vec<KeyRow>,
    fn_row: Retained<NSStackView>,
    fn_note: Retained<NSTextField>,
    ask: Retained<NSButton>,
}

struct KeyRow {
    state: Retained<NSTextField>,
    change: Retained<NSButton>,
    remove: Retained<NSButton>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `DictationActions` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermDictationSettings"]
    struct DictationActions;

    unsafe impl NSObjectProtocol for DictationActions {}

    impl DictationActions {
        #[unsafe(method(termDictationLowspeakChoices:))]
        fn lowspeak_choices(&self, sender: Option<&AnyObject>) {
            guarded((), || answer_offer(tag_of(sender) == Some(TAKE)));
        }

        #[unsafe(method(termDictationDownload:))]
        fn download(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                if dictation::downloading().is_some() {
                    dictation::cancel_download();
                } else {
                    dictation::download(choices::current().settings.local_model, super::super::config_changed);
                }
                super::super::config_changed();
            });
        }

        #[unsafe(method(termDictationKey:))]
        fn key(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                let Some(provider) = provider_of(sender) else { return };
                if !dictation::has_key(provider) && dictation::lowspeak_has_key(provider) {
                    dictation::take_over_key(provider);
                } else {
                    dictation::enter_key(provider);
                }
                super::super::config_changed();
            });
        }

        #[unsafe(method(termDictationRemoveKey:))]
        fn remove_key(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                let Some(provider) = provider_of(sender) else { return };
                if let Err(error) = dictation::remove_key(provider) {
                    term_base::diag::report("dictation", error);
                }
                super::super::config_changed();
            });
        }

        #[unsafe(method(termDictationAskPermission:))]
        fn ask_permission(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                dictation::ask_fn_permission();
                super::super::config_changed();
            });
        }
    }
);

impl DictationActions {
    fn new(mtm: MainThreadMarker) -> Retained<DictationActions> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's initializer, called once on the newly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}

fn tag_of(sender: Option<&AnyObject>) -> Option<isize> {
    sender.and_then(|s| s.downcast_ref::<NSButton>()).map(|b| b.tag())
}

fn provider_of(sender: Option<&AnyObject>) -> Option<ApiProvider> {
    PROVIDERS.get(usize::try_from(tag_of(sender)?).ok()?).copied()
}

/// Takes Lowspeak's choices into the settings file, or leaves them, and does not offer them again.
fn answer_offer(take: bool) {
    if take {
        let with_fn = crate::state::app().config().dictation.with_fn;
        let stored = choices::to_config(&dictation::lowspeak_choices(), with_fn);
        let written = super::super::apply(|doc| {
            write_section(doc, &stored);
            Ok(true)
        });
        if let Err(error) = written {
            term_base::diag::report("settings", error);
        }
    }
    choices::answered_offer();
    super::super::config_changed();
}

fn write_section(doc: &mut Document, stored: &term_config::Dictation) {
    let text = |doc: &mut Document, key: &str, value: &str| doc.set(&["dictation", key], value);
    text(doc, "mode", &stored.mode);
    text(doc, "local_model", &stored.local_model);
    text(doc, "online_model", &stored.online_model);
    text(doc, "language", &stored.language);
    text(doc, "post_processing", &stored.post_processing);
    text(doc, "microphone", &stored.microphone);
    text(doc, "microphone_name", &stored.microphone_name);
    doc.set(
        &["dictation", "vocabulary"],
        strings(stored.vocabulary.iter().map(String::as_str)),
    );
}

/// The choices the settings in force hold.
fn chosen(config: &Config) -> Choices {
    choices::from_config(&config.dictation)
}

fn button(
    mtm: MainThreadMarker,
    target: &DictationActions,
    title: &str,
    action: objc2::runtime::Sel,
    tag: isize,
) -> Retained<NSButton> {
    // SAFETY: The button sends its action to `target`, which answers it, set right below.
    let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, mtm) };
    set_action(&button, target, action);
    button.setTag(tag);
    button
}

fn line(mtm: MainThreadMarker, views: &[&NSView]) -> Retained<NSStackView> {
    let stack = NSStackView::new(mtm);
    stack.setSpacing(space::M);
    for view in views {
        stack.addArrangedSubview(view);
    }
    stack
}

fn views(mtm: MainThreadMarker) -> Views {
    let actions = DictationActions::new(mtm);
    let progress = NSProgressIndicator::new(mtm);
    progress.setStyle(NSProgressIndicatorStyle::Bar);
    progress.setIndeterminate(false);
    progress.setMinValue(0.0);
    progress.setMaxValue(1.0);
    progress.setControlSize(NSControlSize::Small);
    progress.widthAnchor().constraintEqualToConstant(120.0).setActive(true);
    let model = secondary(mtm, "");
    let download = button(mtm, &actions, "", sel!(termDictationDownload:), 0);
    let download_row = line(mtm, &[&model, &progress, &download]);
    let offer_text = secondary(
        mtm,
        t("Lowspeak hat eigene Einstellungen für das Diktat. Möchtest du sie übernehmen?"),
    );
    let take = button(
        mtm,
        &actions,
        t("Einstellungen übernehmen"),
        sel!(termDictationLowspeakChoices:),
        TAKE,
    );
    let ignore = button(
        mtm,
        &actions,
        t("Ignorieren"),
        sel!(termDictationLowspeakChoices:),
        IGNORE,
    );
    let offer = line(mtm, &[&offer_text, &line(mtm, &[&take, &ignore])]);
    offer.setOrientation(objc2_app_kit::NSUserInterfaceLayoutOrientation::Vertical);
    offer.setAlignment(objc2_app_kit::NSLayoutAttribute::Leading);
    let keys = (0..PROVIDERS.len())
        .map(|n| {
            let tag = isize::try_from(n).unwrap_or(-1);
            KeyRow {
                state: term_ui::kit::label(mtm, ""),
                change: button(mtm, &actions, "", sel!(termDictationKey:), tag),
                remove: button(mtm, &actions, t("Entfernen"), sel!(termDictationRemoveKey:), tag),
            }
        })
        .collect();
    let ask = button(
        mtm,
        &actions,
        t("Freigabe anfragen"),
        sel!(termDictationAskPermission:),
        0,
    );
    let fn_note = secondary(mtm, "");
    let fn_row = line(mtm, &[&fn_note, &ask]);
    Views {
        _actions: actions,
        offer,
        download_row,
        model,
        progress,
        download,
        keys,
        fn_row,
        fn_note,
        ask,
    }
}

pub(super) fn pane(fields: &Fields) -> Retained<NSView> {
    let mtm = fields.mtm();
    let texts = dictation::lowspeak_texts();
    let views = Rc::new(views(mtm));
    let mut form = Form::new(mtm);
    form.row(t("Lowspeak:"), &views.offer);
    let shown = Rc::clone(&views);
    // A note that stays empty and brings the pane's own buttons up to date whenever the window shows the settings.
    let refresher = fields.note(Box::new(move |config| {
        show(&shown, config);
        Shown::Text(String::new())
    }));
    form.under(&refresher);
    let (transcription, _) = fields.popup(transcription_items(texts), transcription_read(), transcription_write());
    form.row(t("Transkription:"), &transcription);
    form.under(&views.download_row);
    let (language, _) = fields.popup(
        language_items(texts),
        raw(|c| c.settings.language.raw_value()),
        at("language"),
    );
    form.row(t("Sprache:"), &language);
    let (post, _) = fields.popup(
        post_processing_items(texts),
        raw(|c| c.settings.post_processing.raw_value()),
        at("post_processing"),
    );
    form.row(t("Nachbearbeitung:"), &post);
    let (microphone, _) = fields.popup(microphone_items(texts), microphone_read(), microphone_write());
    form.row(t("Mikrofon:"), &microphone);
    let (vocabulary, _) = fields.text(VOCABULARY_WIDTH, vocabulary_read(), vocabulary_write());
    form.row(t("Vokabular:"), &vocabulary);
    form.hint(texts.vocabulary_info);
    form.gap();
    for (n, row) in views.keys.iter().enumerate() {
        let title = if n == 0 { t("API-Schlüssel:") } else { "" };
        form.row(title, &line(mtm, &[&row.state, &row.change, &row.remove]));
    }
    form.gap();
    let (with_fn, _) = fields.check(
        t("Mit fn diktieren, solange Lowspeak nicht läuft"),
        Box::new(|config| Shown::Bool(config.dictation.with_fn)),
        Rc::new(|doc, shown| {
            doc.set(&["dictation", "with_fn"], shown.on());
            Ok(true)
        }),
    );
    form.row(t("fn-Taste:"), &with_fn);
    form.under(&views.fn_row);
    Retained::into_super(form.build())
}

/// Hides `view` and its row of the form, so a row with nothing to say leaves no gap.
fn hide_row(view: &NSView, hidden: bool) {
    view.setHidden(hidden);
    // SAFETY: Read on the main thread, where the view tree keeps the parent alive while the call runs.
    let parent = unsafe { view.superview() };
    let grid = parent.and_then(|parent| parent.downcast::<NSGridView>().ok());
    if let Some(cell) = grid.and_then(|grid| grid.cellForView(view))
        && let Some(row) = cell.row()
    {
        row.setHidden(hidden);
    }
}

/// A pop-up's value read as one of the choices' names.
fn raw(get: fn(&Choices) -> &'static str) -> Read {
    Box::new(move |config| Shown::Text(get(&chosen(config)).to_string()))
}

/// Writes a pop-up's value as the field `key` of the section.
fn at(key: &'static str) -> Write {
    Rc::new(move |doc, shown| {
        doc.set(&["dictation", key], shown.text());
        Ok(true)
    })
}

fn language_items(texts: &'static Texts) -> Items {
    Box::new(move |_| {
        Language::ALL
            .iter()
            .map(|l| (texts.language_name(*l).to_string(), l.raw_value().to_string()))
            .collect()
    })
}

fn post_processing_items(texts: &'static Texts) -> Items {
    Box::new(move |_| {
        PostProcessingMode::ALL
            .iter()
            .map(|m| (texts.post_processing_mode(*m).to_string(), m.raw_value().to_string()))
            .collect()
    })
}

/// The transcription's value: `online:<model>` or `local:<model>`.
fn transcription_value(choices: &Choices) -> String {
    let settings = &choices.settings;
    match settings.mode {
        TranscriptionMode::Online => format!("online:{}", settings.online_model.raw_value()),
        TranscriptionMode::Local => format!("local:{}", settings.local_model.raw_value()),
    }
}

/// The online models by provider, then the local models, each marked when it is not on this Mac.
fn transcription_items(texts: &'static Texts) -> Items {
    Box::new(move |_| {
        let mut items = Vec::new();
        for provider in PROVIDERS {
            for model in OnlineModel::of_provider(provider) {
                let title = format!("{}: {}", provider.display_name(), texts.online_model(model));
                items.push((title, format!("online:{}", model.raw_value())));
            }
        }
        for model in LocalModel::ALL {
            let name = texts.local_model(model);
            let name = if dictation::installed(model) {
                name
            } else {
                (texts.not_installed)(&name)
            };
            items.push((
                format!("{}: {name}", texts.local),
                format!("local:{}", model.raw_value()),
            ));
        }
        items
    })
}

fn transcription_read() -> Read {
    Box::new(|config| Shown::Text(transcription_value(&chosen(config))))
}

fn transcription_write() -> Write {
    Rc::new(|doc, shown| {
        let Some((mode, model)) = shown.text().split_once(':') else {
            return Ok(false);
        };
        doc.set(&["dictation", "mode"], mode);
        let key = if mode == "local" { "local_model" } else { "online_model" };
        doc.set(&["dictation", key], model);
        Ok(true)
    })
}

/// The system's default, the microphones connected now and a chosen one that is not, by id and name.
fn microphones(config: &Config) -> Vec<(String, String, bool)> {
    let mut listed: Vec<(String, String, bool)> = dictation::microphones()
        .into_iter()
        .map(|m| (m.id, m.name, true))
        .collect();
    let section = &config.dictation;
    if !section.microphone.is_empty() && !listed.iter().any(|(id, _, _)| *id == section.microphone) {
        listed.push((section.microphone.clone(), section.microphone_name.clone(), false));
    }
    listed
}

fn microphone_items(texts: &'static Texts) -> Items {
    Box::new(move |config| {
        let mut items = vec![(texts.system_default.to_string(), String::new())];
        for (id, name, connected) in microphones(config) {
            let title = if connected { name } else { (texts.not_connected)(&name) };
            items.push((title, id));
        }
        items
    })
}

fn microphone_read() -> Read {
    Box::new(|config| Shown::Text(config.dictation.microphone.clone()))
}

fn microphone_write() -> Write {
    Rc::new(|doc, shown| {
        let id = shown.text();
        let config = crate::state::app().config();
        let name = microphones(&config)
            .into_iter()
            .find(|(own, _, _)| own == id)
            .map(|(_, name, _)| name)
            .unwrap_or_default();
        doc.set(&["dictation", "microphone"], id);
        doc.set(&["dictation", "microphone_name"], name.as_str());
        Ok(true)
    })
}

fn vocabulary_read() -> Read {
    Box::new(|config| Shown::Text(chosen(config).vocabulary.join(", ")))
}

fn vocabulary_write() -> Write {
    Rc::new(|doc, shown| {
        let terms = lowspeak::config::vocabulary::parse(shown.text());
        doc.set(&["dictation", "vocabulary"], strings(terms.iter().map(String::as_str)));
        Ok(true)
    })
}

/// Brings the offer, the download, the keys and fn's note up to date.
fn show(views: &Views, config: &Config) {
    let texts = dictation::lowspeak_texts();
    hide_row(&views.offer, !choices::lowspeak_offer(&dictation::lowspeak_choices()));
    show_download(views, texts, &chosen(config));
    show_keys(views);
    show_fn(views);
}

/// The row under the transcription: a chosen local model that is not on this Mac, with its download.
fn show_download(views: &Views, texts: &Texts, current: &Choices) {
    let chosen = current.settings.local_model;
    let running = dictation::downloading();
    let missing = current.settings.mode == TranscriptionMode::Local && !dictation::installed(chosen);
    hide_row(&views.download_row, running.is_none() && !missing);
    views.progress.setHidden(running.is_none());
    match running {
        Some((model, fraction)) => {
            views.progress.setDoubleValue(fraction);
            let formatter = NSNumberFormatter::new();
            formatter.setNumberStyle(NSNumberFormatterStyle::PercentStyle);
            let percent = formatter
                .stringFromNumber(&NSNumber::new_f64(fraction))
                .map(|p| p.to_string())
                .unwrap_or_default();
            views
                .model
                .setStringValue(&ns(&tf("{} wird geladen: {}", &[model.spec().name, &percent])));
            views.download.setTitle(&ns(t("Abbrechen")));
        }
        None => {
            let name = chosen.spec().name;
            let line = if dictation::download_failed() == Some(chosen) {
                (texts.download_failed_info)(name)
            } else {
                (texts.not_installed)(name)
            };
            views.model.setStringValue(&ns(&line));
            views.download.setTitle(&ns(&(texts.download_model)(name)));
        }
    }
}

fn show_keys(views: &Views) {
    for (provider, row) in PROVIDERS.iter().zip(&views.keys) {
        let name = provider.display_name();
        let has = dictation::has_key(*provider);
        let (line, action, spoken) = if has {
            (
                tf("{}: im Schlüsselbund", &[name]),
                t("Ändern …"),
                tf("Schlüssel für {} ändern", &[name]),
            )
        } else if dictation::lowspeak_has_key(*provider) {
            (
                tf("{}: in Lowspeak", &[name]),
                t("Schlüssel übernehmen …"),
                tf("Schlüssel für {} aus Lowspeak übernehmen", &[name]),
            )
        } else {
            (
                tf("{}: kein Schlüssel", &[name]),
                t("Eingeben …"),
                tf("Schlüssel für {} eingeben", &[name]),
            )
        };
        row.state.setStringValue(&ns(&line));
        row.change.setTitle(&ns(action));
        // Each provider has the same two buttons, which VoiceOver could not tell apart by their titles; it reads the
        // cell, not the button.
        if let Some(cell) = row.change.cell() {
            cell.setAccessibilityLabel(Some(&ns(&spoken)));
        }
        if let Some(cell) = row.remove.cell() {
            cell.setAccessibilityLabel(Some(&ns(&tf("Schlüssel für {} entfernen", &[name]))));
        }
        row.remove.setEnabled(has);
    }
}

fn show_fn(views: &Views) {
    let state = dictation::fn_state();
    let note = match state {
        FnState::Off => String::new(),
        FnState::NotAsked => {
            t("fn braucht die Freigabe für Bedienungshilfen; macOS fragt danach nach dem ersten Diktat.").to_string()
        }
        FnState::Refused => t("fn bleibt aus, bis die Freigabe für Bedienungshilfen erteilt ist.").to_string(),
        FnState::LowspeakHasIt => tf(
            "Lowspeak läuft und diktiert mit fn; in {} dann mit dem Mikrofon oder dem Kurzbefehl.",
            &[&term_base::identity().name],
        ),
        FnState::Ready => t("fn halten und sprechen, zweimal tippen diktiert freihändig.").to_string(),
    };
    views.fn_note.setStringValue(&ns(&note));
    views.fn_note.setHidden(note.is_empty());
    views.ask.setHidden(state != FnState::Refused);
    hide_row(&views.fn_row, note.is_empty() && state != FnState::Refused);
}
