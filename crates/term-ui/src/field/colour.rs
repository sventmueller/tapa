//! The colours of the command in the input field (F92), as Warp draws them: the command, its subcommands, options,
//! variables and operators each in a colour of the terminal's theme, other arguments in the text's, and a command the
//! shell cannot run with a dashed red underline. They are rendering attributes, which leave the text and its undo
//! alone.

use std::ops::Range;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass};
use objc2_app_kit::{
    NSColor, NSForegroundColorAttributeName, NSTextContentStorage, NSTextElementProvider, NSTextInputClient,
    NSTextRange, NSUnderlineColorAttributeName, NSUnderlineStyle, NSUnderlineStyleAttributeName,
};
use objc2_foundation::{NSNumber, NSRange};
use term_render::constant;
use term_render::palette::Theme;
use term_session::Rgb;
use term_suggest::{Highlight, Part};

use super::InputField;
use crate::TerminalView;

/// The colours the field draws its text in, from the terminal's theme.
pub(crate) struct Colours {
    pub(super) text: Retained<NSColor>,
    pub(super) ghost: Retained<NSColor>,
    command: Retained<NSColor>,
    subcommand: Retained<NSColor>,
    option: Retained<NSColor>,
    variable: Retained<NSColor>,
    operator: Retained<NSColor>,
    error: Retained<NSColor>,
}

/// `colour` as AppKit draws it.
pub(crate) fn ns_colour(colour: Rgb) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        f64::from(colour.r) / 255.0,
        f64::from(colour.g) / 255.0,
        f64::from(colour.b) / 255.0,
        1.0,
    )
}

impl Colours {
    /// The colours of `theme`, kept readable on its background as the terminal keeps its text.
    pub(crate) fn of(theme: &Theme) -> Colours {
        let palette = &theme.palette;
        let ground = palette.background;
        let ansi = |index: usize| ns_colour(theme.readable(palette.ansi[index], ground));
        Colours {
            text: ns_colour(palette.foreground),
            ghost: ns_colour(palette.foreground.mix(ground, term_render::ghost::SHARE)),
            command: ansi(2),
            subcommand: ansi(6),
            option: ansi(4),
            variable: ansi(5),
            operator: ansi(3),
            error: ansi(1),
        }
    }

    fn of_part(&self, part: Part) -> Option<&NSColor> {
        match part {
            Part::Command => Some(&self.command),
            Part::Subcommand => Some(&self.subcommand),
            Part::Option => Some(&self.option),
            Part::Variable => Some(&self.variable),
            Part::Operator => Some(&self.operator),
            Part::Unknown | Part::Argument => None,
        }
    }
}

/// The UTF-16 offset of byte `byte` in `text`.
fn utf16_at(text: &str, byte: usize) -> usize {
    text.get(..byte).map_or(0, |head| head.encode_utf16().count())
}

fn text_range(content: &NSTextContentStorage, range: Range<usize>) -> Option<Retained<NSTextRange>> {
    let start = content.documentRange().location();
    let from = content.locationFromLocation_withOffset(&start, isize::try_from(range.start).ok()?)?;
    let to = content.locationFromLocation_withOffset(&start, isize::try_from(range.end).ok()?)?;
    NSTextRange::initWithLocation_endLocation(NSTextRange::alloc(), &from, Some(&to))
}

impl InputField {
    /// Draws `text` with the colours of `highlight`, colouring its parts where `colours` and underlining what the
    /// shell cannot run where `underline`; without a highlight in the text's colour alone.
    pub(super) fn paint(&self, text: &str, highlight: Option<&Highlight>, colours: bool, underline: bool) {
        let (Some(layout), Some(content), Some(storage)) = (
            self.text.textLayoutManager(),
            self.text.textContentStorage(),
            // SAFETY: The text view's own storage, read and given attributes on the main thread only.
            unsafe { self.text.textStorage() },
        ) else {
            return;
        };
        let whole = content.documentRange();
        let foreground = constant!(NSForegroundColorAttributeName);
        let underline_style = constant!(NSUnderlineStyleAttributeName);
        let underline_colour = constant!(NSUnderlineColorAttributeName);
        layout.removeRenderingAttribute_forTextRange(foreground, &whole);
        // TextKit 2 draws a colour from the rendering attributes but no underline, so that one goes into the text.
        let marked = self.text.hasMarkedText();
        let length = storage.length();
        if !marked {
            storage.beginEditing();
            for key in [underline_style, underline_colour] {
                storage.removeAttribute_range(key, NSRange::new(0, length));
            }
        }
        let palette = self.colours.borrow();
        let dashed: Retained<AnyObject> =
            NSNumber::new_isize(NSUnderlineStyle::Single.0 | NSUnderlineStyle::PatternDash.0).into();
        for span in highlight.map_or(&[][..], |highlight| &highlight.spans) {
            let utf16 = utf16_at(text, span.range.start)..utf16_at(text, span.range.end);
            if utf16.end > length {
                continue;
            }
            if colours
                && let Some(colour) = palette.of_part(span.part)
                && let Some(range) = text_range(&content, utf16.clone())
            {
                let colour: &AnyObject = colour;
                // SAFETY: The value of the foreground colour attribute is a colour.
                unsafe { layout.addRenderingAttribute_value_forTextRange(foreground, Some(colour), &range) };
            }
            if underline && !marked && span.part == Part::Unknown {
                let range = NSRange::new(utf16.start, utf16.end - utf16.start);
                let red: &AnyObject = &palette.error;
                // SAFETY: The underline style is a number of `NSUnderlineStyle` bits, its colour a colour.
                unsafe {
                    storage.addAttribute_value_range(underline_style, &dashed, range);
                    storage.addAttribute_value_range(underline_colour, red, range);
                }
            }
        }
        if !marked {
            storage.endEditing();
        }
    }
}

impl TerminalView {
    /// Colours the field's `text` as the settings ask; a command still being typed at the end of the text is not marked
    /// unknown yet. A spec the subcommands need but that is not parsed yet is parsed on the side and the text coloured
    /// again once it is.
    pub(super) fn field_colour(&self, field: &InputField, text: &str) {
        let input = crate::host().config().input.clone();
        if !input.highlighting && !input.underline_errors {
            field.paint(text, None, false, false);
            return;
        }
        let folder = self.cwd();
        let home = term_base::home();
        let shell = &self.ivars().shell;
        let known = |word: &str| shell.borrow_mut().knows(word, &folder, &home);
        let mut highlight = term_suggest::highlight(text, &known);
        for span in &mut highlight.spans {
            if span.part == Part::Unknown && span.range.end == text.len() {
                span.part = Part::Argument;
            }
        }
        field.paint(text, Some(&highlight), input.highlighting, input.underline_errors);
        if let Some(missing) = highlight.missing_spec {
            term_suggest::warm_spec(&missing);
            let (id, line) = (self.id(), text.to_string());
            crate::kit::main_after(std::time::Duration::from_millis(250), move || {
                if let Some(view) = crate::find(id)
                    && let Ok(field) = view.ivars().field.try_borrow()
                    && let Some(field) = field.as_ref()
                    && field.text() == line
                {
                    view.field_colour(field, &line);
                }
            });
        }
    }
}

impl TerminalView {
    /// The words of the field's command with the parts its colours show, as `word:Part`, for the self-test.
    #[cfg(feature = "selftest")]
    pub fn field_parts(&self) -> Vec<String> {
        let Some(text) = self.field_view().map(|field| field.text()) else {
            return Vec::new();
        };
        let (folder, home) = (self.cwd(), term_base::home());
        let shell = &self.ivars().shell;
        let known = |word: &str| shell.borrow_mut().knows(word, &folder, &home);
        term_suggest::highlight(&text, &known)
            .spans
            .into_iter()
            .map(|span| format!("{}:{:?}", &text[span.range], span.part))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_offsets_become_utf16_offsets() {
        assert_eq!(utf16_at("echo ä 😀 x", 0), 0);
        assert_eq!(utf16_at("echo ä 😀 x", "echo ä".len()), 6);
        assert_eq!(utf16_at("echo ä 😀 x", "echo ä 😀".len()), 9);
    }
}
