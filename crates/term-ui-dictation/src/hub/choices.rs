//! The dictation choices in the app's settings file, the `[dictation]` section: mode, models, language,
//! post-processing, microphone and vocabulary under lowspeak's stored names, which lowspeak's pipeline reads at the
//! start of each dictation. Lowspeak's own choices are offered for taking over once.

use std::sync::Mutex;

use lowspeak::config::settings::{ChosenMicrophone, Settings as Engine};
use lowspeak::config::vocabulary;
use lowspeak::dictation::Choices;
use lowspeak::models::{Language, LocalModel, OnlineModel, PostProcessingMode, TranscriptionMode};
use term_base::LockIgnoringPoison;
use term_config::Dictation;

/// The section as the app read it last; lowspeak's threads read it at each start.
static CONFIG: Mutex<Option<Dictation>> = Mutex::new(None);

/// The user default that says Lowspeak's choices were offered and the user answered.
const OFFERED: &str = "DictationLowspeakOffered";

/// Takes the settings' `[dictation]` section, which the app hands over at launch and after each reload.
pub fn set_config(config: &Dictation) {
    let changed = {
        let mut stored = CONFIG.lock_ignoring_poison();
        let fn_changed = stored.as_ref().is_none_or(|old| old.with_fn != config.with_fn);
        *stored = Some(config.clone());
        fn_changed
    };
    if changed {
        super::fn_key::setting_changed(config.with_fn);
    }
}

/// Whether fn dictates, as the settings say.
pub(crate) fn with_fn() -> bool {
    CONFIG
        .lock_ignoring_poison()
        .as_ref()
        .is_none_or(|config| config.with_fn)
}

/// Lowspeak's defaults, which are the app's until the user chooses.
pub fn defaults() -> Choices {
    Choices {
        settings: Engine {
            mode: TranscriptionMode::Online,
            local_model: LocalModel::LargeV3Turbo,
            online_model: OnlineModel::GptTranscribe,
            language: Language::Auto,
            post_processing: PostProcessingMode::Off,
            preprocessing: true,
            start_sound: false,
        },
        microphone: None,
        vocabulary: Vec::new(),
    }
}

/// The choices `config` holds; an empty or unknown name falls back alone to Lowspeak's default.
pub fn from_config(config: &Dictation) -> Choices {
    let base = defaults().settings;
    Choices {
        settings: Engine {
            mode: TranscriptionMode::from_raw(&config.mode).unwrap_or(base.mode),
            local_model: LocalModel::from_raw(&config.local_model).unwrap_or(base.local_model),
            online_model: OnlineModel::from_raw(&config.online_model).unwrap_or(base.online_model),
            language: Language::from_raw(&config.language).unwrap_or(base.language),
            post_processing: PostProcessingMode::from_raw(&config.post_processing).unwrap_or(base.post_processing),
            ..base
        },
        microphone: (!config.microphone.is_empty()).then(|| ChosenMicrophone {
            id: config.microphone.clone(),
            name: config.microphone_name.clone(),
        }),
        vocabulary: vocabulary::parse(&config.vocabulary.join(",")),
    }
}

/// `choices` as the settings file keeps them, with fn as `with_fn`.
pub fn to_config(choices: &Choices, with_fn: bool) -> Dictation {
    let settings = &choices.settings;
    Dictation {
        mode: settings.mode.raw_value().into(),
        local_model: settings.local_model.raw_value().into(),
        online_model: settings.online_model.raw_value().into(),
        language: settings.language.raw_value().into(),
        post_processing: settings.post_processing.raw_value().into(),
        microphone: choices.microphone.as_ref().map(|m| m.id.clone()).unwrap_or_default(),
        microphone_name: choices.microphone.as_ref().map(|m| m.name.clone()).unwrap_or_default(),
        vocabulary: choices.vocabulary.clone(),
        with_fn,
    }
}

/// The app's choices as the settings say now.
pub fn current() -> Choices {
    CONFIG
        .lock_ignoring_poison()
        .as_ref()
        .map_or_else(defaults, from_config)
}

/// Whether Lowspeak's choices wait to be offered: the user never answered, and Lowspeak has some other than its
/// defaults, which are the app's too.
pub fn lowspeak_offer(lowspeak: &Choices) -> bool {
    !offered() && *lowspeak != defaults()
}

fn offered() -> bool {
    objc2_foundation::NSUserDefaults::standardUserDefaults().boolForKey(&crate::util::ns(OFFERED))
}

/// Notes that the user answered the offer, which then does not come again; taking the choices over is the app's
/// write of the settings file.
pub fn answered_offer() {
    objc2_foundation::NSUserDefaults::standardUserDefaults().setBool_forKey(true, &crate::util::ns(OFFERED));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lowspeaks() -> Choices {
        let mut choices = defaults();
        choices.settings.mode = TranscriptionMode::Local;
        choices.settings.local_model = LocalModel::ParakeetUltra;
        choices.settings.language = Language::De;
        choices.microphone = Some(ChosenMicrophone {
            id: "coreaudio:AirPods".into(),
            name: "AirPods".into(),
        });
        choices.vocabulary = vec!["Postgres".into(), "Traefik".into()];
        choices
    }

    #[test]
    fn nothing_stored_gives_lowspeaks_defaults_and_what_is_stored_comes_back() {
        assert_eq!(from_config(&Dictation::default()), defaults());
        let stored = to_config(&lowspeaks(), false);
        assert!(!stored.with_fn);
        assert_eq!(from_config(&stored), lowspeaks());
    }

    #[test]
    fn an_unknown_name_falls_back_alone_and_the_vocabulary_is_cleaned() {
        let config = Dictation {
            mode: "local".into(),
            local_model: "telepathy".into(),
            language: "de".into(),
            vocabulary: vec!["Postgres".into(), "postgres".into(), "<x>".into()],
            ..Dictation::default()
        };
        let choices = from_config(&config);
        assert_eq!(choices.settings.mode, TranscriptionMode::Local);
        assert_eq!(choices.settings.local_model, LocalModel::LargeV3Turbo);
        assert_eq!(choices.settings.language, Language::De);
        assert_eq!(choices.vocabulary, ["Postgres", "x"]);
    }
}
