//! A read-only tab with the differences between a file's version on disk and an editor's unsaved text, as a unified
//! diff with both line numbers.

use std::cell::RefCell;
use std::path::Path;

use objc2::AnyThread;
use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSAccessibility, NSBackgroundColorAttributeName, NSColor, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSTextView, NSUserInterfaceLayoutDirection, NSWorkspace,
};
use objc2_foundation::{NSMutableAttributedString, NSRange, NSString};
use splat_i18n::{t, tf};
use term_document::merge::{self, ComparedLine, LineChange};

use crate::util::{constant, ns};
use term_ui::workspace_host::{TabKind, Tabs, Workspace};

/// Unchanged lines shown around each change.
const CONTEXT: usize = 3;

/// The text views of the open comparison tabs, by tab key.
#[derive(Default)]
pub(crate) struct Comparisons {
    shown: RefCell<Vec<(String, Weak<NSTextView>)>>,
}

fn with_shown<R>(f: impl FnOnce(&RefCell<Vec<(String, Weak<NSTextView>)>>) -> R) -> R {
    f(&crate::state().comparisons.shown)
}

/// Shows the differences between `disk`, the file's version, and `buffer`, the editor's text, in a tab of
/// `controller`, the one it showed before for `path` if there is one.
pub(crate) fn show(controller: &Workspace, path: &Path, disk: &str, buffer: &str) {
    let mtm = controller.mtm();
    let key = format!("compare:{}", path.display());
    if let Some(id) = controller.tab_with_key(&key) {
        controller.close_tab(id);
    }
    let scroll = NSTextView::scrollableTextView(mtm);
    let Some(text_view) = scroll
        .documentView()
        .and_then(|view| view.downcast::<NSTextView>().ok())
    else {
        return;
    };
    let name = super::display_name(path);
    text_view.setEditable(false);
    text_view.setSelectable(true);
    text_view.setTextContainerInset(objc2_foundation::NSSize::new(12.0, 10.0));
    text_view.setUserInterfaceLayoutDirection(NSUserInterfaceLayoutDirection::LeftToRight);
    text_view.setAccessibilityLabel(Some(&ns(&tf(
        "Unterschiede zwischen der Platte und dem Editor: {}",
        &[&name],
    ))));
    // SAFETY: The text view made by `scrollableTextView` owns its text system; asking for the storage keeps TextKit 2.
    if let Some(storage) = unsafe { text_view.textStorage() } {
        storage.setAttributedString(&styled(&merge::compare(disk, buffer, CONTEXT)));
    }
    with_shown(|shown| {
        let mut shown = shown.borrow_mut();
        shown.retain(|(k, view)| *k != key && view.load().is_some());
        shown.push((key.clone(), Weak::from(&*text_view)));
    });
    let title = tf("{} (Platte ↔ Editor)", &[&name]);
    controller.add_tab(TabKind::File, Some(key), &title, scroll.into_super());
}

/// The comparison as text: a line naming each side, then each run of changes with its line numbers.
fn styled(hunks: &[Vec<ComparedLine>]) -> Retained<NSMutableAttributedString> {
    let size = crate::host::settings().font_size;
    let font = NSFont::monospacedSystemFontOfSize_weight(size, constant!(objc2_app_kit::NSFontWeightRegular));
    let mut text = String::new();
    let mut quiet = Vec::new();
    let mut shaded = Vec::new();
    let push = |text: &mut String, line: &str| {
        let start = text.encode_utf16().count();
        text.push_str(line);
        text.push('\n');
        NSRange::new(start, line.encode_utf16().count())
    };
    quiet.push(push(&mut text, &format!("− {}", t("auf der Platte"))));
    quiet.push(push(&mut text, &format!("+ {}", t("in diesem Editor"))));
    if hunks.is_empty() {
        quiet.push(push(&mut text, t("Keine Unterschiede.")));
    }
    for hunk in hunks {
        let header = format!(
            "@@ −{} +{} @@",
            hunk.iter().find_map(|l| l.old).unwrap_or(0),
            hunk.iter().find_map(|l| l.new).unwrap_or(0)
        );
        quiet.push(push(&mut text, ""));
        quiet.push(push(&mut text, &header));
        for line in hunk {
            let number = |n: Option<usize>| n.map_or_else(String::new, |n| n.to_string());
            let mark = match line.change {
                LineChange::Same => ' ',
                LineChange::Removed => '−',
                LineChange::Added => '+',
            };
            let shown = format!("{:>5} {:>5} {mark} {}", number(line.old), number(line.new), line.text);
            let range = push(&mut text, &shown);
            if line.change != LineChange::Same {
                shaded.push((range, line.change));
            }
        }
    }
    let styled = NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &ns(&text));
    let whole = NSRange::new(0, text.encode_utf16().count());
    let set = |key: &NSString, value: &AnyObject, range: NSRange| {
        // SAFETY: Each key is AppKit's attribute key for a value of the type passed with it.
        unsafe { styled.addAttribute_value_range(key, value, range) };
    };
    set(constant!(NSFontAttributeName), &font, whole);
    set(constant!(NSForegroundColorAttributeName), &NSColor::textColor(), whole);
    for range in quiet {
        set(
            constant!(NSForegroundColorAttributeName),
            &NSColor::secondaryLabelColor(),
            range,
        );
    }
    for (range, change) in shaded {
        set(constant!(NSBackgroundColorAttributeName), &background(change), range);
    }
    styled
}

/// The shade of a removed or added line, stronger with Increase Contrast, as in the Git diff tab.
fn background(change: LineChange) -> Retained<NSColor> {
    let strong = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldIncreaseContrast();
    let alpha = if strong { 0.32 } else { 0.16 };
    match change {
        LineChange::Added => NSColor::systemGreenColor(),
        _ => NSColor::systemRedColor(),
    }
    .colorWithAlphaComponent(alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comparison_names_both_sides_and_numbers_each_line() {
        let styled = styled(&merge::compare("a\nb\n", "a\nB\n", 3));
        let text = styled.string().to_string();
        assert!(text.starts_with(&format!("− {}\n+ {}\n", t("auf der Platte"), t("in diesem Editor"))));
        assert!(text.contains("@@ −1 +1 @@\n"), "{text}");
        assert!(text.contains("    2       − b\n"), "{text}");
        assert!(text.contains("          2 + B\n"), "{text}");
    }
}

#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub(crate) use selftest_probe::*;
