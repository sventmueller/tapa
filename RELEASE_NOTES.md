Tapa 1.0.6 guards against Markdown built to exhaust memory and keeps a program's last output in the terminal.

## Fixed
- A Markdown document with hundreds of thousands of empty table cells, in a preview or an agent's reply, no longer takes seconds and gigabytes of memory: beyond 500,000 empty cells its tables show as text.
- A terminal no longer misses the end of its program or loses the program's last output when the Mac is busy.

## Improvements
- The agent timeline leaves out hooks that ran fine or are still running; a hook that failed still shows.
