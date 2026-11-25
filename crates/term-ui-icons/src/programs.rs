//! The icon of the program in front of a session (E9, F211): its logo where Simple Icons has one we may use, else an
//! SF Symbol for its kind; a shell waiting for input shows the terminal symbol.

use crate::kinds::{Icon, Shape, Tint};

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

/// A shell waiting for input.
pub const SHELL: Icon = symbol("shell", "terminal", Tint::Plain);
/// A session on another machine.
pub const REMOTE: Icon = symbol("remote", "network", Tint::Blue);
/// A program without an icon of its own.
pub const PROGRAM: Icon = symbol("program", "gearshape", Tint::Plain);
/// A browser tab.
pub const BROWSER: Icon = symbol("browser", "globe", Tint::Blue);

/// Programs by the name of their executable.
const PROGRAMS: &[(&str, Icon)] = &[
    ("git", logo("git", Tint::Orange)),
    ("lazygit", logo("git", Tint::Orange)),
    ("tig", logo("git", Tint::Orange)),
    ("node", logo("nodedotjs", Tint::Green)),
    ("npx", logo("npm", Tint::Red)),
    ("npm", logo("npm", Tint::Red)),
    ("pnpm", logo("pnpm", Tint::Yellow)),
    ("pnpx", logo("pnpm", Tint::Yellow)),
    ("yarn", logo("yarn", Tint::Blue)),
    ("bun", logo("bun", Tint::Brown)),
    ("bunx", logo("bun", Tint::Brown)),
    ("deno", logo("deno", Tint::Plain)),
    ("python", logo("python", Tint::Blue)),
    ("python3", logo("python", Tint::Blue)),
    ("ipython", logo("python", Tint::Blue)),
    ("uv", logo("python", Tint::Blue)),
    ("pip", logo("python", Tint::Blue)),
    ("ruby", logo("ruby", Tint::Red)),
    ("irb", logo("ruby", Tint::Red)),
    ("cargo", logo("rust", Tint::Orange)),
    ("rustc", logo("rust", Tint::Orange)),
    ("go", logo("go", Tint::Cyan)),
    ("docker", logo("docker", Tint::Blue)),
    ("docker-compose", logo("docker", Tint::Blue)),
    ("kubectl", logo("kubernetes", Tint::Blue)),
    ("k9s", logo("kubernetes", Tint::Blue)),
    ("helm", logo("kubernetes", Tint::Blue)),
    ("terraform", logo("terraform", Tint::Purple)),
    ("brew", logo("homebrew", Tint::Orange)),
    ("nvim", logo("neovim", Tint::Green)),
    ("vim", logo("vim", Tint::Green)),
    ("vi", logo("vim", Tint::Green)),
    ("tmux", logo("tmux", Tint::Green)),
    ("htop", logo("htop", Tint::Green)),
    ("btop", symbol("btop", "gauge.with.dots.needle.33percent", Tint::Green)),
    ("top", symbol("top", "gauge.with.dots.needle.33percent", Tint::Green)),
    ("psql", logo("postgresql", Tint::Blue)),
    ("mysql", logo("mysql", Tint::Blue)),
    ("redis-cli", logo("redis", Tint::Red)),
    ("sqlite3", logo("sqlite", Tint::Blue)),
    ("claude", logo("claude", Tint::Orange)),
    ("gemini", logo("googlegemini", Tint::Blue)),
    ("codex", symbol("codex", "sparkles", Tint::Plain)),
    ("opencode", symbol("opencode", "sparkles", Tint::Indigo)),
    ("agy", logo("googlegemini", Tint::Purple)),
    ("less", symbol("less", "doc.text.magnifyingglass", Tint::Plain)),
    ("man", symbol("man", "book", Tint::Plain)),
    ("make", symbol("make", "hammer", Tint::Plain)),
    ("just", symbol("just", "hammer", Tint::Plain)),
    ("cmake", symbol("cmake", "hammer", Tint::Plain)),
    ("curl", symbol("curl", "arrow.down.circle", Tint::Plain)),
    ("wget", symbol("wget", "arrow.down.circle", Tint::Plain)),
];

/// The icon of program `name`, as `ps` names it; a path is reduced to its last part.
pub fn program_icon(name: &str) -> Icon {
    let name = name.rsplit('/').next().unwrap_or(name).trim_start_matches('-');
    PROGRAMS
        .iter()
        .find(|(program, _)| *program == name)
        .map_or(PROGRAM, |(_, icon)| *icon)
}

/// The icon of shell `name` waiting for input: the terminal symbol for every shell, since a sidebar of shells is a
/// list of terminals rather than of programs.
pub fn shell_icon(name: &str) -> Icon {
    let _ = name;
    SHELL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_get_their_logo_and_others_a_symbol() {
        assert_eq!(program_icon("git").shape, Shape::Logo("git"));
        assert_eq!(program_icon("/opt/homebrew/bin/nvim").shape, Shape::Logo("neovim"));
        assert_eq!(program_icon("claude").shape, Shape::Logo("claude"));
        assert_eq!(program_icon("frobnicate"), PROGRAM);
        assert_eq!(shell_icon("-zsh"), SHELL);
        assert_eq!(shell_icon("fish"), SHELL);
    }

    #[test]
    fn every_logo_of_a_program_is_vendored_and_parses() {
        for (_, icon) in PROGRAMS {
            if let Shape::Logo(name) = icon.shape {
                assert!(crate::logo::path(name).is_some(), "{name}");
            }
        }
    }
}
