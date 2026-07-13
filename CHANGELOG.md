# Changelog

## Unreleased

- Key rebinding API (revisiting a narrowing; rush's `bind` builtin):
  the emacs/vi-insert commands are now the public `EditorAction` enum,
  and `Editor::bind`/`unbind`/`bindings` remap single keys using
  readline's key-spec spellings (`\C-x`, `\M-f`, `\e[1;5C`, backslash
  escapes). `unbind` masks a default (readline `bind -r`); `bindings()`
  lists the effective keymap (`bind -P`). Multi-key chords, vi normal
  mode, and `.inputrc` parsing stay out of scope.
- Host-command bindings — bash's `bind -x`: `Editor::bind_host` tags a
  key; pressing it suspends raw mode and calls the new
  `Hooks::host_binding(tag, &mut line, &mut cursor)`
  (`READLINE_LINE`/`READLINE_POINT` contract), then repaints.
- Readline variables: `set_completion_ignore_case`,
  `set_show_all_if_ambiguous`, `set_menu_complete` (Tab becomes
  readline's `menu-complete`), and `set_bell_style` (`BellStyle`;
  audible by default like readline — the editor now rings on completion
  with no candidates, so hosts wanting the old silence set
  `BellStyle::None`).
- Read deadline (bash `$TMOUT`): `Editor::read_line_timeout` and the
  new `ReadResult::TimedOut` variant. The deadline is measured from the
  call and checked on the idle poll tick and between keystrokes; hosts
  matching on `ReadResult` exhaustively need a new arm.
- History timestamps (bash `HISTTIMEFORMAT` file format): entries are
  stamped on add; `load_history` parses `#<epoch>` comment lines (plain
  files still load, both formats round-trip); `save_history`/
  `append_history` emit them only under `set_history_timestamps(true)`,
  so existing plain files are not rewritten unasked;
  `history_timestamps()` exposes the stamps for the host's `history`
  builtin.
- In-place history replacement: `Editor::replace_history` resyncs the
  editor's list after a host's `history -c`/`-d` without rebuilding the
  editor (kill ring and session state survive); replaced entries count
  as persisted, so `append_history` stays incremental.
- Terminal facilities: public `terminal_size()` — (cols, rows) from
  stdout, for `$COLUMNS`/`$LINES` (bash `checkwinsize`) — and
  `with_echo_disabled(f)` — panic-safe echo-off around a closure, the
  termios replacement for shelling out to `stty` in `read -s`.
- New `examples/timeout.rs`; pty tests for the timeout, a rebound key,
  and a host binding; `examples/hooked.rs` grew a rebinding and a host
  binding to drive them.
- Fix: the non-Unix `read_line_timeout` fallback printed the prompt
  unconditionally, unlike the Unix path (which suppresses it for a
  non-tty stdin, falling back to `read_line_plain`). A script piped into
  an "interactive" host on Windows — rush's own `-i` test harness, for
  one — got prompt text mixed into captured stdout. Now gated on
  `IsTerminal::is_terminal`, matching the Unix behavior.

## 0.2.0 — 2026-07-11

- The terminal syscall surface (termios/poll/read/winsize) moved into
  `src/term_sys.rs` with two backends: the hand-rolled `rusty_libc`
  raw-syscall crate — now the default on Linux, linking no third-party
  libc bindings — and the `libc` crate (other Unix; on Linux via
  `--no-default-features --features libc-backend`). CI exercises both.
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
