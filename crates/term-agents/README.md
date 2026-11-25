# term-agents

Copied from Dango at commit `041f7ca1995c632beed490f033756e12642b65a3`: ide-session without the own agent, the work orders (`Order`, `Role`, `Contract`, `OrderRule`), the checkpoints (`Checkpoint` and `CommitId` are types without a value) and Codex's shared server; `script.rs` and the program `session-stand-in` are behind the feature `selftest` (E2, E21). `Launch::trusted` carries the workspace's trust to every path (E16). P7 brings later changes of either side over; until then this copy is the one that changes here.


One driver over every agent session: Claude, Codex, OpenCode and ACP agents. `Registry::start` starts a `Launch` under a `SessionId` the app keeps in its context; the `Driver` sends prompts, answers requests by the `RequestRule` it is handed, fans the events out to any number of subscribers and ends every turn with a typed `TurnEnd`.

```mermaid
flowchart TB
    launch["Launch: path, folder, environment, model, resume, choices, trusted"] -->|"Registry::start"| start["start: the choices and the trust in the path's terms, or LaunchError"]
    start --> session["one of the four sessions, or the scripted one"]
    session -->|"events on its own threads"| inbox["inbox: control events in order, invalidations coalesced"]
    inbox -->|"Wake: the app's hop to the main thread"| driver["Driver::flush on the main thread"]
    driver -->|"Driver::send"| session
    driver -->|"each request once"| rule["request rule"]
    rule -->|"allow, deny with a reason"| session
    rule -->|"left to the user: Signal::Waiting"| feeds
    driver -->|"gap in the stream: resync, or NeedsPause"| session
    driver -->|"signals and coalesced changes"| feeds["one feed per subscriber"]
    feeds -->|"Driver::take: Batch, a redraw after a backlog"| views["views: agent tabs"]
    driver -->|"Signal::TurnEnded: succeeded, failed, cancelled, connection lost"| views
```

The API is documented in [`src/lib.rs`](src/lib.rs).
