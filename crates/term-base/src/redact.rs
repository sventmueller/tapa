//! Masking secrets in terminal text before it is stored, shared or indexed (F180, E17): tokens with a known shape,
//! values after names such as `password=`, passwords in URLs, bearer tokens and private key blocks.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;

/// What a secret is replaced with, the same whatever its length, so its length tells nothing either.
pub const MASK: &str = "••••••••";

/// Tokens recognisable on their own by their prefix and shape.
static TOKENS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"\b(?:",
        r"gh[pousr]_[A-Za-z0-9]{20,}",
        r"|github_pat_[A-Za-z0-9_]{20,}",
        r"|glpat-[A-Za-z0-9_\-]{20,}",
        r"|sk-[A-Za-z0-9_\-]{20,}",
        r"|xox[abprs]-[A-Za-z0-9\-]{10,}",
        r"|(?:AKIA|ASIA)[0-9A-Z]{16}",
        r"|AIza[0-9A-Za-z_\-]{35}",
        r"|(?:sk|rk|pk)_(?:live|test)_[0-9A-Za-z]{16,}",
        r"|npm_[A-Za-z0-9]{30,}",
        r"|eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}",
        r")"
    ))
    .expect("the token pattern is valid")
});

/// A value after a name that says it is secret, the value in group `value`.
static ASSIGNED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r#"(?i)\b(?:[A-Z0-9_]*(?:password|passwd|pwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|auth))"#,
        r#"["']?\s*[:=]\s*["']?(?P<value>[^\s"']{4,})"#
    ))
    .expect("the assignment pattern is valid")
});

/// The password in `scheme://user:password@host`, in group `value`.
static URL_PASSWORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z][A-Za-z0-9+.\-]*://[^\s:/@]+:(?P<value>[^\s/@]+)@").expect("the URL pattern is valid")
});

/// The token after `Bearer` or `Basic`, in group `value`.
static BEARER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:bearer|basic)\s+(?P<value>[A-Za-z0-9._~+/=\-]{12,})").expect("the bearer pattern is valid")
});

/// `text` with every secret it holds masked; borrowed when it holds none.
pub fn redact(text: &str) -> Cow<'_, str> {
    let mut out = Cow::Borrowed(text);
    for pattern in [&*ASSIGNED, &*URL_PASSWORD, &*BEARER] {
        if pattern.is_match(&out) {
            out = Cow::Owned(mask_group(pattern, &out));
        }
    }
    if TOKENS.is_match(&out) {
        out = Cow::Owned(TOKENS.replace_all(&out, MASK).into_owned());
    }
    out
}

fn mask_group(pattern: &Regex, text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for captures in pattern.captures_iter(text) {
        let Some(value) = captures.name("value") else { continue };
        if value.as_str() == MASK {
            continue;
        }
        out.push_str(&text[last..value.start()]);
        out.push_str(MASK);
        last = value.end();
    }
    out.push_str(&text[last..]);
    out
}

/// `lines` with their secrets masked, the lines of a private key block from its `BEGIN` to its `END` line too.
pub fn redact_lines(lines: &mut [String]) {
    let mut in_key = false;
    for line in lines.iter_mut() {
        let begins = line.contains("-----BEGIN") && line.contains("PRIVATE KEY");
        let ends = line.contains("-----END") && line.contains("PRIVATE KEY");
        if in_key && !ends {
            *line = MASK.to_string();
            continue;
        }
        in_key = (begins || in_key) && !ends;
        if let Cow::Owned(masked) = redact(line) {
            *line = masked;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_of_known_shapes_are_masked() {
        let cases = [
            "export GITHUB=ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "aws AKIAIOSFODNN7EXAMPLE here",
            "key sk-ant-api03-abcdefghijklmnopqrstuvwx",
            "slack xoxb-1234567890-abcdefghij",
            "jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
        ];
        for case in cases {
            let masked = redact(case);
            assert!(masked.contains(MASK), "{case}");
        }
        assert_eq!(redact("aws AKIAIOSFODNN7EXAMPLE here"), format!("aws {MASK} here"));
    }

    #[test]
    fn values_named_secret_are_masked_but_the_name_stays() {
        assert_eq!(redact("PASSWORD=hunter22"), format!("PASSWORD={MASK}"));
        assert_eq!(
            redact(r#""api_key": "abcd1234efgh""#),
            format!(r#""api_key": "{MASK}""#)
        );
        assert_eq!(redact("DB_TOKEN: s3cr3tvalue x"), format!("DB_TOKEN: {MASK} x"));
    }

    #[test]
    fn passwords_in_urls_and_bearer_tokens_are_masked() {
        assert_eq!(
            redact("git clone https://me:topsecret@example.com/repo"),
            format!("git clone https://me:{MASK}@example.com/repo")
        );
        assert_eq!(
            redact("Authorization: Bearer abcdefghijklmnop"),
            format!("Authorization: Bearer {MASK}")
        );
    }

    #[test]
    fn ordinary_output_is_left_alone() {
        for text in [
            "total 48",
            "drwxr-xr-x  5 me  staff  160 Nov 24 10:00 src",
            "token count: 12",
            "https://example.com/path",
        ] {
            assert!(matches!(redact(text), Cow::Borrowed(_)), "{text}");
        }
    }

    #[test]
    fn a_private_key_block_is_masked_line_by_line() {
        let mut lines: Vec<String> = [
            "$ cat id_ed25519",
            "-----BEGIN OPENSSH PRIVATE KEY-----",
            "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQ",
            "QyNTUxOQAAACDx",
            "-----END OPENSSH PRIVATE KEY-----",
            "$ ls",
        ]
        .iter()
        .map(|line| (*line).to_string())
        .collect();
        redact_lines(&mut lines);
        assert_eq!(lines[0], "$ cat id_ed25519");
        assert_eq!(lines[2], MASK);
        assert_eq!(lines[3], MASK);
        assert_eq!(lines[4], "-----END OPENSSH PRIVATE KEY-----");
        assert_eq!(lines[5], "$ ls");
    }
}
