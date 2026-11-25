//! Drawing the vendored Simple Icons logos natively: each one path in a 24-unit square, parsed once and drawn by Core
//! Graphics as a template image that AppKit tints and draws anew for each scale.

use std::cell::RefCell;
use std::collections::HashMap;

use block2::RcBlock;
use kurbo::{BezPath, PathEl};
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_app_kit::{NSGraphicsContext, NSImage};
use objc2_core_graphics::CGContext;
use objc2_foundation::{NSRect, NSSize};

use term_base::guarded;

/// The side of an icon's square, in points.
pub(crate) const SIDE: f64 = 16.0;

/// The side a logo takes in it: a logo fills its square, where a symbol leaves room around its shape.
const DRAWN: f64 = 13.0;

/// The side of the square Simple Icons draws in.
const VIEW_BOX: f64 = 24.0;

thread_local! {
    static IMAGES: RefCell<HashMap<&'static str, Retained<NSImage>>> = RefCell::new(HashMap::new());
}

/// The SVG file of the logo `name`, see `assets/simple-icons/README.md`.
fn svg(name: &str) -> Option<&'static str> {
    macro_rules! logos {
        ($($name:literal),* $(,)?) => {
            match name {
                $($name => Some(include_str!(concat!("../../../assets/simple-icons/icons/", $name, ".svg"))),)*
                _ => None,
            }
        };
    }
    logos!(
        "c",
        "cplusplus",
        "css",
        "dart",
        "docker",
        "elixir",
        "git",
        "go",
        "graphql",
        "haskell",
        "html5",
        "javascript",
        "kotlin",
        "lua",
        "markdown",
        "openjdk",
        "php",
        "prisma",
        "python",
        "react",
        "ruby",
        "rust",
        "scala",
        "svelte",
        "swift",
        "typescript",
        "yaml",
        "zig",
        "nodedotjs",
        "bun",
        "kubernetes",
        "neovim",
        "vim",
        "claude",
        "googlegemini",
        "npm",
        "pnpm",
        "yarn",
        "deno",
        "homebrew",
        "tmux",
        "htop",
        "mysql",
        "postgresql",
        "redis",
        "sqlite",
        "terraform",
    )
}

/// The outline of the logo `name`, from the `d` of its one path.
pub(crate) fn path(name: &str) -> Option<BezPath> {
    let svg = svg(name)?;
    let start = svg.find(" d=\"")? + 4;
    let length = svg[start..].find('"')?;
    BezPath::from_svg(&svg[start..start + length]).ok()
}

/// A template image of the logo `name`, centred in an icon's square.
pub(crate) fn image(name: &'static str) -> Option<Retained<NSImage>> {
    if let Some(image) = IMAGES.with(|images| images.borrow().get(name).cloned()) {
        return Some(image);
    }
    let outline = path(name)?;
    let handler = RcBlock::new(move |rect: NSRect| -> Bool {
        guarded(Bool::NO, || {
            let Some(context) = NSGraphicsContext::currentContext() else {
                return Bool::NO;
            };
            let ctx = context.CGContext();
            // A copy resized for a larger interface draws the logo in proportion.
            let drawn = DRAWN * rect.size.width / SIDE;
            let scale = drawn / VIEW_BOX;
            let left = rect.origin.x + (rect.size.width - drawn) / 2.0;
            let top = rect.origin.y + (rect.size.height + drawn) / 2.0;
            // SVG counts down from the top, the image up from the bottom.
            let at = |p: kurbo::Point| (left + p.x * scale, top - p.y * scale);
            CGContext::set_rgb_fill_color(Some(&ctx), 0.0, 0.0, 0.0, 1.0);
            CGContext::begin_path(Some(&ctx));
            for element in outline.elements() {
                match *element {
                    PathEl::MoveTo(p) => {
                        let (x, y) = at(p);
                        CGContext::move_to_point(Some(&ctx), x, y);
                    }
                    PathEl::LineTo(p) => {
                        let (x, y) = at(p);
                        CGContext::add_line_to_point(Some(&ctx), x, y);
                    }
                    PathEl::QuadTo(c, p) => {
                        let ((cx, cy), (x, y)) = (at(c), at(p));
                        CGContext::add_quad_curve_to_point(Some(&ctx), cx, cy, x, y);
                    }
                    PathEl::CurveTo(c1, c2, p) => {
                        let ((x1, y1), (x2, y2), (x, y)) = (at(c1), at(c2), at(p));
                        CGContext::add_curve_to_point(Some(&ctx), x1, y1, x2, y2, x, y);
                    }
                    PathEl::ClosePath => CGContext::close_path(Some(&ctx)),
                }
            }
            CGContext::fill_path(Some(&ctx));
            Bool::YES
        })
    });
    let image = NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(SIDE, SIDE), false, &handler);
    image.setTemplate(true);
    IMAGES.with(|images| images.borrow_mut().insert(name, image.clone()));
    Some(image)
}
