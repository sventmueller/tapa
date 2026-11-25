Tapa 1.0.0 is the first release as a disk image: open `Tapa-1.0.0.dmg` and drag Tapa onto the Applications folder. Tapa 0.1.0 updates itself to it, and Homebrew installs the same image.

## New
- Closing Tapa's last window quits Tapa and keeps everything: workspaces, tabs, splits, history and agent sessions come back at the next start as you left them. Tapa asks first only when a program other than a shell still runs.
- Drag workspaces in the sidebar into any order; the Terminal always stays at the top.
- A new window opens with the size and sidebar width of the window you last resized, also when Tapa restores no windows, and Tapa remembers whether the input field is shown.
- When Tapa starts with the sidebar collapsed, the New Workspace button stays hidden.
- Output a program writes in a burst shows once the program pauses, at most 8 ms later, so half-drawn screens no longer flash.
- Every session runs on Tapa's own terminal core; the setting that switched to Alacritty's core is gone.
- Folder and branch above the input field are plain grey text, aligned with the placeholder.
- In an agent tab, /resume lists the agent's past sessions in this folder and continues one in a tab of its own.
- A long list in an agent tab's overlay scrolls instead of making the window taller.
- An agent counts as finished only after three idle seconds, so its status no longer flickers between steps.
- In full screen the sidebar stays below the toolbar and slides down with it when the menu bar shows.

## Tapa at a glance
- Workspaces in a sidebar, each with its tabs, splits, agents, editors and browser tabs; windows and agent sessions come back after a restart.
- Agent tabs for Claude, Codex, OpenCode and any ACP agent from the registry, installed, updated and rolled back from the settings.
- Files and Git panels, Markdown and picture previews, a command input field with dictation.
- Light, dark or system appearance, four font sizes, 35 interface languages.
