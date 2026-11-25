# Reference tests

The 45 reference cases of Alacritty (`alacritty_terminal/tests/ref`, Apache-2.0, see `LICENSE-APACHE`), taken over at Alacritty commit `791efff6d74c66bee390c259e322626ca93aadb6` (E11). Each folder holds the original `recording` and the screen and history `alacritty_terminal` 0.26.0 ended up with, in the neutral format of `term_vt::reference` (text per line, then every cell with colours or attributes). The text of all 45 matches Alacritty's own `grid.json`.

`term-vt`'s test `reference` holds the core to these expectations. A deliberate deviation from Alacritty changes the case's `expected` and adds a line to `deviations.md` with the reason (R15). The project no longer depends on `alacritty_terminal`, so the files are not made anew.
