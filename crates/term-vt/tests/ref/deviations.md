# Deviations from Alacritty

Each line names what `term-vt` does instead of `alacritty_terminal`, and why (R15). None of the 45 reference cases is affected so far.

- LF, VT and FF with LNM (mode 20) set also return the carriage, as xterm and the VT100 manual have it; Alacritty ignores LNM for them.
- Erasing clears every cell it covers. Alacritty skips cells of a row it erased before when the row's last cell already has the erasing colour, which after a resize leaves an old background behind and moves what clearing the screen pushes into the history; the resize fuzzing generates no background colours for that reason.
- An OSC keeps all its parts; `vte` stops splitting after 16, so a title or link with more semicolons loses its tail there.
- An OSC string is cut after 64 KiB (`MAX_OSC`); `vte` with `std` keeps any length, so an OSC 52 copy beyond that reaches the clipboard whole only in Alacritty.
- The 32-parameter limit counts parameters, not their subparameters; `vte` counts both, so a long run of `:`-subparameters drops attributes there that apply here.
- ESC \ after an OSC or DCS is taken whole as ST; `vte` also hands the `\` on as an escape sequence, which Alacritty ignores, so nothing shows the difference.
- A search match that starts on a wide character pushed to the next row starts on that character, not on the leading spacer at the end of the row above it, which holds nothing to highlight (`tests/search.rs`, from Alacritty's `leading_spacer`).
- Search uses the `regex` crate's leftmost-first matches and drops empty ones, so an alternative that may be empty and comes first (`;*|rust`) hides a longer one at the same place; Alacritty's DFA finds `rust` there (`anchored_empty`).
- A selection is anchored in global line numbers, so scrolling inside a scroll region leaves it on its rows of the screen instead of moving and clipping it with the scrolled text as Alacritty's `Selection::rotate` does (`rotate_in_region_*`, not ported).
- Styles live in a table of at most 65 536 per terminal; when more are in use at once than fit even after unused ones are dropped, new ones are drawn in the default style until a screen of rows was written anew. Alacritty keeps the colours in each cell and has no such limit (`tests/history.rs`).
- As in xterm and Ghostty: IL and DL return the cursor to the left margin; LF, IND, RI, VT, FF, EL, ED, ECH, DCH, ICH, IL and DL end a pending wrap, and EL 0 there clears the last cell. Alacritty keeps the column and the pending wrap, so after a full line `ESC [ K` and a character wrap there (`erase_in_line`, whose `expected.own` holds the screen with this).
- ED 1 clears every line above the cursor; Alacritty skips the first line when the cursor is on the second (`cursor.line > 1`).
- DECSTBM clamps its bottom to the screen and ignores a region that is then empty, as xterm and Ghostty do; Alacritty sets it and homes the cursor.
- DECALN also resets the margins, origin mode and the rendition and homes the cursor, as xterm and Ghostty do (`decaln_reset`, whose `expected.own` holds the screen with this).
- CBT with no tab stop left of the cursor goes to the first column, as xterm's `TabPrev` and Ghostty do; Alacritty stays.
- Leaving the alternate screen with mode 1049 restores the saved cursor even when the main screen already shows, as xterm does; Alacritty then does nothing.
- HT with a pending wrap tabs from where the cursor is and keeps the pending wrap, as xterm (without its curses option) and Ghostty do; Alacritty wraps first.
- ICH, DCH, ECH, EL and ED that take one half of a wide character away blank its other half, as xterm (`DamagedCurCells`) and Ghostty do; Alacritty leaves it, a wide character without its spacer or a spacer without its character.
- Resetting origin mode (DECOM) homes the cursor as setting it does, as in xterm and Ghostty; Alacritty homes it only when setting.
- DCH deletes at most the characters from the cursor to the end of the line; Alacritty, asked for more than that, also blanks the ones left of the cursor.
- Insert mode that pushes a wide character over the line's end blanks its remaining half, and inserting a wide character keeps it whole, as in xterm and Ghostty.
- CUU and CUD stop at the scroll region's edge when they start inside it, as xterm's `CursorUp`/`CursorDown` do; Alacritty goes on to the screen's edge. CHA, HPA and VPR in origin mode count from the region's top once, where Alacritty adds it twice.
- Asking for the alternate screen (1049) while it shows saves the cursor and clears it again, as xterm does; Alacritty ignores it.
- Writing over one half of a wide character blanks the other half in the current background, as xterm and Ghostty do; Alacritty keeps that half's style.
- DECSC and SCOSC save origin mode with the cursor and DECRC and SCORC restore it, as xterm does; Alacritty leaves the mode as it is.
- BS ends a pending wrap also in the first column, as xterm's `CursorBack` does; Alacritty only when it moves. With autowrap off a wide character that does not fit at the line's end is dropped without leaving a pending wrap behind, as in Ghostty.
