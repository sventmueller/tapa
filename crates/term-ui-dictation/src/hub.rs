//! The dictations through lowspeak: the registered fields, the running recording and its transcriptions, the keys,
//! and the actions of the microphone buttons and the menu action.

mod ask;
pub mod choices;
mod controls;
mod desk;
mod engine;
mod fn_key;
mod keys;
mod microphone;
mod models;
#[cfg(feature = "selftest")]
pub mod selftest;

use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use lowspeak::audio::recorder::StartError;
use lowspeak::dictation::{Dictation, Event, Failure, Target, texts_for_locale};
use lowspeak::i18n::Texts;
use lowspeak::models::ApiProvider;
use lowspeak::transcription::error::TranscriptionError;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObject};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSBeep, NSButton, NSPasteboard, NSPasteboardTypeString, NSStackView, NSStackViewGravity,
    NSTextField, NSTextInputClient, NSTextView,
};
use objc2_foundation::{NSBundle, NSObjectProtocol};
use splat_i18n::{t, tf};

pub use controls::Mic;
use desk::{Act, Desk, Phase};
pub use fn_key::{FnState, ask_permission as ask_fn_permission, fn_state, setting_changed as fn_setting_changed};
pub use keys::PROVIDERS;
pub use models::{cancel_download, download, download_failed, downloading, installed};

pub use choices::set_config;
pub use engine::EngineError;
pub use keys::KeyError;

use crate::DictationField;
use crate::util::{constant, guarded, main_after, main_soon, ns};

/// How long a note under a field stays, such as why a dictation failed.
const NOTE: Duration = Duration::from_secs(8);
/// How long a start the busy microphone refused waits before it asks again, and how often.
const RETRY: Duration = Duration::from_millis(250);
const RETRIES: u32 = 20;
/// How long quitting waits for the local model to let go.
const SHUTDOWN: Duration = Duration::from_secs(3);

/// How the user asked for a dictation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Via {
    Button,
    Menu,
    Fn,
}

#[derive(Clone)]
struct Note {
    text: String,
    error: bool,
    id: u64,
}

struct Place {
    field: Rc<dyn DictationField>,
    text: Weak<NSTextView>,
    mic: Mic,
    line: Weak<NSTextField>,
    note: Option<Note>,
    pulsing: bool,
}

struct Hub {
    desk: Desk,
    places: HashMap<Target, Place>,
    next: Target,
    notes: u64,
    keys: Arc<keys::Keys>,
    dictation: Option<Dictation>,
    asked: HashSet<ApiProvider>,
    retries: HashMap<Target, u32>,
    shortcut: String,
}

thread_local! {
    static TARGET: OnceCell<Retained<DictationTarget>> = const { OnceCell::new() };
}

/// The dictations and what fn does.
#[derive(Default)]
pub struct State {
    hub: RefCell<Option<Hub>>,
    fn_key: RefCell<fn_key::FnKey>,
    models: RefCell<models::Downloads>,
}

/// The app dictation runs in, as far as it needs it.
pub trait Host {
    /// The dictations of the running app.
    fn state(&self) -> Rc<State>;
    /// The shortcut of the dictation's action, as macOS writes shortcuts; empty without one.
    fn shortcut(&self) -> String;
    /// Whether the self-test drives the app, which then plays a microphone and an engine instead of lowspeak's.
    fn selftest(&self) -> bool {
        false
    }
}

static HOST: OnceLock<&'static (dyn Host + Sync)> = OnceLock::new();

/// Makes `host` the app the dictations belong to; once, before the first field registers.
pub fn install(host: &'static (dyn Host + Sync)) {
    let _ = HOST.set(host);
}

/// Whether the self-test plays the microphone, the engine and the keys.
fn in_selftest() -> bool {
    cfg!(feature = "selftest") && HOST.get().is_some_and(|host| host.selftest())
}

fn state() -> Rc<State> {
    HOST.get()
        .expect("the app installs its host before the first dictation")
        .state()
}

/// `f` with the hub, made on first use; `None` while the hub is in use further up, which a modal session can cause.
fn with_hub<R>(f: impl FnOnce(&mut Hub) -> R) -> Option<R> {
    let state = state();
    let mut hub = state.hub.try_borrow_mut().ok()?;
    Some(f(hub.get_or_insert_with(Hub::new)))
}

impl Hub {
    fn new() -> Hub {
        Hub {
            desk: Desk::default(),
            places: HashMap::new(),
            next: 1,
            notes: 0,
            keys: Arc::new(keys::Keys::new(secret_store())),
            dictation: None,
            asked: HashSet::new(),
            retries: HashMap::new(),
            shortcut: String::new(),
        }
    }

    fn engine(&mut self) -> &Dictation {
        let keys = Arc::clone(&self.keys);
        self.dictation
            .get_or_insert_with(|| engine::dictation(keys, |event| main_soon(move || on_event(event))))
    }

    fn text_of(&self, target: Target) -> Option<Retained<NSTextView>> {
        self.places.get(&target)?.text.load()
    }

    /// The fields whose views are gone and that wait for nothing leave the hub.
    fn prune(&mut self) {
        let desk = &self.desk;
        self.places
            .retain(|target, place| place.text.load().is_some() || desk.is_busy(*target));
    }

    fn target_of(&self, view: &NSTextView) -> Option<Target> {
        self.places
            .iter()
            .find(|(_, place)| place.text.load().is_some_and(|v| std::ptr::eq(&*v, view)))
            .map(|(target, _)| *target)
    }

    fn shown(&self, target: Target) -> controls::Shown {
        let phase = self.desk.phase(target);
        let waiting = self.desk.is_waiting(target);
        let transcribing = self.desk.transcribing(target) > 0;
        let note = self.places.get(&target).and_then(|p| p.note.clone());
        let (line, error) = if phase == Phase::Listening {
            (t("Höre zu …").to_string(), false)
        } else if phase != Phase::Idle || waiting {
            (String::new(), false)
        } else if transcribing {
            (t("Transkribiere …").to_string(), false)
        } else {
            note.map_or((String::new(), false), |note| (note.text, note.error))
        };
        controls::Shown {
            phase,
            waiting,
            transcribing,
            line,
            error,
            shortcut: self.shortcut.clone(),
        }
    }
}

/// The keychain under the app's identity; the self-test keeps keys in memory instead.
fn secret_store() -> Arc<dyn term_seal::secrets::SecretStore> {
    if in_selftest() {
        return Arc::new(term_seal::secrets::Memory::default());
    }
    Arc::new(term_seal::secrets::Keychain)
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `DictationTarget` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermDictationTarget"]
    pub(crate) struct DictationTarget;

    unsafe impl NSObjectProtocol for DictationTarget {}

    impl DictationTarget {
        #[unsafe(method(termDictate:))]
        fn dictate(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                let tag = sender.and_then(|s| s.downcast_ref::<NSButton>()).map(|b| b.tag());
                if let Some(target) = tag.and_then(|tag| Target::try_from(tag).ok()) {
                    press(target, Via::Button);
                }
            });
        }
    }
);

/// The object the microphone buttons send their action to.
fn target(mtm: MainThreadMarker) -> Retained<DictationTarget> {
    TARGET.with(|cell| {
        cell.get_or_init(|| {
            let this = DictationTarget::alloc(mtm).set_ivars(());
            // SAFETY: NSObject's initializer, called once on the newly allocated object.
            unsafe { msg_send![super(this), init] }
        })
        .clone()
    })
}

/// Before the menus are built: macOS's own dictation would add a second "start dictation" to the Edit menu.
pub fn prepare() {
    let defaults = objc2_foundation::NSUserDefaults::standardUserDefaults();
    let yes = objc2_foundation::NSNumber::new_bool(true);
    let value: &AnyObject = &yes;
    let disabled = objc2_foundation::NSDictionary::from_slices(&[&*ns("NSDisabledDictationMenuItem")], &[value]);
    // SAFETY: The registered defaults are a string key with a property-list value.
    unsafe { defaults.registerDefaults(&disabled) };
}

/// After launch: the fn key, and releasing the local model when the app quits.
pub fn launched() {
    fn_key::launched();
    fn_key::observe(
        constant!(objc2_app_kit::NSApplicationWillTerminateNotification),
        None,
        |_| will_terminate(),
    );
}

/// whisper.cpp fails in its exit-time destructors while a model is loaded; what lowspeak's own app does on quit.
fn will_terminate() {
    let released = with_hub(|hub| hub.dictation.as_ref().is_none_or(|d| d.shutdown(SHUTDOWN))).unwrap_or(true);
    lowspeak::util::filelog::flush_all();
    if !released {
        // SAFETY: Everything the app keeps is written by now; skipping the remaining exit handlers is the point.
        unsafe { libc::_exit(0) };
    }
}

/// The microphone controls of a field, at `points`, to register with `register` once the field is built.
pub fn mic(mtm: MainThreadMarker, points: f64) -> Mic {
    Mic::new(mtm, points, &target(mtm), sel!(termDictate:))
}

/// The line that says what dictation does, beside the field.
pub fn line(mtm: MainThreadMarker) -> Retained<NSTextField> {
    controls::line(mtm)
}

/// Makes `text` a field that dictation can go into, with its controls.
pub fn register(field: Rc<dyn DictationField>, text: &NSTextView, mic: &Mic, line: &NSTextField) {
    let target = with_hub(|hub| {
        hub.prune();
        let target = hub.next;
        hub.next += 1;
        if hub.shortcut.is_empty() {
            hub.shortcut = HOST.get().map(|host| host.shortcut()).unwrap_or_default();
        }
        mic.set_field(isize::try_from(target).unwrap_or(-1));
        hub.places.insert(
            target,
            Place {
                field,
                text: Weak::from(text),
                mic: mic.clone(),
                line: Weak::from(line),
                note: None,
                pulsing: false,
            },
        );
        target
    });
    if let Some(target) = target {
        show(target);
    }
}

/// Puts the microphone and its line into the status bar `stack` of the editor `text`.
pub fn editor_items(mtm: MainThreadMarker, field: Rc<dyn DictationField>, text: &NSTextView, stack: &NSStackView) {
    let mic = mic(mtm, 12.0);
    let line = line(mtm);
    stack.addView_inGravity(&line, NSStackViewGravity::Trailing);
    stack.addView_inGravity(&mic.view, NSStackViewGravity::Trailing);
    register(field, text, &mic, &line);
}

/// The shortcut changed in the settings; the tooltips of fields registered from now on show it.
pub fn shortcut_changed() {
    with_hub(|hub| hub.shortcut.clear());
}

/// The menu action and its shortcut: the field with the focus starts or ends; without one, a recording elsewhere
/// ends.
pub fn toggle(mtm: MainThreadMarker) {
    toggle_focused(mtm, Via::Menu);
}

/// Whether a dictation records now, for the title of the menu item.
pub fn recording() -> bool {
    with_hub(|hub| hub.desk.recording().is_some()).unwrap_or(false)
}

/// Whether the menu action can run: a recording runs, or the field with the focus takes dictation.
pub fn can_toggle(mtm: MainThreadMarker) -> bool {
    recording() || focused(mtm).is_some()
}

/// Esc in `view`: ends its recording without text and drops its transcriptions; whether it had any.
pub fn cancel_in(view: &NSTextView) -> bool {
    let cancelled = with_hub(|hub| {
        let target = hub.target_of(view)?;
        if !hub.desk.cancel(target) {
            return None;
        }
        hub.retries.remove(&target);
        if let Some(dictation) = &hub.dictation {
            dictation.cancel(target);
        }
        Some(target)
    })
    .flatten();
    if let Some(target) = cancelled {
        fn_key::ended(target);
        show(target);
    }
    cancelled.is_some()
}

/// The field with the keyboard focus in the key window, if dictation can go there.
fn focused(mtm: MainThreadMarker) -> Option<Target> {
    let app = NSApplication::sharedApplication(mtm);
    // An app in the background has no key window, as in the self-test; its front window stands in.
    let window = app
        .keyWindow()
        .or_else(|| app.mainWindow())
        .or_else(|| app.orderedWindows().iter().find(|w| w.isVisible()))?;
    let responder = window.firstResponder()?;
    let view = responder.downcast_ref::<NSTextView>()?;
    with_hub(|hub| hub.target_of(view)).flatten()
}

/// The menu item and the shortcut: the field with the focus starts or ends; without one, a recording elsewhere ends.
fn toggle_focused(mtm: MainThreadMarker, via: Via) {
    let recording = with_hub(|hub| hub.desk.recording()).flatten();
    match (focused(mtm), recording) {
        (Some(field), _) => press(field, via),
        (None, Some(running)) => press(running, via),
        (None, None) => NSBeep(),
    }
}

/// A start or a stop for the field `target`.
fn press(target: Target, via: Via) {
    let running = with_hub(|hub| hub.desk.recording() == Some(target)).unwrap_or(false);
    if running {
        let acts = with_hub(|hub| hub.desk.toggle(target)).unwrap_or_default();
        return run(acts);
    }
    begin(target, via);
}

/// fn pressed with the focus in `target`: a recording of its own there is kept rather than ended.
fn begin_with_fn(target: Target) -> bool {
    let running = with_hub(|hub| hub.desk.recording() == Some(target)).unwrap_or(false);
    if !running {
        begin(target, Via::Fn);
    }
    with_hub(|hub| hub.desk.recording() == Some(target)).unwrap_or(false)
}

/// fn released: ends the recording of `target` if it still runs.
fn end_with_fn(target: Target) {
    if with_hub(|hub| hub.desk.recording() == Some(target)).unwrap_or(false) {
        press(target, Via::Fn);
    }
}

/// Starts a dictation into `target`, after asking for a missing key once.
fn begin(target: Target, via: Via) {
    let Some(view) = with_hub(|hub| hub.text_of(target)).flatten() else {
        return;
    };
    let Some(window) = view.window().filter(|_| view.isEditable()) else {
        NSBeep();
        return;
    };
    if microphone::undecided() {
        microphone::ask(move |granted| {
            // fn came up while macOS asked, so the recording it asked for is over.
            if granted && via != Via::Fn {
                begin(target, via);
            }
        });
        return;
    }
    let question = with_hub(|hub| {
        let missing = hub.engine().missing_key();
        keys::question(missing, &mut hub.asked, engine::lowspeak_has_key)
    })
    .flatten();
    if let Some(question) = question {
        let keys = with_hub(|hub| Arc::clone(&hub.keys));
        if let Some(keys) = keys {
            ask::ask(question, &keys);
        }
        if via == Via::Fn {
            // fn came up while the question was open, so the recording it asked for is over.
            return;
        }
    }
    if via == Via::Button {
        window.makeFirstResponder(Some(&view));
    }
    let acts = with_hub(|hub| hub.desk.toggle(target)).unwrap_or_default();
    run(acts);
}

fn run(acts: Vec<Act>) {
    for act in acts {
        match act {
            Act::Start(target) => {
                let taken = with_hub(|hub| hub.engine().start(target).is_ok()).unwrap_or(false);
                let more = with_hub(|hub| {
                    if taken {
                        hub.retries.remove(&target);
                    }
                    hub.desk.started(target, taken)
                });
                run(more.unwrap_or_default());
            }
            Act::Stop(target) => {
                with_hub(|hub| hub.engine().stop(target));
                fn_key::ended(target);
            }
            Act::Retry(target) => main_after(RETRY, move || retry(target)),
            Act::Insert(target, text) => insert(target, &text),
            Act::Failed(target, failure) => note(target, describe(&failure), true),
            Act::NoSpeech(target) => note(target, t("Keine Sprache erkannt").to_string(), false),
            Act::LimitReached(target) => {
                note(
                    target,
                    t("Die Aufnahme hat ihre Höchstdauer erreicht.").to_string(),
                    false,
                );
                fn_key::ended(target);
            }
            Act::Ended(target) => {
                fn_key::ended(target);
                fn_key::after_dictation();
            }
            Act::Show(target) => show(target),
        }
    }
}

/// Asks lowspeak again for a start the busy microphone refused, a limited number of times.
fn retry(target: Target) {
    let again = with_hub(|hub| {
        if !hub.desk.retry_due(target) {
            return false;
        }
        let tries = hub.retries.entry(target).or_default();
        *tries += 1;
        if *tries <= RETRIES {
            return true;
        }
        hub.retries.remove(&target);
        hub.desk.cancel(target);
        false
    })
    .unwrap_or(false);
    if again {
        run(vec![Act::Start(target)]);
    } else {
        show(target);
    }
}

fn on_event(event: Event) {
    let acts = with_hub(|hub| hub.desk.event(event)).unwrap_or_default();
    run(acts);
}

/// Brings the controls of `target` up to date, and keeps its button pulsing while it listens.
fn show(target: Target) {
    let pulse = with_hub(|hub| {
        let shown = hub.shown(target);
        let place = hub.places.get_mut(&target)?;
        place.mic.show(&shown);
        if let Some(line) = place.line.load() {
            controls::show_line(&line, &shown);
        }
        let start = shown.phase == Phase::Listening && !place.pulsing && !controls::reduce_motion();
        place.pulsing |= start;
        Some(start)
    })
    .flatten();
    if pulse == Some(true) {
        main_after(controls::PULSE, move || pulse_step(target, false));
    }
}

fn pulse_step(target: Target, bright: bool) {
    let going = with_hub(|hub| {
        let listening = hub.desk.phase(target) == Phase::Listening;
        let place = hub.places.get_mut(&target)?;
        place.pulsing = listening;
        if listening {
            place.mic.pulse(bright);
        } else {
            place.mic.button.setAlphaValue(1.0);
        }
        Some(listening)
    })
    .flatten();
    if going == Some(true) {
        main_after(controls::PULSE, move || pulse_step(target, !bright));
    }
}

/// Shows `text` under the field for a while.
fn note(target: Target, text: String, error: bool) {
    let id = with_hub(|hub| {
        hub.notes += 1;
        let id = hub.notes;
        if let Some(place) = hub.places.get_mut(&target) {
            place.note = Some(Note { text, error, id });
        }
        id
    });
    show(target);
    let Some(id) = id else { return };
    main_after(NOTE, move || {
        with_hub(|hub| {
            if let Some(place) = hub.places.get_mut(&target)
                && place.note.as_ref().is_some_and(|n| n.id == id)
            {
                place.note = None;
            }
        });
        show(target);
    });
}

/// The text of a dictation goes in at the cursor of its field, as typing would put it; a field that closed meanwhile
/// leaves it on the clipboard.
fn insert(target: Target, text: &str) {
    let (view, field) =
        with_hub(|hub| (hub.text_of(target), hub.places.get(&target).map(|p| p.field.clone()))).unwrap_or_default();
    match view.filter(|v| v.window().is_some() && v.isEditable()) {
        Some(view) => put(&view, text),
        None => {
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            pasteboard.setString_forType(&ns(text), constant!(NSPasteboardTypeString));
            let shown = field.is_some_and(|field| {
                field.notice(t(
                    "Das Feld des Diktats ist nicht mehr offen. Der Text liegt in der Zwischenablage.",
                ))
            });
            if !shown {
                NSBeep();
            }
        }
    }
}

/// Replaces the selection of `view` with `text` through its input path, as one undo step.
fn put(view: &NSTextView, text: &str) {
    let range = view.selectedRange();
    let replacement = ns(text);
    crate::util::undo_step(view, || {
        // SAFETY: The replacement is a string and the range is the view's own selection, as `insertText` asks.
        unsafe { view.insertText_replacementRange(&replacement, range) };
    });
    view.scrollRangeToVisible(view.selectedRange());
}

/// The texts of lowspeak in the language of the interface texts the app loaded.
fn texts() -> &'static Texts {
    let language = NSBundle::mainBundle().preferredLocalizations().firstObject();
    texts_for_locale(&language.map_or_else(|| "en".to_string(), |l| l.to_string()))
}

/// Why a dictation failed, in the interface language. Lowspeak's texts name its own menu and app where the user acts,
/// so those few are the app's own.
fn describe(failure: &Failure) -> String {
    match failure {
        Failure::Microphone(StartError::PermissionDenied) => tf(
            "{} hat keinen Zugriff auf das Mikrofon. Erlaube ihn in den Systemeinstellungen unter Datenschutz & Sicherheit > Mikrofon.",
            &[&term_base::identity().name],
        ),
        Failure::Transcription(TranscriptionError::NoApiKey(provider)) => tf(
            "Kein API-Schlüssel für {}. Trage ihn in den Einstellungen unter Diktat ein.",
            &[provider.display_name()],
        ),
        Failure::Transcription(TranscriptionError::ModelNotAvailable) => {
            t("Das lokale Modell ist nicht installiert. Lade es in den Einstellungen unter Diktat.").to_string()
        }
        Failure::Transcription(TranscriptionError::ModelRetired(model)) => tf(
            "{} bietet {} nicht mehr an. Wähle in den Einstellungen unter Diktat ein anderes Modell.",
            &[model.provider().display_name(), model.name()],
        ),
        other => other.describe(texts()),
    }
}

/// The app's dictation keys, for the settings.
pub fn has_key(provider: ApiProvider) -> bool {
    with_hub(|hub| hub.keys.has(provider)).unwrap_or(false)
}

/// Asks for a key for `provider` and keeps it.
pub fn enter_key(provider: ApiProvider) {
    if let Some(keys) = with_hub(|hub| Arc::clone(&hub.keys)) {
        ask::enter(provider, &keys, None);
    }
}

/// Whether Lowspeak keeps a key for `provider`, found without reading it.
pub fn lowspeak_has_key(provider: ApiProvider) -> bool {
    engine::lowspeak_has_key(provider)
}

/// Offers to take Lowspeak's key for `provider` over, or to enter one instead.
pub fn take_over_key(provider: ApiProvider) {
    if let Some(keys) = with_hub(|hub| Arc::clone(&hub.keys)) {
        ask::ask(keys::Question::TakeOver(provider), &keys);
    }
}

/// Deletes the app's key for `provider` from the keychain.
pub fn remove_key(provider: ApiProvider) -> Result<(), keys::KeyError> {
    with_hub(|hub| hub.keys.remove(provider)).unwrap_or(Err(keys::KeyError::Busy))
}

/// Lowspeak's choices as its menu left them, for the settings to offer taking them over.
pub fn lowspeak_choices() -> lowspeak::dictation::Choices {
    engine::lowspeak_choices()
}

/// The input devices connected now, for the settings.
pub fn microphones() -> Vec<lowspeak::audio::recorder::Microphone> {
    engine::microphones()
}

/// Lowspeak's texts in the interface language, for the settings' names of its choices.
pub fn lowspeak_texts() -> &'static Texts {
    texts()
}
