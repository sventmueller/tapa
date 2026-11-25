//! The dictation pane: Lowspeak's engine as this app uses it, written to the settings' `[dictation]` section, the
//! download of a missing local model, the API keys in the keychain and fn; Lowspeak's own choices are offered for
//! taking over once. A build without the feature `dictation` says that it has none.

use objc2::rc::Retained;
use objc2_app_kit::NSView;

use super::form::Fields;

#[cfg(feature = "dictation")]
mod on;

pub(super) fn pane(fields: &Fields) -> Retained<NSView> {
    #[cfg(feature = "dictation")]
    return on::pane(fields);
    #[cfg(not(feature = "dictation"))]
    {
        let mut form = super::form::Form::new(fields.mtm());
        form.hint(&splat_i18n::tf(
            "Diese Ausgabe von {} ist ohne Diktat gebaut.",
            &[term_product::NAME],
        ));
        Retained::into_super(form.build())
    }
}
