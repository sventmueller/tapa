//! The built-in themes (F162): the Ghostty folder of the iTerm2-Color-Schemes collection (MIT), copied into the app's
//! resources with its license; nothing is fetched while building.

use std::path::Path;

use crate::root;

/// Copies every theme of `source`, a checkout's `ghostty` folder, into `resources/themes`.
pub(crate) fn import(source: &Path) -> Result<(), String> {
    let target = root().join("resources/themes");
    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    let mut count = 0;
    for entry in std::fs::read_dir(source)
        .map_err(|e| format!("{}: {e}", source.display()))?
        .flatten()
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        if !(text.contains("background") && text.contains("foreground")) {
            continue;
        }
        std::fs::write(target.join(entry.file_name()), text).map_err(|e| e.to_string())?;
        count += 1;
    }
    let license = source.parent().map(|checkout| checkout.join("LICENSE"));
    if let Some(license) = license.filter(|license| license.exists()) {
        std::fs::copy(license, root().join("resources/themes-LICENSE")).map_err(|e| e.to_string())?;
    }
    println!("themes: {count} copied");
    Ok(())
}
