//! The self-test's steps for the command palette and copy mode: `palette:<query>` types into the open palette and
//! prints what it found, ending in `\n` runs the first; `copy:<keys>` feeds keys written as Vim writes them, such as
//! `vjj$` or `<C-v>`, to the copy mode of the session in front, and `fieldvim:<keys>` to its input field's Vim keys,
//! `fieldicon:<name>` prints the toolbar's field switch, `responder:<name>` what has the keyboard, `keys:<text>` types
//! through the window as the keyboard does;
//! `clipboard:<name>` prints the pasteboard's text; `appearance:<light|dark|system>` draws the app in that appearance;
//! `paste:<text>` pastes as ⌘V does (`\n` a line break, `\e` Escape) and prints whether a sheet asks first, which
//! `sheet:accept` or `sheet:cancel` answers; `secure:<name>` prints the state of secure keyboard entry;
//! `blocks:<name>` prints the command blocks the session in front shows, and `click:<line>.<column>`
//! clicks that cell of its screen as a plain click moves the cursor at a prompt (F83).

use crate::state::app;

/// Runs `step` if it is one of these; whether it was.
pub(super) fn run(step: &str) -> bool {
    if keyboard(step) || super::workspace::run(step) {
        return true;
    }
    if let Some(query) = step.strip_prefix("palette:") {
        let (query, go) = query.strip_suffix("\\n").map_or((query, false), |query| (query, true));
        let found = term_ui::palette::drive(query, go);
        println!("palette {query:?}: {:?}", &found[..found.len().min(5)]);
    } else if let Some(keys) = step.strip_prefix("copy:") {
        if let Some(view) = app().front().and_then(|window| window.focused_view()) {
            println!("copy {keys:?}: {}", view.copy_keys(keys));
        }
    } else if let Some(name) = step.strip_prefix("fieldicon:") {
        if let Some(window) = app().front() {
            println!("fieldicon {name}: {}", window.field_switch());
        }
    } else if let Some(keys) = step.strip_prefix("fieldvim:") {
        if let Some(view) = app().front().and_then(|window| window.focused_view()) {
            println!("fieldvim {keys:?}: {}", view.field_vim_keys(keys));
        }
    } else if let Some(name) = step.strip_prefix("blocks:") {
        if let Some(view) = app().front().and_then(|window| window.focused_view()) {
            println!("blocks {name}: {}", view.blocks_report());
        }
    } else if let Some(cell) = step.strip_prefix("click:") {
        let (line, column) = cell.split_once('.').unwrap_or((cell, "0"));
        let point = term_session::Point {
            line: line.parse().unwrap_or(0),
            column: column.parse().unwrap_or(0),
        };
        if let Some(view) = app().front().and_then(|window| window.focused_view()) {
            println!("click {cell}: moved {}", view.click_at(point));
        }
    } else if let Some(text) = step.strip_prefix("paste:") {
        let text = text.replace("\\n", "\n").replace("\\e", "\x1b");
        if let Some(view) = app().front().and_then(|window| window.focused_view()) {
            println!("paste {text:?}: sheet {}", view.paste_text_for_selftest(&text));
        }
    } else if let Some(answer) = step.strip_prefix("sheet:") {
        if let Some(view) = app().front().and_then(|window| window.focused_view()) {
            println!("sheet {answer}: {}", view.answer_sheet(answer == "accept"));
        }
    } else if let Some(name) = step.strip_prefix("secure:") {
        println!(
            "secure {name}: manual {} on {} system {}",
            term_ui::secure::manual(),
            term_ui::secure::active(),
            term_ui::secure::system_active()
        );
    } else if let Some(name) = step.strip_prefix("clipboard:") {
        let text = objc2_app_kit::NSPasteboard::generalPasteboard()
            .stringForType(term_render::constant!(objc2_app_kit::NSPasteboardTypeString))
            .map(|text| text.to_string());
        println!("clipboard {name}: {text:?}");
    } else if let Some(cell) = step.strip_prefix("doubleclick:") {
        let view = app().front().and_then(|window| window.focused_view());
        if let (Some(view), [point]) = (view, &cells(cell)[..]) {
            println!("doubleclick {cell}: {:?}", view.double_click_at(*point));
        }
    } else if let Some(corners) = step.strip_prefix("blockselect:") {
        let view = app().front().and_then(|window| window.focused_view());
        if let (Some(view), [from, to]) = (view, &cells(corners)[..]) {
            println!("blockselect {corners}: {:?}", view.block_select(*from, *to));
        }
    } else if let Some(name) = step.strip_prefix("scrollbar:") {
        if let Some(view) = app().front().and_then(|window| window.focused_view()) {
            println!("scrollbar {name}: {}", view.scrollbar_state());
        }
    } else if let Some(name) = step.strip_prefix("appearance:") {
        appearance(name);
    } else if let Some(name) = step.strip_prefix("richclip:") {
        let pasteboard = objc2_app_kit::NSPasteboard::generalPasteboard();
        let rtf = pasteboard
            .dataForType(term_render::constant!(objc2_app_kit::NSPasteboardTypeRTF))
            .map(|data| data.len());
        let html = pasteboard
            .stringForType(term_render::constant!(objc2_app_kit::NSPasteboardTypeHTML))
            .map(|html| html.to_string());
        println!("richclip {name}: rtf {rtf:?} bytes, html {html:?}");
    } else {
        return false;
    }
    true
}

/// Draws the app in the appearance `name`, `light`, `dark` or their `contrast` forms, or as the system has it.
fn appearance(name: &str) {
    use objc2_app_kit::{
        NSAppearance, NSAppearanceNameAccessibilityHighContrastAqua, NSAppearanceNameAccessibilityHighContrastDarkAqua,
        NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    };
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        return;
    };
    let wanted = match name {
        "light" => Some(term_render::constant!(NSAppearanceNameAqua)),
        "dark" => Some(term_render::constant!(NSAppearanceNameDarkAqua)),
        "light-contrast" => Some(term_render::constant!(NSAppearanceNameAccessibilityHighContrastAqua)),
        "dark-contrast" => Some(term_render::constant!(
            NSAppearanceNameAccessibilityHighContrastDarkAqua
        )),
        _ => None,
    };
    let appearance = wanted.and_then(NSAppearance::appearanceNamed);
    objc2_app_kit::NSApplication::sharedApplication(mtm).setAppearance(appearance.as_deref());
    println!("appearance {name}");
}

/// The screen cells `line:column`, each pair after the other, in `text` such as `3:4` or `0:2:5:9`.
fn cells(text: &str) -> Vec<term_session::Point> {
    let numbers: Vec<usize> = text.split(':').filter_map(|part| part.parse().ok()).collect();
    let (pairs, _) = numbers.as_chunks::<2>();
    pairs
        .iter()
        .map(|&[line, column]| term_session::Point { line, column })
        .collect()
}

/// The steps `responder:` and `keys:`; whether `step` was one.
fn keyboard(step: &str) -> bool {
    if let Some(name) = step.strip_prefix("responder:") {
        let view = app().front().and_then(|window| window.focused_view());
        let responder = app()
            .front()
            .and_then(|window| window.front_ns_window())
            .and_then(|window| window.firstResponder());
        let class = responder.map_or_else(
            || "none".to_string(),
            |responder| responder.class().name().to_string_lossy().into_owned(),
        );
        let field = view.map(|view| view.field_contents());
        println!("responder {name}: {class}, field {field:?}");
    } else if let Some(text) = step.strip_prefix("keys:") {
        let text = text.replace("\\n", "\r");
        if let Some(window) = app().front().and_then(|window| window.front_ns_window()) {
            for c in text.chars() {
                let event = objc2_app_kit::NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                    objc2_app_kit::NSEventType::KeyDown,
                    objc2_foundation::NSPoint::ZERO,
                    objc2_app_kit::NSEventModifierFlags::empty(),
                    0.0,
                    window.windowNumber(),
                    None,
                    &term_ui::kit::ns(&c.to_string()),
                    &term_ui::kit::ns(&c.to_string()),
                    false,
                    0,
                );
                if let Some(event) = event {
                    window.sendEvent(&event);
                }
            }
        }
    } else {
        return false;
    }
    true
}
