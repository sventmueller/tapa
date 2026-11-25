Tapa 1.0.2 uses less memory when tabs are in the background.

## Improvements
- Hidden terminal tabs release their Metal buffers, and hidden agent tabs release their rendered conversation views. Terminal sessions, scrollback, and conversations are preserved.
- Tab switching, scrolling, and resizing do less work on the main thread. Restoring a workspace also keeps the interface responsive.
- Claude and Codex handle queued requests and replies more reliably.
