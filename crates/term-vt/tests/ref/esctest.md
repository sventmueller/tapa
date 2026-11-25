# esctest and vttest

Third-party test programs run in the app (E11 point 4; E4's gate asked for at least Alacritty's esctest pass rate). The runs below compared `term-vt` with `alacritty_terminal`, which the app could still choose then; `make esctest` repeats the esctest run for `term-vt`, vttest's comparison went with Alacritty. The programs live in `~/.cache/tapa-tools`, not in this repository.

## esctest

Run of 2026-11-23, esctest2 at `2798f12149a19c3295e9b4853ab2da4b2eff1b2b`, flags `--expected-terminal=xterm --max-vt-level=5 --xterm-checksum=334 --xterm-reverse-wrap=383 --timeout=1` (a recent xterm: empty cells count as blanks in checksums, reverse wrap is mode 1045).

| Engine | Passed | Failed | Known bugs |
|---|---|---|---|
| `tapa` (`term-vt`) | 262 | 262 | 43 |
| `alacritty` | 121 | 403 | 43 |

No test fails on `tapa` and passes on `alacritty`. The remaining failures are features neither engine has: xterm window operations (28), mode reports for modes that do not exist here (DECRQM 25, DECSET 15), selective erase (DECSED, DECSEL, DECSERA), DECRQSS, the rectangle operations (DECCRA, DECFRA, DECERA), colour queries and changes (OSC 4/10–19/104–119 reports), left and right margins (DECLRMM, behind DECIC, DECDC, DECBI, DECFI and the margin cases of BS, CR, CUB, CNL, CPL, SU, SD, IL, DL, ICH, DCH), DECSCL and titeInhibit.

## vttest

Run of 2026-11-23, vttest 2.7 (20251205), menus 1 (cursor movements), 2 (screen features), 4 (double-sized characters) and 8 (VT102 features) on an 80 × 24 window, 58 screens compared as text between the engines (`make vttest`; the texts land in `target/vttest`).

| Screen | Judgement |
|---|---|
| Menu 2, screens 7 and 9 (soft and jump scroll in region 12..13) | `tapa` is right: vttest writes in origin mode inside the two-line region only, and `tapa` keeps all text there; `alacritty` shows text on line 1, outside the region. |
| Menu 8, screens 11 and 12 (delete character, 132-column round) | Neither engine switches to 132 columns, so vttest's cursor positions past column 80 land on the last column. There DCH deletes that one cell in `tapa`, as in xterm; `alacritty` also blanks cells left of the cursor (deviations.md), which happens to draw the stagger vttest asks for. |

The other 54 screens are the same in both engines.
