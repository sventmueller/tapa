//! The page of splat's hidden renderer of formulas and diagrams, copied from splat's resources into the bundle's as
//! Dango does: `render.html` and the libraries in `js/`, without those of splat's formula field.

use std::path::Path;

const PAGE: &str = "render.html";

/// What only the formula field of splat's editor loads.
pub(crate) const FORMULA_FIELD: [&str; 4] = [
    "mathlive.min.js",
    "mathlive-fonts",
    "mathfield-page.js",
    "LICENSE-mathlive",
];

/// Copies the page and its libraries from `splat`, splat's `resources`, into `resources`.
pub(crate) fn copy(splat: &Path, resources: &Path) -> Result<(), String> {
    std::fs::create_dir_all(resources).map_err(|e| e.to_string())?;
    copy_file(&splat.join(PAGE), &resources.join(PAGE))?;
    let (from, to) = (splat.join("js"), resources.join("js"));
    std::fs::create_dir_all(&to).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(&from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", from.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || FORMULA_FIELD.contains(&name.as_str()) {
            continue;
        }
        copy_any(&entry.path(), &to.join(&name))?;
    }
    Ok(())
}

fn copy_any(from: &Path, to: &Path) -> Result<(), String> {
    if !from.is_dir() {
        return copy_file(from, to);
    }
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(from)
        .map_err(|e| format!("{}: {e}", from.display()))?
        .flatten()
    {
        copy_any(&entry.path(), &to.join(entry.file_name()))?;
    }
    Ok(())
}

fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::copy(from, to)
        .map(drop)
        .map_err(|e| format!("{}: {e}", from.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_comes_without_the_formula_field() {
        let base = std::env::temp_dir().join(format!("tapa-render-page-{}", std::process::id()));
        let (splat, resources) = (base.join("splat"), base.join("app"));
        std::fs::create_dir_all(splat.join("js/mathjax")).unwrap();
        std::fs::write(splat.join(PAGE), "<html>").unwrap();
        std::fs::write(splat.join("js/render.js"), "").unwrap();
        std::fs::write(splat.join("js/mathjax/tex.js"), "").unwrap();
        std::fs::write(splat.join("js/mathlive.min.js"), "").unwrap();
        copy(&splat, &resources).unwrap();
        assert!(resources.join(PAGE).exists() && resources.join("js/mathjax/tex.js").exists());
        assert!(!resources.join("js/mathlive.min.js").exists());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
