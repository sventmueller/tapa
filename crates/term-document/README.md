# term-document

Copied from Dango at commit `041f7ca1995c632beed490f033756e12642b65a3`: ide-document whole (E2, E22). Unlike Dango, the recovery copies are sealed with the key and scheme of the restored state from `term-seal` (E17), so no text, base or description lies on disk in plain text. P7 brings later changes of either side over; until then this copy is the one that changes here.

The document model behind every editor: the text of one file shared by the views that show it, each edit in UTF-16 units with what a language server needs to follow it, the file it was read from and is saved to, changes other programs make to that file, copies against a crash, colours, brackets and folds by lines, and the edits typing help makes. AppKit keeps the displayed text and its undo; this crate follows it. It starts no threads or processes: blocking calls are handed to the caller's lanes.

```mermaid
flowchart TB
    view["editor view, main thread"] -->|"Document::apply(Edit)"| doc["Document: Text, revision, syntax"]
    doc -->|"Change: bytes, UTF-16, points"| lsp["language server sync in the app"]
    view -->|"highlight_more in slices"| syntax["syntax: per-line lexer state, brackets, folds"]
    view -->|"the lines around the selection"| typing["typing: pairs, Return, Tab, comments"]
    typing -->|"Step for the view to make"| view
    doc -->|"begin_save: SaveJob"| job["SaveJob::run, any thread"]
    job -->|"write and stamp"| file[("the file")]
    job -->|"Saved"| saved["Document::end_save"]
    doc -->|"disk_probe: DiskProbe"| probe["DiskProbe::check, any thread"]
    probe -->|"read"| file
    probe -->|"DiskCheck"| take["Document::take_disk"]
    take -->|"merge::reconcile"| merge["reload, three-way merge or Conflict"]
    take -->|"Outcome with the Changes for the view to make"| view
    view -->|"adopt_disk after the user settled a conflict"| doc
    doc -->|"snapshot, constant time"| other["other threads"]
    copies["the app's recovery lane"] -->|"recovery::Recovery, sealed"| folder[("copies in the data folder")]
```

The API is documented in [`src/lib.rs`](src/lib.rs) and its modules.
