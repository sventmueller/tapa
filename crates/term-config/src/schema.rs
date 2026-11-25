//! The sections of the settings file and their defaults.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::Warning;
use crate::agent::Agent;
use crate::browser::Browser;
use crate::dictation::Dictation;
use crate::editor::{Editor, Files};
use crate::keys::Binding;
use crate::profile::Profile;
use crate::quick::QuickTerminal;
use crate::restore::Restore;

/// The version this build writes; `migrate` brings older files up to it.
pub const VERSION: u32 = 1;

/// All settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// The version of the file's layout.
    pub version: u32,
    /// The face the grid is drawn in.
    pub font: Font,
    /// The colours for the light and the dark appearance.
    pub theme: ThemeChoice,
    /// How the cursor looks.
    pub cursor: Cursor,
    /// Windows, tabs and panes.
    pub window: Window,
    /// How the terminal treats programs and keys.
    pub terminal: Terminal,
    /// The input field and suggestions (E7, E8).
    pub input: Input,
    /// The drop-down terminal of the global hotkey (F45).
    pub quick_terminal: QuickTerminal,
    /// What comes back after quitting (F71).
    pub restore: Restore,
    /// The agent tabs (E21).
    pub agent: Agent,
    /// The browser tabs (E22).
    pub browser: Browser,
    /// The editor tabs (E22).
    pub editor: Editor,
    /// The file list of a workspace's sidebar entry (E22).
    pub files: Files,
    /// Dictation through Lowspeak.
    pub dictation: Dictation,
    /// The profile new sessions start with.
    pub default_profile: String,
    /// Named sets of font, colours, shell, environment and folder (F161).
    #[serde(default, skip_serializing_if = "Vec::is_empty", rename = "profile")]
    pub profiles: Vec<Profile>,
    /// Key bindings on top of the defaults, chord to action; an empty action unbinds the chord.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            version: VERSION,
            font: Font::default(),
            theme: ThemeChoice::default(),
            cursor: Cursor::default(),
            window: Window::default(),
            terminal: Terminal::default(),
            input: Input::default(),
            quick_terminal: QuickTerminal::default(),
            restore: Restore::default(),
            agent: Agent::default(),
            browser: Browser::default(),
            editor: Editor::default(),
            files: Files::default(),
            dictation: Dictation::default(),
            default_profile: "Default".into(),
            profiles: Vec::new(),
            keys: BTreeMap::new(),
        }
    }
}

impl Config {
    /// The bindings in force: the defaults with the user's changes over them; warnings for chords or actions that
    /// do not exist.
    pub fn bindings(&self) -> (Vec<Binding>, Vec<Warning>) {
        crate::keys::merged(&self.keys)
    }
}

/// The face the grid is drawn in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Font {
    /// The family name, as the Font Book shows it.
    pub family: String,
    /// The size in points.
    pub size: f64,
    /// Ligatures where the face has them (F13).
    pub ligatures: bool,
    /// OpenType features (F13): `ss01` turns one on, `-calt` off, `cv05=2` picks an alternate.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// Axes of a variable font by their tag, such as `wght = 450` (F13).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub variations: BTreeMap<String, f64>,
    /// Extra space between lines, as a multiple of the face's own.
    pub line_height: f64,
}

impl Default for Font {
    fn default() -> Font {
        Font {
            family: "SF Mono".into(),
            size: 13.0,
            ligatures: true,
            features: Vec::new(),
            variations: BTreeMap::new(),
            line_height: 1.0,
        }
    }
}

impl Font {
    /// The OpenType features as tag and value, ligatures turned off where the settings say so and no feature names
    /// them; entries that are no four-letter tag are left out.
    pub fn feature_settings(&self) -> Vec<(String, i32)> {
        let mut settings: Vec<(String, i32)> = self.features.iter().filter_map(|entry| feature(entry)).collect();
        if !self.ligatures {
            for tag in ["liga", "calt"] {
                if !settings.iter().any(|(known, _)| known == tag) {
                    settings.push((tag.to_string(), 0));
                }
            }
        }
        settings
    }

    /// The axes of a variable font as Core Text names them, the tag read as a big-endian number; tags that are not
    /// four letters are left out.
    pub fn variation_settings(&self) -> Vec<(u32, f64)> {
        self.variations
            .iter()
            .filter_map(|(tag, &value)| Some((four_letters(tag)?, value)))
            .collect()
    }
}

fn feature(entry: &str) -> Option<(String, i32)> {
    let entry = entry.trim();
    let (tag, value) = match entry.strip_prefix('-') {
        Some(tag) => (tag, 0),
        None => match entry.split_once('=') {
            Some((tag, value)) => (tag.trim(), value.trim().parse().ok()?),
            None => (entry.strip_prefix('+').unwrap_or(entry), 1),
        },
    };
    four_letters(tag)?;
    Some((tag.to_string(), value))
}

fn four_letters(tag: &str) -> Option<u32> {
    let bytes: [u8; 4] = tag.as_bytes().try_into().ok()?;
    bytes
        .iter()
        .all(|b| b.is_ascii_graphic())
        .then(|| u32::from_be_bytes(bytes))
}

/// The colours for the light and the dark appearance, following the system (F162).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeChoice {
    /// The theme in the light appearance.
    pub light: String,
    /// The theme in the dark appearance.
    pub dark: String,
    /// The least contrast text keeps against its background, 1 to 21; 1 leaves the theme's colours alone (F164).
    pub minimum_contrast: f64,
}

impl Default for ThemeChoice {
    fn default() -> ThemeChoice {
        ThemeChoice {
            light: "Tapa Light".into(),
            dark: "Tapa Dark".into(),
            minimum_contrast: 1.0,
        }
    }
}

/// How the cursor looks (F166).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cursor {
    /// Its shape unless a program asks for another.
    pub style: CursorStyle,
    /// Whether it blinks.
    pub blink: bool,
    /// A faint band along the cursor's line.
    pub guide: bool,
}

impl Default for Cursor {
    fn default() -> Cursor {
        Cursor {
            style: CursorStyle::Block,
            blink: true,
            guide: false,
        }
    }
}

/// The cursor's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    /// A filled cell.
    Block,
    /// A thin bar before the cell.
    Bar,
    /// A line under the cell.
    Underline,
}

/// Whether the app follows the system's light or dark appearance or keeps one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    /// As the system has it.
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

impl Appearance {
    /// Its word in the settings file.
    pub fn id(self) -> &'static str {
        match self {
            Appearance::System => "system",
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        }
    }
}

/// Windows, tabs and panes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Window {
    /// Light, dark, or as the system has it, for the whole app.
    pub appearance: Appearance,
    /// The size of the interface's text in points: the sidebar, the tab bars, the panels, the window's title and the
    /// editors' bars; the code, the terminal and the agent tabs have sizes of their own.
    pub interface_font_size: f64,
    /// New windows show the sidebar of workspaces (E9, E20).
    pub sidebar: bool,
    /// A click on the selected workspace's name or on the pencil in its row renames it; without, only the row's menu
    /// does, and the rows show no pencil.
    pub rename_on_click: bool,
    /// Which new sessions start in the folder of the one in front (F51).
    pub inherit_directory: Inherit,
    /// Asks before closing a tab, window or the app while a program runs (F213).
    pub confirm_close: bool,
    /// Columns and rows of a new window.
    pub columns: u16,
    /// Rows of a new window.
    pub rows: u16,
}

impl Default for Window {
    fn default() -> Window {
        Window {
            appearance: Appearance::System,
            interface_font_size: 13.0,
            sidebar: true,
            rename_on_click: true,
            inherit_directory: Inherit::default(),
            confirm_close: true,
            columns: 100,
            rows: 30,
        }
    }
}

/// Where a new session takes the folder of the session in front from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Inherit {
    /// Not read since new tabs start in their workspace's folder (E20); kept so that older files read as before.
    pub tab: bool,
    /// A new window, from the selected workspace's folder.
    pub window: bool,
    /// A new pane.
    pub split: bool,
}

impl Default for Inherit {
    fn default() -> Inherit {
        Inherit {
            tab: true,
            window: false,
            split: true,
        }
    }
}

/// How the terminal treats programs and keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Terminal {
    /// Lines of history a session keeps.
    pub scrollback: usize,
    /// Which ⌥ keys send Meta instead of typing characters.
    pub option_as_meta: OptionAsMeta,
    /// Copies a selection as soon as it is made (F218).
    pub copy_on_select: bool,
    /// Lets programs read the pasteboard through OSC 52; off, because a program on a remote host could read it.
    pub allow_clipboard_read: bool,
    /// Lets programs switch on the kitty keyboard protocol.
    pub kitty_keyboard: bool,
    /// Shell integration for zsh, bash and fish (F77).
    pub shell_integration: bool,
    /// The shell integration's `ssh` takes it along to the shell on the other host (F89).
    pub ssh_integration: bool,
    /// Marks each command as a block: the bar in the gutter, the header of a command scrolled away and its actions
    /// under the pointer (F78–F87).
    pub blocks: bool,
    /// Shows a paste first that would run commands or press keys, unless the program reads it as bracketed paste (F98).
    pub paste_protection: bool,
}

impl Default for Terminal {
    fn default() -> Terminal {
        Terminal {
            scrollback: 10_000,
            option_as_meta: OptionAsMeta::None,
            copy_on_select: false,
            allow_clipboard_read: false,
            kitty_keyboard: true,
            shell_integration: true,
            ssh_integration: true,
            blocks: false,
            paste_protection: true,
        }
    }
}

/// Which ⌥ keys send Meta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OptionAsMeta {
    /// Neither; ⌥ types characters such as `@` and `|` on German keyboards.
    None,
    /// The left one.
    Left,
    /// The right one.
    Right,
    /// Both.
    Both,
}

/// The input field and suggestions (E7, E8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Input {
    /// New sessions start with the input field on.
    pub field: bool,
    /// Vim keys in the input field (F95).
    pub vim: bool,
    /// Suggestions while typing and the next command without input.
    pub suggestions: bool,
    /// The parts of a command in the input field in colours of their own (F92).
    pub highlighting: bool,
    /// A command the shell cannot run underlined in red in the input field.
    pub underline_errors: bool,
}

impl Default for Input {
    fn default() -> Input {
        Input {
            field: false,
            vim: false,
            suggestions: true,
            highlighting: true,
            underline_errors: true,
        }
    }
}

/// Reads each section of `table` on its own, so a mistake in one leaves the others as the user wrote them.
pub(crate) fn read(mut table: toml::Table, warnings: &mut Vec<Warning>) -> Config {
    let defaults = Config::default();
    let mut take = |name: &str| table.remove(name);
    let font = section(take("font"), "font", defaults.font, warnings);
    let theme = section(take("theme"), "theme", defaults.theme, warnings);
    let cursor = section(take("cursor"), "cursor", defaults.cursor, warnings);
    let window = section(take("window"), "window", defaults.window, warnings);
    let terminal = section(take("terminal"), "terminal", defaults.terminal, warnings);
    let input = section(take("input"), "input", defaults.input, warnings);
    let quick_terminal = section(
        take("quick_terminal"),
        "quick_terminal",
        defaults.quick_terminal,
        warnings,
    );
    let restore = section(take("restore"), "restore", defaults.restore, warnings);
    let agent = section(take("agent"), "agent", defaults.agent, warnings);
    let browser = section(take("browser"), "browser", defaults.browser, warnings);
    let editor = section(take("editor"), "editor", defaults.editor, warnings);
    let files = section(take("files"), "files", defaults.files, warnings);
    let dictation = section(take("dictation"), "dictation", defaults.dictation, warnings);
    let default_profile = section(
        take("default_profile"),
        "default_profile",
        defaults.default_profile,
        warnings,
    );
    let profiles = section(take("profile"), "profile", defaults.profiles, warnings);
    let keys = section(take("keys"), "keys", defaults.keys, warnings);
    table.remove("version");
    for unknown in table.keys() {
        warnings.push(Warning(format!("unknown setting `{unknown}`")));
    }
    Config {
        version: VERSION,
        font,
        theme,
        cursor,
        window,
        terminal,
        input,
        quick_terminal,
        restore,
        agent,
        browser,
        editor,
        files,
        dictation,
        default_profile,
        profiles,
        keys,
    }
}

fn section<T: DeserializeOwned>(value: Option<toml::Value>, name: &str, default: T, warnings: &mut Vec<Warning>) -> T {
    let Some(value) = value else { return default };
    match value.try_into() {
        Ok(read) => read,
        Err(error) => {
            warnings.push(Warning(format!("{name}: {error}")));
            default
        }
    }
}
