# rusty_lines

[![CI](https://github.com/baileyrd/rusty_lines/actions/workflows/ci.yml/badge.svg)](https://github.com/baileyrd/rusty_lines/actions/workflows/ci.yml)

A hand-rolled line editor for Rust — a readline alternative with no
dependency on readline, ncurses, or any editing crate (just `libc` for
termios and `unicode-width` for display columns). Grown inside the
[rush shell](https://github.com/baileyrd/rush) as its `rustyline`
replacement, then extracted.

```rust
use rusty_lines::{Editor, NoHooks, ReadResult};

let mut ed = Editor::new();
match ed.read_line("prompt> ", "right-prompt", &NoHooks)? {
    ReadResult::Line(line) => { /* … */ }
    ReadResult::Interrupted => { /* Ctrl-C */ }
    ReadResult::Eof => { /* Ctrl-D on an empty line */ }
}
```

The host integrates through the `Hooks` trait — every method has a no-op
default, so `&NoHooks` gives plain editing:

```rust
pub trait Hooks {
    fn complete(&self, line: &str, pos: usize) -> (usize, Vec<Candidate>);
    fn hint(&self, line: &str, history: &[String]) -> Option<String>;
    fn highlight(&self, line: &str) -> String;
    fn expand_abbreviation(&self, line: &str, cursor: usize) -> Option<(usize, String)>;
    fn vi_mode(&self) -> bool;              // checked live, per read_line
    fn external_editor(&self) -> Option<String>; // C-x C-e; falls back $VISUAL/$EDITOR/vi
    fn on_interrupted_read(&self);          // EINTR: fire pending signal traps
    fn host_binding(&self, tag: &str, line: &mut String, cursor: &mut usize); // bind -x
}
```

Try it: `cargo run --example demo`.

## Feature matrix

The feature set was audited against the line editors in wide use — GNU
readline, libedit, zsh ZLE, fish, ksh93's emacs/vi modes, linenoise,
replxx, rustyline, reedline, prompt_toolkit/PSReadLine — and the gaps
closed. References are to the editor whose behavior is matched. The
full per-editor survey behind this table is in
[docs/survey.md](docs/survey.md).

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

- **Multi-line *buffer editing*** (zsh/fish/reedline edit a `\n`-separated
  buffer with per-line cursor movement). The buffer is one logical line;
  embedded newlines (from a paste or C-v C-j) render as `⏎` and return
  correctly, but Up/Down navigate history, not buffer rows. C-x C-e hands
  real multi-line editing to `$EDITOR`.
- **Full keymap programmability** (readline's `.inputrc`, ZLE widgets,
  fish's `bind` functions). Single-key rebinding of the named actions and
  host-command bindings *are* supported (see the matrix — revisiting a
  narrowing); what stays declined: user-defined widgets, multi-key chord
  bindings beyond the built-in C-x set, rebinding vi normal mode, and
  `.inputrc` file parsing (the host's `bind` builtin passes specs through).
- **Keyboard macros** (readline C-x `(` … `)`), **numeric arguments in
  emacs mode** (M-digit; vi counts are supported), **mark/region**
  (C-@, C-x C-x), **redo** (readline has none either).
- **vi registers, `.` repeat, `/` history search** (C-r covers search from
  insert mode; the unnamed register is the kill ring).
- **Completion paging and menu-select UI** (fish's pager, ZLE's
  interactive menu with a highlighted selection): long candidate lists
  print unpaged; repeated-Tab cycling (see the matrix) stands in for
  menu-select.
- **Signal-driven resize repaint** (readline installs a SIGWINCH
  handler). The width is re-read from the tty on every repaint, and a
  resize while idle at the prompt is noticed by the input poll tick and
  repainted; only the signal handler itself is declined — installing
  one from a library is the host's business, not the editor's.
- **Grapheme-cluster cursor math** (combining marks, emoji ZWJ
  sequences). Width is per-`char` via `unicode-width`; getting clusters
  right would add a segmentation dependency while terminals themselves
  disagree on cluster widths, so the common-case behavior is kept.
- **Non-tty / non-Unix**: piped stdin gets a plain line read; non-Unix
  builds get a buffered prompt-and-read.

## Verification

Pure helpers (word motions in all three flavors, vi find targets, kill
ring append/prepend, yank-pop rotation, undo, case ops, word transpose,
last-arg cycling, prefix search, control-char visualization, CSI decode)
are unit-tested in `src/lib.rs`. End-to-end behavior — raw-mode escape
sequences, repaint math, bracketed paste, the full keymaps under a real
pseudo-terminal — is exercised downstream by rush's pty harness
(`tests/pty/editor_pty_test.py` in the rush repo, 28 scenarios).

## License

MIT.
