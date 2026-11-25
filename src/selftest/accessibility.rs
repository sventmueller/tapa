//! The self-test's step `axtree:<name>`: the accessibility tree of the settings window while it is open, else of the
//! front window, as VoiceOver reads it through AppKit, into `<name>.ax.txt` of the self-test's folder; it prints each
//! control that has no name to read out.

use std::fmt::Write as _;
use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{msg_send, sel};
use objc2_foundation::{NSArray, NSString};

use crate::state::app;

/// The roles a user acts on, which need a name of their own unless a subrole, such as a close button's, names them.
const CONTROLS: &[&str] = &[
    "AXButton",
    "AXCheckBox",
    "AXRadioButton",
    "AXPopUpButton",
    "AXMenuButton",
    "AXTextField",
    "AXTextArea",
    "AXComboBox",
    "AXSlider",
    "AXIncrementor",
    "AXDisclosureTriangle",
    "AXLink",
    "AXColorWell",
    "AXSearchField",
];

/// Where the walk stops: web content lives in another process, and a table's rows can be many.
const LIMIT: usize = 4000;

/// Runs `step` if it is `axtree:`; whether it was.
pub(super) fn run(step: &str) -> bool {
    let Some(name) = step.strip_prefix("axtree:") else {
        return false;
    };
    let window = crate::settings::ns_window().or_else(|| app().front().and_then(|window| window.front_ns_window()));
    let Some(window) = window else {
        println!("axtree {name}: no window");
        return true;
    };
    let mut walk = Walk::default();
    walk.visit(&window, 0);
    if let Some(dir) = std::env::var_os("TAPA_SELFTEST_DIR") {
        let _ = std::fs::write(PathBuf::from(dir).join(format!("{name}.ax.txt")), &walk.text);
    }
    println!(
        "axtree {name}: {} elements, {} controls, {} without a name",
        walk.count,
        walk.controls,
        walk.unnamed.len()
    );
    for line in &walk.unnamed {
        println!("axtree {name} unnamed: {line}");
    }
    true
}

#[derive(Default)]
struct Walk {
    text: String,
    count: usize,
    controls: usize,
    unnamed: Vec<String>,
}

impl Walk {
    fn visit(&mut self, element: &AnyObject, depth: usize) {
        if self.count >= LIMIT {
            return;
        }
        self.count += 1;
        let role = string(element, sel!(accessibilityRole)).unwrap_or_default();
        let subrole = string(element, sel!(accessibilitySubrole)).unwrap_or_default();
        let spoken = string(element, sel!(accessibilityRoleDescription)).unwrap_or_default();
        let label = string(element, sel!(accessibilityLabel)).filter(|text| !text.is_empty());
        let title = string(element, sel!(accessibilityTitle)).filter(|text| !text.is_empty());
        let placeholder = string(element, sel!(accessibilityPlaceholderValue)).filter(|text| !text.is_empty());
        let help = string(element, sel!(accessibilityHelp)).filter(|text| !text.is_empty());
        let titled_by = title_element(element);
        let value = string(element, sel!(accessibilityValue))
            .filter(|_| role == "AXStaticText")
            .map(|text| text.chars().take(60).collect::<String>());
        let class = element.class().name().to_string_lossy().into_owned();
        let line = format!(
            "{role} {subrole} ({spoken}) value={value:?} label={label:?} title={title:?} placeholder={placeholder:?} titled-by={titled_by:?} help={help:?} <{class}>"
        );
        let _ = writeln!(self.text, "{}{line}", "  ".repeat(depth));
        if CONTROLS.contains(&role.as_str()) {
            self.controls += 1;
            let named = label.is_some()
                || title.is_some()
                || placeholder.is_some()
                || titled_by.is_some()
                || !subrole.is_empty();
            if !named && !named_by_children(element) {
                self.unnamed.push(line);
            }
        }
        if role == "AXWebArea" {
            return;
        }
        for child in children(element) {
            self.visit(&child, depth + 1);
        }
    }
}

/// Whether a static text inside `element` names it, as a button's or a cell's text does.
fn named_by_children(element: &AnyObject) -> bool {
    children(element).iter().any(|child| {
        string(child, sel!(accessibilityRole)).as_deref() == Some("AXStaticText")
            && string(child, sel!(accessibilityValue)).is_some_and(|text| !text.is_empty())
    })
}

/// The text of the label that names `element`, the title element a form row sets.
fn title_element(element: &AnyObject) -> Option<String> {
    let title = object(element, sel!(accessibilityTitleUIElement), "AXTitleUIElement")?;
    string(&title, sel!(accessibilityValue))
        .or_else(|| string(&title, sel!(accessibilityLabel)))
        .filter(|text| !text.is_empty())
}

fn responds(element: &AnyObject, selector: Sel) -> bool {
    // SAFETY: `respondsToSelector:` is NSObject's, which every accessibility element is.
    unsafe { msg_send![element, respondsToSelector: selector] }
}

/// What the getter `selector` answers, or else the older attribute `attribute`, which AppKit's own elements such as
/// a table's rows still answer only to.
fn object(element: &AnyObject, selector: Sel, attribute: &str) -> Option<Retained<AnyObject>> {
    if responds(element, selector) {
        // SAFETY: The accessibility getters take no argument and return an object or nil.
        let value: Option<Retained<AnyObject>> = unsafe { msg_send![element, performSelector: selector] };
        if value.is_some() {
            return value;
        }
    }
    if !responds(element, sel!(accessibilityAttributeValue:)) {
        return None;
    }
    let name = NSString::from_str(attribute);
    // SAFETY: `accessibilityAttributeValue:` takes an attribute's name and returns an object or nil.
    unsafe { msg_send![element, accessibilityAttributeValue: &*name] }
}

/// What the getter `selector` answers as a string, or none; a value that is not a string counts as none.
fn string(element: &AnyObject, selector: Sel) -> Option<String> {
    let name = selector.name().to_string_lossy();
    let attribute = match name.strip_prefix("accessibility").unwrap_or(&name) {
        "Label" => "AXDescription".to_string(),
        rest => format!("AX{rest}"),
    };
    let value = object(element, selector, &attribute)?;
    value.downcast::<NSString>().ok().map(|text| text.to_string())
}

/// The accessibility children of `element` that VoiceOver reaches, the ignored ones replaced by theirs.
fn children(element: &AnyObject) -> Vec<Retained<AnyObject>> {
    let Some(all) = object(element, sel!(accessibilityChildren), "AXChildren") else {
        return Vec::new();
    };
    let Ok(all) = all.downcast::<NSArray>() else {
        return Vec::new();
    };
    // SAFETY: The array holds accessibility elements, as `accessibilityChildren` returned it.
    let unignored = unsafe { objc2_app_kit::NSAccessibilityUnignoredChildren(&all) };
    // SAFETY: Unignoring keeps the elements, which are objects.
    let unignored: Retained<NSArray<AnyObject>> = unsafe { Retained::cast_unchecked(unignored) };
    unignored.to_vec()
}
