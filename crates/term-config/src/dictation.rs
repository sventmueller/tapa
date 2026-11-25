//! Dictation through Lowspeak: the engine's choices under Lowspeak's stored names, which the app reads with lowspeak,
//! and whether fn dictates.

use serde::{Deserialize, Serialize};

/// The dictation choices; an empty or unknown name stands for Lowspeak's default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Dictation {
    /// `online` or `local`.
    pub mode: String,
    /// The local Whisper or Parakeet model.
    pub local_model: String,
    /// The online model of OpenAI or Groq.
    pub online_model: String,
    /// The spoken language, `auto` to detect it.
    pub language: String,
    /// What a language model does with the text afterwards.
    pub post_processing: String,
    /// CoreAudio's id of the chosen microphone; empty for the system's default.
    pub microphone: String,
    /// The chosen microphone's name, shown while it is not connected.
    pub microphone_name: String,
    /// Words the transcription spells as written here.
    pub vocabulary: Vec<String>,
    /// Whether holding fn dictates while Lowspeak does not run.
    pub with_fn: bool,
}

impl Default for Dictation {
    fn default() -> Dictation {
        Dictation {
            mode: String::new(),
            local_model: String::new(),
            online_model: String::new(),
            language: String::new(),
            post_processing: String::new(),
            microphone: String::new(),
            microphone_name: String::new(),
            vocabulary: Vec::new(),
            with_fn: true,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_dictation_section_reads_what_the_user_wrote_and_keeps_the_rest() {
        let (config, warnings) = crate::Config::parse(
            "version = 1\n[dictation]\nmode = \"local\"\nvocabulary = [\"Postgres\"]\nwith_fn = false\n",
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config.dictation.mode, "local");
        assert_eq!(config.dictation.vocabulary, ["Postgres"]);
        assert!(!config.dictation.with_fn);
        assert!(config.dictation.language.is_empty());
        let (plain, _) = crate::Config::parse("version = 1\n");
        assert!(plain.dictation.with_fn);
    }
}
