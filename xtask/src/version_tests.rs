use super::*;

pub(crate) const MANIFEST_TEXT: &str = "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.package]\n# The one version.\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[package]\nname = \"app\"\nversion.workspace = true\n\n[dependencies]\nother = { version = \"0.1.0\", path = \"../other\" }\n";

pub(crate) const LOCK_TEXT: &str = "version = 4\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\n \"other\",\n]\n\n[[package]]\nname = \"other\"\nversion = \"0.1.0\"\n";

fn named(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| name.to_string()).collect()
}

#[test]
fn the_version_is_the_workspaces_and_only_its_packages_take_it() {
    assert_eq!(of_manifest(MANIFEST_TEXT), Some("0.1.0"));
    assert_eq!(of_manifest("[package]\nversion = \"1.0.0\"\n"), None);
    assert_eq!(member(MANIFEST_TEXT), Some("app"));
    assert_eq!(member("[package]\nname = \"own\"\nversion = \"0.0.32\"\n"), None);
    assert_eq!(
        member("[package]\nname = \"spaced\"\nversion . workspace = true\n"),
        Some("spaced")
    );
}

#[test]
fn raising_the_version_changes_only_its_lines() {
    let raised = manifest_with(MANIFEST_TEXT, "0.1.1").unwrap();
    assert_eq!(
        raised,
        MANIFEST_TEXT.replacen("version = \"0.1.0\"\nedition", "version = \"0.1.1\"\nedition", 1)
    );
    assert!(manifest_with("[package]\nname = \"a\"\n", "0.1.1").is_err());
    let lock = lock_with(LOCK_TEXT, &named(&["app"]), "0.1.1");
    assert_eq!(
        lock,
        LOCK_TEXT.replacen("\"app\"\nversion = \"0.1.0\"", "\"app\"\nversion = \"0.1.1\"", 1)
    );
    assert_eq!(lock_with(&lock, &named(&["app"]), "0.1.0"), LOCK_TEXT);
}

#[test]
fn the_placeholder_is_the_same_for_every_version() {
    let members = named(&["app"]);
    let raised = manifest_with(MANIFEST_TEXT, "2.0.0").unwrap();
    assert_eq!(
        neutral(MANIFEST, MANIFEST_TEXT, &members).unwrap(),
        neutral(MANIFEST, &raised, &members).unwrap()
    );
    let lock = lock_with(LOCK_TEXT, &members, "2.0.0");
    assert_eq!(
        neutral(LOCK, LOCK_TEXT, &members).unwrap(),
        neutral(LOCK, &lock, &members).unwrap()
    );
    assert_eq!(neutral("a.txt", "a", &members).unwrap(), None);
}

/// The workspace itself: every package takes the one version, and `Cargo.lock` holds it for each of them.
#[test]
fn every_package_of_the_workspace_carries_the_products_version() {
    let current = version().unwrap();
    let mut manifests = vec![root().join(MANIFEST), root().join("xtask").join(MANIFEST)];
    for entry in std::fs::read_dir(root().join("crates")).unwrap() {
        let manifest = entry.unwrap().path().join(MANIFEST);
        if manifest.exists() {
            manifests.push(manifest);
        }
    }
    let texts: Vec<String> = manifests
        .iter()
        .map(|path| files::read_to_string(path).unwrap())
        .collect();
    for (path, text) in manifests.iter().zip(&texts) {
        assert!(
            member(text).is_some(),
            "{} lacks version.workspace = true",
            path.display()
        );
    }
    let members = members(texts.iter().map(String::as_str));
    let lock = files::read_to_string(root().join(LOCK)).unwrap();
    assert_eq!(
        lock_with(&lock, &members, &current),
        lock,
        "Cargo.lock lists a package of the workspace at another version than {current}"
    );
}
