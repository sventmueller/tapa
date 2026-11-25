//! The self-test's steps for dictation, with the crate's stand-ins for the microphone, the engine and the keychain,
//! never the real ones: `dictate:<text>` dictates `text` into the field with the focus through the menu action, as
//! ⌃⌥D does, the stand-in engine answering after a moment; `dictatefail:<nospeech|nokey|nomodel>` ends the next
//! dictation that way; `dictatecancel:` starts one and cancels it with Esc; `dictation:<name>` prints what the
//! controls of the field with the focus show, the menu action, the keys kept and the choices the engine starts its
//! next dictation with; `composer:<name>` gives the composer of the agent tab in front the keyboard and prints its
//! text. A build without the feature `dictation` says so for each.

#[cfg(feature = "dictation")]
mod on {
    use std::time::Duration;

    use lowspeak::models::ApiProvider;
    use lowspeak::transcription::error::TranscriptionError;
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::{NSApplication, NSTextView};
    use term_ui_dictation::selftest::{self as probe, Answer};

    /// How long the stand-in microphone records before the step stops it.
    const RECORDING: Duration = Duration::from_millis(400);

    pub(super) fn run(name: &str, argument: &str) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        match name {
            "dictate" => dictate(mtm, Ok(argument.to_string())),
            "dictatefail" => fail(mtm, argument),
            "dictatecancel" => cancel(mtm),
            "dictation" => print(mtm, argument),
            "composer" => composer(argument),
            _ => return false,
        }
        true
    }

    /// The text view with the keyboard in the window in front.
    fn focused(mtm: MainThreadMarker) -> Option<Retained<NSTextView>> {
        let app = NSApplication::sharedApplication(mtm);
        let window = app
            .keyWindow()
            .or_else(|| app.mainWindow())
            .or_else(|| app.orderedWindows().iter().find(|w| w.isVisible()))?;
        window.firstResponder()?.downcast::<NSTextView>().ok()
    }

    /// Keys for both providers, so that no question about a missing key stops the run.
    fn keys() {
        for key in ["sk-selftest-openai", "gsk_selftest-groq"] {
            probe::give_key(key);
        }
    }

    fn dictate(mtm: MainThreadMarker, result: Result<String, TranscriptionError>) {
        keys();
        probe::answer([Answer {
            after: Duration::from_millis(300),
            result,
        }]);
        term_ui_dictation::toggle(mtm);
        stop_later();
    }

    /// Ends the recording the step started, once the stand-in microphone recorded a moment.
    fn stop_later() {
        term_base::main_after(RECORDING, || {
            if let Some(mtm) = MainThreadMarker::new() {
                term_ui_dictation::toggle(mtm);
            }
        });
    }

    fn fail(mtm: MainThreadMarker, how: &str) {
        match how {
            "nospeech" => {
                keys();
                probe::too_short();
                term_ui_dictation::toggle(mtm);
                stop_later();
            }
            "nokey" => dictate(mtm, Err(TranscriptionError::NoApiKey(ApiProvider::OpenAi))),
            "nomodel" => dictate(mtm, Err(TranscriptionError::ModelNotAvailable)),
            other => println!("dictatefail: no failure {other}; nospeech, nokey or nomodel"),
        }
    }

    fn cancel(mtm: MainThreadMarker) {
        keys();
        term_ui_dictation::toggle(mtm);
        term_base::main_after(RECORDING, || {
            let Some(mtm) = MainThreadMarker::new() else { return };
            let cancelled = focused(mtm).is_some_and(|view| term_ui_dictation::cancel_in(&view));
            println!("dictatecancel: cancelled {cancelled}");
        });
    }

    /// Gives the composer of the agent tab in front the keyboard and prints its text.
    fn composer(name: &str) {
        match super::super::agent::front_agent() {
            Some(tab) => {
                tab.focus_composer();
                println!("composer {name}: {:?}", tab.composer_text());
            }
            None => println!("composer {name}: no agent tab in front"),
        }
    }

    fn print(mtm: MainThreadMarker, name: &str) {
        let field = focused(mtm).and_then(|view| probe::target_of(&view));
        match field {
            Some(target) => {
                let seen = probe::seen(target);
                println!(
                    "dictation {name}: field {target} recording {} listening {} transcribing {} spinning {} line {:?} \
                     button {:?} value {:?}",
                    seen.recording, seen.listening, seen.transcribing, seen.spinning, seen.line, seen.label, seen.value
                );
            }
            None => println!("dictation {name}: no field with the focus takes dictation"),
        }
        let (enabled, recording) = probe::menu_item(mtm);
        println!("dictation {name}: menu enabled {enabled} recording {recording}");
        println!("dictation {name}: keys {:?}", probe::keys_kept());
        let choices = probe::engine_choices();
        let settings = choices.settings;
        println!(
            "dictation {name}: engine {} online {} local {} language {} post-processing {} microphone {:?} \
             vocabulary {:?}",
            settings.mode.raw_value(),
            settings.online_model.raw_value(),
            settings.local_model.raw_value(),
            settings.language.raw_value(),
            settings.post_processing.raw_value(),
            choices.microphone.map(|m| m.id),
            choices.vocabulary
        );
        println!("dictation {name}: fn {:?}", term_ui_dictation::fn_state());
    }
}

/// Runs `step` if it is one of these; whether it was.
pub(super) fn run(step: &str) -> bool {
    let Some((name, argument)) = step.split_once(':') else {
        return false;
    };
    if !matches!(
        name,
        "dictate" | "dictatefail" | "dictatecancel" | "dictation" | "composer"
    ) {
        return false;
    }
    #[cfg(feature = "dictation")]
    return on::run(name, argument);
    #[cfg(not(feature = "dictation"))]
    {
        println!("{name}: {argument}: the app is built without dictation (feature `dictation`)");
        true
    }
}
