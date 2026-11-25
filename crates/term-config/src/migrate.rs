//! Older layouts of the file brought up to `VERSION`, one step per version, so a file is never read wrongly.

use crate::Warning;
use crate::schema::VERSION;

/// The steps, the first turning version 1 into version 2; empty while there is only version 1.
const STEPS: &[fn(&mut toml::Table)] = &[];

/// `table` in the current layout; a file from a newer build is read as far as it goes, with a warning.
pub(crate) fn to_current(mut table: toml::Table, warnings: &mut Vec<Warning>) -> toml::Table {
    let version = match table.get("version") {
        None => 1,
        Some(toml::Value::Integer(n)) => u32::try_from(*n).unwrap_or(1).max(1),
        Some(_) => {
            warnings.push(Warning("version: not a number".into()));
            1
        }
    };
    if version > VERSION {
        warnings.push(Warning(format!(
            "the file is of version {version}, written by a newer build; this one knows {VERSION}"
        )));
        return table;
    }
    let first = usize::try_from(version - 1).unwrap_or(0);
    for step in STEPS.iter().skip(first) {
        step(&mut table);
    }
    table.insert("version".into(), toml::Value::Integer(i64::from(VERSION)));
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_lead_to_the_current_version() {
        assert_eq!(STEPS.len() + 1, usize::try_from(VERSION).unwrap());
    }

    #[test]
    fn a_file_without_version_is_version_one_and_a_newer_one_warns() {
        let mut warnings = Vec::new();
        let table = to_current(toml::Table::new(), &mut warnings);
        assert_eq!(table["version"].as_integer(), Some(i64::from(VERSION)));
        assert!(warnings.is_empty());
        let mut newer = toml::Table::new();
        newer.insert("version".into(), toml::Value::Integer(99));
        to_current(newer, &mut warnings);
        assert_eq!(warnings.len(), 1);
    }
}
