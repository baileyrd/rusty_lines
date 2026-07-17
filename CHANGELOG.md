# Changelog

## Unreleased

- Operate-and-get-next (readline, bash C-o): `EditorAction::OperateAndGetNext`
  accepts the line and pre-loads the next `read_line` with the history
  entry after the one just executed, for replaying command sequences.
- New named actions: `KillWholeLine` (unbound by default, like readline)
  and `DeleteHorizontalSpace` (M-\; a delete, not a kill — readline's
  spaces-and-tabs-around-point rule).
- vi mode additions (vim semantics; readline `vi-fetch-history` for `G`):
  `%` matching-bracket motion (inclusive under operators, both
  directions), `G` fetches the most recent history entry — or entry N
  with a count — and the `iw`/`aw` word text objects for `d`/`c`/`y`
  (`diw`, `caw`, `yiw` …).
- Mode indicator: `Editor::set_show_mode_in_prompt` prefixes readline's
  default mode strings — `(ins)`/`(cmd)` in vi mode, `@` in emacs mode —
  to the prompt (readline `show-mode-in-prompt`).
- Completion listing matches readline more closely: candidates are
  sorted and laid out column-major, and a list of
  `completion-query-items` or more (default 100; 0 disables;
  `set_completion_query_items`) asks `Display all N possibilities?
  (y or n)` before printing.
- Incremental search failure feedback: a query with no match shows
  readline's `(failed reverse-i-search)` label, keeps the last match
  visible, and rings the bell. The bell (still governed by
  `set_bell_style`) now also rings on history motion past either end,
  a failed prefix search, and a failed vi `f F t T ; ,` find.
- History ignore-space option: `Editor::set_history_ignore_space` makes
  `add_history_entry` skip lines starting with a space (bash
  `HISTCONTROL=ignorespace`).
- Kill-ring and undo depths are configurable (`set_max_kill_ring_len`,
  `set_max_undo_len`; defaults unchanged at 32/200), and both now evict
  in O(1) (`VecDeque`) instead of shifting the whole buffer per
  keystroke once full.
- `Hooks::hint` is now called at most once per keystroke (memoized on
  the buffer content) instead of up to twice — render and the
  Right/End/M-f accept paths share the cached value.
- Fix: a bracketed paste in vi *normal* mode was silently discarded;
  it now inserts literally at the cursor (vim, readline vi mode).
- Fix: Ctrl-Space and Ctrl-\ / Ctrl-] / Ctrl-^ self-inserted raw
  NUL/FS/GS/RS bytes into the buffer; they now decode as `\C-@`,
  `\C-\\`, `\C-]`, `\C-^` — unbound by default but rebindable — and
  `key_spec` escapes backslashes so such bindings round-trip.
- Fix: the C-x C-e scratch file had a predictable name in a shared
  `$TMPDIR` (symlink-attack window) and default permissions; it is now
  created `O_EXCL` with an unpredictable name and mode 0600, and the
  path is shell-quoted so a `$TMPDIR` with spaces works.
- Fix: a stray UTF-8 continuation byte swallowed the next three
  keystrokes as "continuations"; invalid lead bytes now become U+FFFD
  immediately, and continuation reads are time-bounded.
- Fix: the `read_line_timeout` deadline is now honored inside C-x
  chords and quoted-insert (which block on their own follow-up key),
  and half-delivered escape sequences / unterminated pastes give up
  after a bounded wait instead of hanging the read.
- Fix: the piped-stdin fallback (`read_line_plain`) left a trailing
  `\r` on CRLF input; it now strips it, matching the non-Unix fallback.
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
