# Changelog

## Unreleased

- Releases are git-based (tag + GitHub release), not published to
  crates.io: `publish = false` in Cargo.toml, and `docs/RELEASING.md`
  reworked accordingly (the crates.io section replaced by a downstream
  rush harness check).
- Menu cycling: Tab after the candidate list walks the candidates
  in-line, wrapping around (zsh `AUTO_MENU`, readline `menu-complete`) —
  revisiting a narrowing; the paging/menu-select UI stays declined.
- Resize while idle at the prompt now repaints within a poll tick
  (~200ms), without a SIGWINCH handler. The idle tick also calls
  `Hooks::on_interrupted_read`, so pending host traps fire promptly even
  with no input arriving.
- History dedup option: `Editor::set_history_dedup` erases earlier
  duplicates on add (bash `HISTCONTROL=erasedups`, fish). Off by default.
- Revert line: M-r undoes every edit to the line at once (readline
  `revert-line`).
- New `examples/hooked.rs` (completion + hints demo) used by new pty
  tests for menu cycling, hint acceptance, and idle-resize repaint.
- `docs/RELEASING.md`: release checklist (crates.io blockers, MSRV sync
  points, tag/publish steps).
- History cap: `Editor::set_max_history_len` (readline `stifle_history`,
  bash `HISTSIZE`); oldest entries drop past the limit.
- Append-only history persistence: `Editor::append_history` writes only
  entries added since the last load/save/append (bash `histappend`), so
  concurrent sessions interleave instead of overwriting. `save_history`
  now takes `&mut self` to track what's persisted.
- Partial hint acceptance: M-f / Ctrl-Right at end of line accepts one
  word of the history hint (fish's forward-word on an autosuggestion).
- Documented existing C-l clear-screen in the feature matrix; recorded
  positions on eager resize repaint and grapheme-cluster width math as
  deliberate narrowings.
- `docs/survey.md`: the per-editor line-editor field survey behind the
  README's feature matrix (reconstructed).
- End-to-end pty test suite (`tests/pty.rs`): drives `examples/demo` under
  a pseudo-terminal — typing/echo, emacs editing, kill/yank, history
  recall, Ctrl-C, and bracketed paste.
- CI (build/test on Linux, macOS, Windows; clippy, rustfmt, docs, MSRV),
  dependabot, `rust-version = "1.88"`, API docs for all public items with
  `#![warn(missing_docs)]`, and a compiled crate-level usage example.

## 0.1.0

- Initial import: the line editor extracted from the
  [rush shell](https://github.com/baileyrd/rush). Emacs + vi keymaps,
  kill ring, undo, incremental and prefix history search, bracketed
  paste, completion/hint/highlight/abbreviation hooks, right-side
  prompt, wide-char and ANSI-aware rendering.
