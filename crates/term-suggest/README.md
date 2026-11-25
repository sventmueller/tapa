# term-suggest

Command suggestions without network (E8), in two stages.

The instant stage answers every key within 2 ms (PLAN section 3; `tests/latency.rs`, checked with `cargo test --release -p term-suggest --test latency`). It draws on four sources:

- the history: every command the sessions ran, with folder, branch, the command before and the exit code, in SQLite at `history.sqlite` in the data folder (A5) and in memory, ranked by frequency and recency, weighed up in the same folder, branch and after the same command;
- the subcommands, options and values of the Fig completion specs (F93), for specs already parsed;
- file and folder names where a spec's argument takes them;
- `next_command`, what followed the previous command (F210).

The delayed stage runs in a pool of two threads with a 500 ms limit, a cache of a few seconds and a generation counter per asker; stale or late answers are dropped. It does three things:

- It parses specs the instant stage did not have yet.
- It asks the dynamic sources, which file reads cover except where only a program knows:
  - git branches and remotes from `.git`;
  - `package.json` scripts, Makefile targets and justfile recipes;
  - Cargo packages and binaries;
  - ssh hosts from `~/.ssh/config` and `known_hosts`;
  - docker and podman containers and images, and kubectl resources, from those programs found in absolute `PATH` folders outside the project (E16).
- It works out the next command without input: a typo fix or a `brew install` from Homebrew's local `executables.txt` after "command not found", and an install when a lockfile is newer than what the last install left.

## Fig specs

`specs/fig.tsv` holds the static part of [withfig/autocomplete](https://github.com/withfig/autocomplete) (MIT, `specs/LICENSE-fig`) at commit `aef52acff84c45edde61ae610cc2c964802b9a38`. It has one line per spec, `name<TAB>json`, sorted by name so a spec is found by bisecting without reading the others. The data is bundled with `include_str!`; nothing is fetched at runtime.

The file is converted by hand, not in the build (R17):

```sh
git clone --depth 1 https://github.com/withfig/autocomplete /tmp/fig
cargo run --release --manifest-path xtask/specs/Cargo.toml -- /tmp/fig
```

The converter in `xtask/specs` parses the TypeScript with oxc and evaluates only literals, top-level constants and relative imports; nothing of the checkout runs. Functions and generators drop out, and a generator's name or script is kept under `dynamic` for the dynamic sources to recognise (R10). Three things are left out:

- the per-service sub-specs of aws, gcloud and az, 67 of 78 MB;
- hidden entries;
- descriptions beyond their first sentence, at most 100 characters.
