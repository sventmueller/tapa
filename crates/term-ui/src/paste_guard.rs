//! Paste protection (F98, F182): text that would run commands or press keys is shown in a sheet first, unless the
//! program reads it as bracketed paste or the settings turn this off.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{MainThreadOnly, Message};
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSModalResponse};
use splat_i18n::{t, tf};
use term_session::paste::{PasteRisk, paste_risk};

use super::view::TerminalView;
use crate::kit::{guarded, ns};

/// How many lines of the text the sheet shows, and how many characters of each.
const PREVIEW_LINES: usize = 6;
const PREVIEW_WIDTH: usize = 60;

impl TerminalView {
    /// Pastes `text`, first asking in a sheet when it is risky; whether a sheet asks.
    pub(super) fn paste_text(&self, text: &str) -> bool {
        let protect = super::host().config().terminal.paste_protection;
        let risk = self
            .with_session(|session| paste_risk(text, session.input_modes().bracketed_paste))
            .flatten();
        let (Some(risk), Some(window), true) = (risk, self.window(), protect) else {
            self.with_session(|session| session.paste(text));
            return false;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setAlertStyle(NSAlertStyle::Warning);
        let (message, detail) = match risk {
            PasteRisk::Lines(1) => (
                t("Text mit Zeilenumbruch einsetzen?").to_string(),
                t("Was vor dem Zeilenumbruch steht, läuft sofort als Befehl."),
            ),
            PasteRisk::Lines(lines) => (
                tf("Text mit {} Zeilenumbrüchen einsetzen?", &[&lines.to_string()]),
                t("Jede Zeile läuft sofort als eigener Befehl."),
            ),
            PasteRisk::Controls => (
                t("Text mit Steuerzeichen einsetzen?").to_string(),
                t("Steuerzeichen wirken im Terminal wie Tastendrücke."),
            ),
        };
        alert.setMessageText(&ns(&message));
        alert.setInformativeText(&ns(detail));
        alert.setAccessoryView(Some(&preview_view(self.mtm(), text)));
        alert.addButtonWithTitle(&ns(t("Einsetzen")));
        let cancel = alert.addButtonWithTitle(&ns(t("Abbrechen")));
        cancel.setKeyEquivalent(&ns("\u{1b}"));
        alert.setShowsSuppressionButton(true);
        if let Some(suppression) = alert.suppressionButton() {
            suppression.setTitle(&ns(t("Nicht mehr fragen")));
        }
        let asked = alert.clone();
        let view: Retained<TerminalView> = self.retain();
        let text = text.to_string();
        let handler = RcBlock::new(move |response: NSModalResponse| {
            guarded((), || {
                if response == NSAlertFirstButtonReturn {
                    view.with_session(|session| session.paste(&text));
                    let suppressed = asked
                        .suppressionButton()
                        .is_some_and(|button| button.state() == objc2_app_kit::NSControlStateValueOn);
                    if suppressed {
                        super::host().switch_off(&["terminal", "paste_protection"]);
                    }
                }
            });
        });
        alert.beginSheetModalForWindow_completionHandler(&window, Some(&handler));
        true
    }
}

#[cfg(feature = "selftest")]
impl TerminalView {
    /// Pastes `text` as ⌘V would; whether a sheet asks first.
    pub fn paste_text_for_selftest(&self, text: &str) -> bool {
        self.paste_text(text)
    }

    /// Ends the sheet on the view's window with its first button or with Cancel; whether there was one.
    pub fn answer_sheet(&self, accept: bool) -> bool {
        let Some(window) = self.window() else { return false };
        let Some(sheet) = window.attachedSheet() else {
            return false;
        };
        let response = if accept {
            NSAlertFirstButtonReturn
        } else {
            objc2_app_kit::NSAlertSecondButtonReturn
        };
        window.endSheet_returnCode(&sheet, response);
        true
    }
}

/// The text's start in the terminal's kind of face, as the sheet shows it below its explanation.
fn preview_view(mtm: objc2::MainThreadMarker, text: &str) -> Retained<objc2_app_kit::NSView> {
    let label = crate::kit::label(mtm, &preview(text));
    label.setFont(Some(&objc2_app_kit::NSFont::monospacedSystemFontOfSize_weight(
        objc2_app_kit::NSFont::smallSystemFontSize(),
        0.0,
    )));
    label.setTextColor(Some(&objc2_app_kit::NSColor::secondaryLabelColor()));
    label.setSelectable(true);
    label.sizeToFit();
    Retained::into_super(Retained::into_super(label))
}

/// The start of `text` as the sheet shows it, with control characters written as `^X`.
fn preview(text: &str) -> String {
    let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalised.split('\n').collect();
    let mut out: Vec<String> = lines
        .iter()
        .take(PREVIEW_LINES)
        .map(|line| {
            let shown: String = line.chars().flat_map(visible).collect();
            if shown.chars().count() > PREVIEW_WIDTH {
                format!("{}…", shown.chars().take(PREVIEW_WIDTH).collect::<String>())
            } else {
                shown
            }
        })
        .collect();
    if lines.len() > PREVIEW_LINES {
        out.push(tf(
            "… und {} weitere Zeilen",
            &[&(lines.len() - PREVIEW_LINES).to_string()],
        ));
    }
    out.join("\n")
}

fn visible(c: char) -> Vec<char> {
    match c {
        '\t' => vec!['\t'],
        c if u32::from(c) < 0x20 => vec!['^', char::from_u32(u32::from(c) + 0x40).unwrap_or('?')],
        '\u{7f}' => vec!['^', '?'],
        c if c.is_control() => vec!['�'],
        c => vec![c],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preview_shows_controls_and_cuts_long_texts() {
        assert_eq!(preview("echo a\x1b[201~\x03\r\nls"), "echo a^[[201~^C\nls");
        let many = (0..10).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
        let shown = preview(&many);
        assert!(shown.starts_with("0\n1\n2\n3\n4\n5\n"), "{shown}");
        assert_eq!(shown.lines().count(), PREVIEW_LINES + 1);
        assert!(preview(&"x".repeat(200)).ends_with('…'));
    }
}
