//! The app icon: `assets/logo/app-icon.svg` rendered into `resources/AppIcon.icns`, which the bundle copies along.
//! Needs `resvg` (Homebrew) and `iconutil` (macOS); the result is checked in, so only changing the picture needs them.

use std::process::Command;

use crate::{root, run, target_dir};

/// The edge lengths an icon set holds, each also at twice the size.
const SIZES: [u32; 5] = [16, 32, 128, 256, 512];

/// Renders the icon set and joins it into the icon file.
pub(crate) fn build() -> Result<(), String> {
    let source = root().join("assets/logo/app-icon.svg");
    let set = target_dir().join("AppIcon.iconset");
    let _ = std::fs::remove_dir_all(&set);
    std::fs::create_dir_all(&set).map_err(|e| e.to_string())?;
    for size in SIZES {
        for (scale, suffix) in [(1, ""), (2, "@2x")] {
            run(Command::new("resvg")
                .arg("-w")
                .arg((size * scale).to_string())
                .arg(&source)
                .arg(set.join(format!("icon_{size}x{size}{suffix}.png"))))?;
        }
    }
    let icns = root().join("resources/AppIcon.icns");
    run(Command::new("iconutil").args(["-c", "icns", "-o"]).arg(&icns).arg(&set))?;
    println!("{}", icns.display());
    Ok(())
}
