# Changes against modalkit 0.0.32

This directory is a maintained copy of `modalkit` 0.0.32 as published on crates.io (git `da2e8d33`, see
`.cargo_vcs_info.json`; `.crate` SHA-256 `7b65714e623849687ac7730864ca2e5ec08265f0e4666bb51a23c8ebe604cfce`),
included through `[patch.crates-io]` in the root `Cargo.toml` (E104, R23). `Cargo.toml` is edited by hand and is the
only manifest; the generated header and `Cargo.toml.orig`, which used upstream's workspace inheritance, are gone.
Changes carry a `Dango:` comment in the code. To move to a newer upstream version, download its `.crate`, apply the
changes below again and diff the result against this list.

## Manifest (`Cargo.toml`)

- `ropey` 1.6.1 without default features, only `cr_lines` and `simd`: its default `unicode_lines` would reach
  `ide-document` through feature unification and count U+2028, U+0085, VT and FF as line breaks (E104, F1).
- `shellexpand` without default features, only `base-0`: drops `tilde`, whose `dirs` brings `option-ext`.
- `anymap2` removed (RUSTSEC-2026-0319, unmaintained; B118).
- `crossterm` optional behind the new feature `crossterm`, off by default; it brought `signal-hook` 0.3, `mio` and
  `winapi` into Dango's graph without being used (B124).

## Sources

- `src/editing/lineinfo.rs`: a private `SendSyncAnyMap` over `HashMap<TypeId, Box<dyn Any + Send + Sync>>` in place
  of `anymap2::SendSyncAnyMap` (B118).
- `src/key/event.rs` (new): `KeyCode`, `KeyEvent`, `KeyModifiers`, `MediaKeyCode` and `ModifierKeyCode`, the subset of
  `crossterm::event` that `TerminalKey` is built on, with the same variants and bit values; with the `crossterm`
  feature it converts from crossterm's types (B124).
- `src/key/mod.rs`, `src/key/parse.rs`, `src/env/mod.rs`, `src/util.rs` and the tests in `src/editing/key.rs`,
  `src/env/keyparse.rs`, `src/env/{vim,emacs,kak}/keybindings.rs`: `crate::key::event` instead of `crossterm::event`;
  the examples in the Vim and Kakoune key docs use `modalkit::key::event` (B124).
- `src/key/mod.rs`: `From<crossterm::event::KeyEvent> for TerminalKey` only with the `crossterm` feature (B124).
- `src/lib.rs`: `pub use crossterm` only with the `crossterm` feature (B124).
- `src/editing/completion.rs` and `src/env/vim/command/parse.rs`: no `~` expansion, which needs `shellexpand`'s
  `tilde`.
- `src/editing/buffer/mod.rs`: `EditBuffer::get_leader_state` is public.
- `src/editing/rope/mod.rs`: `EditRope::as_rope` for comparing a copy leaf by leaf and `From<Rope> for EditRope` for a copy that shares the editor's leaves; `aw`/`aW` take the blanks after the word, or before it when none follow (`with_blanks`); `{`, `}`, `(` and `)` are implemented and exclusive.
- `src/editing/rope/prose.rs` (new): the paragraph and sentence motions after Vim's `findpar()` and `findsent()`.
- `src/env/vim/keybindings.rs`: `aw` and `aW` pass `false` to `edit_range_end!`, which `EditRope::range` reads as
  "with the white space around".
- `src/editing/store/register.rs`: without the `clipboard` feature, `"+` and `"*` live in a register the host keeps in
  step with the system clipboard (`set_system_text`, `take_system_written`), and `unnamed_text`/`set_unnamed_text`
  read and write the unnamed register (E105, E106).
