//! git's configuration files read without git, every entry with its exact names. The reader follows splat-git's
//! `config.rs` (Splat commit e552246), which is private there and only checks a repository's format.

/// One `key = value` of a configuration file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    /// The section's name, lowercased as git compares it.
    pub(crate) section: Vec<u8>,
    /// The subsection as git keys it: exact when quoted, lowercased in the old `[section.sub]` form.
    pub(crate) subsection: Option<Vec<u8>>,
    /// The key's name, lowercased.
    pub(crate) key: Vec<u8>,
    /// `None` for a key without `=`, which git reads as true.
    pub(crate) value: Option<Vec<u8>>,
}

impl Entry {
    /// The name git's `-c` and `GIT_CONFIG_KEY_<n>` take for this entry.
    pub(crate) fn name(&self) -> Vec<u8> {
        let mut name = self.section.clone();
        if let Some(subsection) = &self.subsection {
            name.push(b'.');
            name.extend_from_slice(subsection);
        }
        name.push(b'.');
        name.extend_from_slice(&self.key);
        name
    }
}

/// The file is no configuration git would read the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Damaged;

/// Every entry of `config` in order. Includes are entries like any other; the caller follows them.
pub(crate) fn entries(config: &[u8]) -> Result<Vec<Entry>, Damaged> {
    if config.contains(&0) {
        return Err(Damaged);
    }
    let mut reader = Reader {
        rest: config.strip_prefix(b"\xef\xbb\xbf").unwrap_or(config),
    };
    let mut section: Option<(Vec<u8>, Option<Vec<u8>>)> = None;
    let mut entries = Vec::new();
    loop {
        reader.skip_while(|byte| byte.is_ascii_whitespace());
        match reader.rest.first() {
            None => return Ok(entries),
            Some(b'#' | b';') => reader.skip_while(|byte| byte != b'\n'),
            Some(b'[') => section = Some(reader.section()?),
            Some(byte) if byte.is_ascii_alphabetic() => {
                let key = reader.key();
                let value = reader.value()?;
                let Some((name, subsection)) = &section else {
                    return Err(Damaged);
                };
                entries.push(Entry {
                    section: name.clone(),
                    subsection: subsection.clone(),
                    key,
                    value,
                });
            }
            Some(_) => return Err(Damaged),
        }
    }
}

struct Reader<'a> {
    rest: &'a [u8],
}

impl Reader<'_> {
    fn skip_while(&mut self, keep_going: impl Fn(u8) -> bool) {
        let end = self
            .rest
            .iter()
            .position(|&byte| !keep_going(byte))
            .unwrap_or(self.rest.len());
        self.rest = &self.rest[end..];
    }

    fn next(&mut self) -> Option<u8> {
        let (&first, rest) = self.rest.split_first()?;
        self.rest = rest;
        Some(first)
    }

    /// `[name]`, `[name "subsection"]` or the old `[name.subsection]`.
    fn section(&mut self) -> Result<(Vec<u8>, Option<Vec<u8>>), Damaged> {
        self.next();
        let mut name: Vec<u8> = self
            .rest
            .iter()
            .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'))
            .map(u8::to_ascii_lowercase)
            .collect();
        self.rest = &self.rest[name.len()..];
        let subsection = match name.iter().position(|&byte| byte == b'.') {
            Some(dot) => {
                let legacy = name[dot + 1..].to_vec();
                name.truncate(dot);
                Some(legacy)
            }
            None => self.subsection()?,
        };
        if self.next() != Some(b']') || name.is_empty() {
            return Err(Damaged);
        }
        Ok((name, subsection))
    }

    /// A quoted subsection, in which a backslash keeps the character after it, as git reads it.
    fn subsection(&mut self) -> Result<Option<Vec<u8>>, Damaged> {
        self.skip_while(|byte| matches!(byte, b' ' | b'\t'));
        if self.rest.first() != Some(&b'"') {
            return Ok(None);
        }
        self.next();
        let mut subsection = Vec::new();
        loop {
            match self.next() {
                Some(b'"') => return Ok(Some(subsection)),
                Some(b'\\') => match self.next() {
                    Some(b'\n') | None => return Err(Damaged),
                    Some(byte) => subsection.push(byte),
                },
                Some(b'\n') | None => return Err(Damaged),
                Some(byte) => subsection.push(byte),
            }
        }
    }

    fn key(&mut self) -> Vec<u8> {
        let key: Vec<u8> = self
            .rest
            .iter()
            .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'-')
            .map(u8::to_ascii_lowercase)
            .collect();
        self.rest = &self.rest[key.len()..];
        key
    }

    /// What stands behind a key: nothing at all, or `=` and a value that may hold quotes, escapes, continued lines and a
    /// comment, with white space around it dropped.
    fn value(&mut self) -> Result<Option<Vec<u8>>, Damaged> {
        self.skip_while(|byte| matches!(byte, b' ' | b'\t' | b'\r'));
        match self.rest.first() {
            Some(b'=') => {
                self.next();
            }
            Some(b'\n' | b'#' | b';') | None => return Ok(None),
            Some(_) => return Err(Damaged),
        }
        self.skip_while(|byte| matches!(byte, b' ' | b'\t'));
        let mut value = Vec::new();
        let mut kept = 0;
        let mut quoted = false;
        loop {
            match self.next() {
                None | Some(b'\n') if quoted => return Err(Damaged),
                None | Some(b'\n') => break,
                Some(b'"') => {
                    quoted = !quoted;
                    kept = value.len();
                }
                Some(b'\\') => {
                    match self.next() {
                        Some(b'\n') => {}
                        Some(b'n') => value.push(b'\n'),
                        Some(b't') => value.push(b'\t'),
                        Some(b'b') => value.push(8),
                        Some(byte @ (b'"' | b'\\')) => value.push(byte),
                        _ => return Err(Damaged),
                    }
                    kept = value.len();
                }
                Some(b'#' | b';') if !quoted => {
                    self.skip_while(|byte| byte != b'\n');
                    break;
                }
                Some(byte) => {
                    value.push(byte);
                    if quoted || !byte.is_ascii_whitespace() {
                        kept = value.len();
                    }
                }
            }
        }
        value.truncate(kept);
        Ok(Some(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(config: &str) -> Vec<(String, Option<String>)> {
        entries(config.as_bytes())
            .unwrap()
            .iter()
            .map(|entry| {
                (
                    String::from_utf8_lossy(&entry.name()).into_owned(),
                    entry.value.as_ref().map(|v| String::from_utf8_lossy(v).into_owned()),
                )
            })
            .collect()
    }

    #[test]
    fn sections_and_keys_are_lowercased_and_quoted_subsections_kept_exactly() {
        assert_eq!(
            names("[Filter \"My.Drv\"]\n\tClean = touch x\n[core]\nsshCommand=evil\n"),
            [
                ("filter.My.Drv.clean".into(), Some("touch x".into())),
                ("core.sshcommand".into(), Some("evil".into())),
            ]
        );
    }

    #[test]
    fn the_old_form_lowercases_its_subsection() {
        assert_eq!(
            names("[Diff.Drv]\ntextconv = a"),
            [("diff.drv.textconv".into(), Some("a".into()))]
        );
    }

    #[test]
    fn a_backslash_in_a_subsection_keeps_what_follows() {
        assert_eq!(
            names("[filter \"a\\\"b\\\\c\\d\"]\nclean = x"),
            [("filter.a\"b\\cd.clean".into(), Some("x".into()))]
        );
    }

    #[test]
    fn values_are_unquoted_unescaped_and_a_bare_key_has_none() {
        assert_eq!(
            names("[core]\npager = \"less  -R\" # c\nbare\neditor = a\\\nb\n"),
            [
                ("core.pager".into(), Some("less  -R".into())),
                ("core.bare".into(), None),
                ("core.editor".into(), Some("ab".into())),
            ]
        );
    }

    #[test]
    fn several_entries_may_share_a_line_with_their_section() {
        assert_eq!(
            names("[core] fsmonitor = x"),
            [("core.fsmonitor".into(), Some("x".into()))]
        );
    }

    #[test]
    fn what_git_would_not_read_is_damaged() {
        for config in [
            "key = value",
            "[core",
            "[]",
            "[core \"open]",
            "[other]\nkey = \"open\n",
            "[other]\nkey ! value",
            "[core]\nfsmonitor = a\0b",
        ] {
            assert_eq!(entries(config.as_bytes()), Err(Damaged), "{config:?}");
        }
    }
}
