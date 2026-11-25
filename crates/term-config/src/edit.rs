//! Changing the settings file the way the settings window does (F159): one field at a time in the user's own text,
//! so comments, order, formatting and keys this build does not know stay as they were.

mod keys;
mod profiles;

pub use keys::{chords_text, parse_chords};
pub use profiles::{EditError, Origin, Part, unique_profile_name};

use toml_edit::{DocumentMut, InlineTable, Item, Table, TableLike, Value};

/// The settings file as text that can be changed field by field.
#[derive(Debug, Clone)]
pub struct Document {
    doc: DocumentMut,
}

impl Document {
    /// The file's text; an empty text is a new file of the current version.
    pub fn parse(text: &str) -> Result<Document, String> {
        let source = if text.trim().is_empty() {
            format!("version = {}\n", crate::VERSION)
        } else {
            text.to_string()
        };
        source
            .parse::<DocumentMut>()
            .map(|doc| Document { doc })
            .map_err(|error| error.to_string())
    }

    /// The text to write back.
    pub fn text(&self) -> String {
        self.doc.to_string()
    }

    /// Sets the setting at `path`, such as `["font", "size"]`, keeping the comment beside the value it replaces.
    pub fn set(&mut self, path: &[&str], value: impl Into<Value>) {
        set_in(self.doc.as_table_mut(), false, path, value.into());
    }

    /// Takes the setting at `path` out of the file, so its default applies again.
    pub fn remove(&mut self, path: &[&str]) {
        remove_in(self.doc.as_table_mut(), path);
    }

    /// The value at `path`, as the file has it.
    pub fn get(&self, path: &[&str]) -> Option<&Value> {
        get_in(self.doc.as_item(), path)?.as_value()
    }
}

/// `value` as the file writes a number: without a fraction where it has none, as people write sizes.
pub fn number(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        #[expect(clippy::cast_possible_truncation, reason = "a whole number well inside i64")]
        let whole = value as i64;
        Value::from(whole)
    } else {
        Value::from(value)
    }
}

/// A list of strings as the file writes one.
pub fn strings<'a>(items: impl IntoIterator<Item = &'a str>) -> Value {
    Value::Array(items.into_iter().collect())
}

fn get_in<'a>(item: &'a Item, path: &[&str]) -> Option<&'a Item> {
    match path.split_first() {
        None => Some(item),
        Some((first, rest)) => get_in(item.as_table_like()?.get(first)?, rest),
    }
}

/// Sets `value` at `path` below `table`, making the tables on the way; inside an inline table they are inline too.
fn set_in(table: &mut dyn TableLike, inline: bool, path: &[&str], mut value: Value) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    if rest.is_empty() {
        if let Some(Item::Value(old)) = table.get(first) {
            *value.decor_mut() = old.decor().clone();
        }
        table.insert(first, Item::Value(value));
        return;
    }
    if table.get(first).and_then(Item::as_table_like).is_none() {
        let fresh = if inline {
            Item::Value(Value::InlineTable(InlineTable::new()))
        } else {
            Item::Table(Table::new())
        };
        table.insert(first, fresh);
    }
    if let Some(child) = table.get_mut(first) {
        let inline = child.is_inline_table();
        if let Some(child) = child.as_table_like_mut() {
            set_in(child, inline, rest, value);
        }
    }
}

/// Removes `path` below `table`, and a table it leaves empty.
fn remove_in(table: &mut dyn TableLike, path: &[&str]) {
    match path {
        [] => {}
        [last] => {
            table.remove(last);
        }
        [first, rest @ ..] => {
            let Some(child) = table.get_mut(first).and_then(Item::as_table_like_mut) else {
                return;
            };
            remove_in(child, rest);
            if child.is_empty() {
                table.remove(first);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    const FILE: &str = "# my settings\nversion = 1\nfancy = \"kept\"\n\n[font]\n# the face I like\nfamily = \"Iosevka\"\nsize = 13 # small\n\n[later]\nunknown = true\n";

    #[test]
    fn a_change_keeps_comments_unknown_keys_and_order() {
        let mut doc = Document::parse(FILE).unwrap();
        doc.set(&["font", "size"], number(15.0));
        let text = doc.text();
        assert_eq!(text, FILE.replace("size = 13 # small", "size = 15 # small"));
        let (config, _) = Config::parse(&text);
        assert_eq!(config.font.size, 15.0);
        assert_eq!(config.font.family, "Iosevka");
    }

    #[test]
    fn a_missing_section_is_added_and_an_emptied_one_goes() {
        let mut doc = Document::parse(FILE).unwrap();
        doc.set(&["cursor", "style"], "bar");
        doc.set(&["window", "inherit_directory", "tab"], false);
        let (config, warnings) = Config::parse(&doc.text());
        assert_eq!(config.cursor.style, crate::CursorStyle::Bar);
        assert!(!config.window.inherit_directory.tab);
        assert_eq!(warnings.len(), 2, "the two unknown keys: {warnings:?}");
        doc.remove(&["cursor", "style"]);
        assert!(!doc.text().contains("[cursor]"), "{}", doc.text());
        assert!(doc.text().contains("# the face I like"));
    }

    #[test]
    fn inside_an_inline_table_new_tables_are_inline() {
        let mut doc = Document::parse("window = { sidebar = false }\n").unwrap();
        doc.set(&["window", "inherit_directory", "split"], false);
        assert_eq!(
            doc.get(&["window", "inherit_directory", "split"])
                .and_then(Value::as_bool),
            Some(false)
        );
        let (config, warnings) = Config::parse(&doc.text());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(!config.window.sidebar);
        assert!(!config.window.inherit_directory.split);
    }

    #[test]
    fn an_empty_file_becomes_one_of_the_current_version() {
        let mut doc = Document::parse("").unwrap();
        doc.set(&["font", "features"], strings(["ss01", "-calt"]));
        let (config, warnings) = Config::parse(&doc.text());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config.font.features, ["ss01", "-calt"]);
        assert!(doc.text().starts_with("version = 1"));
    }

    #[test]
    fn whole_numbers_are_written_without_a_fraction() {
        assert_eq!(number(14.0).to_string(), "14");
        assert_eq!(number(1.25).to_string(), "1.25");
    }

    #[test]
    fn a_broken_file_is_not_taken() {
        assert!(Document::parse("font = [").is_err());
    }
}
