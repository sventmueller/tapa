//! The allow rules Claude keeps for a project (E27): read from its settings files for the settings window, and
//! removed one entry at a time with every other byte of the file left as it was.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Which of a project's settings files holds a rule, as Claude's `destination` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    /// `.claude/settings.local.json`, this Mac's own and not shared through git: `localSettings`.
    Local,
    /// `.claude/settings.json`, shared with everyone who checks the project out: `projectSettings`.
    Project,
}

impl From<Scope> for term_thread::RuleScope {
    fn from(scope: Scope) -> term_thread::RuleScope {
        match scope {
            Scope::Local => term_thread::RuleScope::ProjectLocal,
            Scope::Project => term_thread::RuleScope::Project,
        }
    }
}

impl Scope {
    /// The file in `project` that holds rules of this scope.
    pub fn file(self, project: &Path) -> PathBuf {
        project.join(".claude").join(match self {
            Scope::Local => "settings.local.json",
            Scope::Project => "settings.json",
        })
    }
}

/// A rule that lets a tool run without asking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRule {
    /// The file it is in.
    pub scope: Scope,
    /// The rule as Claude writes it, such as `Bash(cargo test:*)`.
    pub rule: String,
}

/// The allow rules of `project`, those only of this Mac first; files that are missing or not valid JSON hold none.
pub fn allow_rules(project: &Path) -> Vec<StoredRule> {
    [Scope::Local, Scope::Project]
        .into_iter()
        .flat_map(|scope| {
            let text = std::fs::read_to_string(scope.file(project)).unwrap_or_default();
            let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let rules: Vec<String> = value
                .pointer("/permissions/allow")
                .and_then(Value::as_array)
                .map(|rules| rules.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default();
            rules.into_iter().map(move |rule| StoredRule { scope, rule })
        })
        .collect()
}

/// Removes the first entry `rule` from the allow rules in `project`'s file of `scope`; false when it is not there.
pub fn remove_allow_rule(project: &Path, scope: Scope, rule: &str) -> io::Result<bool> {
    let path = scope.file(project);
    let text = std::fs::read_to_string(&path)?;
    let Some(changed) = without_rule(&text, rule)? else {
        return Ok(false);
    };
    // Replaced whole where the name leads, so a crash leaves the old rules and a linked file stays linked.
    let target = std::fs::canonicalize(&path)?;
    let permissions = std::fs::metadata(&target)?.permissions();
    term_base::atomic_write::write(&target, changed.as_bytes())?;
    let _ = std::fs::set_permissions(&target, permissions);
    Ok(true)
}

fn invalid(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.to_string())
}

/// `text` without the first allow entry `rule`; `None` when it has none.
fn without_rule(text: &str, rule: &str) -> io::Result<Option<String>> {
    let mut before: Value = serde_json::from_str(text).map_err(|e| invalid(&e.to_string()))?;
    let Some(allow) = allow_span(text)? else {
        return Ok(None);
    };
    let Some(n) = allow.items.iter().position(|item| {
        serde_json::from_str::<Value>(&text[item.0..item.1])
            .ok()
            .as_ref()
            .and_then(Value::as_str)
            == Some(rule)
    }) else {
        return Ok(None);
    };
    let (start, end) = match (n.checked_sub(1).map(|p| allow.items[p]), allow.items.get(n + 1)) {
        (_, Some(next)) => (allow.items[n].0, next.0),
        (Some(previous), None) => (previous.1, allow.items[n].1),
        (None, None) => (allow.open + 1, allow.close),
    };
    let changed = format!("{}{}", &text[..start], &text[end..]);
    if let Some(Value::Array(rules)) = before.pointer_mut("/permissions/allow") {
        rules.remove(n);
    }
    let after: Value = serde_json::from_str(&changed).map_err(|e| invalid(&e.to_string()))?;
    if after != before {
        return Err(invalid("the file changed in more than the rule"));
    }
    Ok(Some(changed))
}

/// Where the allow array lies: its brackets and each entry's span.
struct Span {
    open: usize,
    close: usize,
    items: Vec<(usize, usize)>,
}

/// The span of `permissions.allow` in `text`, whose last member of a name counts, as for any JSON reader.
fn allow_span(text: &str) -> io::Result<Option<Span>> {
    let mut reader = Reader {
        bytes: text.as_bytes(),
        at: 0,
    };
    let Some(permissions) = reader.member(0, "permissions")? else {
        return Ok(None);
    };
    let Some(allow) = reader.member(permissions, "allow")? else {
        return Ok(None);
    };
    reader.at = allow;
    if reader.peek() != Some(b'[') {
        return Ok(None);
    }
    let open = reader.at;
    reader.at += 1;
    let mut items = Vec::new();
    loop {
        reader.space();
        if reader.peek() == Some(b']') {
            break;
        }
        let start = reader.at;
        reader.value()?;
        items.push((start, reader.at));
        reader.space();
        match reader.next()? {
            b',' => {}
            b']' => {
                reader.at -= 1;
                break;
            }
            _ => return Err(invalid("expected , or ]")),
        }
    }
    Ok(Some(Span {
        open,
        close: reader.at,
        items,
    }))
}

/// Walks JSON text that `serde_json` already accepted, by byte offsets.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn next(&mut self) -> io::Result<u8> {
        let byte = self.peek().ok_or_else(|| invalid("unexpected end"))?;
        self.at += 1;
        Ok(byte)
    }

    fn space(&mut self) {
        while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
            self.at += 1;
        }
    }

    /// Where the value of the last member `name` of the object at `from` starts, if it is an object with one.
    fn member(&mut self, from: usize, name: &str) -> io::Result<Option<usize>> {
        self.at = from;
        self.space();
        if self.peek() != Some(b'{') {
            return Ok(None);
        }
        self.at += 1;
        let mut found = None;
        loop {
            self.space();
            if self.peek() == Some(b'}') {
                return Ok(found);
            }
            let key_start = self.at;
            self.string()?;
            let key: String =
                serde_json::from_slice(&self.bytes[key_start..self.at]).map_err(|e| invalid(&e.to_string()))?;
            self.space();
            if self.next()? != b':' {
                return Err(invalid("expected :"));
            }
            self.space();
            if key == name {
                found = Some(self.at);
            }
            self.value()?;
            self.space();
            match self.next()? {
                b',' => {}
                b'}' => return Ok(found),
                _ => return Err(invalid("expected , or }")),
            }
        }
    }

    fn string(&mut self) -> io::Result<()> {
        if self.next()? != b'"' {
            return Err(invalid("expected a string"));
        }
        loop {
            match self.next()? {
                b'\\' => {
                    self.next()?;
                }
                b'"' => return Ok(()),
                _ => {}
            }
        }
    }

    fn value(&mut self) -> io::Result<()> {
        match self.peek().ok_or_else(|| invalid("unexpected end"))? {
            b'"' => self.string(),
            b'{' | b'[' => {
                let mut depth = 0usize;
                loop {
                    match self.peek().ok_or_else(|| invalid("unexpected end"))? {
                        b'"' => self.string()?,
                        b'{' | b'[' => {
                            depth += 1;
                            self.at += 1;
                        }
                        b'}' | b']' => {
                            depth -= 1;
                            self.at += 1;
                            if depth == 0 {
                                return Ok(());
                            }
                        }
                        _ => self.at += 1,
                    }
                }
            }
            _ => {
                while self
                    .peek()
                    .is_some_and(|b| !matches!(b, b',' | b'}' | b']') && !b.is_ascii_whitespace())
                {
                    self.at += 1;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"{
  "permissions": {
    "allow": [
      "Bash(cargo test:*)",
      "Bash(git log:*)",
      "WebFetch(domain:docs.rs)"
    ],
    "deny": ["Bash(git log:*)"],
    "additionalDirectories": ["/tmp"]
  },
  "enableAllProjectMcpServers": false,
  "note": "keep [this] {as} \"it\" is"
}
"#;

    fn removed(text: &str, rule: &str) -> Option<String> {
        without_rule(text, rule).unwrap()
    }

    #[test]
    fn removing_a_rule_takes_out_that_entry_and_nothing_else() {
        let middle = removed(FILE, "Bash(git log:*)").unwrap();
        assert_eq!(middle, FILE.replace("      \"Bash(git log:*)\",\n", ""));
        let last = removed(FILE, "WebFetch(domain:docs.rs)").unwrap();
        assert_eq!(last, FILE.replace(",\n      \"WebFetch(domain:docs.rs)\"", ""));
        let first = removed(FILE, "Bash(cargo test:*)").unwrap();
        assert_eq!(first, FILE.replace("      \"Bash(cargo test:*)\",\n", ""));
        assert!(first.contains(r#""deny": ["Bash(git log:*)"]"#));
    }

    #[test]
    fn the_only_rule_leaves_an_empty_list() {
        let text = "{\"permissions\":{\"allow\":[ \"Read\" ],\"ask\":[]}}";
        assert_eq!(
            removed(text, "Read").as_deref(),
            Some("{\"permissions\":{\"allow\":[],\"ask\":[]}}")
        );
    }

    #[test]
    fn a_rule_that_is_not_there_or_a_file_without_rules_changes_nothing() {
        assert_eq!(removed(FILE, "Bash(rm:*)"), None);
        assert_eq!(removed(r#"{"model": "haiku"}"#, "Read"), None);
        assert_eq!(removed(r#"{"permissions": {"deny": ["Read"]}}"#, "Read"), None);
        assert_eq!(removed(r#"{"permissions": {"allow": "Read"}}"#, "Read"), None);
        assert!(without_rule("{ not json", "Read").is_err());
    }

    #[test]
    fn escaped_rules_and_duplicates_are_matched_by_their_value() {
        let text = r#"{"permissions": {"allow": ["Bash(echo \"a\":*)", "Read", "Read"]}}"#;
        assert_eq!(
            removed(text, "Bash(echo \"a\":*)").as_deref(),
            Some(r#"{"permissions": {"allow": ["Read", "Read"]}}"#)
        );
        assert_eq!(
            removed(text, "Read").as_deref(),
            Some(r#"{"permissions": {"allow": ["Bash(echo \"a\":*)", "Read"]}}"#)
        );
    }

    #[test]
    fn the_last_permissions_member_is_the_one_that_counts() {
        let text = r#"{"permissions": {"allow": ["Read"]}, "permissions": {"allow": ["Edit", "Read"]}}"#;
        assert_eq!(
            removed(text, "Read").as_deref(),
            Some(r#"{"permissions": {"allow": ["Read"]}, "permissions": {"allow": ["Edit"]}}"#)
        );
    }

    #[test]
    fn rules_are_read_from_both_files_and_removed_on_disk() {
        let project = term_base::testing::scratch("claude-rules");
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        std::fs::write(Scope::Local.file(&project), FILE).unwrap();
        std::fs::write(
            Scope::Project.file(&project),
            r#"{"permissions": {"allow": ["Bash(make:*)"]}}"#,
        )
        .unwrap();
        let rules = allow_rules(&project);
        assert_eq!(rules.len(), 4);
        assert_eq!(
            rules[3],
            StoredRule {
                scope: Scope::Project,
                rule: "Bash(make:*)".into()
            }
        );
        assert!(remove_allow_rule(&project, Scope::Local, "Bash(git log:*)").unwrap());
        assert!(!remove_allow_rule(&project, Scope::Local, "Bash(git log:*)").unwrap());
        assert_eq!(allow_rules(&project).len(), 3);
        assert!(allow_rules(&project.join("missing")).is_empty());
    }

    #[test]
    fn a_linked_settings_file_stays_linked_and_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let project = term_base::testing::scratch("claude-rules-link");
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        let shared = project.join("shared.json");
        std::fs::write(&shared, FILE).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&shared, Scope::Local.file(&project)).unwrap();
        assert!(remove_allow_rule(&project, Scope::Local, "Bash(git log:*)").unwrap());
        let link = std::fs::symlink_metadata(Scope::Local.file(&project)).unwrap();
        assert!(link.file_type().is_symlink());
        assert_eq!(
            std::fs::read_to_string(&shared).unwrap(),
            FILE.replace("      \"Bash(git log:*)\",\n", "")
        );
        assert_eq!(std::fs::metadata(&shared).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
