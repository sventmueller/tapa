# term-session

Copied from Dango at commit `01bba21a061a1ee15e24336c2bc9b3bd644b6d6c`: ide-terminal (scan.rs moved to term-vt) (E2). P7 brings later changes of either side over; until then this copy is the one that changes here.

Terminal sessions without a window: the user's shell in a PTY with its screen kept by `term-vt`, keys, mouse and paste turned into bytes, shell integration for prompts and the working directory, the program in front with coding agents recognised, and what pasting an image does. The view draws snapshots and sends input; events arrive on the session's reader thread.

```mermaid
flowchart TB
    view["terminal view, main thread"] -->|"Session::spawn(Options)"| spawn["spawn"]
    spawn -->|"pty::spawn with the login-shell environment"| pty["PTY"]
    pty --> shell(["login shell with integration scripts, or a command"])
    spawn -->|"Supervisor::register as Kind::Terminal"| supervisor["Supervisor"]
    spawn -->|"starts"| reader
    view -->|"key, mouse, paste, write: bytes over a channel, poller woken"| reader["thread terminal-reader: one poll loop"]
    reader <-->|"read output, write pending input"| pty
    reader -->|"term-vt parser"| term[("term-vt Terminal: grid and history")]
    reader -->|"scan: OSC 133, OSC 7, dev server URLs"| events["listener(Event) on the reader thread"]
    events -->|"Redraw, Title, Prompt, DevServer, Exited, ..."| view
    view -->|"Session::snapshot"| term
    view -->|"Session::foreground: process group of the PTY"| front["Foreground: agent, remote, shell, program"]
    supervisor -->|"on_stop: SIGHUP to the shell and the job in front"| shell
    reader -->|"child exited: Event::Exited, Supervisor::forget"| supervisor
```

The API is documented in [`src/lib.rs`](src/lib.rs) and its modules.
