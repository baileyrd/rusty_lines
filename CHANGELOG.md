# Changelog

## Unreleased

- Fix (raw-mode recipe): `ISTRIP` and `IGNCR` are now cleared on entry
  to raw mode. If the inherited termios had `ISTRIP` set, every UTF-8
  high byte lost its top bit (all non-ASCII input corrupted); with
  `IGNCR` set, the `\r` that Enter sends was discarded — Enter was
  dead. readline clears both.
- Fix: `poll_stdin` treated EINTR (and any poll error) as "no input",
  so a signal landing during the short polls misdecoded ESC as a lone
  Esc, abandoned half-read escape sequences, and made bounded waits
  give up early. Both backends now retry on EINTR.
- Self-healing raw mode: the 200ms idle tick verifies the terminal
  still matches the raw recipe and re-asserts it (plus a repaint on a
  fresh line) when an external SIGTSTP/`fg` or a host command's `stty`
  left the tty cooked — readline's SIGCONT re-preparation, without
  installing a signal handler (which stays the host's business).
- Raw editing now also requires *stdout* to be a terminal: with stdout
  piped (`host | tee log`) the editor used to spray repaint escape
  sequences into the pipe; it now falls back to the plain read, same
  as for a piped stdin.
- Fix: rapid double-Esc (a vi user mashing into normal mode) was
  swallowed whole — ESC followed by ESC within the 30ms window decoded
  to nothing. It now decodes as Esc.
- History round-trip: entries that *look like* timestamp comments are
  no longer eaten by `load_history`. Interactive shells store comment
  lines, so `#42` is a legitimate entry; now only epoch-scale stamps
  (nine or more digits) parse as timestamps, a stamp always pairs with
  the following entry line (even a stamp-shaped one), and a dangling
  stamp is kept as an entry. A lone epoch-scale comment followed by
  another entry remains inherently ambiguous in bash's format
  (documented on `load_history`).
- CI: every job now builds `--locked`, so the committed `Cargo.lock`
  is authoritative and a push to the `rusty_libc` git dependency's
  tracked branch can no longer change or break CI builds silently.
- Chaos tests: a seeded, deterministic byte-soup test hammers the pure
  decoders and text helpers (`parse_key_spec`, `decode_key_bytes`,
  `csi_key`, `visualize*`, `display_width`, the word/object/find
  helpers, `clamp_start`) asserting nothing panics — always-on cheap
  fuzzing with no cargo-fuzz dependency.
- Render: the buffer is now visualized once per keystroke instead of
  twice — a single marked pass measures the paint, the cursor column,
  and the total width together (`visualize_marked`).
- API polish: `ReadResult` derives `Debug`/`Clone`/`PartialEq`/`Eq`
  (hosts can `assert_eq!` on it), `Candidate` derives `Debug`/`Clone`,
  and `Editor` derives `Debug`.
- Pre-seeded lines: `Editor::read_line_with_initial(prompt, rprompt,
  hooks, (left, right))` starts the edit with text in the buffer and
  the cursor between the halves (rustyline's `readline_with_initial`,
  zsh `print -z`) — `fc`-style edit-and-rerun, offered corrections.
  Ignored on a non-tty stdin and on the non-Unix fallback.
- Command-name mapping: `EditorAction::name()` and
  `EditorAction::from_name()` expose readline's command names
  (`kill-line`, `menu-complete-backward`, …) so a host's `bind` builtin
  doesn't maintain its own drift-prone table; the exhaustive match
  forces future actions to get names.
- Host-binding introspection: `Editor::host_bindings()` lists the
  `bind_host` entries as (key spec, tag) pairs — bash's `bind -X`,
  which `bindings()` deliberately omits.
- Fix: `operate-and-get-next` (C-o) recalled the wrong entry when
  `erasedups` (or a host history edit) shifted indices between lines;
  the recall now stores the entry text and re-locates it by content.
- Fix: pasting during incremental search exited the search and
  discarded the paste; it now types into the query (bash's behavior),
  with newlines flattened to spaces.
- Incremental search starts from the current history position after an
  unedited Up-recall (readline continues backward from point in
  history) instead of restarting at the newest entry; typed characters
  search at-or-before the current match. An edited recall does not
  seed the position, so exiting an empty search can never clobber
  edits.
- Hardening: out-of-range or mid-character word-start offsets returned
  by `Hooks::complete` / `Hooks::expand_abbreviation` are clamped to a
  char boundary at or before the cursor instead of panicking the read
  loop — a hook bug must not take the host shell down.
- Fix: the piped-stdin path (`read_line_plain`) swallowed EINTR without
  calling `Hooks::on_interrupted_read`, so hosts running piped scripts
  never got their trap callback; it now fires like the raw path.
- Fix: Shift-Tab was dead in vi normal mode (it tore down the armed
  completion menu and did nothing); it now reverse-cycles there, like
  Tab completes there.
- Performance: the alphanumeric word motions no longer collect the
  line into a `Vec` per keystroke, and case-insensitive search matching
  compares case-folded char streams instead of allocating two lowercase
  `String`s per history entry per keystroke.
- Incremental search now leaves the cursor *on* the matched text when
  the search is accepted or exited (readline's point), instead of at
  end-of-line — you searched for it to edit it.
- Prefix search (PageUp/PageDown) re-anchors on the buffer up to the
  cursor whenever the previous key wasn't itself a prefix search (zsh's
  rule). Previously the anchor was captured only when off history, so a
  PageUp after a plain Up searched with a stale or empty prefix.
- Undo now chunks runs of self-insert into groups of at most 20
  characters (readline's grouping): one C-_ no longer wipes an entire
  typed line.
- Completion append character (readline's
  `rl_completion_append_character`): a unique, fully-inserted match is
  followed by a space so the cursor is ready for the next word;
  `set_completion_append_character` changes or disables it.
- `possible-completions` (M-?): list the candidates without touching
  the buffer; `insert-completions` (M-*): insert every match,
  space-separated — both stock readline commands.
- `character-search` (C-]): reads one character and moves the cursor to
  its next occurrence; `character-search-backward` is available unbound
  (its readline default M-C-] isn't a decodable chord here).
- vi: the Delete key now kills into the ring like `x` (vim), honoring
  counts, instead of discarding the text.
- Alt + non-ASCII chords (`\M-ö`) decode: the ESC-prefixed UTF-8
  sequence is assembled whole — previously the lead byte was consumed
  and the continuations decoded as garbage keys.
- Tabs render at 8-column stops of the true display offset (prompt
  included) instead of a fixed four spaces, so tab-indented pastes line
  up the way terminals show them; cursor math tracks the same expansion.
- `BellStyle::Visible` holds the reverse-video flash ~80ms before
  clearing it (input ends it early) — set-then-unset in a single write
  could render zero frames on many terminals.
- Multi-line prompts (a `PS1` with newlines) now work: everything up to
  the prompt's last newline paints once per edit region (readline's
  approach); only the final line joins the per-keystroke repaint. The
  old row accounting duplicated the prompt down the screen on every
  keystroke.
- `Hooks::highlight` contract change: the hook now receives the *raw*
  buffer (exactly what Enter returns — real tabs/newlines, true byte
  offsets) instead of the control-char-visualized text, so a parser
  highlights what the user actually typed. The hook's SGR markup passes
  through and the editor re-applies the `^X`/`⏎`/tab visualization
  around it; a non-SGR escape from the hook is neutralized rather than
  sent to the terminal, and a buffer containing a literal ESC paints
  unhighlighted (the markup would be ambiguous). Hosts that compensated
  for the visualized input should drop that compensation.
- Flicker-free repaint: the render used to clear the edit region and
  then repaint it, showing a blank frame every keystroke (visible on
  slow terminals and ssh). It now overwrites in place and erases only
  the leftover tail (paint-then-clear, readline's redisplay order).
- History file safety: `save_history` writes atomically (sibling temp
  file + rename, so a crash mid-write can't truncate the history) and
  new files are created mode 0600 on Unix, like bash's history file —
  history routinely contains secrets. Existing files keep their
  permissions; `append_history`'s create path is 0600 too.
- Case-insensitive history search: `Editor::set_search_ignore_case`
  (readline 8.1's `search-ignore-case`) covers C-r/C-s incremental
  search and the PageUp/PageDown prefix search.
- Reverse menu cycling: Shift-Tab (`CSI Z`, decoded with or without
  modifier parameters) is `EditorAction::MenuCompleteBackward` —
  readline's `menu-complete-backward`, zsh's `reverse-menu-complete`;
  a cold backward step starts on the last candidate.
- vi counts for `r`, `p`, `P` (vim semantics): `3rx` replaces three
  characters — failing outright, with a bell, when fewer remain — and
  `3p`/`3P` paste three copies.
- OSC-aware width math: `display_width` now skips OSC sequences
  (BEL- or ST-terminated) — a prompt carrying an OSC-8 hyperlink or a
  window-title sequence had its whole payload counted as printable
  width, misplacing the cursor.
- Fix: an unrecognized CSI sequence was cut at its first non-digit
  byte, so an SGR mouse report (`ESC[<65;5;10M`) leaked `65;5;10M`
  into the buffer as typed text. The decoder now consumes the full
  ECMA-48 grammar (parameter bytes 0x30–0x3F, intermediates 0x20–0x2F,
  one final byte) and swallows unknown sequences whole.
- New `examples/vi.rs` (vi mode + mode indicator + a deliberately
  multi-line prompt) and pty tests for normal-mode editing, `daw`,
  count-replace, and once-per-region prompt-prefix painting.
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
