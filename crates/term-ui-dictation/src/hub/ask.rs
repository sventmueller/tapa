//! The questions before a dictation that needs an API key the app lacks: take Lowspeak's over, or enter one.

use lowspeak::models::ApiProvider;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSSecureTextField};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use splat_i18n::{t, tf};
use term_ui::kit::alert::{Alert, Role, first};

use super::keys::{self, Keys, Question, TakeOver};
use crate::util::ns;

/// Asks `question` and keeps the key the user gives; "not now" leaves the dictation to fall back or fail.
pub(super) fn ask(question: Question, keys: &Keys) {
    match question {
        Question::TakeOver(provider) => offer_takeover(provider, keys),
        Question::Enter(provider) => enter(provider, keys, None),
    }
}

fn offer_takeover(provider: ApiProvider, keys: &Keys) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let name = term_base::identity().name.as_str();
    let message = tf(
        "Lowspeak hat einen Schlüssel für {}. Für {} übernehmen?",
        &[provider.display_name(), name],
    );
    let answer = Alert::new(mtm, &message)
        .informative(&tf(
            "{} liest ihn dafür einmal, und macOS fragt nach. Wähle dort „Erlauben“, nicht „Immer erlauben“: So bleibt Lowspeaks Eintrag, wie er ist. {} legt den Schlüssel in einem eigenen Eintrag ab.",
            &[name, name],
        ))
        .button(t("Übernehmen"), Role::Plain)
        .button(t("Selbst eingeben …"), Role::Plain)
        .button(t("Nicht jetzt"), Role::Plain)
        .run_modal();
    if answer == NSAlertFirstButtonReturn {
        match keys::take_over(keys, provider, super::engine::read_lowspeak_key) {
            TakeOver::Done(_) => {}
            TakeOver::Unread(reason) => {
                term_base::diag::report("dictation", format!("taking Lowspeak's key over: {reason}"));
                enter(
                    provider,
                    keys,
                    Some(t(
                        "Lowspeaks Schlüssel ließ sich nicht lesen. Gib den Schlüssel selbst ein.",
                    )),
                );
            }
        }
    } else if answer == NSAlertSecondButtonReturn {
        enter(provider, keys, None);
    }
}

/// Asks for a key for `provider`, with `problem` above the field if the last attempt failed; again after a key in the
/// wrong format.
pub(super) fn enter(provider: ApiProvider, keys: &Keys, problem: Option<&str>) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let explanation = tf(
        "Lowspeak transkribiert online mit {}. {} braucht dafür einen eigenen Schlüssel und legt ihn im Schlüsselbund ab.",
        &[provider.display_name(), &term_base::identity().name],
    );
    let informative = match problem {
        Some(problem) => format!("{problem}\n\n{explanation}"),
        None => explanation,
    };
    let field = NSSecureTextField::initWithFrame(
        NSSecureTextField::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(300.0, 24.0)),
    );
    field.setPlaceholderString(Some(&ns(match provider {
        ApiProvider::OpenAi => "sk-…",
        ApiProvider::Groq => "gsk_…",
    })));
    let answer = Alert::new(mtm, &tf("API-Schlüssel für {}", &[provider.display_name()]))
        .informative(&informative)
        .accessory(&field)
        .button(t("Sichern"), Role::Plain)
        .button(t("Nicht jetzt"), Role::Plain)
        .focus(&field)
        .run_modal();
    if !first(answer) {
        return;
    }
    let typed = field.stringValue().to_string();
    if typed.trim().is_empty() {
        return;
    }
    if let Err(error) = keys.save(&typed) {
        term_base::diag::report("dictation", format!("saving a key: {error}"));
        enter(
            provider,
            keys,
            Some(t("Das ist kein Schlüssel von OpenAI (sk-…) oder Groq (gsk_…).")),
        );
    }
}
