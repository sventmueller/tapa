//! Colours that follow the appearance: an accent that keeps its contrast on light and dark grounds, and with Increase
//! Contrast more of it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAccessibilityHighContrastAqua, NSAppearanceNameAccessibilityHighContrastDarkAqua,
    NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSColor,
};
use objc2_foundation::NSArray;
use term_render::constant;

use super::guarded;

/// A colour by its channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// The appearance a colour is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Appearance {
    /// Light.
    Light,
    /// Dark.
    Dark,
    /// Light with Increase Contrast.
    LightHighContrast,
    /// Dark with Increase Contrast.
    DarkHighContrast,
}

impl Appearance {
    /// Whether it is one of the dark ones.
    pub fn is_dark(self) -> bool {
        matches!(self, Appearance::Dark | Appearance::DarkHighContrast)
    }

    /// Whether Increase Contrast is on.
    pub fn is_high_contrast(self) -> bool {
        matches!(self, Appearance::LightHighContrast | Appearance::DarkHighContrast)
    }
}

/// Which of the four appearances `appearance` is closest to.
pub fn appearance_of(appearance: &NSAppearance) -> Appearance {
    let names = NSArray::from_slice(&[
        constant!(NSAppearanceNameAqua),
        constant!(NSAppearanceNameDarkAqua),
        constant!(NSAppearanceNameAccessibilityHighContrastAqua),
        constant!(NSAppearanceNameAccessibilityHighContrastDarkAqua),
    ]);
    let best = appearance
        .bestMatchFromAppearancesWithNames(&names)
        .map(|n| n.to_string())
        .unwrap_or_default();
    if best == constant!(NSAppearanceNameAccessibilityHighContrastDarkAqua).to_string() {
        Appearance::DarkHighContrast
    } else if best == constant!(NSAppearanceNameAccessibilityHighContrastAqua).to_string() {
        Appearance::LightHighContrast
    } else if best == constant!(NSAppearanceNameDarkAqua).to_string() {
        Appearance::Dark
    } else {
        Appearance::Light
    }
}

fn srgb(c: Rgb) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        f64::from(c.0) / 255.0,
        f64::from(c.1) / 255.0,
        f64::from(c.2) / 255.0,
        1.0,
    )
}

/// A colour that AppKit resolves anew for each appearance, from `pick`.
pub fn dynamic(pick: impl Fn(Appearance) -> Rgb + 'static) -> Retained<NSColor> {
    // The block keeps the fallback alive, so it may hand AppKit the fallback's pointer without retaining it.
    let fallback = NSColor::labelColor();
    let block = RcBlock::new(move |appearance: NonNull<NSAppearance>| -> NonNull<NSColor> {
        guarded(NonNull::from(&*fallback), || {
            // SAFETY: `NonNull` rules out null, and AppKit keeps the appearance alive while it asks for its colour.
            let appearance = unsafe { appearance.as_ref() };
            let pointer = Retained::autorelease_return(srgb(pick(appearance_of(appearance))));
            // SAFETY: `autorelease_return` hands back the pointer of a live object, which is never null.
            unsafe { NonNull::new_unchecked(pointer) }
        })
    });
    // SAFETY: The provider only reads the appearance it is given and returns an autoreleased colour.
    unsafe { NSColor::colorWithName_dynamicProvider(None, &block) }
}

thread_local! {
    static ACCENTS: RefCell<HashMap<Rgb, Retained<NSColor>>> = RefCell::new(HashMap::new());
}

const LIGHT_GROUND: Rgb = Rgb(0xFF, 0xFF, 0xFF);
const DARK_GROUND: Rgb = Rgb(0x1E, 0x1E, 0x1E);
/// WCAG's contrast for symbols and borders, and with Increase Contrast the one for text.
const ACCENT_CONTRAST: f64 = 3.0;
const HIGH_CONTRAST: f64 = 4.5;
const MIX_STEPS: i32 = 40;

fn luminance(c: Rgb) -> f64 {
    let channel = |v: u8| {
        let v = f64::from(v) / 255.0;
        if v <= 0.040_45 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(c.0) + 0.7152 * channel(c.1) + 0.0722 * channel(c.2)
}

fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// `base` mixed towards black or white, whichever is away from `ground`, until it has `wanted` contrast with it.
fn against(base: Rgb, ground: Rgb, wanted: f64) -> Rgb {
    let towards: u8 = if luminance(ground) > 0.5 { 0 } else { u8::MAX };
    let mix = |v: u8, step: i32| {
        let moved = i32::from(v) + (i32::from(towards) - i32::from(v)) * step / MIX_STEPS;
        u8::try_from(moved).unwrap_or(towards)
    };
    (0..=MIX_STEPS)
        .map(|step| Rgb(mix(base.0, step), mix(base.1, step), mix(base.2, step)))
        .find(|candidate| contrast(*candidate, ground) >= wanted)
        .unwrap_or(Rgb(towards, towards, towards))
}

/// `base` as an accent: as it is where it stands out enough, else darker on light and lighter on dark grounds, more
/// so with Increase Contrast.
pub fn accent(base: Rgb) -> Retained<NSColor> {
    ACCENTS.with(|cache| {
        cache
            .borrow_mut()
            .entry(base)
            .or_insert_with(|| {
                dynamic(move |appearance| match appearance {
                    Appearance::Light => against(base, LIGHT_GROUND, ACCENT_CONTRAST),
                    Appearance::Dark => against(base, DARK_GROUND, ACCENT_CONTRAST),
                    Appearance::LightHighContrast => against(base, LIGHT_GROUND, HIGH_CONTRAST),
                    Appearance::DarkHighContrast => against(base, DARK_GROUND, HIGH_CONTRAST),
                })
            })
            .clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_accent_keeps_its_contrast_with_the_ground_in_every_appearance() {
        for colour in [Rgb(0xD9, 0x77, 0x57), Rgb(0x47, 0x96, 0xE3), Rgb(0xF5, 0xC5, 0x18)] {
            for (ground, wanted) in [
                (LIGHT_GROUND, ACCENT_CONTRAST),
                (DARK_GROUND, ACCENT_CONTRAST),
                (LIGHT_GROUND, HIGH_CONTRAST),
                (DARK_GROUND, HIGH_CONTRAST),
            ] {
                assert!(contrast(against(colour, ground, wanted), ground) >= wanted);
            }
        }
    }
}
