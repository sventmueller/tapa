//! Key bindings (F167, without key tables and chains in 1.0): chords such as `cmd+shift+t`, the actions they run,
//! the defaults, and the conflicts a settings window shows.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use crate::Warning;

/// A key with its modifiers, written `ctrl+alt+shift+cmd+key` in any order of the modifiers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Chord {
    /// ⌃.
    pub ctrl: bool,
    /// ⌥.
    pub alt: bool,
    /// ⇧.
    pub shift: bool,
    /// ⌘.
    pub cmd: bool,
    /// The key: a character in lower case, or a name such as `enter`, `left` or `f5`.
    pub key: String,
}

/// Why a chord could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// The chord has no key after its modifiers.
    NoKey(String),
    /// A part is neither a modifier nor a key.
    Unknown(String),
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyError::NoKey(chord) => write!(f, "`{chord}` has no key"),
            KeyError::Unknown(part) => write!(f, "`{part}` is no key"),
        }
    }
}

/// The names of keys that are not characters.
const NAMED: &[&str] = &[
    "enter",
    "tab",
    "space",
    "backspace",
    "delete",
    "escape",
    "left",
    "right",
    "up",
    "down",
    "home",
    "end",
    "pageup",
    "pagedown",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
];

impl FromStr for Chord {
    type Err = KeyError;

    fn from_str(text: &str) -> Result<Chord, KeyError> {
        let mut chord = Chord {
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
            key: String::new(),
        };
        let lower = text.trim().to_lowercase();
        let parts: Vec<&str> = if lower.ends_with("++") {
            let mut parts: Vec<&str> = lower[..lower.len() - 2].split('+').collect();
            parts.push("+");
            parts
        } else {
            lower.split('+').collect()
        };
        let Some((key, modifiers)) = parts.split_last() else {
            return Err(KeyError::NoKey(text.into()));
        };
        for modifier in modifiers {
            match *modifier {
                "ctrl" | "control" | "⌃" => chord.ctrl = true,
                "alt" | "opt" | "option" | "⌥" => chord.alt = true,
                "shift" | "⇧" => chord.shift = true,
                "cmd" | "command" | "super" | "⌘" => chord.cmd = true,
                other => return Err(KeyError::Unknown(other.into())),
            }
        }
        if key.is_empty() {
            return Err(KeyError::NoKey(text.into()));
        }
        if key.chars().count() != 1 && !NAMED.contains(key) {
            return Err(KeyError::Unknown((*key).into()));
        }
        chord.key = (*key).into();
        Ok(chord)
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [
            (self.ctrl, "ctrl+"),
            (self.alt, "alt+"),
            (self.shift, "shift+"),
            (self.cmd, "cmd+"),
        ] {
            if on {
                f.write_str(name)?;
            }
        }
        f.write_str(&self.key)
    }
}

macro_rules! actions {
    ($($variant:ident => $name:literal, $doc:literal;)*) => {
        /// What a key binding does.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Action {
            $(#[doc = $doc] $variant,)*
        }

        impl Action {
            /// Every action, in the order of the settings window.
            pub const ALL: &'static [Action] = &[$(Action::$variant,)*];

            /// The name the settings file uses.
            pub fn name(self) -> &'static str {
                match self {
                    $(Action::$variant => $name,)*
                }
            }

            /// What the action does, in English.
            pub fn description(self) -> &'static str {
                match self {
                    $(Action::$variant => $doc,)*
                }
            }

            /// The action named `name` in the settings file.
            pub fn named(name: &str) -> Option<Action> {
                match name {
                    $($name => Some(Action::$variant),)*
                    _ => None,
                }
            }
        }
    };
}

actions! {
    NewWindow => "new_window", "Opens a window.";
    NewWorkspace => "new_workspace", "Opens a workspace for a folder, in which its tabs start (E20).";
    NewTab => "new_tab", "Opens a tab in the workspace's folder.";
    RenameWorkspace => "rename_workspace", "Gives the workspace another name.";
    NewClaudeTab => "new_claude_tab", "Opens a Claude session in the workspace's folder (E21).";
    NewCodexTab => "new_codex_tab", "Opens a Codex session in the workspace's folder (E21).";
    NewOpenCodeTab => "new_opencode_tab", "Opens an OpenCode session in the workspace's folder (E21).";
    NewBrowserTab => "new_browser_tab", "Opens a browser tab in the workspace (E22).";
    NewFile => "new_file", "Opens an empty, untitled editor tab in the workspace (E22).";
    OpenFile => "open_file", "Opens a file in an editor tab of the workspace (E22).";
    Save => "save", "Saves the file of the editor in front.";
    SaveAs => "save_as", "Saves the file of the editor in front under another name.";
    SaveAll => "save_all", "Saves every editor with unsaved changes.";
    ToggleTrust => "toggle_trust", "Trusts the workspace, so its agents load the project's configuration, or takes the trust back (E21).";
    Close => "close", "Closes the pane, the tab with its last pane.";
    CloseTab => "close_tab", "Closes the tab with all its panes.";
    CloseWorkspace => "close_workspace", "Removes the workspace with all its tabs; the Terminal stays.";
    CloseWindow => "close_window", "Closes the window.";
    SplitRight => "split_right", "Splits the pane, the new one to the right.";
    SplitDown => "split_down", "Splits the pane, the new one below.";
    ClosePane => "close_pane", "Closes the pane.";
    ZoomPane => "zoom_pane", "Lets the pane fill the tab, or ends that.";
    EqualizePanes => "equalize_panes", "Gives the panes of the tab equal shares.";
    ResizeLeft => "resize_left", "Moves the divider beside the pane to the left (F43).";
    ResizeRight => "resize_right", "Moves the divider beside the pane to the right (F43).";
    ResizeUp => "resize_up", "Moves the divider beside the pane up (F43).";
    ResizeDown => "resize_down", "Moves the divider beside the pane down (F43).";
    NextPane => "next_pane", "Moves to the next pane.";
    PreviousPane => "previous_pane", "Moves to the previous pane.";
    NextTab => "next_tab", "Shows the next tab.";
    PreviousTab => "previous_tab", "Shows the previous tab.";
    NextWorkspace => "next_workspace", "Shows the next workspace.";
    PreviousWorkspace => "previous_workspace", "Shows the previous workspace.";
    TabSwitcher => "tab_switcher", "Finds a tab across all windows by its name (F57).";
    CommandPalette => "command_palette", "Lists every action to find one by its name and run it (F102).";
    Copy => "copy", "Copies the selection.";
    CopyFormatted => "copy_formatted", "Copies the selection with its colours as RTF and HTML (F114).";
    Paste => "paste", "Pastes.";
    SelectAll => "select_all", "Selects the whole history.";
    Clear => "clear", "Clears the screen and the history (F218).";
    Find => "find", "Searches the history.";
    FindNext => "find_next", "Goes to the next match.";
    FindPrevious => "find_previous", "Goes to the previous match.";
    PreviousPrompt => "previous_prompt", "Scrolls to the previous prompt (F120).";
    NextPrompt => "next_prompt", "Scrolls to the next prompt (F120).";
    SelectBlock => "select_block", "Selects the current command with its output (F78).";
    CopyOutput => "copy_output", "Copies the output of the current command (F81).";
    SaveOutput => "save_output", "Saves the output of the current command as a file (F81).";
    ToggleBlock => "toggle_block", "Collapses the output of the current command, or shows it again (F78).";
    ToggleBookmark => "toggle_bookmark", "Sets a bookmark on the current line, or removes it (F87).";
    PreviousBookmark => "previous_bookmark", "Scrolls to the previous bookmark (F87).";
    NextBookmark => "next_bookmark", "Scrolls to the next bookmark (F87).";
    ScrollTop => "scroll_top", "Scrolls to the oldest line.";
    ScrollBottom => "scroll_bottom", "Scrolls back to the live screen.";
    PageUp => "page_up", "Scrolls a screen towards older output.";
    PageDown => "page_down", "Scrolls a screen towards newer output.";
    FontBigger => "font_bigger", "Makes the text bigger.";
    FontSmaller => "font_smaller", "Makes the text smaller.";
    FontReset => "font_reset", "Sets the text to its size in the settings.";
    ToggleInputField => "toggle_input_field", "Turns the input field of the session on or off (F209).";
    ToggleSidebar => "toggle_sidebar", "Shows or hides the sidebar of workspaces.";
    ToggleFiles => "toggle_files", "Opens or closes the workspace's file list in its sidebar entry (E22).";
    ToggleGit => "toggle_git", "Opens or closes the workspace's Git panel in its sidebar entry (E22).";
    ToggleDictation => "toggle_dictation", "Starts a dictation through Lowspeak into the agent's input or the editor with the focus, or ends it.";
    CopyMode => "copy_mode", "Moves over the history with Vim's keys to select and copy (F111).";
    QuickTerminal => "quick_terminal", "Shows or hides the quick terminal (F45).";
    Settings => "settings", "Opens the settings.";
    ReloadConfig => "reload_config", "Reads the settings file again.";
    SecureKeyboardEntry => "secure_keyboard_entry", "Keeps other apps from reading the keys typed (F60).";
    Tab1 => "tab_1", "Shows the first tab.";
    Tab2 => "tab_2", "Shows the second tab.";
    Tab3 => "tab_3", "Shows the third tab.";
    Tab4 => "tab_4", "Shows the fourth tab.";
    Tab5 => "tab_5", "Shows the fifth tab.";
    Tab6 => "tab_6", "Shows the sixth tab.";
    Tab7 => "tab_7", "Shows the seventh tab.";
    Tab8 => "tab_8", "Shows the eighth tab.";
    LastTab => "last_tab", "Shows the last tab.";
}

/// A chord and the action it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// The chord.
    pub chord: Chord,
    /// The action.
    pub action: Action,
}

/// The chords and actions of `default_bindings`.
const DEFAULTS: &[(&str, Action)] = &[
    ("cmd+n", Action::NewWindow),
    ("cmd+shift+n", Action::NewWorkspace),
    ("cmd+t", Action::NewTab),
    ("cmd+shift+b", Action::NewBrowserTab),
    ("cmd+alt+n", Action::NewFile),
    ("cmd+o", Action::OpenFile),
    ("cmd+s", Action::Save),
    ("cmd+shift+s", Action::SaveAs),
    ("cmd+w", Action::Close),
    ("cmd+alt+w", Action::CloseTab),
    ("cmd+shift+w", Action::CloseWindow),
    ("ctrl+cmd+t", Action::SplitRight),
    ("ctrl+alt+cmd+t", Action::SplitDown),
    ("cmd+d", Action::SplitRight),
    ("cmd+shift+d", Action::SplitDown),
    ("ctrl+shift+cmd+w", Action::ClosePane),
    ("ctrl+shift+cmd+enter", Action::ZoomPane),
    ("ctrl+cmd+=", Action::EqualizePanes),
    ("ctrl+cmd+left", Action::ResizeLeft),
    ("ctrl+cmd+right", Action::ResizeRight),
    ("ctrl+cmd+up", Action::ResizeUp),
    ("ctrl+cmd+down", Action::ResizeDown),
    ("cmd+]", Action::NextPane),
    ("cmd+[", Action::PreviousPane),
    ("cmd+shift+]", Action::NextTab),
    ("cmd+shift+[", Action::PreviousTab),
    ("ctrl+cmd+]", Action::NextWorkspace),
    ("ctrl+cmd+[", Action::PreviousWorkspace),
    ("cmd+p", Action::TabSwitcher),
    ("cmd+shift+p", Action::CommandPalette),
    ("cmd+c", Action::Copy),
    ("shift+alt+cmd+c", Action::CopyFormatted),
    ("cmd+v", Action::Paste),
    ("cmd+a", Action::SelectAll),
    ("cmd+k", Action::Clear),
    ("cmd+f", Action::Find),
    ("cmd+g", Action::FindNext),
    ("cmd+shift+g", Action::FindPrevious),
    ("cmd+up", Action::PreviousPrompt),
    ("cmd+down", Action::NextPrompt),
    ("cmd+shift+a", Action::SelectBlock),
    ("cmd+alt+c", Action::CopyOutput),
    ("cmd+alt+s", Action::SaveOutput),
    ("cmd+shift+enter", Action::ToggleBlock),
    ("cmd+shift+m", Action::ToggleBookmark),
    ("cmd+shift+up", Action::PreviousBookmark),
    ("cmd+shift+down", Action::NextBookmark),
    ("cmd+home", Action::ScrollTop),
    ("cmd+end", Action::ScrollBottom),
    ("cmd+pageup", Action::PageUp),
    ("cmd+pagedown", Action::PageDown),
    ("cmd+=", Action::FontBigger),
    ("cmd++", Action::FontBigger),
    ("cmd+-", Action::FontSmaller),
    ("cmd+0", Action::FontReset),
    ("cmd+i", Action::ToggleInputField),
    ("ctrl+cmd+s", Action::ToggleSidebar),
    ("cmd+shift+c", Action::CopyMode),
    ("ctrl+alt+d", Action::ToggleDictation),
    ("ctrl+alt+cmd+space", Action::QuickTerminal),
    ("cmd+,", Action::Settings),
    ("cmd+shift+,", Action::ReloadConfig),
    ("cmd+1", Action::Tab1),
    ("cmd+2", Action::Tab2),
    ("cmd+3", Action::Tab3),
    ("cmd+4", Action::Tab4),
    ("cmd+5", Action::Tab5),
    ("cmd+6", Action::Tab6),
    ("cmd+7", Action::Tab7),
    ("cmd+8", Action::Tab8),
    ("cmd+9", Action::LastTab),
];

/// The bindings before the user changed any, as in Terminal.app and Ghostty; splits as in Dango (E12).
pub fn default_bindings() -> Vec<Binding> {
    DEFAULTS
        .iter()
        .filter_map(|(chord, action)| {
            Some(Binding {
                chord: chord.parse().ok()?,
                action: *action,
            })
        })
        .collect()
}

/// The defaults with the user's `changes` over them: a chord bound anew replaces its default, an empty action
/// unbinds it.
pub(crate) fn merged(changes: &BTreeMap<String, String>) -> (Vec<Binding>, Vec<Warning>) {
    let mut warnings = Vec::new();
    let mut bindings = default_bindings();
    for (chord, action) in changes {
        let chord: Chord = match chord.parse() {
            Ok(chord) => chord,
            Err(error) => {
                warnings.push(Warning(format!("keys: {error}")));
                continue;
            }
        };
        bindings.retain(|binding| binding.chord != chord);
        if action.is_empty() {
            continue;
        }
        match Action::named(action) {
            Some(action) => bindings.push(Binding { chord, action }),
            None => warnings.push(Warning(format!("keys: no action `{action}`"))),
        }
    }
    (bindings, warnings)
}

/// Chords bound to more than one action, each with its actions, for the settings window to mark.
pub fn conflicts(bindings: &[Binding]) -> Vec<(Chord, Vec<Action>)> {
    let mut by_chord: BTreeMap<&Chord, Vec<Action>> = BTreeMap::new();
    for binding in bindings {
        by_chord.entry(&binding.chord).or_default().push(binding.action);
    }
    by_chord
        .into_iter()
        .filter(|(_, actions)| actions.len() > 1)
        .map(|(chord, actions)| (chord.clone(), actions))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_read_in_any_order_and_write_in_one() {
        let chord: Chord = "shift+cmd+T".parse().unwrap();
        assert_eq!(chord.to_string(), "shift+cmd+t");
        assert_eq!("cmd++".parse::<Chord>().unwrap().key, "+");
        assert_eq!(
            "ctrl+alt+cmd+enter".parse::<Chord>().unwrap().to_string(),
            "ctrl+alt+cmd+enter"
        );
        assert!(matches!("cmd+".parse::<Chord>(), Err(KeyError::NoKey(_))));
        assert!(matches!("hyper+x".parse::<Chord>(), Err(KeyError::Unknown(_))));
        assert!(matches!("cmd+banana".parse::<Chord>(), Err(KeyError::Unknown(_))));
    }

    #[test]
    fn every_default_reads_and_none_conflicts() {
        let unread: Vec<&str> = DEFAULTS
            .iter()
            .map(|(chord, _)| *chord)
            .filter(|chord| chord.parse::<Chord>().is_err())
            .collect();
        assert!(unread.is_empty(), "{unread:?}");
        assert!(conflicts(&default_bindings()).is_empty());
    }

    #[test]
    fn every_action_has_a_name_that_names_it() {
        for action in Action::ALL {
            assert_eq!(Action::named(action.name()), Some(*action));
            assert!(action.description().ends_with('.'), "{action:?}");
        }
    }

    #[test]
    fn the_users_changes_replace_and_unbind_defaults() {
        let mut changes = BTreeMap::new();
        changes.insert("cmd+d".to_string(), "split_down".to_string());
        changes.insert("cmd+k".to_string(), String::new());
        changes.insert("cmd+j".to_string(), "fly".to_string());
        let (bindings, warnings) = merged(&changes);
        let of = |chord: &str| {
            let chord: Chord = chord.parse().unwrap();
            bindings.iter().find(|b| b.chord == chord).map(|b| b.action)
        };
        assert_eq!(of("cmd+d"), Some(Action::SplitDown));
        assert_eq!(of("cmd+k"), None);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn a_chord_bound_twice_is_a_conflict() {
        let mut bindings = default_bindings();
        bindings.push(Binding {
            chord: "cmd+t".parse().unwrap(),
            action: Action::Find,
        });
        let found = conflicts(&bindings);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1, [Action::NewTab, Action::Find]);
    }
}
