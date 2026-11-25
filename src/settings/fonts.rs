//! The installed monospaced font families the font pop-up offers.

use std::cell::OnceCell;

use objc2_app_kit::{NSFontManager, NSFontTraitMask};
use objc2_foundation::NSNumber;

thread_local! {
    static FAMILIES: OnceCell<Vec<String>> = const { OnceCell::new() };
}

/// The family that is the default and that the font manager does not list, being a system face.
const SYSTEM_MONO: &str = "SF Mono";

/// The monospaced families, sorted, read once per run.
pub(super) fn monospaced() -> Vec<String> {
    FAMILIES.with(|cell| cell.get_or_init(read).clone())
}

fn read() -> Vec<String> {
    let manager = NSFontManager::sharedFontManager(crate::state::app().mtm);
    let mut families: Vec<String> = manager
        .availableFontFamilies()
        .iter()
        .filter(|family| !family.to_string().starts_with('.'))
        .filter(|family| {
            // Each member is [PostScript name, style, weight, traits].
            manager.availableMembersOfFontFamily(family).is_some_and(|members| {
                members.iter().any(|member| {
                    member
                        .iter()
                        .nth(3)
                        .and_then(|traits| traits.downcast::<NSNumber>().ok())
                        .is_some_and(|traits| {
                            traits.unsignedIntegerValue() & NSFontTraitMask::FixedPitchFontMask.0 != 0
                        })
                })
            })
        })
        .map(|family| family.to_string())
        .collect();
    if !families.iter().any(|family| family == SYSTEM_MONO) {
        families.push(SYSTEM_MONO.into());
    }
    families.sort_by_key(|family| family.to_lowercase());
    families.dedup();
    families
}
