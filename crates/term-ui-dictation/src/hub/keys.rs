//! The API keys of dictation, in the app's own keychain items (R4): Lowspeak's items are read only when the user
//! agreed to take a key over, since every read of another app's item makes macOS ask and "Always Allow" would change
//! that item.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use term_base::LockIgnoringPoison;

use lowspeak::config::api_keys::KeyStore;
use lowspeak::models::ApiProvider;
use term_seal::secrets::SecretStore;

/// The providers dictation may need a key of.
pub const PROVIDERS: [ApiProvider; 2] = [ApiProvider::OpenAi, ApiProvider::Groq];

/// The keychain account of `provider`'s key, under the app's own service.
pub(super) fn account(provider: ApiProvider) -> &'static str {
    match provider {
        ApiProvider::OpenAi => "dictation.openai",
        ApiProvider::Groq => "dictation.groq",
    }
}

/// The app's dictation keys, read from the store once and then kept in memory.
pub(super) struct Keys {
    store: Arc<dyn SecretStore>,
    known: Mutex<HashMap<ApiProvider, Option<String>>>,
}

impl Keys {
    pub(super) fn new(store: Arc<dyn SecretStore>) -> Keys {
        Keys {
            store,
            known: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn get(&self, provider: ApiProvider) -> Option<String> {
        let mut known = self.known.lock_ignoring_poison();
        known
            .entry(provider)
            .or_insert_with(|| match self.store.get(account(provider)) {
                Ok(key) => key.filter(|k| !k.is_empty()),
                Err(error) => {
                    term_base::diag::report(
                        "dictation",
                        format!("reading the {} key: {error}", provider.display_name()),
                    );
                    None
                }
            })
            .clone()
    }

    pub(super) fn has(&self, provider: ApiProvider) -> bool {
        self.get(provider).is_some()
    }

    /// Keeps `key` for the provider its prefix names; the provider, or why not.
    pub(super) fn save(&self, key: &str) -> Result<ApiProvider, KeyError> {
        let key = key.trim();
        let provider = ApiProvider::from_api_key(key).ok_or(KeyError::UnknownFormat)?;
        self.store.set(account(provider), key).map_err(KeyError::Keychain)?;
        self.known
            .lock_ignoring_poison()
            .insert(provider, Some(key.to_string()));
        Ok(provider)
    }

    pub(super) fn remove(&self, provider: ApiProvider) -> Result<(), KeyError> {
        self.store.delete(account(provider)).map_err(KeyError::Keychain)?;
        self.known.lock_ignoring_poison().insert(provider, None);
        Ok(())
    }
}

/// Why a key was not kept, read or removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// Its prefix names no provider.
    UnknownFormat,
    /// The keychain refused.
    Keychain(term_seal::secrets::SecretError),
    /// Lowspeak keeps no key for the provider any more.
    NotInLowspeak,
    /// Lowspeak's item could not be read.
    Lowspeak(super::engine::EngineError),
    /// The dictation is busy with something else.
    Busy,
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyError::UnknownFormat => f.write_str("unknown key format"),
            KeyError::Keychain(error) => write!(f, "{error}"),
            KeyError::NotInLowspeak => f.write_str("Lowspeak has no key any more"),
            KeyError::Lowspeak(error) => write!(f, "{error}"),
            KeyError::Busy => f.write_str("busy"),
        }
    }
}

impl term_base::explain::Explain for KeyError {
    fn explain(&self) -> String {
        match self {
            KeyError::UnknownFormat => {
                splat_i18n::t("Das ist kein Schlüssel von OpenAI (sk-…) oder Groq (gsk_…).").to_string()
            }
            KeyError::Keychain(error) => splat_i18n::tf("Der Schlüsselbund lehnt ab: {}", &[&error.to_string()]),
            KeyError::NotInLowspeak => splat_i18n::t("Lowspeak hat keinen Schlüssel mehr.").to_string(),
            KeyError::Lowspeak(error) => error.explain(),
            KeyError::Busy => splat_i18n::t("Das Diktat ist gerade beschäftigt.").to_string(),
        }
    }
}

/// The keys as lowspeak's pipeline asks for them.
pub(super) struct Shared(pub Arc<Keys>);

impl KeyStore for Shared {
    fn load(&self, provider: ApiProvider) -> Option<String> {
        self.0.get(provider)
    }
}

/// What the app asks before a dictation that needs a key it lacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Question {
    /// Lowspeak keeps a key for the provider: take it over?
    TakeOver(ApiProvider),
    /// Enter a key for the provider.
    Enter(ApiProvider),
}

/// The question for a dictation whose missing key is `missing`, once per provider and run; `lowspeak_has` tells
/// whether Lowspeak keeps a key, asked without reading it.
pub(super) fn question(
    missing: Option<ApiProvider>,
    asked: &mut HashSet<ApiProvider>,
    lowspeak_has: impl Fn(ApiProvider) -> bool,
) -> Option<Question> {
    let provider = missing?;
    if !asked.insert(provider) {
        return None;
    }
    Some(if lowspeak_has(provider) {
        Question::TakeOver(provider)
    } else {
        Question::Enter(provider)
    })
}

/// What became of taking Lowspeak's key over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TakeOver {
    Done(ApiProvider),
    /// Lowspeak's item was gone, or macOS did not hand it out; the user may enter a key instead.
    Unread(KeyError),
}

/// Reads Lowspeak's key for `provider` once, through `read`, and keeps it as the app's own.
pub(super) fn take_over(
    keys: &Keys,
    provider: ApiProvider,
    read: impl FnOnce(ApiProvider) -> Result<Option<String>, super::engine::EngineError>,
) -> TakeOver {
    match read(provider) {
        Ok(Some(key)) => match keys.save(&key) {
            Ok(saved) => TakeOver::Done(saved),
            Err(error) => TakeOver::Unread(error),
        },
        Ok(None) => TakeOver::Unread(KeyError::NotInLowspeak),
        Err(error) => TakeOver::Unread(KeyError::Lowspeak(error)),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use term_seal::secrets::Memory;

    use super::*;

    fn keys() -> Keys {
        Keys::new(Arc::new(Memory::default()))
    }

    #[test]
    fn each_provider_is_asked_once_and_lowspeak_is_offered_only_where_it_has_a_key() {
        let mut asked = HashSet::new();
        let has = |provider| provider == ApiProvider::Groq;
        assert_eq!(question(None, &mut asked, has), None, "nothing is missing");
        assert_eq!(
            question(Some(ApiProvider::OpenAi), &mut asked, has),
            Some(Question::Enter(ApiProvider::OpenAi))
        );
        assert_eq!(question(Some(ApiProvider::OpenAi), &mut asked, has), None, "once a run");
        assert_eq!(
            question(Some(ApiProvider::Groq), &mut asked, has),
            Some(Question::TakeOver(ApiProvider::Groq))
        );
    }

    #[test]
    fn a_key_taken_over_is_read_once_and_kept_as_the_apps_own() {
        let keys = keys();
        let reads = Cell::new(0);
        let read = |_| {
            reads.set(reads.get() + 1);
            Ok(Some("gsk_from_lowspeak".to_string()))
        };
        assert_eq!(
            take_over(&keys, ApiProvider::Groq, read),
            TakeOver::Done(ApiProvider::Groq)
        );
        assert_eq!(reads.get(), 1);
        assert_eq!(keys.get(ApiProvider::Groq).as_deref(), Some("gsk_from_lowspeak"));
        assert_eq!(
            keys.store.get(account(ApiProvider::Groq)),
            Ok(Some("gsk_from_lowspeak".to_string()))
        );
    }

    #[test]
    fn a_refused_or_missing_key_leaves_the_store_alone() {
        let keys = keys();
        let refused = take_over(&keys, ApiProvider::OpenAi, |_| {
            Err(super::super::engine::EngineError::Lowspeak("user denied".to_string()))
        });
        assert!(matches!(refused, TakeOver::Unread(reason) if reason.to_string().contains("denied")));
        assert!(matches!(
            take_over(&keys, ApiProvider::OpenAi, |_| Ok(None)),
            TakeOver::Unread(_)
        ));
        assert!(!keys.has(ApiProvider::OpenAi));
    }

    #[test]
    fn keys_are_filed_by_their_prefix_and_removed_per_provider() {
        let keys = keys();
        assert_eq!(keys.save("  sk-test \n"), Ok(ApiProvider::OpenAi));
        assert_eq!(keys.get(ApiProvider::OpenAi).as_deref(), Some("sk-test"));
        assert!(keys.save("not a key").is_err());
        keys.remove(ApiProvider::OpenAi).unwrap();
        assert!(!keys.has(ApiProvider::OpenAi));
        assert_eq!(Shared(Arc::new(keys)).load(ApiProvider::Groq), None);
    }
}
