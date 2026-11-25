Tapa 1.0.3 opens workspaces on other computers over SSH and suggests commands as you type.

## New
- Remote workspaces: New Workspace offers a folder on another computer: a host from `~/.ssh/config` or an address, port, user and password where needed, and the folder there. Its terminals and the agents installed there run on that computer through one SSH connection. A password can be kept in the keychain, and an unknown host key is never accepted without confirmation. The file list, Git panel, editor and previews are not available there.
- The sidebar shows a remote workspace's connection with Reconnect, Disconnect and Edit Connection. When a lost connection comes back, terminals start again and agent tabs continue their conversation.
- Suggestions as you type: from the second character on, the rest of a suggested command appears faintly after the cursor, and → takes it. Tab takes a certain suggestion or opens a completion menu that describes each candidate, with or without the input field.
- Command colours: commands, subcommands, options, variables and operators take the theme's colours, and a command the shell cannot run gets a dashed red underline. Settings › Input turns suggestions and colours off.
- In an agent tab, the buttons for subagents and background processes and /tasks open an activity view with each item's status and running time, with Show and Stop.

## Improvements
- Settings › General › Window can turn off renaming a workspace by clicking its name.
- The sidebar shows the number of changed files, the Git button and the folder button in this order, and the Git button only inside a Git repository.
