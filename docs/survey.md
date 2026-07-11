# The line-editor field survey

The audit behind `rusty_lines`: every line editor in wide use was
examined, its behaviors catalogued, and each capability either matched
(with a named reference implementation) or consciously declined (with a
reason). The README carries the distilled matrix; this document is the
fuller per-editor record it was distilled from, reconstructed after the
extraction from rush.

Two questions drove the audit, in both directions:

1. **Coverage** — is there a capability users of *any* mainstream editor
   would reach for and miss here?
2. **Fidelity** — where a capability exists, does it match the semantics
   of the editor that defined it (not a lookalike approximation)?

## The field

### GNU readline

The baseline against which everything else defines itself; bash, gdb,
psql, and hundreds of others link it.

- **Taken as the reference for:** the emacs keymap core (C-a/C-e/C-b/C-f,
  C-d, C-h, C-t, M-t, M-u/M-l/M-c); the kill ring with append/prepend on
  consecutive kills and M-y yank-pop rotation; the *two word classes*
  (M-b/M-f/M-d over alphanumeric words vs `unix-word-rubout` C-w over
  whitespace words — a distinction most clones flatten); undo with
  self-insert runs coalesced (and, notably, *no redo* — readline itself
  has none); C-r/C-s incremental search with mid-search direction
  switching; M-. / M-_ insert-last-argument with repeat cycling; C-v/C-q
  quoted insert with `^X`-style control-char rendering; C-x C-e
  edit-in-`$EDITOR`; bracketed paste (default-on since readline 8.1);
  the vi keymap tradition (below, shared with ksh); list-style completion
  (`CompletionType::List`: longest-common-prefix insertion, then a
  columned candidate list); C-l clear-screen; the history cap
  (`stifle_history` / bash `HISTSIZE`), bash's `histappend`
  append-only persistence and `HISTCONTROL=erasedups` dedup (opt-in),
  and `revert-line` (M-r); single-key rebinding (`bind '"\C-x": function'`,
  `bind -P`, `bind -r`) over a named-action enum, bash's `bind -x`
  host-command contract (`READLINE_LINE`/`READLINE_POINT`), the
  `completion-ignore-case` / `show-all-if-ambiguous` / `menu-complete` /
  `bell-style` variables, and the read timeout (`rl_set_timeout`,
  bash `$TMOUT`).
- **Declined:** `.inputrc` *file parsing* (the host's `bind` builtin
  passes readline key specs through instead); multi-key chord bindings
  beyond the built-in C-x set; keyboard macros (C-x `(` … C-x `)`);
  emacs-mode numeric arguments (M-digit, C-u); mark/region (C-@,
  C-x C-x). (`menu-complete` and single-key rebinding were initially
  declined, then adopted — see the matrix.)

### libedit (editline)

BSD's independently licensed readline-alike (`.editrc`), shipped where
GPL is unwanted (macOS system tools, LLDB). Functionally a subset of
readline — surveying it added no capability readline didn't already
demand, but it confirms which subset of the emacs keymap is the
portable, load-bearing core.

### zsh ZLE

The most featureful shell editor; everything is a widget bound in
`emacs`/`viins`/`vicmd` keymaps.

- **Taken as the reference for:** the right-side prompt (`$RPS1`),
  including hiding it once the line grows into it; prefix history search
  (`history-beginning-search-backward/-forward`, here on
  PageUp/PageDown and M-p/M-n); forward incremental search being a
  first-class peer of backward (which requires IXON off so C-s reaches
  the editor); repeated-Tab candidate cycling after the list (`AUTO_MENU`,
  without the highlighted menu-select UI); case ops and
  insert-last-argument parity with readline.
- **Declined:** user-defined widgets and rebindable keymaps; the
  editable multi-line buffer (ZLE moves the cursor across embedded
  newlines); menu-select completion; ZLE's undo *and redo* pair (redo
  declined to stay with readline semantics).

### fish

The editor that made "helpful by default" the expectation.

- **Taken as the reference for:** history hints / autosuggestions (the
  dimmed inline continuation, accepted with Right/End, or one word at a
  time with M-f / Ctrl-Right, fish's forward-word on a suggestion) —
  surfaced here through `Hooks::hint`; syntax highlighting while typing —
  `Hooks::highlight`; abbreviation expansion on space (`abbr`) —
  `Hooks::expand_abbreviation`; C-z as an undo binding alongside
  readline's C-_ / C-x C-u; Up-arrow prefix search behavior folded into
  the prefix-search design.
- **Declined:** the completion pager with interactive selection; true
  multi-line buffer editing (fish edits command blocks in-buffer);
  fish's `bind` programmability.

### ksh93 (emacs + vi modes)

Where shell vi mode actually comes from; readline's vi keymap follows
it. Its emacs mode is also the historical source of several readline
defaults.

- **Taken as the reference for:** vi-mode structure and quirks shared
  with readline vi mode: counts; `d`/`c`/`y` operators over motions;
  `h l 0 ^ $ w W b B e E f F t T ; ,`; `x X D C s S Y r ~ p P u`;
  `i I a A`; `k`/`j` for history; the `cw` ≡ `ce` quirk (change-word
  stops at word end, vim-style, not vi-motion-style); Esc backing the
  cursor up one column on exit from insert.
- **Declined:** nothing beyond what the vi narrowings below already
  cover — ksh's vi mode is close to the floor of the tradition.

### linenoise

The minimalist pole (~1k lines, redis/valkey): proof of how small an
editor can be. Surveyed to calibrate the floor, and for one idea that
generalized well — the **hints callback**, which linenoise pioneered in
C and fish popularized as autosuggestions; `Hooks::hint` descends from
both. Its single-line-repaint model also informed the render engine's
"repaint the whole edit region each keystroke" approach (extended here
with soft-wrap row accounting linenoise lacks).

### replxx

The C++ continuation of the linenoise lineage (via linenoise-ng), used
by ClickHouse and others. Confirms the "modern minimal" feature floor:
UTF-8, syntax highlighting, hints, completion, incremental search.
Highlighting-while-typing as a callback (rather than a plugin system, as
in ZLE) matches the `Hooks::highlight` shape.

### rustyline

The Rust readline port this editor replaced inside rush — the survey's
most direct comparison.

- **Matched for continuity:** the host-integration shape (rustyline's
  `Helper` trait ≈ `Hooks`: completer, hinter, highlighter); emacs + vi
  keymaps; kill ring; undo; and one deliberate migration affordance —
  `load_history` skips rustyline `FileHistory`'s `#V2` header so an
  existing history file keeps working after the swap.
- **Declined:** the validator hook (multi-line continuation decisions
  belong to the host's parser, not the editor); configurable
  completion types beyond list-style.

### reedline

Nushell's editor; the other prominent Rust implementation.

- **Confirms:** right prompt, hinter/highlighter hooks, bracketed
  paste, vi mode — the same modern baseline this editor targets.
- **Declined:** menu system (completion menus, history menu);
  multi-line buffer editing; its keybinding configuration layer.

### prompt_toolkit / PSReadLine

The out-of-shell poles — Python's REPL toolkit (ptpython, IPython) and
PowerShell's editor.

- **Taken as the reference for:** prefix history search on arrows
  (PSReadLine's `HistorySearchBackward` matches the PageUp behavior
  here); predictions/autosuggestions (PSReadLine `Prediction`,
  prompt_toolkit `auto_suggest`) reinforcing the hint design.
- **Declined:** full-screen application features (prompt_toolkit is a
  TUI toolkit that happens to include a prompt); PSReadLine's
  undo/redo pair; completion menus in both.

## Capability audit

Every row names the editor whose semantics are matched — fidelity is to
that implementation, not a generic average. (This table also lives in
the README; kept in both places deliberately.)

| Capability | Reference behavior |
|---|---|
| Emacs basics: C-a/C-e/C-b/C-f, C-d, C-h, C-t, arrows, Home/End/Delete | readline, everywhere |
| Kill ring: C-k, C-u, C-w, M-d, M-Backspace kill *into* a ring; C-y yanks; M-y rotates; consecutive kills grow one entry (append forward / prepend backward); ring survives across lines | readline, ZLE, fish |
| Word flavors: M-b/M-f/M-d/M-Backspace use alphanumeric words, C-w whitespace words (unix-word-rubout) | readline's two word classes |
| Ctrl-arrow / Alt-arrow word motion (`CSI 1;5C` etc.) | every modern terminal editor |
| Undo: C-_ , C-x C-u (and C-z, fish-style); runs of self-insert undo as one unit; M-r reverts the whole line | readline (incl. `revert-line`), ZLE, fish |
| Transpose: C-t chars (two-before at EOL), M-t words | readline |
| Case ops: M-u / M-l / M-c | readline, ZLE |
| Insert last argument: M-. / M-_ , repeat cycles older entries | readline, ZLE |
| Quoted insert: C-v / C-q; control chars render `^X`-style | readline |
| Edit line in `$VISUAL`/`$EDITOR`: C-x C-e (emacs), `v` (vi normal); result returned as the line | readline, ZLE, fish (Alt-e) |
| History: Up/Down with draft preservation, C-p/C-n, M-< / M-> | readline |
| History cap: `set_max_history_len` drops oldest past the limit | readline `stifle_history`, bash `HISTSIZE` |
| History dedup option: `set_history_dedup` erases earlier duplicates | bash `HISTCONTROL=erasedups`, fish |
| History persistence: `save_history` rewrites, `append_history` appends only new entries; `load_history` tolerates a rustyline `#V2` header | bash `histappend`; rustyline migration |
| Clear screen: C-l clears and repaints the edit region at the top | readline `clear-screen` |
| Incremental search: C-r backward *and* C-s forward (IXON is off), direction switching mid-search | readline, ZLE |
| Prefix history search: PageUp/PageDown, M-p/M-n | ZLE `history-beginning-search`, fish Up, PSReadLine |
| History hints (autosuggestions) via `Hooks::hint`, Right/End accepts; M-f / Ctrl-Right at end of line accepts one word | fish, PSReadLine, linenoise hints |
| Syntax highlighting while typing via `Hooks::highlight` | fish, ZLE plugins, replxx |
| Tab completion via `Hooks::complete`: LCP insertion + columned candidate list | readline `CompletionType::List` |
| Menu cycling: Tab after the candidate list walks the candidates in-line, wrapping around | zsh `AUTO_MENU`, readline `menu-complete` |
| Abbreviation expansion on space via `Hooks::expand_abbreviation` | fish `abbr` |
| Right-side prompt (second `read_line` argument), hidden when the line grows into it | zsh `$RPS1`, fish, reedline |
| Bracketed paste: paste arrives as one event — tabs/ESC insert literally, nothing executes until Enter; multi-line pastes keep their newlines (shown `⏎`) and return as a unit; multi-line history entries stored joined with `; ` (bash `cmdhist`) | readline 8.1+, ZLE, fish, reedline |
| vi mode (`Hooks::vi_mode`): counts; `d`/`c`/`y` operators over motions; `h l 0 ^ $ w W b B e E f F t T ; ,`; `x X D C s S Y r ~ p P u`; `i I a A`; `k`/`j` history; `cw`≡`ce` quirk; Esc backs the cursor up one | readline vi mode, ksh, ZLE |
| Wide chars + UTF-8 input assembly; ANSI-aware width math; soft-wrap repaint; `^X` control-char visualization keeps cursor math exact | all modern |
| Resize: width re-read on every repaint; a resize while idle at the prompt repaints within a poll tick (~200ms) | readline SIGWINCH, approximated without signals |
| Key rebinding: `bind`/`unbind` accept readline key-spec spellings (`\C-x`, `\M-f`, `\e[1;5C`), remapping single keys to named `EditorAction`s; `bindings()` lists the effective keymap | readline `bind '"\C-x": kill-line'`, `bind -P`, `bind -r` |
| Host-command bindings: `bind_host` + `Hooks::host_binding` — raw mode suspends, the host runs its command against the line/cursor, the edit resumes | bash `bind -x` (`READLINE_LINE`/`READLINE_POINT`) |
| Readline variables: `set_completion_ignore_case`, `set_show_all_if_ambiguous`, `set_menu_complete`, `set_bell_style` | readline `set completion-ignore-case on` … |
| Read deadline: `read_line_timeout` returns `ReadResult::TimedOut` when no complete line arrives in time | bash `$TMOUT`, readline `rl_set_timeout` |
| History timestamps: `#<epoch>` comment lines, written only under `set_history_timestamps`, always parsed on load (both formats round-trip); `history_timestamps()` exposes them | bash `HISTTIMEFORMAT` file format |
| In-place history replacement: `replace_history` resyncs the list after a host's history edits without rebuilding the editor (kill ring and session state survive) | bash `history -c` / `history -d` support |
| Terminal facilities: `terminal_size()` (cols, rows) and `with_echo_disabled` (panic-safe echo-off around a closure) | bash `checkwinsize` `$COLUMNS`/`$LINES`; `read -s` |

## Deliberate narrowings

Checked against the same field and consciously not modeled — each is
either niche, terminal-hostile, or a different program's job:

- **Multi-line *buffer editing*** (zsh/fish/reedline/prompt_toolkit edit
  a `\n`-separated buffer with per-line cursor movement). The buffer is
  one logical line; embedded newlines (from a paste or C-v C-j) render
  as `⏎` and return correctly, but Up/Down navigate history, not buffer
  rows. C-x C-e hands real multi-line editing to `$EDITOR`.
- **Full keymap programmability** (readline's `.inputrc`, ZLE widgets,
  fish's `bind` functions, reedline's keybinding config). Single-key
  rebinding of the named actions and host-command bindings *are*
  supported (see the matrix — revisiting a narrowing); what stays
  declined: user-defined widgets (actions are the built-in enum, not
  host code — except `bind_host`, which is exactly bash's `bind -x`
  scope), multi-key chord bindings beyond the built-in C-x set,
  rebinding vi normal mode, and `.inputrc` file parsing (the host's
  `bind` builtin passes readline key specs through verbatim).
- **Keyboard macros** (readline C-x `(` … `)`), **numeric arguments in
  emacs mode** (M-digit; vi counts *are* supported), **mark/region**
  (C-@, C-x C-x), **redo** (ZLE, fish, and PSReadLine have it; readline
  has none either — readline semantics win).
- **vi registers, `.` repeat, `/` history search** (C-r covers search
  from insert mode; the unnamed register is the kill ring).
- **Completion paging and menu-select UI** (fish's pager, ZLE's
  interactive menu with a highlighted selection, reedline/prompt_toolkit
  menus): long candidate lists print unpaged; repeated-Tab cycling (see
  the matrix) stands in for menu-select.
- **Signal-driven resize repaint** (readline installs a SIGWINCH
  handler). The width is re-read from the tty on every repaint, and a
  resize while idle at the prompt is noticed by the input poll tick and
  repainted; only the signal handler itself is declined — installing
  one from a library is the host's business, not the editor's, and the
  host already owns signal delivery via `Hooks::on_interrupted_read`.
- **Grapheme-cluster cursor math** (combining marks, emoji ZWJ
  sequences). Width is per-`char` via `unicode-width`; getting clusters
  right would add a `unicode-segmentation` dependency against the
  crate's two-dependency budget, while terminals themselves disagree on
  cluster widths — the common-case behavior (wide CJK, zero-width
  combining marks) is what `unicode-width` already gives.
- **Non-tty / non-Unix**: piped stdin gets a plain line read; non-Unix
  builds get a buffered prompt-and-read.

## Maintaining this document

When adding a capability, name the reference editor and match *its*
semantics, then add the row to both this table and the README's. When
declining one, record it under narrowings with the reason — the
narrowings list is the part of the audit that keeps future feature
creep honest.
