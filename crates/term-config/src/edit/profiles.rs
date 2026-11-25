//! Profiles in the file (F161): adding, renaming, deleting, choosing the default, changing a profile's fields, and
//! where a profile's value comes from, for the settings window to mark what is inherited.

use std::fmt;

use toml_edit::{ArrayOfTables, Item, Table, Value};

use super::{Document, remove_in, set_in};
use crate::schema::Config;

/// Why a change to the profiles was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// A profile needs a name.
    Empty,
    /// Another profile has this name already.
    Taken(String),
    /// No profile has this name.
    Missing(String),
    /// The file writes its profiles in a way that cannot be changed field by field.
    Layout,
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::Empty => f.write_str("a profile needs a name"),
            EditError::Taken(name) => write!(f, "a profile `{name}` exists already"),
            EditError::Missing(name) => write!(f, "no profile `{name}`"),
            EditError::Layout => f.write_str("the profiles are not written as [[profile]] tables"),
        }
    }
}

/// The parts of a profile that it sets as a whole or takes from where it inherits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// `font`.
    Font,
    /// `theme`.
    Theme,
    /// `command`.
    Command,
    /// `directory`.
    Directory,
}

/// Where a profile's part comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// The profile sets it itself.
    Own,
    /// It inherits it from the profile of this name.
    Profile(String),
    /// Nothing on the way sets it: the settings' own, or for command and folder the login shell and the usual folder.
    Settings,
}

impl Config {
    /// Where the profile `name` takes `part` from, following `inherits` as `Config::profile` does.
    pub fn origin(&self, name: &str, part: Part) -> Origin {
        let mut seen: Vec<&str> = Vec::new();
        let mut next = Some(name);
        while let Some(current) = next {
            if seen.contains(&current) {
                break;
            }
            seen.push(current);
            let Some(profile) = self.profiles.iter().find(|p| p.name == current) else {
                break;
            };
            let sets = match part {
                Part::Font => profile.font.is_some(),
                Part::Theme => profile.theme.is_some(),
                Part::Command => profile.command.is_some(),
                Part::Directory => profile.directory.is_some(),
            };
            if sets {
                return if current == name {
                    Origin::Own
                } else {
                    Origin::Profile(current.to_string())
                };
            }
            next = profile.inherits.as_deref();
        }
        Origin::Settings
    }
}

/// `base`, or `base 2`, `base 3` and on, whichever no profile has yet.
pub fn unique_profile_name(config: &Config, base: &str) -> String {
    let taken = |name: &str| config.profiles.iter().any(|p| p.name == name);
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|name| !taken(name))
        .unwrap_or_else(|| base.to_string())
}

fn name_of(table: &Table) -> Option<&str> {
    table.get("name")?.as_str()
}

impl Document {
    fn profiles(&self) -> Option<&ArrayOfTables> {
        self.doc.get("profile")?.as_array_of_tables()
    }

    fn profiles_mut(&mut self) -> Result<&mut ArrayOfTables, EditError> {
        if self.doc.get("profile").is_none() {
            self.doc.insert("profile", Item::ArrayOfTables(ArrayOfTables::new()));
        }
        self.doc
            .get_mut("profile")
            .and_then(Item::as_array_of_tables_mut)
            .ok_or(EditError::Layout)
    }

    fn profile_mut(&mut self, name: &str) -> Result<&mut Table, EditError> {
        self.profiles_mut()?
            .iter_mut()
            .find(|table| name_of(table) == Some(name))
            .ok_or_else(|| EditError::Missing(name.to_string()))
    }

    fn has_profile(&self, name: &str) -> bool {
        self.profiles()
            .is_some_and(|profiles| profiles.iter().any(|table| name_of(table) == Some(name)))
    }

    /// Adds an empty profile `name`, which inherits everything from the settings.
    pub fn add_profile(&mut self, name: &str) -> Result<(), EditError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(EditError::Empty);
        }
        if self.has_profile(name) {
            return Err(EditError::Taken(name.to_string()));
        }
        let mut table = Table::new();
        table.insert("name", toml_edit::value(name));
        self.profiles_mut()?.push(table);
        Ok(())
    }

    /// Renames the profile `old`, and with it the profiles that inherit from it and the default profile.
    pub fn rename_profile(&mut self, old: &str, new: &str) -> Result<(), EditError> {
        let new = new.trim();
        if new.is_empty() {
            return Err(EditError::Empty);
        }
        if new == old {
            return Ok(());
        }
        if self.has_profile(new) {
            return Err(EditError::Taken(new.to_string()));
        }
        set_in(self.profile_mut(old)?, false, &["name"], Value::from(new));
        self.repoint(old, Some(new))?;
        if self.default_profile() == Some(old) {
            self.set(&["default_profile"], new);
        }
        Ok(())
    }

    /// Deletes the profile `name`; the profiles that inherited from it inherit from its parent instead, and the
    /// default falls back to the settings when it was this one.
    pub fn delete_profile(&mut self, name: &str) -> Result<(), EditError> {
        let profiles = self.profiles_mut()?;
        let index = profiles
            .iter()
            .position(|table| name_of(table) == Some(name))
            .ok_or_else(|| EditError::Missing(name.to_string()))?;
        let removed = profiles.remove(index);
        if profiles.is_empty() {
            self.doc.remove("profile");
        }
        let parent = removed.get("inherits").and_then(Item::as_str).map(str::to_string);
        self.repoint(name, parent.as_deref())?;
        if self.default_profile() == Some(name) {
            self.remove(&["default_profile"]);
        }
        Ok(())
    }

    /// Makes `name` the profile new sessions start with.
    pub fn set_default_profile(&mut self, name: &str) {
        self.set(&["default_profile"], name);
    }

    /// Sets the field at `path` of the profile `name`, such as `["font", "size"]`.
    pub fn set_in_profile(&mut self, name: &str, path: &[&str], value: impl Into<Value>) -> Result<(), EditError> {
        set_in(self.profile_mut(name)?, false, path, value.into());
        Ok(())
    }

    /// Takes the field at `path` out of the profile `name`, so it inherits it again.
    pub fn remove_from_profile(&mut self, name: &str, path: &[&str]) -> Result<(), EditError> {
        remove_in(self.profile_mut(name)?, path);
        Ok(())
    }

    fn default_profile(&self) -> Option<&str> {
        self.doc.get("default_profile")?.as_str()
    }

    /// Has every profile that inherits from `old` inherit from `new` instead, or from nothing.
    fn repoint(&mut self, old: &str, new: Option<&str>) -> Result<(), EditError> {
        if self.doc.get("profile").is_none() {
            return Ok(());
        }
        for table in self.profiles_mut()?.iter_mut() {
            if table.get("inherits").and_then(Item::as_str) != Some(old) {
                continue;
            }
            match new {
                Some(new) => set_in(table, false, &["inherits"], Value::from(new)),
                None => {
                    table.remove("inherits");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "version = 1\ndefault_profile = \"Work\"\n\n# work stuff\n[[profile]]\nname = \"Work\"\ndirectory = \"/work\" # mine\n[profile.font]\nsize = 15\n\n[[profile]]\nname = \"Prod\"\ninherits = \"Work\"\n";

    fn read(doc: &Document) -> Config {
        let (config, warnings) = Config::parse(&doc.text());
        assert!(warnings.is_empty(), "{warnings:?}\n{}", doc.text());
        config
    }

    #[test]
    fn a_rename_follows_into_inherits_and_the_default() {
        let mut doc = Document::parse(FILE).unwrap();
        doc.rename_profile("Work", "Office").unwrap();
        let config = read(&doc);
        assert_eq!(config.default_profile, "Office");
        assert_eq!(config.profiles[1].inherits.as_deref(), Some("Office"));
        assert!(doc.text().contains("# work stuff") && doc.text().contains("# mine"));
        assert_eq!(
            doc.rename_profile("Office", "Prod"),
            Err(EditError::Taken("Prod".into()))
        );
        assert_eq!(doc.rename_profile("Office", " "), Err(EditError::Empty));
    }

    #[test]
    fn a_deleted_profiles_children_inherit_from_its_parent() {
        let mut doc = Document::parse(FILE).unwrap();
        doc.add_profile("Base").unwrap();
        doc.set_in_profile("Work", &["inherits"], "Base").unwrap();
        doc.delete_profile("Work").unwrap();
        let config = read(&doc);
        assert_eq!(config.default_profile, "Default");
        let prod = config.profiles.iter().find(|p| p.name == "Prod").unwrap();
        assert_eq!(prod.inherits.as_deref(), Some("Base"));
        doc.delete_profile("Prod").unwrap();
        doc.delete_profile("Base").unwrap();
        assert!(!doc.text().contains("profile"), "{}", doc.text());
        assert_eq!(doc.delete_profile("Base"), Err(EditError::Missing("Base".into())));
    }

    #[test]
    fn profiles_are_added_set_and_chosen() {
        let mut doc = Document::parse("").unwrap();
        doc.add_profile("Solo").unwrap();
        assert_eq!(doc.add_profile("Solo"), Err(EditError::Taken("Solo".into())));
        doc.set_in_profile("Solo", &["theme", "dark"], "Dracula").unwrap();
        doc.set_in_profile("Solo", &["env", "STAGE"], "dev").unwrap();
        doc.set_default_profile("Solo");
        let config = read(&doc);
        assert_eq!(config.default_profile, "Solo");
        let solo = config.profile("Solo").unwrap();
        assert_eq!(solo.theme.unwrap().dark, "Dracula");
        assert_eq!(solo.env["STAGE"], "dev");
        doc.remove_from_profile("Solo", &["theme"]).unwrap();
        assert_eq!(read(&doc).profiles[0].theme, None);
        assert_eq!(
            doc.set_in_profile("None", &["command"], "x"),
            Err(EditError::Missing("None".into()))
        );
    }

    #[test]
    fn origins_follow_the_inheritance() {
        let config = read(&Document::parse(FILE).unwrap());
        assert_eq!(config.origin("Work", Part::Font), Origin::Own);
        assert_eq!(config.origin("Prod", Part::Font), Origin::Profile("Work".into()));
        assert_eq!(config.origin("Prod", Part::Directory), Origin::Profile("Work".into()));
        assert_eq!(config.origin("Prod", Part::Theme), Origin::Settings);
        assert_eq!(config.origin("Work", Part::Command), Origin::Settings);
    }

    #[test]
    fn new_names_do_not_collide() {
        let config = read(&Document::parse(FILE).unwrap());
        assert_eq!(unique_profile_name(&config, "New"), "New");
        assert_eq!(unique_profile_name(&config, "Work"), "Work 2");
    }

    #[test]
    fn inline_profiles_are_refused_not_broken() {
        let mut doc = Document::parse("profile = [{ name = \"A\" }]\n").unwrap();
        assert_eq!(doc.add_profile("B"), Err(EditError::Layout));
        assert_eq!(doc.text(), "profile = [{ name = \"A\" }]\n");
    }
}
