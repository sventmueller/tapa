//! Which icon a file gets: a language's logo for the common languages, an SF Symbol for every other kind of file,
//! each in a system colour, by the file's name, the beginning of its name or its extensions.

/// What an icon draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
    /// An SF Symbol by its name.
    Symbol(&'static str),
    /// A logo from `assets/simple-icons` by its file name without `.svg`.
    Logo(&'static str),
}

/// The system colour an icon is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Tint {
    /// The colour of secondary text, for kinds that need no colour of their own.
    Plain,
    Blue,
    Brown,
    Cyan,
    Gray,
    Green,
    Indigo,
    Orange,
    Pink,
    Purple,
    Red,
    Teal,
    Yellow,
}

/// The icon of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Icon {
    /// Its name in this table, such as `rust`.
    pub id: &'static str,
    /// What it draws.
    pub shape: Shape,
    pub(crate) tint: Tint,
}

const fn logo(id: &'static str, tint: Tint) -> Icon {
    Icon {
        id,
        shape: Shape::Logo(id),
        tint,
    }
}

const fn symbol(id: &'static str, name: &'static str, tint: Tint) -> Icon {
    Icon {
        id,
        shape: Shape::Symbol(name),
        tint,
    }
}

pub(crate) const DOCUMENT: Icon = symbol("document", "doc", Tint::Plain);
const TEXT: Icon = symbol("text", "doc.plaintext", Tint::Plain);
const README: Icon = symbol("readme", "book.closed", Tint::Blue);
const LICENSE: Icon = symbol("license", "checkmark.seal", Tint::Yellow);
const CHANGELOG: Icon = symbol("changelog", "clock.arrow.circlepath", Tint::Plain);
const TODO: Icon = symbol("todo", "checklist", Tint::Blue);
const BUILD: Icon = symbol("build", "hammer", Tint::Gray);
const CONFIG: Icon = symbol("config", "gearshape", Tint::Gray);
const SECRETS: Icon = symbol("secrets", "key", Tint::Yellow);
const LOCK: Icon = symbol("lock", "lock", Tint::Gray);
const JSON: Icon = symbol("json", "curlybraces", Tint::Yellow);
const MARKUP: Icon = symbol("markup", "chevron.left.forwardslash.chevron.right", Tint::Teal);
const VUE: Icon = symbol("vue", "chevron.left.forwardslash.chevron.right", Tint::Green);
const SHELL: Icon = symbol("shell", "terminal", Tint::Green);
const CSHARP: Icon = symbol("csharp", "number", Tint::Purple);
const DATABASE: Icon = symbol("database", "cylinder", Tint::Indigo);
const TABLE: Icon = symbol("table", "tablecells", Tint::Green);
const PDF: Icon = symbol("pdf", "doc.richtext", Tint::Red);
const IMAGE: Icon = symbol("image", "photo", Tint::Teal);
const VECTOR: Icon = symbol("vector", "square.on.circle", Tint::Orange);
const AUDIO: Icon = symbol("audio", "waveform", Tint::Pink);
const VIDEO: Icon = symbol("video", "film", Tint::Pink);
const FONT: Icon = symbol("font", "textformat", Tint::Gray);
const ARCHIVE: Icon = symbol("archive", "archivebox", Tint::Brown);
const DIFF: Icon = symbol("diff", "plus.forwardslash.minus", Tint::Gray);

const RUST: Icon = logo("rust", Tint::Orange);
const GO: Icon = logo("go", Tint::Cyan);
const TYPESCRIPT: Icon = logo("typescript", Tint::Blue);
const JAVASCRIPT: Icon = logo("javascript", Tint::Yellow);
const REACT: Icon = logo("react", Tint::Cyan);
const PYTHON: Icon = logo("python", Tint::Blue);
const SWIFT: Icon = logo("swift", Tint::Orange);
const C: Icon = logo("c", Tint::Blue);
const CPP: Icon = logo("cplusplus", Tint::Indigo);
const JAVA: Icon = logo("openjdk", Tint::Red);
const KOTLIN: Icon = logo("kotlin", Tint::Purple);
const RUBY: Icon = logo("ruby", Tint::Red);
const PHP: Icon = logo("php", Tint::Indigo);
const HTML: Icon = logo("html5", Tint::Orange);
const CSS: Icon = logo("css", Tint::Purple);
const DOCKER: Icon = logo("docker", Tint::Blue);
const GIT: Icon = logo("git", Tint::Orange);
const MARKDOWN: Icon = logo("markdown", Tint::Plain);
const YAML: Icon = logo("yaml", Tint::Red);
const DART: Icon = logo("dart", Tint::Teal);
const LUA: Icon = logo("lua", Tint::Indigo);
const ZIG: Icon = logo("zig", Tint::Orange);
const SVELTE: Icon = logo("svelte", Tint::Orange);
const HASKELL: Icon = logo("haskell", Tint::Purple);
const ELIXIR: Icon = logo("elixir", Tint::Purple);
const SCALA: Icon = logo("scala", Tint::Red);
const GRAPHQL: Icon = logo("graphql", Tint::Pink);
const PRISMA: Icon = logo("prisma", Tint::Indigo);

/// Every logo the table draws, for the test that each one parses.
#[cfg(test)]
pub(crate) const LOGOS: &[Icon] = &[
    RUST, GO, TYPESCRIPT, JAVASCRIPT, REACT, PYTHON, SWIFT, C, CPP, JAVA, KOTLIN, RUBY, PHP, HTML, CSS, DOCKER, GIT,
    MARKDOWN, YAML, DART, LUA, ZIG, SVELTE, HASKELL, ELIXIR, SCALA, GRAPHQL, PRISMA,
];

/// Whole file names, lower case.
const NAMES: &[(&str, Icon)] = &[
    ("makefile", BUILD),
    ("gnumakefile", BUILD),
    ("justfile", BUILD),
    ("cmakelists.txt", BUILD),
    ("meson.build", BUILD),
    ("build.gradle", BUILD),
    ("build.gradle.kts", BUILD),
    ("gemfile", RUBY),
    ("rakefile", RUBY),
    ("podfile", RUBY),
    ("dockerfile", DOCKER),
    ("containerfile", DOCKER),
    ("package-lock.json", LOCK),
    ("pnpm-lock.yaml", LOCK),
    ("bun.lockb", LOCK),
    (".bashrc", SHELL),
    (".bash_profile", SHELL),
    (".zshrc", SHELL),
    (".zshenv", SHELL),
    (".zprofile", SHELL),
    (".profile", SHELL),
    (".envrc", SHELL),
    ("version", CHANGELOG),
];

/// Names before their first dot, lower case, which also take a suffix after a dash or underscore, as `license-mit`.
const STEMS: &[(&str, Icon)] = &[
    ("readme", README),
    ("license", LICENSE),
    ("licence", LICENSE),
    ("copying", LICENSE),
    ("changelog", CHANGELOG),
    ("changes", CHANGELOG),
    ("todo", TODO),
];

/// Beginnings and endings of file names, lower case; an empty ending takes any.
const PATTERNS: &[(&str, &str, Icon)] = &[
    ("dockerfile.", "", DOCKER),
    ("containerfile.", "", DOCKER),
    ("compose.", ".yml", DOCKER),
    ("compose.", ".yaml", DOCKER),
    ("docker-compose.", ".yml", DOCKER),
    ("docker-compose.", ".yaml", DOCKER),
    (".env.", "", SECRETS),
];

/// Extensions without the dot, lower case; they may hold dots themselves, such as `d.ts`.
const EXTENSIONS: &[(&[&str], Icon)] = &[
    (&["rs"], RUST),
    (&["go"], GO),
    (&["ts", "mts", "cts"], TYPESCRIPT),
    (&["js", "mjs", "cjs"], JAVASCRIPT),
    (&["tsx", "jsx"], REACT),
    (&["py", "pyi", "pyw"], PYTHON),
    (&["swift"], SWIFT),
    (&["c", "h"], C),
    (
        &["cpp", "cc", "cxx", "c++", "hpp", "hh", "hxx", "h++", "ino", "mm"],
        CPP,
    ),
    (&["java", "jar"], JAVA),
    (&["kt", "kts"], KOTLIN),
    (&["rb", "erb", "rake", "gemspec"], RUBY),
    (&["php"], PHP),
    (&["html", "htm", "xhtml"], HTML),
    (&["css", "scss", "sass", "less"], CSS),
    (&["dockerfile", "containerfile"], DOCKER),
    (&["gitignore", "gitattributes", "gitmodules", "gitkeep", "mailmap"], GIT),
    (&["md", "markdown", "mdx"], MARKDOWN),
    (&["yml", "yaml"], YAML),
    (&["dart"], DART),
    (&["lua"], LUA),
    (&["zig"], ZIG),
    (&["svelte"], SVELTE),
    (&["hs", "lhs"], HASKELL),
    (&["ex", "exs"], ELIXIR),
    (&["scala", "sc"], SCALA),
    (&["graphql", "gql"], GRAPHQL),
    (&["prisma"], PRISMA),
    (&["cs", "csx"], CSHARP),
    (&["vue"], VUE),
    (&["json", "jsonc", "json5", "jsonl", "webmanifest"], JSON),
    (&["xml", "plist", "xsd", "xsl", "xslt", "storyboard", "xib"], MARKUP),
    (&["sh", "bash", "zsh", "fish", "command", "ps1", "bat", "cmd"], SHELL),
    (
        &[
            "toml",
            "ini",
            "cfg",
            "conf",
            "properties",
            "editorconfig",
            "prettierrc",
            "eslintrc",
            "npmrc",
        ],
        CONFIG,
    ),
    (&["env"], SECRETS),
    (&["lock", "lockb"], LOCK),
    (&["sql", "sqlite", "db", "duckdb"], DATABASE),
    (&["csv", "tsv", "xls", "xlsx", "numbers", "parquet"], TABLE),
    (&["pdf"], PDF),
    (
        &[
            "png", "jpg", "jpeg", "gif", "webp", "heic", "heif", "avif", "tif", "tiff", "bmp", "ico", "icns",
        ],
        IMAGE,
    ),
    (&["svg"], VECTOR),
    (
        &["mp3", "wav", "m4a", "aac", "aif", "aiff", "flac", "ogg", "opus"],
        AUDIO,
    ),
    (&["mp4", "mov", "m4v", "avi", "mkv", "webm"], VIDEO),
    (&["ttf", "otf", "ttc", "woff", "woff2"], FONT),
    (
        &["zip", "tar", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "dmg"],
        ARCHIVE,
    ),
    (&["diff", "patch"], DIFF),
    (&["txt", "text", "rtf", "log"], TEXT),
];

/// The icon of a file called `name`: by its whole name, then by the name before its first dot, then by how it begins
/// and ends, then by its extensions from the longest, else a plain document.
pub(crate) fn icon_of(name: &str) -> Icon {
    let lower = name.to_lowercase();
    let named = || NAMES.iter().find(|(n, _)| *n == lower).map(|(_, icon)| *icon);
    let stem = lower.split('.').next().unwrap_or_default();
    let stemmed = || {
        STEMS
            .iter()
            .find(|(word, _)| {
                stem.strip_prefix(word)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '_']))
            })
            .map(|(_, icon)| *icon)
    };
    let patterned = || {
        PATTERNS
            .iter()
            .find(|(start, end, _)| lower.starts_with(start) && lower.ends_with(end))
            .map(|(.., icon)| *icon)
    };
    let extended = || {
        extensions(&lower).find_map(|extension| {
            EXTENSIONS
                .iter()
                .find(|(all, _)| all.contains(&extension))
                .map(|(_, icon)| *icon)
        })
    };
    named()
        .or_else(stemmed)
        .or_else(patterned)
        .or_else(extended)
        .unwrap_or(DOCUMENT)
}

/// The extensions of a lower-case file name, the longest first: `a.spec.ts` has `spec.ts` and `ts`, `.gitignore` has
/// `gitignore`.
fn extensions(lower: &str) -> impl Iterator<Item = &str> {
    lower
        .match_indices('.')
        .map(|(at, _)| &lower[at + 1..])
        .filter(|extension| !extension.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_are_tried_from_the_longest() {
        assert_eq!(extensions("a.spec.ts").collect::<Vec<_>>(), ["spec.ts", "ts"]);
        assert_eq!(extensions(".gitignore").collect::<Vec<_>>(), ["gitignore"]);
        assert_eq!(extensions("makefile").count(), 0);
        assert_eq!(extensions("trailing.").count(), 0);
    }

    #[test]
    fn every_name_pattern_and_extension_is_lower_case_and_listed_once() {
        let mut seen = std::collections::HashSet::new();
        for (all, icon) in EXTENSIONS {
            for extension in *all {
                assert_eq!(*extension, extension.to_lowercase(), "{}", icon.id);
                assert!(seen.insert(*extension), "{extension} twice");
            }
        }
        for (name, _) in NAMES.iter().chain(STEMS) {
            assert_eq!(*name, name.to_lowercase());
        }
        for (start, end, _) in PATTERNS {
            assert_eq!((*start, *end), (&*start.to_lowercase(), &*end.to_lowercase()));
        }
    }
}
