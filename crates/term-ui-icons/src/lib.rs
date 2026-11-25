//! The icon of a program or a file in lists: the logo of a common program or language from Simple Icons, vendored in
//! `assets/simple-icons` (see the README there), or an SF Symbol for its kind, in a system colour that keeps its
//! contrast in every appearance. Folders have no icon, only the outline view's disclosure triangle, and keep the icon's room so
//! their names line up with the files'. The icons are decoration: VoiceOver reads the file's name beside them.
//!
//! A file's kind follows from its name alone:
//!
//! ```
//! use term_ui_icons::{Shape, icon_of};
//!
//! assert_eq!(icon_of("main.rs").shape, Shape::Logo("rust"));
//! assert_eq!(icon_of("README.md").id, "readme");
//! assert_eq!(icon_of("noextension").id, "document");
//! ```

#![forbid(unsafe_code)]

mod kinds;
mod logo;
mod programs;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSFontWeightRegular, NSImage, NSImageScaling, NSImageSymbolConfiguration, NSImageView};
use objc2_foundation::{NSCopying, NSSize};
use splat_markdown::palette::Rgb;

pub(crate) use kinds::Tint;
pub use kinds::{Icon, Shape};
pub use programs::{BROWSER, PROGRAM, REMOTE, SHELL, program_icon, shell_icon};

/// The point size of the symbols, which matches the logos' drawn size.
const SYMBOL_SIZE: f64 = 12.0;

/// The icon of a file called `name`.
pub fn icon_of(name: &str) -> Icon {
    kinds::icon_of(name)
}

/// An image view, 16 points square, with the icon of the file at `path`, or empty for a folder; hidden from
/// VoiceOver.
pub fn image_view(mtm: MainThreadMarker, path: &Path, folder: bool) -> Retained<NSImageView> {
    let view = NSImageView::new(mtm);
    view.setImageScaling(NSImageScaling::ScaleNone);
    show(&view, path, folder);
    term_ui::kit::hide_from_voiceover(&view);
    view
}

/// Shows the icon of the file at `path` in `view`, one of [`image_view`]'s, or nothing for a folder.
pub fn show(view: &NSImageView, path: &Path, folder: bool) {
    show_scaled(view, path, folder, 1.0);
}

/// [`show`] with the icon `scale` times as large, for a list that follows the interface size.
pub fn show_scaled(view: &NSImageView, path: &Path, folder: bool, scale: f64) {
    let icon = (!folder).then(|| icon_of(&file_name(path)));
    let image = icon.as_ref().and_then(|icon| template_at(icon, scale));
    view.setImage(image.as_deref());
    view.setContentTintColor(icon.filter(|_| image.is_some()).map(|icon| color(icon.tint)).as_deref());
}

/// The icon of the file at `path` drawn in its colour, for a place that takes an image rather than an image view,
/// such as the title of a tab.
pub fn image(path: &Path) -> Option<Retained<NSImage>> {
    image_scaled(path, 1.0)
}

/// [`image`] `scale` times as large, for a tab bar that follows the interface size.
pub fn image_scaled(path: &Path, scale: f64) -> Option<Retained<NSImage>> {
    let icon = icon_of(&file_name(path));
    let template = template_at(&icon, scale)?;
    Some(term_ui::kit::tinted(&template, &color(icon.tint)))
}

/// `icon` drawn in its colour at `scale` times its size, for the sidebar's rows and pane bars.
pub fn icon_image(icon: &Icon, scale: f64) -> Option<Retained<NSImage>> {
    let template = template_at(icon, scale)?;
    Some(term_ui::kit::tinted(&template, &color(icon.tint)))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// The icon's shape as a template image.
pub fn template(icon: &Icon) -> Option<Retained<NSImage>> {
    template_at(icon, 1.0)
}

/// [`template`] `scale` times as large, for a list whose rows tint it themselves, such as a sidebar's.
pub fn template_scaled(icon: &Icon, scale: f64) -> Option<Retained<NSImage>> {
    template_at(icon, scale)
}

thread_local! {
    static TEMPLATES: RefCell<HashMap<(Shape, u64), Retained<NSImage>>> = RefCell::new(HashMap::new());
}

/// [`template`] `scale` times as large, made once per shape and scale.
fn template_at(icon: &Icon, scale: f64) -> Option<Retained<NSImage>> {
    let key = (icon.shape, scale.to_bits());
    if let Some(image) = TEMPLATES.with(|cache| cache.borrow().get(&key).cloned()) {
        return Some(image);
    }
    let image = make_template(icon, scale)?;
    TEMPLATES.with(|cache| cache.borrow_mut().insert(key, image.clone()));
    Some(image)
}

fn make_template(icon: &Icon, scale: f64) -> Option<Retained<NSImage>> {
    match icon.shape {
        Shape::Logo(name) => {
            let image = logo::image(name)?;
            if (scale - 1.0).abs() < f64::EPSILON {
                return Some(image);
            }
            // The cached image is shared, so a resized one is a copy.
            let image = image.copy();
            image.setSize(NSSize::new(logo::SIDE * scale, logo::SIDE * scale));
            Some(image)
        }
        Shape::Symbol(name) => {
            let weight = term_render::constant!(NSFontWeightRegular);
            let points = SYMBOL_SIZE * scale;
            let configuration = NSImageSymbolConfiguration::configurationWithPointSize_weight(points, weight);
            term_ui::kit::symbol(name, "")?.imageWithSymbolConfiguration(&configuration)
        }
    }
}

/// The colour of `tint`: Apple's system colour, darker or lighter where it would not stand out from the ground.
fn color(tint: Tint) -> Retained<NSColor> {
    let rgb = match tint {
        Tint::Plain => return NSColor::secondaryLabelColor(),
        Tint::Blue => Rgb(0x00, 0x7A, 0xFF),
        Tint::Brown => Rgb(0xA2, 0x84, 0x5E),
        Tint::Cyan => Rgb(0x55, 0xBE, 0xF0),
        Tint::Gray => Rgb(0x8E, 0x8E, 0x93),
        Tint::Green => Rgb(0x28, 0xCD, 0x41),
        Tint::Indigo => Rgb(0x58, 0x56, 0xD6),
        Tint::Orange => Rgb(0xFF, 0x95, 0x00),
        Tint::Pink => Rgb(0xFF, 0x2D, 0x55),
        Tint::Purple => Rgb(0xAF, 0x52, 0xDE),
        Tint::Red => Rgb(0xFF, 0x3B, 0x30),
        Tint::Teal => Rgb(0x59, 0xAD, 0xC4),
        Tint::Yellow => Rgb(0xFF, 0xCC, 0x00),
    };
    term_ui::kit::colour::accent(term_ui::kit::colour::Rgb(rgb.0, rgb.1, rgb.2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_get_the_icon_of_their_kind() {
        let cases = [
            ("main.rs", "rust"),
            ("server.go", "go"),
            ("app.ts", "typescript"),
            ("types.d.ts", "typescript"),
            ("app.tsx", "react"),
            ("index.js", "javascript"),
            ("index.mjs", "javascript"),
            ("view.jsx", "react"),
            ("tool.py", "python"),
            ("View.swift", "swift"),
            ("lexer.c", "c"),
            ("lexer.cpp", "cplusplus"),
            ("lexer.h", "c"),
            ("Main.java", "openjdk"),
            ("Main.kt", "kotlin"),
            ("app.rb", "ruby"),
            ("Gemfile", "ruby"),
            ("index.php", "php"),
            ("Program.cs", "csharp"),
            ("schema.prisma", "prisma"),
            ("build.sh", "shell"),
            (".zshrc", "shell"),
            ("package.json", "json"),
            ("tsconfig.json", "json"),
            ("package-lock.json", "lock"),
            ("rustfmt.toml", "config"),
            ("Cargo.toml", "config"),
            ("ci.yml", "yaml"),
            ("compose.yaml", "docker"),
            ("docker-compose.override.yml", "docker"),
            ("Info.plist", "markup"),
            ("index.html", "html5"),
            ("site.css", "css"),
            ("site.scss", "css"),
            ("README.md", "readme"),
            ("readme", "readme"),
            ("notes.md", "markdown"),
            ("todo.txt", "todo"),
            ("todolist.rs", "rust"),
            ("schema.sql", "database"),
            ("api.proto", "document"),
            ("Cargo.lock", "lock"),
            ("yarn.lock", "lock"),
            ("logo.png", "image"),
            ("logo.svg", "vector"),
            ("manual.pdf", "pdf"),
            ("dist.zip", "archive"),
            ("data.csv", "table"),
            ("Makefile", "build"),
            ("CMakeLists.txt", "build"),
            ("Dockerfile", "docker"),
            ("Dockerfile.dev", "docker"),
            (".gitignore", "git"),
            (".gitmodules", "git"),
            ("LICENSE", "license"),
            ("LICENSE-MIT", "license"),
            ("CHANGELOG.md", "changelog"),
            ("VERSION", "changelog"),
            (".editorconfig", "config"),
            (".env", "secrets"),
            (".env.local", "secrets"),
            (".envrc", "shell"),
            ("parser_test.go", "go"),
            ("button.test.tsx", "react"),
            ("login.spec.ts", "typescript"),
            ("random.xyz", "document"),
            ("noextension", "document"),
        ];
        for (name, id) in cases {
            assert_eq!(icon_of(name).id, id, "{name}");
        }
    }

    #[test]
    fn the_common_kinds_differ_by_shape_or_colour() {
        let kinds = [
            "a.rs",
            "a.go",
            "a.ts",
            "a.tsx",
            "a.js",
            "a.py",
            "a.swift",
            "a.c",
            "a.cpp",
            "a.java",
            "a.kt",
            "a.rb",
            "a.php",
            "a.cs",
            "a.sh",
            "a.json",
            "a.yml",
            "a.html",
            "a.css",
            "a.md",
            "a.sql",
            "a.csv",
            "a.pdf",
            "a.png",
            "a.svg",
            "a.zip",
            "a.txt",
            "Makefile",
            "Dockerfile",
            ".gitignore",
            "README.md",
            "LICENSE",
            "a.toml",
            ".env",
            "a.lock",
            "a.unknown",
        ];
        let mut seen = std::collections::HashMap::new();
        for name in kinds {
            let icon = icon_of(name);
            if let Some(other) = seen.insert((icon.shape, icon.tint), name) {
                panic!("{name} looks like {other}");
            }
        }
    }

    #[test]
    fn every_logo_is_vendored_and_parses() {
        for icon in kinds::LOGOS {
            let Shape::Logo(name) = icon.shape else { continue };
            let path = logo::path(name).unwrap_or_else(|| panic!("{name} does not parse"));
            let bounds = kurbo::Shape::bounding_box(&path);
            assert!(
                bounds.x0 >= -0.01 && bounds.y0 >= -0.01 && bounds.x1 <= 24.01 && bounds.y1 <= 24.01,
                "{name} leaves its square: {bounds:?}"
            );
            assert!(
                bounds.width() > 12.0 || bounds.height() > 12.0,
                "{name} is tiny: {bounds:?}"
            );
        }
    }
}
