# term-ui-files

Copied from Dango at commit `041f7ca1995c632beed490f033756e12642b65a3`: `src/window/files/{mod,view,controller,hover}.rs`, `src/window/files_header.rs`, `src/project.rs` without quick open and trust, and `ide-core/src/prewarm.rs` (E2, E22). Cut is opening a file in a pane beside or below (`pane_choice.rs`, ⌥-click): an editor is a tab of its own in Tapa, so the context menu leaves out "Daneben öffnen" and "Darunter öffnen". The list belongs to a workspace (`WorkspaceId`) instead of a window and lays out for the workspace's sidebar entry instead of Dango's activity bar; its watcher runs only while the list is built. P7 brings later changes of either side over; until then this copy is the one that changes here.

The file list of a workspace: the folder tree of its base path, read one folder at a time on the disk lane as the user opens it, with a title row for a new file, a new folder and folding every folder, and a context menu for renaming, the Trash, the Finder and copying paths. A click opens a file in the workspace's preview tab, a double click in a tab of its own; the pointer over a file reads it ahead. What the menu means is decided in `term_project::files_panel`. The FSEvents watcher (`term_project::Watcher`) refuses `/` and the home folder itself.

```mermaid
flowchart LR
    app["app: sidebar entry's folder symbol"] -->|"panel_view, detach"| list["FileTreeSource with its page"]
    list -->|"expand and collapse on the disk lane"| tree["term_project::Tree"]
    watcher["term_project::Watcher, while the list exists"] -->|"Tree::apply, main_soon by list id"| list
    watcher -->|"files_changed, git_changed"| editors["term_ui_editor"]
    watcher -->|"Host::changed"| git["the app, for the Git panel"]
    list -->|"Host::open_file, open_as_preview, open_in_browser_tab"| app
```

The API is documented in [`src/lib.rs`](src/lib.rs) and [`src/host.rs`](src/host.rs).
