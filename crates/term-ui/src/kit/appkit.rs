use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::{
    NSAccessibility, NSButton, NSColor, NSControl, NSEventModifierFlags, NSImage, NSImageSymbolConfiguration, NSMenu,
    NSMenuItem, NSTextField,
};
use objc2_foundation::NSRect;

pub use term_render::ns;

/// How many arguments the method behind `selector` takes: its name has one colon for each.
fn arguments_of(selector: Sel) -> usize {
    selector.name().to_bytes().iter().filter(|&&byte| byte == b':').count()
}

/// Whether `target` has a method for `action` that takes one argument, the sender, as AppKit sends it.
fn answers(target: &AnyObject, action: Sel) -> bool {
    arguments_of(action) == 1 && target.class().responds_to(action)
}

/// Has `control` send `action` to `target`.
pub fn set_action(control: &NSControl, target: &AnyObject, action: Sel) {
    assert!(answers(target, action), "the target has no action {action:?}");
    // SAFETY: The target has a method for the action that takes the sender, checked above; AppKit holds the target as
    // a zeroing weak reference, so one that is gone gets nothing.
    unsafe {
        control.setTarget(Some(target));
        control.setAction(Some(action));
    }
}

/// Has `item` send its action to `target` instead of along the responder chain.
pub fn set_target(item: &NSMenuItem, target: &AnyObject) {
    let action = item.action();
    assert!(
        action.is_some_and(|action| answers(target, action)),
        "the target has no action {action:?}"
    );
    // SAFETY: The target has a method for the item's action that takes the sender, checked above; AppKit holds the
    // target as a zeroing weak reference.
    unsafe { item.setTarget(Some(target)) };
}

/// An SF Symbol, described for VoiceOver.
pub fn symbol(name: &str, description: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(name), Some(&ns(description)))
}

/// The symbol `name` at `points`, an SF Symbol's in the regular weight, described for VoiceOver.
pub fn symbol_at(name: &str, description: &str, points: f64) -> Option<Retained<NSImage>> {
    symbol(name, description)?.imageWithSymbolConfiguration(
        &NSImageSymbolConfiguration::configurationWithPointSize_weight(
            points,
            term_render::constant!(objc2_app_kit::NSFontWeightRegular),
        ),
    )
}

/// `image`, a template, drawn in `color` as an image of its own, which follows the appearance where it is drawn.
pub fn tinted(image: &NSImage, color: &NSColor) -> Retained<NSImage> {
    let (image, color) = (image.retain(), color.retain());
    let size = image.size();
    let draw = block2::RcBlock::new(move |rect: NSRect| -> Bool {
        super::guarded(Bool::NO, || {
            image.drawInRect(rect);
            color.set();
            objc2_app_kit::NSRectFillUsingOperation(rect, objc2_app_kit::NSCompositingOperation::SourceAtop);
            Bool::YES
        })
    });
    NSImage::imageWithSize_flipped_drawingHandler(size, false, &draw)
}

/// `image`, a template, in four colours that meet in its middle, as Google's star shows them: `points` are the
/// colours of its left, top, right and bottom.
pub fn four_colours(image: &NSImage, points: &[Retained<NSColor>; 4]) -> Retained<NSImage> {
    let image = image.retain();
    let [left, top, right, bottom] = points.clone();
    let size = image.size();
    let draw = block2::RcBlock::new(move |rect: NSRect| -> Bool {
        super::guarded(Bool::NO, || {
            image.drawInRect(rect);
            if let Some(context) = objc2_app_kit::NSGraphicsContext::currentContext() {
                context.setCompositingOperation(objc2_app_kit::NSCompositingOperation::SourceAtop);
            }
            let fill = |from: &NSColor, to: &NSColor, rect: NSRect, angle: f64| {
                let colors = objc2_foundation::NSArray::from_slice(&[from, to]);
                let gradient = objc2_app_kit::NSGradient::initWithColors(
                    <objc2_app_kit::NSGradient as objc2::AnyThread>::alloc(),
                    &colors,
                );
                if let Some(gradient) = gradient {
                    gradient.drawInRect_angle(rect, angle);
                }
            };
            fill(&left, &right, rect, 0.0);
            let half = NSRect::new(
                rect.origin,
                objc2_foundation::NSSize::new(rect.size.width, rect.size.height / 2.0),
            );
            let upper = NSRect::new(
                objc2_foundation::NSPoint::new(rect.origin.x, rect.origin.y + half.size.height),
                half.size,
            );
            fill(&bottom, &bottom.colorWithAlphaComponent(0.0), half, 90.0);
            fill(&top.colorWithAlphaComponent(0.0), &top, upper, 90.0);
            Bool::YES
        })
    });
    NSImage::imageWithSize_flipped_drawingHandler(size, false, &draw)
}

/// A borderless button titled `title` that sends `action` along the responder chain, or to `target` when given.
pub fn borderless_button(
    mtm: MainThreadMarker,
    title: &str,
    target: Option<&AnyObject>,
    action: Sel,
) -> Retained<NSButton> {
    // SAFETY: A nil target sends the action along the responder chain, where only an object that answers it gets it.
    let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, Some(action), mtm) };
    if let Some(target) = target {
        set_action(&button, target, action);
    }
    button.setBordered(false);
    button
}

/// A borderless button showing the symbol `name` at `points`, labelled `description` for VoiceOver and its tooltip.
pub fn symbol_button(
    mtm: MainThreadMarker,
    name: &str,
    description: &str,
    points: f64,
    target: Option<&AnyObject>,
    action: Sel,
) -> Retained<NSButton> {
    let image = symbol_at(name, description, points).unwrap_or_default();
    // SAFETY: A nil target sends the action along the responder chain, where only an object that answers it gets it.
    let button = unsafe { NSButton::buttonWithImage_target_action(&image, None, Some(action), mtm) };
    if let Some(target) = target {
        set_action(&button, target, action);
    }
    button.setBordered(false);
    button.setSymbolConfiguration(Some(&NSImageSymbolConfiguration::configurationWithPointSize_weight(
        points,
        term_render::constant!(objc2_app_kit::NSFontWeightRegular),
    )));
    button.setToolTip(Some(&ns(description)));
    button
}

/// Shows `name` with the tool tip `description` on `button` for a moment, as the answer to a click that changes
/// nothing on screen, then puts its own picture and tool tip back.
pub fn acknowledge(button: &NSButton, name: &str, description: &str) {
    let mtm = button.mtm();
    let before = (button.image(), button.toolTip());
    if let Some(image) = symbol(name, description) {
        button.setImage(Some(&image));
    }
    button.setToolTip(Some(&ns(description)));
    let button = dispatch2::MainThreadBound::new((button.retain(), before), mtm);
    super::main_after(std::time::Duration::from_millis(1500), move || {
        // SAFETY: main_after runs its work on the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let (button, (image, tip)) = button.into_inner(mtm);
        button.setImage(image.as_deref());
        button.setToolTip(tip.as_deref());
    });
}

/// Sets what VoiceOver calls `control`, or takes it back with `None`. VoiceOver reads a control through its cell, which
/// does not see what is set on the control itself, so both get it.
pub fn set_label(control: &NSControl, label: Option<&str>) {
    let label = label.map(ns);
    control.setAccessibilityLabel(label.as_deref());
    if let Some(cell) = control.cell() {
        cell.setAccessibilityLabel(label.as_deref());
    }
}

/// Keeps VoiceOver from reading `control`, decoration or text said elsewhere; as with `set_label`, its cell too.
pub fn hide_from_voiceover(control: &NSControl) {
    control.setAccessibilityElement(false);
    if let Some(cell) = control.cell() {
        cell.setAccessibilityElement(false);
    }
}

/// Makes VoiceOver name `control` by the label `title` beside it; as with `set_label`, its cell too.
pub fn set_title_element(control: &NSControl, title: &NSTextField) {
    // SAFETY: The title element is a label, the type AppKit expects there; the control and its cell hold it weakly.
    unsafe { control.setAccessibilityTitleUIElement(Some(title)) };
    if let Some(cell) = control.cell() {
        // SAFETY: The cell takes the same label as title element and holds it weakly too, while the view tree keeps it.
        unsafe { cell.setAccessibilityTitleUIElement(Some(title)) };
    }
}

/// A label that does not wrap or take input.
pub fn label(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    NSTextField::labelWithString(&ns(text), mtm)
}

/// The characters AppKit expects as the key equivalent for the key `name` of a shortcut.
fn key_equivalent(name: &str) -> String {
    let function = name
        .strip_prefix('f')
        .and_then(|n| n.parse::<u32>().ok())
        .filter(|n| (1..=19).contains(n));
    match (name, function) {
        (_, Some(n)) => char::from_u32(objc2_app_kit::NSF1FunctionKey + n - 1).map_or_else(String::new, String::from),
        ("escape", _) => "\u{1b}".into(),
        ("space", _) => " ".into(),
        ("return" | "enter", _) => "\r".into(),
        ("backspace", _) => "\u{8}".into(),
        ("delete", _) => char::from_u32(objc2_app_kit::NSDeleteFunctionKey).map_or_else(String::new, String::from),
        ("home", _) => char::from_u32(objc2_app_kit::NSHomeFunctionKey).map_or_else(String::new, String::from),
        ("end", _) => char::from_u32(objc2_app_kit::NSEndFunctionKey).map_or_else(String::new, String::from),
        ("pageup", _) => char::from_u32(objc2_app_kit::NSPageUpFunctionKey).map_or_else(String::new, String::from),
        ("pagedown", _) => char::from_u32(objc2_app_kit::NSPageDownFunctionKey).map_or_else(String::new, String::from),
        ("tab", _) => "\t".into(),
        ("up", _) => char::from_u32(objc2_app_kit::NSUpArrowFunctionKey).map_or_else(String::new, String::from),
        ("down", _) => char::from_u32(objc2_app_kit::NSDownArrowFunctionKey).map_or_else(String::new, String::from),
        ("left", _) => char::from_u32(objc2_app_kit::NSLeftArrowFunctionKey).map_or_else(String::new, String::from),
        ("right", _) => char::from_u32(objc2_app_kit::NSRightArrowFunctionKey).map_or_else(String::new, String::from),
        _ => name.into(),
    }
}

/// A menu item with a key equivalent written like `alt+cmd+p`, as the settings file writes chords.
pub fn item(mtm: MainThreadMarker, title: &str, action: Option<Sel>, key: &str) -> Retained<NSMenuItem> {
    let chord = key.parse::<term_config::Chord>().ok();
    let equivalent = chord
        .as_ref()
        .map_or_else(String::new, |chord| key_equivalent(&chord.key));
    let equivalent = equivalent.as_str();
    let mut mask = NSEventModifierFlags::empty();
    if let Some(chord) = &chord {
        for (on, flag) in [
            (chord.cmd, NSEventModifierFlags::Command),
            (chord.shift, NSEventModifierFlags::Shift),
            (chord.alt, NSEventModifierFlags::Option),
            (chord.ctrl, NSEventModifierFlags::Control),
        ] {
            if on {
                mask |= flag;
            }
        }
    }
    assert!(
        action.is_none_or(|action| arguments_of(action) <= 1),
        "{action:?} takes more than the sender"
    );
    // SAFETY: With at most one argument, checked above, the action takes nothing but the sender AppKit passes.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &ns(title), action, &ns(equivalent))
    };
    if !equivalent.is_empty() {
        item.setKeyEquivalentModifierMask(mask);
    }
    item
}

/// A menu item whose tag tells its action which of several things it means.
pub fn tagged(mtm: MainThreadMarker, title: &str, action: Sel, key: &str, tag: isize) -> Retained<NSMenuItem> {
    let item = item(mtm, title, Some(action), key);
    item.setTag(tag);
    item
}

/// A menu item titled `title` that opens a menu of `items`, and that menu.
pub fn submenu(
    mtm: MainThreadMarker,
    title: &str,
    items: Vec<Retained<NSMenuItem>>,
) -> (Retained<NSMenuItem>, Retained<NSMenu>) {
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &ns(title));
    for item in &items {
        menu.addItem(item);
    }
    let parent = item(mtm, title, None, "");
    parent.setSubmenu(Some(&menu));
    (parent, menu)
}

/// A menu's separator line.
pub fn separator(mtm: MainThreadMarker) -> Retained<NSMenuItem> {
    NSMenuItem::separatorItem(mtm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_keys_become_the_characters_appkit_matches() {
        assert_eq!(key_equivalent("escape"), "\u{1b}");
        assert_eq!(key_equivalent("f12"), "\u{f70f}");
        assert_eq!(key_equivalent("left"), "\u{f702}");
        assert_eq!(key_equivalent("f"), "f");
        assert_eq!(key_equivalent("'"), "'");
    }

    #[test]
    fn a_selector_has_one_argument_for_each_colon() {
        assert_eq!(arguments_of(objc2::sel!(a)), 0);
        assert_eq!(arguments_of(objc2::sel!(a:)), 1);
        assert_eq!(arguments_of(objc2::sel!(a:b:)), 2);
    }
}
