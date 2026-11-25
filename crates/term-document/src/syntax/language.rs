//! Which of splat's languages a file is written in, by its name, its extension or the interpreter its first line names:
//! a language of code splat colours by its tokens, or Markdown, which splat colours from its parse.

use std::path::Path;

/// A language splat's highlighter knows, by the first name it goes by there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Language(&'static str);

/// Markdown's name, which no language of code splat lexes goes by.
const MARKDOWN: &str = "markdown";

/// Whole file names, compared without regard to case.
const FILE_NAMES: &[(&str, &str)] = &[
    (".bash_profile", "bash"),
    (".bashrc", "bash"),
    (".envrc", "bash"),
    (".profile", "bash"),
    (".zprofile", "bash"),
    (".zshenv", "bash"),
    (".zshrc", "bash"),
    ("brewfile", "ruby"),
    ("cargo.lock", "toml"),
    ("gemfile", "ruby"),
    ("pipfile", "toml"),
    ("pkgbuild", "bash"),
    ("podfile", "ruby"),
    ("rakefile", "ruby"),
    ("vagrantfile", "ruby"),
];

/// Extensions, compared without regard to case.
const EXTENSIONS: &[(&str, &str)] = &[
    ("bash", "bash"),
    ("c", "c"),
    ("c++", "cpp"),
    ("cc", "cpp"),
    ("cjs", "javascript"),
    ("cpp", "cpp"),
    ("cs", "csharp"),
    ("csproj", "html"),
    ("css", "css"),
    ("cts", "typescript"),
    ("cxx", "cpp"),
    ("gemspec", "ruby"),
    ("go", "go"),
    ("h", "c"),
    ("hh", "cpp"),
    ("hpp", "cpp"),
    ("htm", "html"),
    ("html", "html"),
    ("hxx", "cpp"),
    ("java", "java"),
    ("js", "javascript"),
    ("json", "json"),
    ("jsonc", "json"),
    ("jsx", "javascript"),
    ("kt", "kotlin"),
    ("kts", "kotlin"),
    ("latex", "tex"),
    ("less", "css"),
    ("markdown", MARKDOWN),
    ("md", MARKDOWN),
    ("mdown", MARKDOWN),
    ("mermaid", "mermaid"),
    ("mkd", MARKDOWN),
    ("mkdn", MARKDOWN),
    ("mjs", "javascript"),
    ("mmd", "mermaid"),
    ("mts", "typescript"),
    ("php", "php"),
    ("plist", "html"),
    ("py", "python"),
    ("pyi", "python"),
    ("pyw", "python"),
    ("rake", "ruby"),
    ("rb", "ruby"),
    ("rs", "rust"),
    ("scss", "css"),
    ("sh", "bash"),
    ("sql", "sql"),
    ("sty", "tex"),
    ("svg", "html"),
    ("swift", "swift"),
    ("tex", "tex"),
    ("toml", "toml"),
    ("ts", "typescript"),
    ("tsx", "typescript"),
    ("vue", "html"),
    ("webmanifest", "json"),
    ("xhtml", "html"),
    ("xml", "html"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
    ("zsh", "bash"),
];

/// Interpreters a first line `#!` may name, without a version such as the `3.12` of `python3.12`.
const INTERPRETERS: &[(&str, &str)] = &[
    ("ash", "bash"),
    ("bash", "bash"),
    ("bun", "javascript"),
    ("dash", "bash"),
    ("deno", "typescript"),
    ("ksh", "bash"),
    ("node", "javascript"),
    ("nodejs", "javascript"),
    ("php", "php"),
    ("python", "python"),
    ("ruby", "ruby"),
    ("sh", "bash"),
    ("swift", "swift"),
    ("ts-node", "typescript"),
    ("tsx", "typescript"),
    ("zsh", "bash"),
];

impl Language {
    pub(crate) const MARKDOWN: Language = Language(MARKDOWN);

    /// The language splat knows by `name` as the first name it goes by, in any case.
    pub(crate) fn named(name: &str) -> Option<Language> {
        Language::all().find(|known| known.0.eq_ignore_ascii_case(name))
    }

    /// Every language splat knows: its languages of code, then Markdown.
    pub(crate) fn all() -> impl Iterator<Item = Language> {
        splat_markdown::code::language_names()
            .map(Language)
            .chain(std::iter::once(Language::MARKDOWN))
    }

    /// The name to hand to `splat_markdown::code::tokens`.
    pub fn name(self) -> &'static str {
        self.0
    }

    /// Whether it is Markdown, which is coloured from splat's parse rather than by tokens.
    pub fn is_markdown(self) -> bool {
        self == Language::MARKDOWN
    }

    /// The marker that starts a line comment, or `None` for a language whose comments only enclose their text.
    pub fn line_comment(self) -> Option<&'static str> {
        match self.0 {
            "python" | "bash" | "yaml" | "toml" | "ruby" => Some("#"),
            "sql" => Some("--"),
            "html" | "css" | "mermaid" | "tex" | MARKDOWN => None,
            _ => Some("//"),
        }
    }

    /// The language of a file at `path` whose text begins with `first_line`: its name decides, then its extension,
    /// then the interpreter a `#!` line names.
    pub fn detect(path: Option<&Path>, first_line: &str) -> Option<Language> {
        let lookup = |table: &[(&str, &'static str)], key: &str| {
            table
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .and_then(|(_, name)| Language::named(name))
        };
        let by_path = path.and_then(|path| {
            let name = path.file_name()?.to_str()?;
            lookup(FILE_NAMES, name).or_else(|| lookup(EXTENSIONS, path.extension()?.to_str()?))
        });
        by_path.or_else(|| lookup(INTERPRETERS, interpreter(first_line)?))
    }
}

/// The program a `#!` line runs, past `env` with its options and variables, without a version at its end.
fn interpreter(first_line: &str) -> Option<&str> {
    let mut words = first_line.strip_prefix("#!")?.split_whitespace();
    let mut program = words.next()?.rsplit('/').next()?;
    if program == "env" {
        program = words.find(|word| !word.starts_with('-') && !word.contains('='))?;
        program = program.rsplit('/').next()?;
    }
    Some(program.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(path: &str, first_line: &str) -> Option<&'static str> {
        Language::detect(Some(Path::new(path)), first_line).map(Language::name)
    }

    #[test]
    fn a_name_beats_an_extension_and_an_extension_beats_the_first_line() {
        assert_eq!(detect("Cargo.lock", ""), Some("toml"));
        assert_eq!(detect("src/main.RS", "#!/bin/sh"), Some("rust"));
        assert_eq!(detect("x.d.ts", ""), Some("typescript"));
        assert_eq!(detect("Gemfile", ""), Some("ruby"));
        assert_eq!(detect("notes.txt", ""), None);
        for name in ["a.md", "b.MARKDOWN", "c.mdown", "d.mkd", "e.mkdn"] {
            assert_eq!(detect(name, ""), Some("markdown"), "{name}");
        }
        assert_eq!(
            detect("README", ""),
            None,
            "a name without extension says nothing of its format"
        );
    }

    #[test]
    fn the_first_line_names_the_interpreter_past_env_and_versions() {
        for (line, name) in [
            ("#!/bin/bash", "bash"),
            ("#!/usr/bin/env python3.12", "python"),
            ("#!/usr/bin/env -S node --no-warnings", "javascript"),
            ("#! /usr/bin/env FOO=1 ruby", "ruby"),
            ("#!/usr/local/bin/zsh -f", "bash"),
        ] {
            assert_eq!(detect("script", line), Some(name), "{line}");
        }
        assert_eq!(detect("script", "# not a shebang"), None);
        assert_eq!(Language::detect(None, "#!/bin/sh").map(Language::name), Some("bash"));
    }

    #[test]
    fn every_language_of_the_tables_is_one_splat_knows() {
        for (_, name) in FILE_NAMES.iter().chain(EXTENSIONS).chain(INTERPRETERS) {
            assert!(Language::named(name).is_some(), "{name}");
        }
        assert!(Language::all().count() >= 23);
        assert!(Language::named("Markdown").is_some_and(Language::is_markdown));
        assert_eq!(Language::named("RUST").map(Language::name), Some("rust"));
        assert_eq!(Language::named("rs"), None, "aliases are not first names");
    }
}
