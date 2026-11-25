//! Text attributes. Nothing outside this file names a key together with a value, so AppKit never gets a value of a
//! type its key does not expect.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, ClassType};
use objc2_app_kit::{
    NSAttachmentAttributeName, NSBackgroundColorAttributeName, NSBaselineOffsetAttributeName, NSColor, NSFont,
    NSFontAttributeName, NSForegroundColorAttributeName, NSLinkAttributeName, NSParagraphStyle,
    NSParagraphStyleAttributeName, NSStrikethroughStyleAttributeName, NSTextAttachment, NSToolTipAttributeName,
    NSUnderlineStyleAttributeName,
};
use objc2_foundation::{
    NSAttributedString, NSAttributedStringKey, NSDictionary, NSMutableAttributedString, NSNumber, NSRange, NSURL,
};

use crate::common::constant;
use crate::common::util::ns;

pub(crate) struct Attribute {
    key: &'static NSAttributedStringKey,
    value: Retained<AnyObject>,
}

impl Attribute {
    fn new<T: ClassType + 'static>(key: &'static NSAttributedStringKey, value: &T) -> Attribute {
        Attribute {
            key,
            value: value.retain().into(),
        }
    }

    pub(crate) fn font(font: &NSFont) -> Attribute {
        Attribute::new(constant!(NSFontAttributeName), font)
    }

    pub(crate) fn foreground(color: &NSColor) -> Attribute {
        Attribute::new(constant!(NSForegroundColorAttributeName), color)
    }

    pub(crate) fn background(color: &NSColor) -> Attribute {
        Attribute::new(constant!(NSBackgroundColorAttributeName), color)
    }

    pub(crate) fn paragraph(style: &NSParagraphStyle) -> Attribute {
        Attribute::new(constant!(NSParagraphStyleAttributeName), style)
    }

    pub(crate) fn strikethrough() -> Attribute {
        Attribute::new(constant!(NSStrikethroughStyleAttributeName), &*NSNumber::new_isize(1))
    }

    pub(crate) fn underline() -> Attribute {
        Attribute::new(constant!(NSUnderlineStyleAttributeName), &*NSNumber::new_isize(1))
    }

    pub(crate) fn baseline_offset(offset: f64) -> Attribute {
        Attribute::new(constant!(NSBaselineOffsetAttributeName), &*NSNumber::new_f64(offset))
    }

    pub(crate) fn tool_tip(text: &str) -> Attribute {
        Attribute::new(constant!(NSToolTipAttributeName), &*ns(text))
    }

    pub(crate) fn link(url: &NSURL) -> Attribute {
        Attribute::new(constant!(NSLinkAttributeName), url)
    }

    pub(crate) fn attachment(attachment: &NSTextAttachment) -> Attribute {
        Attribute::new(constant!(NSAttachmentAttributeName), attachment)
    }
}

pub(crate) struct Attributes(Retained<NSDictionary<NSAttributedStringKey, AnyObject>>);

impl Attributes {
    pub(crate) fn new(attributes: &[Attribute]) -> Attributes {
        let keys: Vec<&NSAttributedStringKey> = attributes.iter().map(|a| a.key).collect();
        let values: Vec<&AnyObject> = attributes.iter().map(|a| &*a.value).collect();
        Attributes(NSDictionary::from_slices(&keys, &values))
    }
}

pub(crate) fn attributed(text: &str, attributes: &[Attribute]) -> Retained<NSAttributedString> {
    let attributes = Attributes::new(attributes);
    // SAFETY: `Attribute` gives every key a value of the type it expects.
    unsafe {
        NSAttributedString::initWithString_attributes(NSAttributedString::alloc(), &ns(text), Some(&attributes.0))
    }
}

pub(crate) fn attributed_mut(text: &str, attributes: &[Attribute]) -> Retained<NSMutableAttributedString> {
    let attributes = Attributes::new(attributes);
    // SAFETY: `Attribute` gives every key a value of the type it expects.
    unsafe {
        NSMutableAttributedString::initWithString_attributes(
            NSMutableAttributedString::alloc(),
            &ns(text),
            Some(&attributes.0),
        )
    }
}

pub(crate) fn add_attribute(text: &NSMutableAttributedString, attribute: &Attribute, range: NSRange) {
    // SAFETY: `Attribute` gives every key a value of the type it expects.
    unsafe { text.addAttribute_value_range(attribute.key, &attribute.value, range) };
}

/// The range of UTF-16 units that the bytes `start..end` of `text` cover, from `base` on.
pub(crate) fn utf16_range(text: &str, start: usize, end: usize, base: usize) -> NSRange {
    let from = text.get(..start).map_or(0, |s| s.encode_utf16().count());
    let length = text.get(start..end).map_or(0, |s| s.encode_utf16().count());
    NSRange::new(base + from, length)
}
