//! `rusty_lines` — a hand-rolled line editor: a readline alternative with
//! no dependency on readline, ncurses, or any editing crate. Grown inside
//! the [rush shell](https://github.com/baileyrd/rush) as its `rustyline`
//! replacement, then extracted; its feature set is audited against the
//! other line editors in the wild — GNU readline, zsh ZLE, fish, ksh93's
//! emacs/vi modes, libedit, linenoise, replxx, rustyline, reedline — see
//! the README for the survey and the documented narrowings.
//!
//! The host integrates through the [`Hooks`] trait (completion, hints,
//! syntax highlighting, abbreviations, a live vi-mode flag, external
//! editor resolution, an interrupted-read callback, and host-command
//! key bindings); [`NoHooks`] gives plain editing.
//!
//! Layers, bottom to top:
//!   * raw terminal mode (termios) behind an RAII guard, so every exit path
//!     — including panics — restores the terminal; bracketed-paste mode
//!     behind a second guard, so a paste arrives as one event instead of a
//!     stream of fake keystrokes;
//!   * key decoding: UTF-8 assembly plus escape-sequence parsing (CSI/SS3
//!     with modifier parameters — Ctrl/Alt-arrows — Alt- chords, and the
//!     bracketed-paste envelope), with a short poll to tell a lone ESC
//!     from a sequence;
//!   * a render engine that repaints the whole edit region per keystroke:
//!     display-width math (via `unicode_width`, ANSI-aware), a readline-
//!     style `^X` visualization for control characters in the buffer,
//!     soft-wrap row accounting, forced wraps at exact column boundaries
//!     (avoiding the delayed-wrap ambiguity), syntax highlighting, the
//!     dimmed history hint, and a right-side prompt (zsh's `$RPS1`),
//!     shown while the first row has room for it;
//!   * keymaps: the emacs set (kill ring with yank/yank-pop, undo,
//!     word-wise motion/kill/case/transpose, insert-last-argument,
//!     quoted-insert, edit-in-`$EDITOR`) by default, plus a vi mode
//!     with counts, the `d`/`c`/`y` operators over motions, `f F t T ; ,`
//!     character finds, and the standard normal-mode edits — selected
//!     live per `read_line` via [`Hooks::vi_mode`], so switching needs
//!     no editor rebuild at all; single keys are rebindable to named
//!     [`EditorAction`]s or host commands ([`Editor::bind`],
//!     [`Editor::bind_host`] — readline's `bind`, bash's `bind -x`);
//!   * history: in-memory with consecutive-dedup (multi-line entries
//!     stored bash-style with `; ` joining), plain-file persistence,
//!     Up/Down navigation with draft preservation, Ctrl-R/Ctrl-S
//!     incremental search in both directions, and prefix search
//!     (PageUp/PageDown, Alt-p/Alt-n);
//!   * completion (Tab: longest-common-prefix insertion, then a columned
//!     candidate list) and abbreviation expansion on space, both driven
//!     by the host's [`Hooks`].
//!
//! # Example
//!
//! ```no_run
//! use rusty_lines::{Editor, NoHooks, ReadResult};
//!
//! # fn main() -> std::io::Result<()> {
//! let mut ed = Editor::new();
//! match ed.read_line("prompt> ", "", &NoHooks)? {
//!     ReadResult::Line(line) => ed.add_history_entry(&line),
//!     ReadResult::Interrupted => { /* Ctrl-C */ }
//!     ReadResult::Eof => { /* Ctrl-D on an empty line */ }
//!     ReadResult::TimedOut => { /* read_line_timeout deadline only */ }
//! }
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]
// The non-Unix build is a plain buffered read: the editing internals and
// their `io::Read` import are compiled out, so don't warn that they're dead.
#![cfg_attr(not(unix), allow(dead_code, unused_imports))]

use std::io::{self, Read, Write};

// Terminal syscall backend: the `libc` crate by default, `rusty_libc` under
// the `rusty-libc` feature.
#[cfg(unix)]
mod term_sys;

/// One completion candidate: the text shown in the columned list, and the
/// text inserted into the buffer.
pub struct Candidate {
    /// The text shown in the candidate list.
    pub display: String,
    /// The text inserted into the buffer when this candidate is chosen.
    pub replacement: String,
}

/// The host application's integration points. Every method has a no-op
/// default, so `&NoHooks` gives plain line editing; a shell implements
/// the lot (completion, hints, highlighting, abbreviations, a live
/// vi-mode flag, its own `$EDITOR` resolution, and a callback for
/// signals that interrupt the blocking read).
pub trait Hooks {
    /// Candidates for the word at `pos`, plus the byte offset that word
    /// starts at (the editor replaces `start..pos`).
    fn complete(&self, _line: &str, _pos: usize) -> (usize, Vec<Candidate>) {
        (0, Vec::new())
    }
    /// The dimmed inline suggestion shown after the buffer (fish-style);
    /// Right/End at end-of-line accepts it.
    fn hint(&self, _line: &str, _history: &[String]) -> Option<String> {
        None
    }
    /// The buffer as painted — return it wrapped in ANSI SGR sequences
    /// for syntax highlighting (widths are computed ANSI-aware).
    fn highlight(&self, line: &str) -> String {
        line.to_string()
    }
    /// Called when space is typed: return `Some((start, replacement))`
    /// to rewrite `start..cursor` first (fish-style abbreviations).
    fn expand_abbreviation(&self, _line: &str, _cursor: usize) -> Option<(usize, String)> {
        None
    }
    /// Checked at the start of every `read_line`: true selects the vi
    /// keymap, so a `set -o vi` needs no editor rebuild.
    fn vi_mode(&self) -> bool {
        false
    }
    /// The command for C-x C-e / vi `v` (edit the line in an editor).
    /// `None` falls back to `$VISUAL`, `$EDITOR`, then `vi`.
    fn external_editor(&self) -> Option<String> {
        None
    }
    /// Called when a signal interrupts the blocking read (`EINTR`) —
    /// a shell fires its pending traps here. The read then resumes.
    fn on_interrupted_read(&self) {}
    /// Invoked for a key bound via [`Editor::bind_host`] — bash's
    /// `bind -x` contract. The editor suspends raw mode, hands over the
    /// current buffer and cursor (byte offset) for the host to run its
    /// command with (`READLINE_LINE`/`READLINE_POINT`), writes back
    /// whatever the host left in them, and repaints. A cursor written
    /// past the end of the line (or inside a UTF-8 sequence) is clamped.
    fn host_binding(&self, _tag: &str, _line: &mut String, _cursor: &mut usize) {}
}

/// A no-op `Hooks`: plain editing with no completion, hints,
/// highlighting, or abbreviations.
pub struct NoHooks;
impl Hooks for NoHooks {}

/// A named edit action — every command in the emacs (and vi-insert)
/// keymap, using readline's command names camel-cased. What
/// [`Editor::bind`] binds a key to and [`Editor::bindings`] reports
/// (a shell's `bind '"\C-x": kill-line'` / `bind -P`).
///
/// The enum is `#[non_exhaustive]`: actions may be added, so downstream
/// name→action tables should match by name with a fallthrough.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorAction {
    /// Insert the typed character (with abbreviation expansion on space).
    SelfInsert,
    /// Finish the line and return it (`accept-line`, Enter).
    AcceptLine,
    /// Abandon the line ([`ReadResult::Interrupted`], C-c).
    Interrupt,
    /// Delete the character under the cursor; on an empty line, EOF
    /// (`delete-char` with bash's C-d end-of-file behavior).
    DeleteCharOrEof,
    /// Delete the character under the cursor (`delete-char`, Delete).
    DeleteChar,
    /// Delete the character before the cursor (`backward-delete-char`).
    BackwardDeleteChar,
    /// Move to the start of the line (`beginning-of-line`, C-a/Home).
    BeginningOfLine,
    /// Move to the end of the line; at end, accept the hint
    /// (`end-of-line`, C-e/End).
    EndOfLine,
    /// Move forward one character; at end, accept the hint
    /// (`forward-char`, C-f/Right).
    ForwardChar,
    /// Move back one character (`backward-char`, C-b/Left).
    BackwardChar,
    /// Move forward one alphanumeric word; at end, accept one hint word
    /// (`forward-word`, M-f/Ctrl-Right).
    ForwardWord,
    /// Move back one alphanumeric word (`backward-word`, M-b/Ctrl-Left).
    BackwardWord,
    /// Kill to the end of the line (`kill-line`, C-k).
    KillLine,
    /// Kill to the start of the line (`unix-line-discard`, C-u).
    UnixLineDiscard,
    /// Kill the whitespace-delimited word before the cursor
    /// (`unix-word-rubout`, C-w).
    UnixWordRubout,
    /// Kill the alphanumeric word after the cursor (`kill-word`, M-d).
    KillWord,
    /// Kill the alphanumeric word before the cursor
    /// (`backward-kill-word`, M-Backspace).
    BackwardKillWord,
    /// Insert the top of the kill ring (`yank`, C-y).
    Yank,
    /// Rotate the last yank to the previous ring entry (`yank-pop`, M-y).
    YankPop,
    /// Transpose the characters around the cursor (`transpose-chars`, C-t).
    TransposeChars,
    /// Transpose the words around the cursor (`transpose-words`, M-t).
    TransposeWords,
    /// Uppercase to the end of the word (`upcase-word`, M-u).
    UpcaseWord,
    /// Lowercase to the end of the word (`downcase-word`, M-l).
    DowncaseWord,
    /// Capitalize the word (`capitalize-word`, M-c).
    CapitalizeWord,
    /// Undo the last edit (`undo`, C-_/C-z).
    Undo,
    /// Undo every edit to this line at once (`revert-line`, M-r).
    RevertLine,
    /// Insert the last word of the previous history entry; repeats cycle
    /// older entries (`insert-last-argument`, M-./M-_).
    InsertLastArgument,
    /// Recall the previous history entry (`previous-history`, Up/C-p).
    PreviousHistory,
    /// Recall the next history entry, or the stashed draft
    /// (`next-history`, Down/C-n).
    NextHistory,
    /// Jump to the oldest history entry (`beginning-of-history`, M-<).
    BeginningOfHistory,
    /// Back to the live draft line (`end-of-history`, M->).
    EndOfHistory,
    /// Previous history entry with the prefix before the cursor
    /// (`history-search-backward`, PageUp/M-p).
    HistorySearchBackward,
    /// Next prefix match, or back to the draft
    /// (`history-search-forward`, PageDown/M-n).
    HistorySearchForward,
    /// Enter incremental search, older matches
    /// (`reverse-search-history`, C-r).
    ReverseSearchHistory,
    /// Enter incremental search, newer matches
    /// (`forward-search-history`, C-s).
    ForwardSearchHistory,
    /// Clear the screen and repaint the line at the top
    /// (`clear-screen`, C-l).
    ClearScreen,
    /// Complete the word at the cursor: LCP insertion, candidate list,
    /// then menu cycling (`complete`, Tab). Behaves as [`MenuComplete`]
    /// under [`Editor::set_menu_complete`].
    ///
    /// [`MenuComplete`]: EditorAction::MenuComplete
    Complete,
    /// Insert the first completion candidate immediately; repeats cycle
    /// through the rest (`menu-complete`).
    MenuComplete,
    /// Insert the next key literally (`quoted-insert`, C-v/C-q).
    QuotedInsert,
    /// Edit the line in `$VISUAL`/`$EDITOR` and execute the result
    /// (`edit-and-execute-command`, C-x C-e).
    EditAndExecuteCommand,
}

/// What ringing the terminal bell does — readline's `bell-style`
/// variable, set via [`Editor::set_bell_style`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BellStyle {
    /// No feedback.
    None,
    /// BEL — the terminal's audible beep (readline's default).
    #[default]
    Audible,
    /// A screen flash (reverse-video flip) instead of a beep.
    Visible,
}

/// A key's resolved binding: a named action, a host command tag
/// (`bind -x`), or explicitly nothing (`bind -r` masking a default).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Binding {
    Action(EditorAction),
    Host(String),
    Unbound,
}

/// The readline-variable knobs, copied into each `read_line`'s state.
#[derive(Debug, Clone, Copy, Default)]
struct EditorConfig {
    completion_ignore_case: bool,
    show_all_if_ambiguous: bool,
    menu_complete: bool,
    bell: BellStyle,
}

/// How a [`Editor::read_line`] call ended.
pub enum ReadResult {
    /// A complete line (Enter).
    Line(String),
    /// Ctrl-C at the prompt.
    Interrupted,
    /// Ctrl-D on an empty line.
    Eof,
    /// The [`Editor::read_line_timeout`] deadline passed with no complete
    /// line (bash's `$TMOUT`: "timed out waiting for input").
    TimedOut,
}

/// Terminal size of stdout as `(columns, rows)`, or `None` when stdout is
/// not a terminal (or on non-Unix builds). What a shell needs to keep
/// `$COLUMNS`/`$LINES` fresh (bash 5's `checkwinsize` default).
pub fn terminal_size() -> Option<(u16, u16)> {
    #[cfg(unix)]
    {
        term_sys::term_size_stdout()
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Run `f` with terminal echo off (a shell's `read -s`), restoring the
/// previous state on every exit path — including a panic in `f` — via an
/// internal RAII guard. When stdin is not a terminal there is no echo to
/// disable and `f` simply runs.
pub fn with_echo_disabled<T>(f: impl FnOnce() -> T) -> io::Result<T> {
    #[cfg(unix)]
    {
        if !term_sys::isatty_stdin() {
            return Ok(f());
        }
        /// Restores the saved attributes on drop.
        struct EchoGuard(term_sys::Termios);
        impl Drop for EchoGuard {
            fn drop(&mut self) {
                let _ = term_sys::tcsetattr_stdin_drain(&self.0);
            }
        }
        let saved = term_sys::tcgetattr_stdin()?;
        let mut silent = saved;
        term_sys::clear_echo_flag(&mut silent);
        term_sys::tcsetattr_stdin_drain(&silent)?;
        let _guard = EchoGuard(saved);
        Ok(f())
    }
    #[cfg(not(unix))]
    {
        Ok(f())
    }
}

/// The line editor: owns the history and the kill ring, both of which
/// persist across [`read_line`](Editor::read_line) calls within a session.
pub struct Editor {
    history: Vec<String>,
    /// Per-entry epoch timestamps, parallel to `history` (`None` for
    /// entries loaded from a file without them).
    timestamps: Vec<Option<i64>>,
    /// The kill ring (readline's): survives across lines within a session.
    kill_ring: Vec<String>,
    /// Cap on history entries (readline's `stifle_history`); oldest are
    /// dropped past it. `usize::MAX` = unbounded, the default.
    max_history: usize,
    /// How many history entries are already in the history file, so
    /// `append_history` writes only the ones added since.
    persisted: usize,
    /// When set, a new entry erases earlier duplicates everywhere in the
    /// history, not just a consecutive repeat.
    dedup: bool,
    /// When set, `save_history`/`append_history` write bash's `#<epoch>`
    /// timestamp comment before each entry (`HISTTIMEFORMAT`'s format).
    write_timestamps: bool,
    /// Host rebindings, overlaid on the default keymap (emacs and
    /// vi-insert modes; vi normal mode is not rebindable).
    bindings: Vec<(Key, Binding)>,
    /// The readline-variable knobs (`set_completion_ignore_case` …).
    cfg: EditorConfig,
}

/// The piped-stdin path: one line, no prompt, no editing. A deadline, if
/// set, is honored between bytes via `poll`.
#[cfg(unix)]
fn read_line_plain(deadline: Option<std::time::Instant>) -> io::Result<ReadResult> {
    let mut line = Vec::new();
    let mut b = [0u8; 1];
    loop {
        if let Some(d) = deadline {
            let Some(remaining) = d.checked_duration_since(std::time::Instant::now()) else {
                return Ok(ReadResult::TimedOut);
            };
            if !term_sys::poll_stdin(remaining.as_millis().min(i32::MAX as u128) as i32) {
                continue; // re-check the deadline, then poll again
            }
        }
        match io::stdin().read(&mut b) {
            Ok(0) => {
                if line.is_empty() {
                    return Ok(ReadResult::Eof);
                }
                break;
            }
            Ok(_) if b[0] == b'\n' => break,
            Ok(_) => line.push(b[0]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(ReadResult::Line(
        String::from_utf8_lossy(&line).into_owned(),
    ))
}

impl Default for Editor {
    fn default() -> Self {
        Editor::new()
    }
}

impl Editor {
    /// A fresh editor with empty history and kill ring.
    pub fn new() -> Self {
        Editor {
            history: Vec::new(),
            timestamps: Vec::new(),
            kill_ring: Vec::new(),
            max_history: usize::MAX,
            persisted: 0,
            dedup: false,
            write_timestamps: false,
            bindings: Vec::new(),
            cfg: EditorConfig::default(),
        }
    }

    /// Rebind `keys` to `action`, replacing the default (or a previous
    /// rebinding) — readline's `bind '"\C-x": function'`. The key spec
    /// accepts readline's spellings: `\C-x`, `\M-f`, `\e[1;5C`, plus the
    /// usual backslash escapes. Bindings apply to the emacs and vi-insert
    /// keymaps; vi normal mode is fixed. Errors on a spec that does not
    /// parse to a single recognized key (multi-key chords are not
    /// supported).
    pub fn bind(&mut self, keys: &str, action: EditorAction) -> io::Result<()> {
        self.bind_internal(keys, Binding::Action(action))
    }

    /// Bind `keys` to a host command — bash's `bind -x`. When the key is
    /// pressed the editor suspends raw mode and calls
    /// [`Hooks::host_binding`] with `tag` and the current line/cursor;
    /// the host runs its command (with `READLINE_LINE`/`READLINE_POINT`
    /// semantics), and whatever it writes back becomes the buffer.
    pub fn bind_host(&mut self, keys: &str, tag: String) -> io::Result<()> {
        self.bind_internal(keys, Binding::Host(tag))
    }

    /// Remove any binding for `keys` — including the default, so the key
    /// does nothing (readline's `bind -r`). Rebind with
    /// [`bind`](Editor::bind) to restore behavior.
    pub fn unbind(&mut self, keys: &str) -> io::Result<()> {
        self.bind_internal(keys, Binding::Unbound)
    }

    fn bind_internal(&mut self, keys: &str, binding: Binding) -> io::Result<()> {
        let key = parse_key_spec(keys).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unparseable key spec: {keys:?}"),
            )
        })?;
        if let Some(slot) = self.bindings.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = binding;
        } else {
            self.bindings.push((key, binding));
        }
        Ok(())
    }

    /// The current action bindings — defaults with rebindings applied —
    /// as (key spec, action) pairs (a shell's `bind -P`). Host-command
    /// bindings and self-inserting character keys are not listed.
    pub fn bindings(&self) -> impl Iterator<Item = (String, EditorAction)> + '_ {
        let defaults = DEFAULT_BINDINGS
            .iter()
            .filter(|(k, _)| !self.bindings.iter().any(|(ck, _)| ck == k))
            .map(|(k, a)| (key_spec(k), *a));
        let custom = self.bindings.iter().filter_map(|(k, b)| match b {
            Binding::Action(a) => Some((key_spec(k), *a)),
            _ => None,
        });
        defaults.chain(custom)
    }

    /// Case-insensitive completion matching (readline's
    /// `completion-ignore-case`): the longest-common-prefix step compares
    /// candidates ignoring case (the first candidate's case is inserted).
    /// Off by default, like readline.
    pub fn set_completion_ignore_case(&mut self, on: bool) {
        self.cfg.completion_ignore_case = on;
    }

    /// List completion candidates immediately when a completion is
    /// ambiguous, instead of on the second Tab (readline's
    /// `show-all-if-ambiguous`). Off by default, like readline.
    pub fn set_show_all_if_ambiguous(&mut self, on: bool) {
        self.cfg.show_all_if_ambiguous = on;
    }

    /// Make Tab cycle through the candidates directly, inserting the
    /// first match immediately (readline's `menu-complete` bound in place
    /// of `complete`). Off by default, like readline.
    pub fn set_menu_complete(&mut self, on: bool) {
        self.cfg.menu_complete = on;
    }

    /// What ringing the bell does (readline's `bell-style`): audible
    /// (the default, like readline), visible, or nothing. The editor
    /// rings on completion with no candidates.
    pub fn set_bell_style(&mut self, style: BellStyle) {
        self.cfg.bell = style;
    }

    /// When enabled, adding an entry removes earlier duplicates anywhere
    /// in the history (bash `HISTCONTROL=erasedups`; fish's behavior),
    /// not just a consecutive repeat. Off by default.
    pub fn set_history_dedup(&mut self, on: bool) {
        self.dedup = on;
    }

    /// Cap the history at `n` entries (readline's `stifle_history`,
    /// bash's `HISTSIZE`): the oldest entries are dropped as new ones
    /// arrive. Applies immediately and to future `add_history_entry`
    /// calls. The default is unbounded.
    pub fn set_max_history_len(&mut self, n: usize) {
        self.max_history = n;
        self.trim_history();
    }

    fn trim_history(&mut self) {
        if self.history.len() > self.max_history {
            let excess = self.history.len() - self.max_history;
            self.history.drain(..excess);
            self.timestamps.drain(..excess);
            self.persisted = self.persisted.saturating_sub(excess);
        }
    }

    /// The history entries, oldest first.
    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// Per-entry epoch timestamps, parallel to [`history`](Editor::history)
    /// — `None` for entries loaded from a file without them. What a shell's
    /// `history` builtin renders through `strftime($HISTTIMEFORMAT)`.
    pub fn history_timestamps(&self) -> &[Option<i64>] {
        &self.timestamps
    }

    /// When enabled, `save_history` and `append_history` precede each entry
    /// with bash's `#<epoch>` timestamp comment — the `HISTTIMEFORMAT` file
    /// format. Off by default, so existing plain history files are not
    /// rewritten into the timestamped format behind the user's back
    /// (bash likewise writes timestamps only when `HISTTIMEFORMAT` is set).
    /// `load_history` understands both formats regardless of this toggle.
    pub fn set_history_timestamps(&mut self, on: bool) {
        self.write_timestamps = on;
    }

    /// Append to history, skipping a consecutive duplicate. A multi-line
    /// entry (a bracketed paste) is joined with `; ` — bash's `cmdhist`
    /// behavior — so recall and the line-oriented history file both work.
    /// The entry is stamped with the current time (rendered to the file
    /// only under [`set_history_timestamps`](Editor::set_history_timestamps)).
    pub fn add_history_entry(&mut self, line: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs() as i64);
        self.add_entry(line, now);
    }

    fn add_entry(&mut self, line: &str, timestamp: Option<i64>) {
        let entry = if line.contains('\n') {
            line.replace('\n', "; ")
        } else {
            line.to_string()
        };
        if self.dedup {
            let mut i = 0;
            while i < self.history.len() {
                if self.history[i] == entry {
                    self.history.remove(i);
                    self.timestamps.remove(i);
                    // A removed entry below the persisted watermark shifts
                    // it down; the file keeps the stale copy until the next
                    // full save_history.
                    if i < self.persisted {
                        self.persisted -= 1;
                    }
                } else {
                    i += 1;
                }
            }
        }
        if self.history.last() != Some(&entry) {
            self.history.push(entry);
            self.timestamps.push(timestamp);
            self.trim_history();
        }
    }

    /// Replace the whole history in place — a shell resynchronizing after
    /// its `history -c` / `history -d` builtin — without rebuilding the
    /// editor (the kill ring and other session state survive). Entries are
    /// treated as already persisted: a following `append_history` writes
    /// only entries added *after* this call, mirroring bash, where deletion
    /// edits the in-memory list and the file catches up on the next full
    /// write. Timestamps are cleared (the caller's mirror is line-only).
    pub fn replace_history(&mut self, entries: Vec<String>) {
        self.history = entries
            .into_iter()
            .map(|e| {
                if e.contains('\n') {
                    e.replace('\n', "; ")
                } else {
                    e
                }
            })
            .collect();
        self.timestamps = vec![None; self.history.len()];
        self.persisted = self.history.len();
        self.trim_history();
    }

    /// Load history from `path`. Plain lines; a `#<epoch>` comment line
    /// (bash's `HISTTIMEFORMAT` file format) stamps the entry that follows
    /// it; a leading `#V2` header (the format `rustyline`'s `FileHistory`
    /// writes, for hosts migrating) is skipped. Files with and without
    /// timestamps both round-trip.
    pub fn load_history(&mut self, path: &std::path::Path) -> io::Result<()> {
        let text = std::fs::read_to_string(path)?;
        let mut pending_ts: Option<i64> = None;
        for (i, line) in text.lines().enumerate() {
            if i == 0 && line == "#V2" {
                continue;
            }
            if let Some(digits) = line.strip_prefix('#')
                && !digits.is_empty()
                && digits.bytes().all(|b| b.is_ascii_digit())
            {
                pending_ts = digits.parse().ok();
                continue;
            }
            if !line.is_empty() {
                self.add_entry(line, pending_ts.take());
            }
        }
        self.persisted = self.history.len();
        Ok(())
    }

    /// One entry (or a `persisted..` tail) in the on-disk format:
    /// `#<epoch>` comment lines only under `set_history_timestamps`.
    fn format_entries(&self, from: usize) -> String {
        let mut out = String::new();
        for (entry, ts) in self.history[from..].iter().zip(&self.timestamps[from..]) {
            if self.write_timestamps
                && let Some(ts) = ts
            {
                out.push_str(&format!("#{ts}\n"));
            }
            out.push_str(entry);
            out.push('\n');
        }
        out
    }

    /// Write the history to `path`, one entry per line (preceded by a
    /// `#<epoch>` timestamp line when
    /// [`set_history_timestamps`](Editor::set_history_timestamps) is on).
    pub fn save_history(&mut self, path: &std::path::Path) -> io::Result<()> {
        std::fs::write(path, self.format_entries(0))?;
        self.persisted = self.history.len();
        Ok(())
    }

    /// Append only the entries added since the last `load_history`,
    /// `save_history`, or `append_history` call — bash's `histappend`:
    /// concurrent sessions interleave instead of overwriting each other.
    pub fn append_history(&mut self, path: &std::path::Path) -> io::Result<()> {
        let from = self.persisted.min(self.history.len());
        if from < self.history.len() {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            f.write_all(self.format_entries(from).as_bytes())?;
        }
        self.persisted = self.history.len();
        Ok(())
    }

    /// Read one line interactively. `rprompt` is the already-expanded
    /// right-side prompt text (zsh's `$RPS1`), or empty for none.
    pub fn read_line(
        &mut self,
        prompt: &str,
        rprompt: &str,
        hooks: &dyn Hooks,
    ) -> io::Result<ReadResult> {
        self.read_line_timeout(prompt, rprompt, hooks, None)
    }

    /// [`read_line`](Editor::read_line) with a deadline: when no complete
    /// line has been entered within `timeout` (measured from the call, not
    /// from the last keystroke — readline's `rl_readline_state` timeout,
    /// bash's `$TMOUT`), returns [`ReadResult::TimedOut`]. `None` never
    /// times out. On non-Unix builds the timeout is ignored (the fallback
    /// is a blocking buffered read).
    pub fn read_line_timeout(
        &mut self,
        prompt: &str,
        rprompt: &str,
        hooks: &dyn Hooks,
        timeout: Option<std::time::Duration>,
    ) -> io::Result<ReadResult> {
        #[cfg(unix)]
        {
            let deadline = timeout.map(|t| std::time::Instant::now() + t);
            // A non-tty stdin (a script piped into an "interactive"
            // host) can't enter raw mode; fall back to a plain silent
            // read, like readline does.
            if !term_sys::isatty_stdin() {
                return read_line_plain(deadline);
            }
            read_line_raw(self, prompt, rprompt, hooks, deadline)
        }
        #[cfg(not(unix))]
        {
            let _ = timeout;
            // No raw terminal on this platform: a plain buffered read
            // with no editing — a documented narrowing. Mirrors the Unix
            // fallback (`read_line_plain`): a non-tty stdin doesn't get the
            // prompt printed either, so a script piped into an
            // "interactive" host doesn't get prompt text mixed into its
            // captured output.
            let _ = (rprompt, hooks);
            if std::io::IsTerminal::is_terminal(&io::stdin()) {
                print!("{prompt}");
                io::stdout().flush()?;
            }
            let mut line = String::new();
            if io::stdin().read_line(&mut line)? == 0 {
                return Ok(ReadResult::Eof);
            }
            while line.ends_with(['\n', '\r']) {
                line.pop();
            }
            Ok(ReadResult::Line(line))
        }
    }
}

/// One decoded input event.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Key {
    Char(char),
    Ctrl(char), // Ctrl('a') for ^A …; Ctrl('_') for ^_
    Alt(char),
    AltBackspace, // ESC DEL — backward-kill-word
    Enter,
    Tab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    WordLeft,  // Ctrl/Alt-Left
    WordRight, // Ctrl/Alt-Right
    PageUp,
    PageDown,
    /// A bracketed paste: the whole pasted text as one event, so tabs and
    /// escape bytes inside it insert literally instead of firing bindings.
    Paste(String),
    Esc,
    Other,
}

/// Raw-mode RAII guard: restores the saved termios on drop, whatever the
/// exit path. Keeps both states so `suspend`/`resume` can hand the
/// terminal to an external `$EDITOR` (C-x C-e) and take it back.
#[cfg(unix)]
struct RawMode {
    saved: term_sys::Termios,
    raw: term_sys::Termios,
}

#[cfg(unix)]
impl RawMode {
    fn enable() -> io::Result<RawMode> {
        let saved = term_sys::tcgetattr_stdin()?;
        // Input: no Ctrl-S/Q flow control (freeing C-s for forward
        // search), no CR→NL mangling. Local: no canonical buffering,
        // no echo, no signal generation (^C becomes a key we handle),
        // no ^V. Output stays cooked so ordinary `println!` keeps
        // working for lists and job notices.
        let mut raw = saved;
        term_sys::apply_raw_flags(&mut raw);
        term_sys::tcsetattr_stdin_drain(&raw)?;
        Ok(RawMode { saved, raw })
    }

    /// Back to the shell's normal (cooked) state, for an external editor.
    fn suspend(&self) {
        let _ = term_sys::tcsetattr_stdin_drain(&self.saved);
    }

    fn resume(&self) {
        let _ = term_sys::tcsetattr_stdin_drain(&self.raw);
    }
}

#[cfg(unix)]
impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = term_sys::tcsetattr_stdin_drain(&self.saved);
    }
}

/// Bracketed-paste RAII guard: terminals wrap a paste in
/// `ESC[200~ … ESC[201~` while enabled, and the decoder turns that into a
/// single `Key::Paste` event.
#[cfg(unix)]
struct BracketedPaste;

#[cfg(unix)]
impl BracketedPaste {
    fn enable() -> BracketedPaste {
        print!("\x1b[?2004h");
        let _ = io::stdout().flush();
        BracketedPaste
    }
}

#[cfg(unix)]
impl Drop for BracketedPaste {
    fn drop(&mut self) {
        print!("\x1b[?2004l");
        let _ = io::stdout().flush();
    }
}

/// Whether fd 0 has a byte ready within `ms` milliseconds — the lone-ESC
/// vs escape-sequence disambiguation.
#[cfg(unix)]
fn input_ready(ms: i32) -> bool {
    term_sys::poll_stdin(ms)
}

/// One byte straight off fd 0 — deliberately *not* through
/// `io::stdin()`, whose userspace buffer would swallow the rest of an
/// escape sequence and make `input_ready`'s `poll` lie about it (the
/// arrow keys literally didn't work through the buffered reader).
#[cfg(unix)]
fn read_byte(hooks: &dyn Hooks) -> io::Result<Option<u8>> {
    loop {
        match term_sys::read_stdin_byte() {
            Ok(outcome) => return Ok(outcome),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                // A signal (e.g. a deferred TERM) landed mid-read; let
                // trap machinery see it at the next safe point and
                // keep reading.
                hooks.on_interrupted_read();
                continue;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Assemble one UTF-8 character whose first byte is `first`.
#[cfg(unix)]
fn read_utf8(hooks: &dyn Hooks, first: u8) -> io::Result<char> {
    let need = match first {
        0x00..=0x7f => 0,
        0xc0..=0xdf => 1,
        0xe0..=0xef => 2,
        _ => 3,
    };
    let mut buf = vec![first];
    for _ in 0..need {
        if let Some(b) = read_byte(hooks)? {
            buf.push(b);
        }
    }
    Ok(String::from_utf8_lossy(&buf)
        .chars()
        .next()
        .unwrap_or('\u{fffd}'))
}

/// Map a CSI escape sequence's final byte (plus parameters) to a key —
/// pure, so the quirk table is unit-testable. Modifier parameters `1;5`
/// (Ctrl) and `1;3` (Alt) on the arrows become word motions.
fn csi_key(params: &str, final_byte: u8) -> Key {
    match (params, final_byte) {
        ("1;5" | "1;3", b'C') => Key::WordRight,
        ("1;5" | "1;3", b'D') => Key::WordLeft,
        (_, b'A') => Key::Up,
        (_, b'B') => Key::Down,
        (_, b'C') => Key::Right,
        (_, b'D') => Key::Left,
        (_, b'H') => Key::Home,
        (_, b'F') => Key::End,
        ("1", b'~') | ("7", b'~') => Key::Home,
        ("4", b'~') | ("8", b'~') => Key::End,
        ("3", b'~') => Key::Delete,
        ("5", b'~') => Key::PageUp,
        ("6", b'~') => Key::PageDown,
        _ => Key::Other,
    }
}

/// The default emacs / vi-insert keymap, as data: what dispatch consults
/// for an unrebound key, and what [`Editor::bindings`] lists. Character
/// keys (self-insert) and the hardcoded C-x chord prefix are not rows.
static DEFAULT_BINDINGS: &[(Key, EditorAction)] = &[
    (Key::Enter, EditorAction::AcceptLine),
    (Key::Ctrl('c'), EditorAction::Interrupt),
    (Key::Ctrl('d'), EditorAction::DeleteCharOrEof),
    (Key::Delete, EditorAction::DeleteChar),
    // C-h arrives as the 0x08 Backspace byte, so it is the same row.
    (Key::Backspace, EditorAction::BackwardDeleteChar),
    (Key::Home, EditorAction::BeginningOfLine),
    (Key::Ctrl('a'), EditorAction::BeginningOfLine),
    (Key::End, EditorAction::EndOfLine),
    (Key::Ctrl('e'), EditorAction::EndOfLine),
    (Key::Right, EditorAction::ForwardChar),
    (Key::Ctrl('f'), EditorAction::ForwardChar),
    (Key::Left, EditorAction::BackwardChar),
    (Key::Ctrl('b'), EditorAction::BackwardChar),
    (Key::WordRight, EditorAction::ForwardWord),
    (Key::Alt('f'), EditorAction::ForwardWord),
    (Key::WordLeft, EditorAction::BackwardWord),
    (Key::Alt('b'), EditorAction::BackwardWord),
    (Key::Ctrl('k'), EditorAction::KillLine),
    (Key::Ctrl('u'), EditorAction::UnixLineDiscard),
    (Key::Ctrl('w'), EditorAction::UnixWordRubout),
    (Key::Alt('d'), EditorAction::KillWord),
    (Key::AltBackspace, EditorAction::BackwardKillWord),
    (Key::Ctrl('y'), EditorAction::Yank),
    (Key::Alt('y'), EditorAction::YankPop),
    (Key::Ctrl('t'), EditorAction::TransposeChars),
    (Key::Alt('t'), EditorAction::TransposeWords),
    (Key::Alt('u'), EditorAction::UpcaseWord),
    (Key::Alt('l'), EditorAction::DowncaseWord),
    (Key::Alt('c'), EditorAction::CapitalizeWord),
    (Key::Ctrl('_'), EditorAction::Undo),
    (Key::Ctrl('z'), EditorAction::Undo),
    (Key::Alt('r'), EditorAction::RevertLine),
    (Key::Alt('.'), EditorAction::InsertLastArgument),
    (Key::Alt('_'), EditorAction::InsertLastArgument),
    (Key::Up, EditorAction::PreviousHistory),
    (Key::Ctrl('p'), EditorAction::PreviousHistory),
    (Key::Down, EditorAction::NextHistory),
    (Key::Ctrl('n'), EditorAction::NextHistory),
    (Key::Alt('<'), EditorAction::BeginningOfHistory),
    (Key::Alt('>'), EditorAction::EndOfHistory),
    (Key::PageUp, EditorAction::HistorySearchBackward),
    (Key::Alt('p'), EditorAction::HistorySearchBackward),
    (Key::PageDown, EditorAction::HistorySearchForward),
    (Key::Alt('n'), EditorAction::HistorySearchForward),
    (Key::Ctrl('r'), EditorAction::ReverseSearchHistory),
    (Key::Ctrl('s'), EditorAction::ForwardSearchHistory),
    (Key::Ctrl('l'), EditorAction::ClearScreen),
    (Key::Tab, EditorAction::Complete),
    (Key::Ctrl('v'), EditorAction::QuotedInsert),
    (Key::Ctrl('q'), EditorAction::QuotedInsert),
];

/// The default action for a key: character keys self-insert, everything
/// else comes from the [`DEFAULT_BINDINGS`] table.
fn default_action(key: &Key) -> Option<EditorAction> {
    if matches!(key, Key::Char(_)) {
        return Some(EditorAction::SelfInsert);
    }
    DEFAULT_BINDINGS
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, a)| *a)
}

/// Parse a readline key spec — `\C-x`, `\M-f`, `\e[1;5C`, backslash
/// escapes (`\e \t \r \n \a \d \\ \xHH \NNN`), plain characters — into
/// the single key it decodes to. `None` for anything unparseable,
/// including multi-key chords.
fn parse_key_spec(spec: &str) -> Option<Key> {
    let mut bytes: Vec<u8> = Vec::new();
    let mut chars = spec.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next()? {
            'C' => {
                if chars.next()? != '-' {
                    return None;
                }
                // The control target may itself be escaped (`\C-\\`).
                let t = match chars.next()? {
                    '\\' => chars.next()?,
                    t => t,
                };
                bytes.push(match t {
                    '?' => 0x7f,
                    t if t.is_ascii() => (t as u8) & 0x1f,
                    _ => return None,
                });
            }
            // Meta is an ESC prefix; the rest of the spec parses as
            // usual, so `\M-\C-?` works.
            'M' => {
                if chars.next()? != '-' {
                    return None;
                }
                bytes.push(0x1b);
            }
            'e' | 'E' => bytes.push(0x1b),
            '\\' => bytes.push(b'\\'),
            'a' => bytes.push(0x07),
            'b' => bytes.push(0x08),
            'd' => bytes.push(0x7f),
            'f' => bytes.push(0x0c),
            'n' => bytes.push(b'\n'),
            'r' => bytes.push(b'\r'),
            't' => bytes.push(b'\t'),
            'v' => bytes.push(0x0b),
            'x' => {
                let mut v: u32 = 0;
                let mut seen = 0;
                while seen < 2
                    && let Some(d) = chars.peek().and_then(|c| c.to_digit(16))
                {
                    v = v * 16 + d;
                    chars.next();
                    seen += 1;
                }
                if seen == 0 {
                    return None;
                }
                bytes.push(v as u8);
            }
            d @ '0'..='7' => {
                let mut v: u32 = d.to_digit(8).unwrap();
                let mut seen = 1;
                while seen < 3
                    && let Some(d) = chars.peek().and_then(|c| c.to_digit(8))
                {
                    v = v * 8 + d;
                    chars.next();
                    seen += 1;
                }
                if v > 0xff {
                    return None;
                }
                bytes.push(v as u8);
            }
            _ => return None,
        }
    }
    decode_key_bytes(&bytes)
}

/// Decode a complete byte sequence into exactly one key — the pure twin
/// of [`read_key`]'s streaming decoder, for the binding parser. `None`
/// when the bytes are empty, leave a remainder, or decode to nothing
/// recognizable.
fn decode_key_bytes(bytes: &[u8]) -> Option<Key> {
    let (&b, rest) = bytes.split_first()?;
    let one = |key: Key| if rest.is_empty() { Some(key) } else { None };
    match b {
        b'\r' | b'\n' => one(Key::Enter),
        b'\t' => one(Key::Tab),
        0x7f | 0x08 => one(Key::Backspace),
        0x1b => decode_escape_bytes(rest),
        0x1f => one(Key::Ctrl('_')),
        0x01..=0x1a => one(Key::Ctrl((b - 1 + b'a') as char)),
        _ => {
            let s = std::str::from_utf8(bytes).ok()?;
            let mut chars = s.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            Some(Key::Char(c))
        }
    }
}

/// The post-ESC half of [`decode_key_bytes`].
fn decode_escape_bytes(rest: &[u8]) -> Option<Key> {
    let Some((&b, rest)) = rest.split_first() else {
        return Some(Key::Esc);
    };
    let one = |key: Key| if rest.is_empty() { Some(key) } else { None };
    match b {
        b'[' => {
            let params_len = rest
                .iter()
                .take_while(|&&c| c.is_ascii_digit() || c == b';')
                .count();
            let (params, fin) = rest.split_at(params_len);
            let (&final_byte, tail) = fin.split_first()?;
            if !tail.is_empty() {
                return None;
            }
            match csi_key(std::str::from_utf8(params).ok()?, final_byte) {
                Key::Other => None,
                key => Some(key),
            }
        }
        b'O' => {
            let (&f, tail) = rest.split_first()?;
            if !tail.is_empty() {
                return None;
            }
            match f {
                b'H' => Some(Key::Home),
                b'F' => Some(Key::End),
                b'A' => Some(Key::Up),
                b'B' => Some(Key::Down),
                b'C' => Some(Key::Right),
                b'D' => Some(Key::Left),
                _ => None,
            }
        }
        0x7f => one(Key::AltBackspace),
        c if c.is_ascii_graphic() => one(Key::Alt(c as char)),
        _ => None,
    }
}

/// Render a key back to a readline-style spec — [`Editor::bindings`]'s
/// output, chosen so it round-trips through [`parse_key_spec`].
fn key_spec(key: &Key) -> String {
    match key {
        Key::Char(c) => c.to_string(),
        Key::Ctrl(c) => format!("\\C-{c}"),
        Key::Alt(c) => format!("\\M-{c}"),
        Key::AltBackspace => "\\M-\\C-?".to_string(),
        Key::Enter => "\\C-m".to_string(),
        Key::Tab => "\\C-i".to_string(),
        Key::Backspace => "\\C-?".to_string(),
        Key::Delete => "\\e[3~".to_string(),
        Key::Up => "\\e[A".to_string(),
        Key::Down => "\\e[B".to_string(),
        Key::Right => "\\e[C".to_string(),
        Key::Left => "\\e[D".to_string(),
        Key::Home => "\\e[H".to_string(),
        Key::End => "\\e[F".to_string(),
        Key::WordLeft => "\\e[1;5D".to_string(),
        Key::WordRight => "\\e[1;5C".to_string(),
        Key::PageUp => "\\e[5~".to_string(),
        Key::PageDown => "\\e[6~".to_string(),
        Key::Esc => "\\e".to_string(),
        Key::Paste(_) | Key::Other => String::new(),
    }
}

/// Collect a bracketed paste: everything up to the closing `ESC[201~`.
#[cfg(unix)]
fn read_paste(hooks: &dyn Hooks) -> io::Result<Key> {
    const END: &[u8] = b"\x1b[201~";
    let mut buf: Vec<u8> = Vec::new();
    while !buf.ends_with(END) {
        match read_byte(hooks)? {
            Some(b) => buf.push(b),
            None => break,
        }
    }
    if buf.ends_with(END) {
        buf.truncate(buf.len() - END.len());
    }
    Ok(Key::Paste(String::from_utf8_lossy(&buf).into_owned()))
}

#[cfg(unix)]
fn read_key(hooks: &dyn Hooks) -> io::Result<Option<Key>> {
    let Some(b) = read_byte(hooks)? else {
        return Ok(None);
    };
    Ok(Some(match b {
        b'\r' | b'\n' => Key::Enter,
        b'\t' => Key::Tab,
        0x7f | 0x08 => Key::Backspace,
        0x1b => {
            if !input_ready(30) {
                return Ok(Some(Key::Esc));
            }
            match read_byte(hooks)? {
                Some(b'[') => {
                    let mut params = String::new();
                    loop {
                        match read_byte(hooks)? {
                            Some(c @ (b'0'..=b'9' | b';')) => params.push(c as char),
                            Some(final_byte) => {
                                if params == "200" && final_byte == b'~' {
                                    return read_paste(hooks).map(Some);
                                }
                                return Ok(Some(csi_key(&params, final_byte)));
                            }
                            None => return Ok(Some(Key::Other)),
                        }
                    }
                }
                Some(b'O') => match read_byte(hooks)? {
                    Some(b'H') => Key::Home,
                    Some(b'F') => Key::End,
                    Some(b'A') => Key::Up,
                    Some(b'B') => Key::Down,
                    Some(b'C') => Key::Right,
                    Some(b'D') => Key::Left,
                    _ => Key::Other,
                },
                Some(0x7f) => Key::AltBackspace,
                Some(c) if c.is_ascii_graphic() => Key::Alt(c as char),
                _ => Key::Other,
            }
        }
        0x1f => Key::Ctrl('_'),
        0x01..=0x1a => Key::Ctrl((b - 1 + b'a') as char),
        _ => Key::Char(read_utf8(hooks, b)?),
    }))
}

/// Terminal width in columns (fallback 80).
#[cfg(unix)]
fn term_cols() -> usize {
    term_sys::term_cols_stdout().unwrap_or(80)
}

/// Display width of `s`, skipping ANSI SGR escape sequences — the prompt
/// and the highlighted buffer both carry them.
fn display_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthChar;
    let mut w = 0;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip `ESC [ ... final` (or a lone two-char escape).
            if chars.peek() == Some(&'[') {
                chars.next();
                for e in chars.by_ref() {
                    if e.is_ascii_alphabetic() || e == '~' {
                        break;
                    }
                }
            } else {
                chars.next();
            }
            continue;
        }
        w += c.width().unwrap_or(0);
    }
    w
}

/// The buffer as painted: control characters shown readline-style —
/// `^X` for C0 bytes, `^?` for DEL, `⏎` for an embedded newline (a
/// multi-line bracketed paste), a tab as four spaces. The raw buffer is
/// what Enter returns; this transform exists only so the render and its
/// cursor math never emit a raw control byte at the terminal.
#[cfg(unix)]
fn visualize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push('⏎'),
            '\t' => out.push_str("    "),
            '\u{7f}' => out.push_str("^?"),
            c if (c as u32) < 0x20 => {
                out.push('^');
                out.push(((c as u8) ^ 0x40) as char);
            }
            c => out.push(c),
        }
    }
    out
}

/// What the previous key did — drives undo coalescing (runs of plain
/// inserts undo as one unit), kill-ring appending (consecutive kills grow
/// one ring entry), yank-pop eligibility, and M-. cycling.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Insert,
    KillFwd,
    KillBack,
    Yank,
    LastArg,
    Undo,
    Other,
}

/// The per-`read_line` editing state.
#[cfg(unix)]
struct LineState<'a> {
    buffer: String,
    cursor: usize, // byte offset into buffer
    prompt: &'a str,
    rprompt: &'a str,
    /// Rows the previous paint occupied, and which row the cursor was
    /// left on — the starting point for the next repaint.
    painted_rows: usize,
    painted_cursor_row: usize,
    /// History navigation: index into `history` (None = live line), the
    /// draft stashed when navigation started, and the anchored prefix for
    /// PageUp/PageDown prefix search.
    hist_index: Option<usize>,
    draft: String,
    prefix: String,
    /// vi mode: true when in normal mode; `vi_op` holds a pending
    /// `d`/`c`/`y` operator, `vi_find` a pending `f F t T`, `vi_count`
    /// the accumulated count, `vi_replace` a pending `r`, `last_find`
    /// the target `;`/`,` repeat.
    vi: bool,
    vi_normal: bool,
    vi_count: usize,
    vi_op: Option<char>,
    vi_find: Option<char>,
    vi_replace: bool,
    last_find: Option<(char, char)>,
    /// Ctrl-R/Ctrl-S incremental search, when active.
    search: Option<SearchState>,
    /// Undo stack: (buffer, cursor) snapshots taken before mutations.
    undo: Vec<(String, usize)>,
    prev_action: Action,
    this_action: Action,
    /// Last yank's span and ring index, for M-y yank-pop.
    yank: Option<(usize, usize, usize)>,
    /// Last M-. insertion: (history index, span start, span end).
    lastarg: Option<(usize, usize, usize)>,
    /// Completion menu cycling state, armed by the candidate list and
    /// cleared by any non-Tab key.
    menu: Option<MenuState>,
    /// The editor's readline-variable knobs, copied per read_line.
    cfg: EditorConfig,
    hooks: &'a dyn Hooks,
}

/// Repeated-Tab candidate cycling (zsh `AUTO_MENU`): the word span being
/// replaced and the candidate list captured when it was printed.
#[cfg(unix)]
struct MenuState {
    /// Byte offset the completed word starts at.
    start: usize,
    /// Length of the replacement currently in the buffer.
    inserted: usize,
    /// Candidate currently inserted; `None` until the first cycle.
    index: Option<usize>,
    candidates: Vec<Candidate>,
}

#[cfg(unix)]
struct SearchState {
    query: String,
    /// Index into history of the current match, if any.
    hit: Option<usize>,
    /// Search direction: C-r steps older, C-s newer.
    forward: bool,
}

#[cfg(unix)]
fn read_line_raw(
    ed: &mut Editor,
    prompt: &str,
    rprompt: &str,
    hooks: &dyn Hooks,
    deadline: Option<std::time::Instant>,
) -> io::Result<ReadResult> {
    let raw = RawMode::enable()?;
    let _paste = BracketedPaste::enable();
    let cfg = ed.cfg;
    let Editor {
        history,
        kill_ring,
        bindings,
        ..
    } = ed;
    let mut st = LineState {
        buffer: String::new(),
        cursor: 0,
        prompt,
        rprompt,
        painted_rows: 1,
        painted_cursor_row: 0,
        hist_index: None,
        draft: String::new(),
        prefix: String::new(),
        vi: hooks.vi_mode(),
        vi_normal: false,
        vi_count: 0,
        vi_op: None,
        vi_find: None,
        vi_replace: false,
        last_find: None,
        search: None,
        undo: Vec::new(),
        prev_action: Action::Other,
        this_action: Action::Other,
        yank: None,
        lastarg: None,
        menu: None,
        cfg,
        hooks,
    };
    render(&mut st, history)?;

    let mut cols = term_cols();
    loop {
        // Idle wait: repaint if the terminal was resized while sitting at
        // the prompt (no SIGWINCH handler — the 200ms poll tick notices).
        // The tick also gives the host a beat to fire pending signal traps
        // even when no input arrives to interrupt. The read_line_timeout
        // deadline is checked on the same tick (and between keystrokes).
        loop {
            if let Some(d) = deadline
                && std::time::Instant::now() >= d
            {
                finish_line(&mut st)?;
                return Ok(ReadResult::TimedOut);
            }
            if input_ready(200) {
                break;
            }
            hooks.on_interrupted_read();
            let now = term_cols();
            if now != cols {
                cols = now;
                render(&mut st, history)?;
            }
        }

        let Some(key) = read_key(hooks)? else {
            // EOF on stdin itself.
            finish_line(&mut st)?;
            return Ok(ReadResult::Eof);
        };

        // Ctrl-R/Ctrl-S search intercepts everything while active.
        if st.search.is_some() {
            match handle_search_key(&mut st, key, history)? {
                SearchOutcome::Continue | SearchOutcome::Exit => {
                    render(&mut st, history)?;
                    continue;
                }
                SearchOutcome::Accept => {
                    finish_line(&mut st)?;
                    return Ok(ReadResult::Line(st.buffer));
                }
            }
        }

        let snapshot = (st.buffer.clone(), st.cursor);
        st.this_action = Action::Other;

        // Any key ends candidate cycling, unless it performs completion
        // itself (Tab by default; rebindable).
        if !key_completes(&key, bindings, st.vi && st.vi_normal) {
            st.menu = None;
        }

        let after = if st.vi && st.vi_normal {
            // vi normal mode is a fixed keymap; only the global controls
            // (line termination, search, undo, C-x chords, quoted insert,
            // `v` external edit) are shared with the binding path.
            match key {
                Key::Enter => AfterKey::Accept,
                Key::Ctrl('c') => AfterKey::Interrupted,
                Key::Ctrl('d') if st.buffer.is_empty() => AfterKey::Eof,
                Key::Ctrl('r') => {
                    start_search(&mut st, false);
                    AfterKey::Done
                }
                Key::Ctrl('s') => {
                    start_search(&mut st, true);
                    AfterKey::Done
                }
                Key::Ctrl('l') => {
                    clear_screen(&mut st);
                    AfterKey::Done
                }
                Key::Ctrl('_') | Key::Ctrl('z') => {
                    undo_cmd(&mut st);
                    AfterKey::Done
                }
                Key::Ctrl('x') => ctrl_x_chord(&mut st)?,
                Key::Ctrl('v') | Key::Ctrl('q') => {
                    quoted_insert(&mut st)?;
                    AfterKey::Done
                }
                Key::Tab => run_action(&mut st, EditorAction::Complete, &key, history, kill_ring)?,
                Key::Char('v') if st.vi_op.is_none() && st.vi_find.is_none() && !st.vi_replace => {
                    // vi normal-mode `v`: edit the line in $EDITOR,
                    // readline's own vi binding.
                    AfterKey::External
                }
                key => {
                    handle_vi_normal(&mut st, key, history, kill_ring);
                    AfterKey::Done
                }
            }
        } else {
            match key {
                Key::Paste(s) => {
                    // Insert the paste verbatim (normalizing line
                    // endings) — no completion, no abbreviations, no
                    // history motion, no bindings.
                    let s = s.replace("\r\n", "\n").replace('\r', "\n");
                    st.buffer.insert_str(st.cursor, &s);
                    st.cursor += s.len();
                    AfterKey::Done
                }
                Key::Esc if st.vi => {
                    st.vi_normal = true;
                    // vi leaves the cursor on the last inserted character.
                    if let Some(prev) = prev_char_start(&st.buffer, st.cursor) {
                        st.cursor = prev;
                    }
                    AfterKey::Done
                }
                // Emacs / vi-insert: host rebindings first, then the
                // default keymap.
                key => match bindings.iter().find(|(k, _)| *k == key) {
                    Some((_, Binding::Unbound)) => AfterKey::Done,
                    Some((_, Binding::Host(tag))) => {
                        run_host_binding(&mut st, &raw, tag)?;
                        AfterKey::Done
                    }
                    Some((_, Binding::Action(action))) => {
                        run_action(&mut st, *action, &key, history, kill_ring)?
                    }
                    None if key == Key::Ctrl('x') => ctrl_x_chord(&mut st)?,
                    None => match default_action(&key) {
                        Some(action) => run_action(&mut st, action, &key, history, kill_ring)?,
                        None => AfterKey::Done,
                    },
                },
            }
        };
        match after {
            AfterKey::Done => {}
            AfterKey::Accept => {
                finish_line(&mut st)?;
                return Ok(ReadResult::Line(st.buffer));
            }
            AfterKey::Interrupted => {
                finish_line(&mut st)?;
                return Ok(ReadResult::Interrupted);
            }
            AfterKey::Eof => {
                finish_line(&mut st)?;
                return Ok(ReadResult::Eof);
            }
            AfterKey::External => {
                if let Some(line) = edit_in_editor(&mut st, &raw)? {
                    return Ok(ReadResult::Line(line));
                }
            }
        }

        // Undo bookkeeping: snapshot any mutation, coalescing runs of
        // plain self-insert (readline groups those too).
        if st.buffer != snapshot.0 && st.this_action != Action::Undo {
            let coalesce = st.this_action == Action::Insert && st.prev_action == Action::Insert;
            if !coalesce {
                st.undo.push(snapshot);
                if st.undo.len() > 200 {
                    st.undo.remove(0);
                }
            }
        }
        st.prev_action = st.this_action;

        render(&mut st, history)?;
    }
}

/// Move to the end of the painted region and start a fresh terminal line,
/// so whatever runs next begins below the edit region.
#[cfg(unix)]
fn finish_line(st: &mut LineState) -> io::Result<()> {
    let down = st.painted_rows.saturating_sub(1 + st.painted_cursor_row);
    if down > 0 {
        print!("\x1b[{down}B");
    }
    println!();
    io::stdout().flush()
}

/// Whether a key resolves to a completion action — the one case that
/// must not tear down the menu-cycling state before dispatch.
#[cfg(unix)]
fn key_completes(key: &Key, bindings: &[(Key, Binding)], vi_normal: bool) -> bool {
    if vi_normal {
        // vi normal mode's keymap is fixed: only Tab completes there.
        return *key == Key::Tab;
    }
    let action = match bindings.iter().find(|(k, _)| k == key) {
        Some((_, Binding::Action(a))) => Some(*a),
        Some(_) => None,
        None => default_action(key),
    };
    matches!(
        action,
        Some(EditorAction::Complete | EditorAction::MenuComplete)
    )
}

/// What the main loop does after a key's action ran: nothing, end the
/// read (three ways), or hand the line to the external editor (which
/// needs the loop's `RawMode` handle).
#[cfg(unix)]
enum AfterKey {
    Done,
    Accept,
    Interrupted,
    Eof,
    External,
}

/// Enter C-r / C-s incremental search.
#[cfg(unix)]
fn start_search(st: &mut LineState, forward: bool) {
    st.search = Some(SearchState {
        query: String::new(),
        hit: None,
        forward,
    });
}

/// C-l: clear the screen; the next render repaints at the top.
#[cfg(unix)]
fn clear_screen(st: &mut LineState) {
    print!("\x1b[2J\x1b[H");
    st.painted_rows = 1;
    st.painted_cursor_row = 0;
}

/// The readline C-x chords supported: C-x C-e (edit the line in
/// $EDITOR) and C-x C-u (undo).
#[cfg(unix)]
fn ctrl_x_chord(st: &mut LineState) -> io::Result<AfterKey> {
    Ok(match read_key(st.hooks)? {
        Some(Key::Ctrl('e')) => AfterKey::External,
        Some(Key::Ctrl('u')) => {
            undo_cmd(st);
            AfterKey::Done
        }
        _ => AfterKey::Done,
    })
}

/// Ring the terminal bell per the configured style.
#[cfg(unix)]
fn bell(style: BellStyle) -> io::Result<()> {
    match style {
        BellStyle::None => return Ok(()),
        BellStyle::Audible => print!("\x07"),
        // Reverse-video flip — the flash without terminfo's `flash`.
        BellStyle::Visible => print!("\x1b[?5h\x1b[?5l"),
    }
    io::stdout().flush()
}

/// A `bind_host` key: suspend raw mode, hand the line and cursor to the
/// host (bash's `bind -x`, `READLINE_LINE`/`READLINE_POINT`), take back
/// whatever it wrote, and repaint on a fresh region.
#[cfg(unix)]
fn run_host_binding(st: &mut LineState, raw: &RawMode, tag: &str) -> io::Result<()> {
    finish_line(st)?;
    raw.suspend();
    let mut line = std::mem::take(&mut st.buffer);
    let mut cursor = st.cursor;
    st.hooks.host_binding(tag, &mut line, &mut cursor);
    raw.resume();
    st.buffer = line;
    st.cursor = cursor.min(st.buffer.len());
    while !st.buffer.is_char_boundary(st.cursor) {
        st.cursor -= 1;
    }
    st.painted_rows = 1;
    st.painted_cursor_row = 0;
    Ok(())
}

/// Execute one named action — the emacs (and vi-insert) command set.
/// `key` supplies the character for `SelfInsert`.
#[cfg(unix)]
fn run_action(
    st: &mut LineState,
    action: EditorAction,
    key: &Key,
    history: &[String],
    ring: &mut Vec<String>,
) -> io::Result<AfterKey> {
    match action {
        EditorAction::AcceptLine => return Ok(AfterKey::Accept),
        EditorAction::Interrupt => return Ok(AfterKey::Interrupted),
        EditorAction::DeleteCharOrEof if st.buffer.is_empty() => return Ok(AfterKey::Eof),
        EditorAction::EditAndExecuteCommand => return Ok(AfterKey::External),
        EditorAction::SelfInsert => {
            let &Key::Char(c) = key else {
                return Ok(AfterKey::Done); // only character keys self-insert
            };
            // Abbreviations (fish-style): a space after one defined in
            // command position rewrites it in place first.
            if c == ' '
                && let Some((start, expansion)) =
                    st.hooks.expand_abbreviation(&st.buffer, st.cursor)
            {
                st.buffer.replace_range(start..st.cursor, &expansion);
                st.cursor = start + expansion.len();
            }
            insert_char(st, c);
            st.this_action = Action::Insert;
        }
        EditorAction::BackwardDeleteChar => {
            if let Some(prev) = prev_char_start(&st.buffer, st.cursor) {
                st.buffer.replace_range(prev..st.cursor, "");
                st.cursor = prev;
            }
        }
        EditorAction::DeleteChar | EditorAction::DeleteCharOrEof => {
            if let Some(next) = next_char_end(&st.buffer, st.cursor) {
                st.buffer.replace_range(st.cursor..next, "");
            }
        }
        EditorAction::BackwardChar => {
            if let Some(prev) = prev_char_start(&st.buffer, st.cursor) {
                st.cursor = prev;
            }
        }
        EditorAction::ForwardChar => {
            // At end of line, the right arrow accepts the history hint.
            if st.cursor == st.buffer.len() {
                if let Some(hint) = st.hooks.hint(&st.buffer, history) {
                    st.buffer.push_str(&hint);
                    st.cursor = st.buffer.len();
                }
            } else if let Some(next) = next_char_end(&st.buffer, st.cursor) {
                st.cursor = next;
            }
        }
        EditorAction::BeginningOfLine => st.cursor = 0,
        EditorAction::EndOfLine => {
            // End at end-of-line also accepts the hint (fish's behavior).
            if st.cursor == st.buffer.len()
                && let Some(hint) = st.hooks.hint(&st.buffer, history)
            {
                st.buffer.push_str(&hint);
            }
            st.cursor = st.buffer.len();
        }
        EditorAction::BackwardWord => st.cursor = word_back_alnum(&st.buffer, st.cursor),
        EditorAction::ForwardWord => {
            // At end of line, accept one word of the history hint
            // (fish's forward-word on an autosuggestion).
            if st.cursor == st.buffer.len() {
                if let Some(hint) = st.hooks.hint(&st.buffer, history) {
                    let take = word_forward_alnum(&hint, 0);
                    st.buffer.push_str(&hint[..take]);
                    st.cursor = st.buffer.len();
                }
            } else {
                st.cursor = word_forward_alnum(&st.buffer, st.cursor);
            }
        }
        EditorAction::KillLine => kill_span(st, ring, st.cursor, st.buffer.len(), true),
        EditorAction::UnixLineDiscard => kill_span(st, ring, 0, st.cursor, false),
        EditorAction::UnixWordRubout => {
            // unix-word-rubout: whitespace-delimited, unlike M-Backspace.
            let start = word_back(&st.buffer, st.cursor);
            kill_span(st, ring, start, st.cursor, false);
        }
        EditorAction::KillWord => {
            let end = word_forward_alnum(&st.buffer, st.cursor);
            kill_span(st, ring, st.cursor, end, true);
        }
        EditorAction::BackwardKillWord => {
            let start = word_back_alnum(&st.buffer, st.cursor);
            kill_span(st, ring, start, st.cursor, false);
        }
        EditorAction::Yank => yank(st, ring),
        EditorAction::YankPop => yank_pop(st, ring),
        EditorAction::TransposeChars => transpose(st),
        EditorAction::TransposeWords => transpose_words(st),
        EditorAction::UpcaseWord => case_word(st, CaseOp::Upper),
        EditorAction::DowncaseWord => case_word(st, CaseOp::Lower),
        EditorAction::CapitalizeWord => case_word(st, CaseOp::Capital),
        EditorAction::Undo => undo_cmd(st),
        EditorAction::RevertLine => {
            // readline revert-line: undo every edit to this line at once.
            if let Some((buf, cur)) = st.undo.first().cloned() {
                st.cursor = cur.min(buf.len());
                st.buffer = buf;
                st.undo.clear();
            }
            st.this_action = Action::Undo; // don't snapshot the revert
        }
        EditorAction::InsertLastArgument => insert_last_arg(st, history),
        EditorAction::BeginningOfHistory => history_first(st, history),
        EditorAction::EndOfHistory => history_last(st),
        EditorAction::PreviousHistory => history_prev(st, history),
        EditorAction::NextHistory => history_next(st, history),
        EditorAction::HistorySearchBackward => history_prefix_prev(st, history),
        EditorAction::HistorySearchForward => history_prefix_next(st, history),
        EditorAction::ReverseSearchHistory => start_search(st, false),
        EditorAction::ForwardSearchHistory => start_search(st, true),
        EditorAction::ClearScreen => clear_screen(st),
        EditorAction::Complete => {
            if st.menu.is_some() {
                menu_next(st);
            } else if st.cfg.menu_complete {
                menu_complete_start(st)?;
            } else {
                complete_at_cursor(st)?;
            }
        }
        EditorAction::MenuComplete => {
            if st.menu.is_some() {
                menu_next(st);
            } else {
                menu_complete_start(st)?;
            }
        }
        EditorAction::QuotedInsert => quoted_insert(st)?,
    }
    Ok(AfterKey::Done)
}

/// vi normal mode: counts, the `d`/`c`/`y` operators over motions
/// (`h l 0 ^ $ w W b B e E f F t T ; ,` and doubled `dd cc yy`), edits
/// `x X D C s S Y r ~ p P u`, inserts `i I a A`, history `k j`. See
/// the README for what's deliberately not modeled (registers,
/// `.` repeat, `/` search).
#[cfg(unix)]
fn handle_vi_normal(st: &mut LineState, key: Key, history: &[String], ring: &mut Vec<String>) {
    // A pending `r`: the next character replaces the one under the cursor.
    if st.vi_replace {
        st.vi_replace = false;
        st.vi_count = 0;
        if let Key::Char(c) = key
            && let Some(next) = next_char_end(&st.buffer, st.cursor)
        {
            st.buffer.replace_range(st.cursor..next, &c.to_string());
        }
        return;
    }
    // A pending `f F t T`: the next character is the find target.
    if let Some(kind) = st.vi_find.take() {
        let n = st.vi_count.max(1);
        st.vi_count = 0;
        if let Key::Char(c) = key {
            st.last_find = Some((kind, c));
            let mut hit: Option<(usize, bool)> = None;
            let mut from = st.cursor;
            for _ in 0..n {
                match vi_find_target(&st.buffer, from, kind, c) {
                    Some((t, inc)) => {
                        hit = Some((t, inc));
                        from = t;
                    }
                    None => {
                        hit = None;
                        break;
                    }
                }
            }
            match (hit, st.vi_op.take()) {
                (Some((t, inc)), Some(op)) => vi_apply_op(st, ring, op, t, inc),
                (Some((t, _)), None) => st.cursor = t,
                (None, _) => {}
            }
        } else {
            st.vi_op = None;
        }
        return;
    }
    // Count accumulation (`0` is a motion unless a count has started).
    if let Key::Char(c @ '0'..='9') = key
        && (c != '0' || st.vi_count > 0)
    {
        st.vi_count = st.vi_count * 10 + (c as usize - '0' as usize);
        return;
    }

    let n = st.vi_count.max(1);

    // Motions: resolve to a target position (and operator inclusivity).
    let motion: Option<(usize, bool)> = match &key {
        Key::Char('h') | Key::Left | Key::Backspace => {
            Some((back_n(&st.buffer, st.cursor, n), false))
        }
        Key::Char('l' | ' ') | Key::Right => Some((fwd_n(&st.buffer, st.cursor, n), false)),
        Key::Char('0') | Key::Home => Some((0, false)),
        Key::Char('^') => Some((first_nonblank(&st.buffer), false)),
        Key::Char('$') | Key::End => Some((st.buffer.len(), false)),
        Key::Char('w' | 'W') => {
            // vi quirk: `cw` behaves like `ce`.
            if st.vi_op == Some('c') {
                Some((apply_n(&st.buffer, st.cursor, n, vi_word_end), true))
            } else {
                Some((apply_n(&st.buffer, st.cursor, n, vi_word_fwd), false))
            }
        }
        Key::Char('b' | 'B') => Some((apply_n(&st.buffer, st.cursor, n, vi_word_back), false)),
        Key::Char('e' | 'E') => Some((apply_n(&st.buffer, st.cursor, n, vi_word_end), true)),
        Key::Char(k @ ('f' | 'F' | 't' | 'T')) => {
            st.vi_find = Some(*k);
            return;
        }
        Key::Char(sc @ (';' | ',')) => st.last_find.and_then(|(kind, target)| {
            let k = if *sc == ',' { invert_find(kind) } else { kind };
            vi_find_target(&st.buffer, st.cursor, k, target)
        }),
        _ => None,
    };
    if let Some((target, inclusive)) = motion {
        st.vi_count = 0;
        if let Some(op) = st.vi_op.take() {
            vi_apply_op(st, ring, op, target, inclusive);
        } else {
            st.cursor = target.min(st.buffer.len());
        }
        return;
    }

    match key {
        Key::Char(o @ ('d' | 'c' | 'y')) => {
            if st.vi_op == Some(o) {
                // dd / cc / yy: linewise.
                st.vi_op = None;
                match o {
                    'd' => kill_span(st, ring, 0, st.buffer.len(), true),
                    'c' => {
                        kill_span(st, ring, 0, st.buffer.len(), true);
                        st.vi_normal = false;
                    }
                    _ => {
                        if !st.buffer.is_empty() {
                            push_ring(ring, st.buffer.clone());
                        }
                        st.cursor = 0;
                    }
                }
            } else {
                st.vi_op = Some(o);
                return; // keep any count for the motion
            }
        }
        Key::Char('x') => {
            let end = fwd_n(&st.buffer, st.cursor, n);
            kill_span(st, ring, st.cursor, end, true);
        }
        Key::Char('X') => {
            let start = back_n(&st.buffer, st.cursor, n);
            kill_span(st, ring, start, st.cursor, false);
        }
        Key::Char('s') => {
            let end = fwd_n(&st.buffer, st.cursor, n);
            kill_span(st, ring, st.cursor, end, true);
            st.vi_normal = false;
        }
        Key::Char('D') => kill_span(st, ring, st.cursor, st.buffer.len(), true),
        Key::Char('C') => {
            kill_span(st, ring, st.cursor, st.buffer.len(), true);
            st.vi_normal = false;
        }
        Key::Char('S') => {
            kill_span(st, ring, 0, st.buffer.len(), true);
            st.vi_normal = false;
        }
        Key::Char('Y') => {
            if !st.buffer.is_empty() {
                push_ring(ring, st.buffer.clone());
            }
        }
        Key::Char('r') => {
            st.vi_replace = true;
            return;
        }
        Key::Char('~') => {
            for _ in 0..n {
                let Some(next) = next_char_end(&st.buffer, st.cursor) else {
                    break;
                };
                let flipped: String = st.buffer[st.cursor..next]
                    .chars()
                    .flat_map(|c| {
                        if c.is_uppercase() {
                            c.to_lowercase().collect::<Vec<_>>()
                        } else {
                            c.to_uppercase().collect()
                        }
                    })
                    .collect();
                let len = flipped.len();
                st.buffer.replace_range(st.cursor..next, &flipped);
                st.cursor += len;
            }
        }
        Key::Char('p') => {
            if let Some(text) = ring.last().cloned()
                && !text.is_empty()
            {
                let at = next_char_end(&st.buffer, st.cursor).unwrap_or(st.cursor);
                st.buffer.insert_str(at, &text);
                st.cursor = prev_char_start(&st.buffer, at + text.len()).unwrap_or(at);
            }
        }
        Key::Char('P') => {
            if let Some(text) = ring.last().cloned()
                && !text.is_empty()
            {
                let at = st.cursor;
                st.buffer.insert_str(at, &text);
                st.cursor = prev_char_start(&st.buffer, at + text.len()).unwrap_or(at);
            }
        }
        Key::Char('u') => undo_cmd(st),
        Key::Char('i') => st.vi_normal = false,
        Key::Char('I') => {
            st.cursor = 0;
            st.vi_normal = false;
        }
        Key::Char('a') => {
            if let Some(next) = next_char_end(&st.buffer, st.cursor) {
                st.cursor = next;
            }
            st.vi_normal = false;
        }
        Key::Char('A') => {
            st.cursor = st.buffer.len();
            st.vi_normal = false;
        }
        Key::Char('k') | Key::Up => history_prev(st, history),
        Key::Char('j') | Key::Down => history_next(st, history),
        Key::Delete => {
            if let Some(next) = next_char_end(&st.buffer, st.cursor) {
                st.buffer.replace_range(st.cursor..next, "");
            }
        }
        _ => {}
    }
    st.vi_count = 0;
    st.vi_op = None; // an unrecognized key cancels a pending operator
}

/// Apply a vi operator over the span from the cursor to `target`.
#[cfg(unix)]
fn vi_apply_op(
    st: &mut LineState,
    ring: &mut Vec<String>,
    op: char,
    target: usize,
    inclusive: bool,
) {
    let (s, e) = if target >= st.cursor {
        let e = if inclusive {
            next_char_end(&st.buffer, target).unwrap_or(st.buffer.len())
        } else {
            target
        };
        (st.cursor, e)
    } else {
        (target, st.cursor)
    };
    if s >= e {
        if op == 'c' {
            st.vi_normal = false;
        }
        return;
    }
    match op {
        'd' => kill_span(st, ring, s, e, target >= st.cursor),
        'c' => {
            kill_span(st, ring, s, e, target >= st.cursor);
            st.vi_normal = false;
        }
        _ => {
            push_ring(ring, st.buffer[s..e].to_string());
            st.cursor = s;
        }
    }
}

#[cfg(unix)]
fn invert_find(kind: char) -> char {
    match kind {
        'f' => 'F',
        'F' => 'f',
        't' => 'T',
        _ => 't',
    }
}

/// Resolve `f F t T` to a target position (and operator inclusivity).
#[cfg(unix)]
fn vi_find_target(s: &str, pos: usize, kind: char, target: char) -> Option<(usize, bool)> {
    match kind {
        'f' | 't' => {
            let from = next_char_end(s, pos)?;
            let found = s[from..]
                .char_indices()
                .find(|&(_, c)| c == target)
                .map(|(i, _)| from + i)?;
            let t = if kind == 't' {
                prev_char_start(s, found)?
            } else {
                found
            };
            if t <= pos { None } else { Some((t, true)) }
        }
        _ => {
            let found = s[..pos]
                .char_indices()
                .rev()
                .find(|&(_, c)| c == target)
                .map(|(i, _)| i)?;
            let t = if kind == 'T' {
                next_char_end(s, found)?
            } else {
                found
            };
            if t >= pos { None } else { Some((t, false)) }
        }
    }
}

#[cfg(unix)]
enum SearchOutcome {
    Continue,
    Accept,
    Exit,
}

/// Ctrl-R/Ctrl-S incremental search (bash's `(reverse-i-search)`); C-r
/// steps to older matches, C-s to newer ones.
#[cfg(unix)]
fn handle_search_key(
    st: &mut LineState,
    key: Key,
    history: &[String],
) -> io::Result<SearchOutcome> {
    let search = st.search.as_mut().expect("search active");
    match key {
        Key::Enter => {
            if let Some(hit) = search.hit {
                st.buffer = history[hit].clone();
                st.cursor = st.buffer.len();
            }
            st.search = None;
            return Ok(SearchOutcome::Accept);
        }
        Key::Ctrl('g') | Key::Esc | Key::Ctrl('c') => {
            st.search = None;
            return Ok(SearchOutcome::Exit);
        }
        Key::Ctrl('r') => {
            // Next older match.
            search.forward = false;
            let below = search.hit.unwrap_or(history.len());
            search.hit = find_match(history, &search.query, below).or(search.hit);
        }
        Key::Ctrl('s') => {
            // Next newer match.
            search.forward = true;
            let above = search.hit.map(|h| h + 1).unwrap_or(0);
            search.hit = find_match_fwd(history, &search.query, above).or(search.hit);
        }
        Key::Backspace => {
            search.query.pop();
            search.hit = find_match(history, &search.query, history.len());
        }
        Key::Char(c) => {
            search.query.push(c);
            search.hit = find_match(history, &search.query, history.len());
        }
        _ => {
            // Any other key: keep the match as the edit buffer and leave
            // search mode.
            if let Some(hit) = search.hit {
                st.buffer = history[hit].clone();
                st.cursor = st.buffer.len();
            }
            st.search = None;
            return Ok(SearchOutcome::Exit);
        }
    }
    Ok(SearchOutcome::Continue)
}

/// Most recent history entry (strictly before `below`) containing `query`.
#[cfg(unix)]
fn find_match(history: &[String], query: &str, below: usize) -> Option<usize> {
    if query.is_empty() {
        return None;
    }
    history[..below.min(history.len())]
        .iter()
        .rposition(|h| h.contains(query))
}

/// Earliest history entry at or after `from` containing `query`.
#[cfg(unix)]
fn find_match_fwd(history: &[String], query: &str, from: usize) -> Option<usize> {
    if query.is_empty() || from >= history.len() {
        return None;
    }
    history[from..]
        .iter()
        .position(|h| h.contains(query))
        .map(|p| p + from)
}

#[cfg(unix)]
fn insert_char(st: &mut LineState, c: char) {
    st.buffer.insert(st.cursor, c);
    st.cursor += c.len_utf8();
}

#[cfg(unix)]
fn prev_char_start(s: &str, pos: usize) -> Option<usize> {
    s[..pos].char_indices().next_back().map(|(i, _)| i)
}

#[cfg(unix)]
fn next_char_end(s: &str, pos: usize) -> Option<usize> {
    s[pos..].chars().next().map(|c| pos + c.len_utf8())
}

#[cfg(unix)]
fn fwd_n(s: &str, pos: usize, n: usize) -> usize {
    let mut p = pos;
    for _ in 0..n {
        match next_char_end(s, p) {
            Some(q) => p = q,
            None => break,
        }
    }
    p
}

#[cfg(unix)]
fn back_n(s: &str, pos: usize, n: usize) -> usize {
    let mut p = pos;
    for _ in 0..n {
        match prev_char_start(s, p) {
            Some(q) => p = q,
            None => break,
        }
    }
    p
}

#[cfg(unix)]
fn apply_n(s: &str, pos: usize, n: usize, f: fn(&str, usize) -> usize) -> usize {
    let mut p = pos;
    for _ in 0..n {
        p = f(s, p);
    }
    p
}

#[cfg(unix)]
fn first_nonblank(s: &str) -> usize {
    s.char_indices()
        .find(|&(_, c)| !c.is_whitespace())
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Start of the word before `pos` (whitespace-delimited — C-w's
/// unix-word-rubout and the vi big-word flavor).
fn word_back(s: &str, pos: usize) -> usize {
    let before = &s[..pos];
    let trimmed = before.trim_end();
    match trimmed.rfind(char::is_whitespace) {
        Some(i) => i + 1,
        None => 0,
    }
}

/// Start of the alphanumeric word before `pos` (readline's M-b flavor).
fn word_back_alnum(s: &str, pos: usize) -> usize {
    let chars: Vec<(usize, char)> = s[..pos].char_indices().collect();
    let mut i = chars.len();
    while i > 0 && !chars[i - 1].1.is_alphanumeric() {
        i -= 1;
    }
    while i > 0 && chars[i - 1].1.is_alphanumeric() {
        i -= 1;
    }
    if i < chars.len() { chars[i].0 } else { pos }
}

/// End of the alphanumeric word after `pos` (readline's M-f flavor).
fn word_forward_alnum(s: &str, pos: usize) -> usize {
    let chars: Vec<(usize, char)> = s[pos..].char_indices().collect();
    let mut i = 0;
    while i < chars.len() && !chars[i].1.is_alphanumeric() {
        i += 1;
    }
    while i < chars.len() && chars[i].1.is_alphanumeric() {
        i += 1;
    }
    if i < chars.len() {
        pos + chars[i].0
    } else {
        s.len()
    }
}

/// vi small-word character class: whitespace / word (alnum + `_`) /
/// punctuation — each non-blank run of one class is a word.
fn vi_class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

/// vi `w`: start of the next small word.
fn vi_word_fwd(s: &str, pos: usize) -> usize {
    let mut it = s[pos..].char_indices().peekable();
    let Some(&(_, c0)) = it.peek() else {
        return s.len();
    };
    let cls = vi_class(c0);
    if cls != 0 {
        while let Some(&(_, c)) = it.peek() {
            if vi_class(c) == cls {
                it.next();
            } else {
                break;
            }
        }
    }
    while let Some(&(_, c)) = it.peek() {
        if vi_class(c) == 0 {
            it.next();
        } else {
            break;
        }
    }
    it.peek().map(|&(i, _)| pos + i).unwrap_or(s.len())
}

/// vi `b`: start of the small word before `pos`.
fn vi_word_back(s: &str, pos: usize) -> usize {
    let chars: Vec<(usize, char)> = s[..pos].char_indices().collect();
    let mut i = chars.len();
    while i > 0 && vi_class(chars[i - 1].1) == 0 {
        i -= 1;
    }
    if i == 0 {
        return 0;
    }
    let cls = vi_class(chars[i - 1].1);
    while i > 0 && vi_class(chars[i - 1].1) == cls {
        i -= 1;
    }
    chars.get(i).map(|&(b, _)| b).unwrap_or(0)
}

/// vi `e`: byte index of the last character of the current/next small
/// word (the cursor lands *on* it; operators over it are inclusive).
fn vi_word_end(s: &str, pos: usize) -> usize {
    let Some(start) = s[pos..].chars().next().map(|c| pos + c.len_utf8()) else {
        return pos;
    };
    let chars: Vec<(usize, char)> = s[start..]
        .char_indices()
        .map(|(i, c)| (start + i, c))
        .collect();
    let mut i = 0;
    while i < chars.len() && vi_class(chars[i].1) == 0 {
        i += 1;
    }
    if i >= chars.len() {
        return pos;
    }
    let cls = vi_class(chars[i].1);
    let mut last = chars[i].0;
    while i < chars.len() && vi_class(chars[i].1) == cls {
        last = chars[i].0;
        i += 1;
    }
    last
}

/// Delete `start..end` into the kill ring. Consecutive kills grow one
/// ring entry (appending for forward kills, prepending for backward) —
/// readline's rule, so C-w C-w C-y restores both words.
#[cfg(unix)]
fn kill_span(st: &mut LineState, ring: &mut Vec<String>, start: usize, end: usize, forward: bool) {
    if start >= end {
        return;
    }
    let text = st.buffer[start..end].to_string();
    let appending = matches!(st.prev_action, Action::KillFwd | Action::KillBack);
    if appending && let Some(last) = ring.last_mut() {
        if forward {
            last.push_str(&text);
        } else {
            last.insert_str(0, &text);
        }
    } else {
        push_ring(ring, text);
    }
    st.buffer.replace_range(start..end, "");
    st.cursor = start;
    st.this_action = if forward {
        Action::KillFwd
    } else {
        Action::KillBack
    };
}

#[cfg(unix)]
fn push_ring(ring: &mut Vec<String>, text: String) {
    ring.push(text);
    if ring.len() > 32 {
        ring.remove(0);
    }
}

/// C-y: insert the top of the kill ring at the cursor.
#[cfg(unix)]
fn yank(st: &mut LineState, ring: &[String]) {
    let Some(text) = ring.last() else { return };
    let start = st.cursor;
    st.buffer.insert_str(start, text);
    st.cursor = start + text.len();
    st.yank = Some((start, st.cursor, ring.len() - 1));
    st.this_action = Action::Yank;
}

/// M-y immediately after a yank: rotate the yanked text to the previous
/// ring entry.
#[cfg(unix)]
fn yank_pop(st: &mut LineState, ring: &[String]) {
    if st.prev_action != Action::Yank || ring.is_empty() {
        return;
    }
    let Some((start, end, idx)) = st.yank else {
        return;
    };
    let new_idx = if idx == 0 { ring.len() - 1 } else { idx - 1 };
    let text = &ring[new_idx];
    st.buffer.replace_range(start..end, text);
    st.cursor = start + text.len();
    st.yank = Some((start, st.cursor, new_idx));
    st.this_action = Action::Yank;
}

/// C-_ / C-x C-u / vi `u`: pop the undo stack.
#[cfg(unix)]
fn undo_cmd(st: &mut LineState) {
    if let Some((buf, cur)) = st.undo.pop() {
        st.cursor = cur.min(buf.len());
        st.buffer = buf;
    }
    st.this_action = Action::Undo;
}

/// C-t: transpose the characters around the cursor (at end of line, the
/// two before it — readline's rule).
#[cfg(unix)]
fn transpose(st: &mut LineState) {
    if st.cursor == st.buffer.len()
        && let Some(prev) = prev_char_start(&st.buffer, st.cursor)
    {
        st.cursor = prev;
    }
    if let Some(prev) = prev_char_start(&st.buffer, st.cursor)
        && let Some(next) = next_char_end(&st.buffer, st.cursor)
    {
        let a: String = st.buffer[prev..st.cursor].to_string();
        let b: String = st.buffer[st.cursor..next].to_string();
        st.buffer.replace_range(prev..next, &format!("{b}{a}"));
        st.cursor = prev + b.len() + a.len();
    }
}

/// M-t: transpose the word at/after the cursor with the one before it,
/// leaving the cursor after the moved pair.
#[cfg(unix)]
fn transpose_words(st: &mut LineState) {
    let e2 = word_forward_alnum(&st.buffer, st.cursor);
    let s2 = {
        let chars: Vec<(usize, char)> = st.buffer[..e2].char_indices().collect();
        let mut i = chars.len();
        while i > 0 && chars[i - 1].1.is_alphanumeric() {
            i -= 1;
        }
        chars.get(i).map(|&(b, _)| b).unwrap_or(e2)
    };
    let chars: Vec<(usize, char)> = st.buffer[..s2].char_indices().collect();
    let mut i = chars.len();
    while i > 0 && !chars[i - 1].1.is_alphanumeric() {
        i -= 1;
    }
    let e1 = if i < chars.len() { chars[i].0 } else { s2 };
    while i > 0 && chars[i - 1].1.is_alphanumeric() {
        i -= 1;
    }
    let s1 = chars.get(i).map(|&(b, _)| b).unwrap_or(e1);
    if s1 >= e1 || s2 >= e2 || e1 > s2 {
        return;
    }
    let w1 = st.buffer[s1..e1].to_string();
    let sep = st.buffer[e1..s2].to_string();
    let w2 = st.buffer[s2..e2].to_string();
    st.buffer.replace_range(s1..e2, &format!("{w2}{sep}{w1}"));
    st.cursor = s1 + w2.len() + sep.len() + w1.len();
}

#[cfg(unix)]
enum CaseOp {
    Upper,
    Lower,
    Capital,
}

/// M-u / M-l / M-c: upcase / downcase / capitalize from the cursor to the
/// end of the word, moving the cursor past it.
#[cfg(unix)]
fn case_word(st: &mut LineState, op: CaseOp) {
    let end = word_forward_alnum(&st.buffer, st.cursor);
    if end <= st.cursor {
        return;
    }
    let seg = st.buffer[st.cursor..end].to_string();
    let mut out = String::with_capacity(seg.len());
    let mut first = true;
    for c in seg.chars() {
        match op {
            CaseOp::Upper => out.extend(c.to_uppercase()),
            CaseOp::Lower => out.extend(c.to_lowercase()),
            CaseOp::Capital => {
                if c.is_alphanumeric() && first {
                    first = false;
                    out.extend(c.to_uppercase());
                } else if c.is_alphanumeric() {
                    out.extend(c.to_lowercase());
                } else {
                    out.push(c);
                }
            }
        }
    }
    st.buffer.replace_range(st.cursor..end, &out);
    st.cursor += out.len();
}

/// M-. / M-_: insert the last word of the previous history entry;
/// repeated presses cycle back through older entries, replacing the
/// previous insertion.
#[cfg(unix)]
fn insert_last_arg(st: &mut LineState, history: &[String]) {
    if history.is_empty() {
        return;
    }
    let (idx, span) = if st.prev_action == Action::LastArg
        && let Some((i, s, e)) = st.lastarg
    {
        (i.saturating_sub(1), Some((s, e)))
    } else {
        (history.len() - 1, None)
    };
    let word = history[idx]
        .split_whitespace()
        .last()
        .unwrap_or("")
        .to_string();
    let start = match span {
        Some((s, e)) => {
            st.buffer.replace_range(s..e, "");
            s
        }
        None => st.cursor,
    };
    st.buffer.insert_str(start, &word);
    st.cursor = start + word.len();
    st.lastarg = Some((idx, start, start + word.len()));
    st.this_action = Action::LastArg;
}

/// C-v / C-q: insert the next key literally (a tab, an ESC, a ^C…);
/// the render shows it `^X`-style.
#[cfg(unix)]
fn quoted_insert(st: &mut LineState) -> io::Result<()> {
    let hooks = st.hooks;
    if let Some(b) = read_byte(hooks)? {
        let c = if b < 0x80 {
            b as char
        } else {
            read_utf8(hooks, b)?
        };
        insert_char(st, c);
        st.this_action = Action::Insert;
    }
    Ok(())
}

/// C-x C-e (emacs) / `v` (vi normal): hand the line to `$VISUAL`/`$EDITOR`
/// in a temp file; on a clean exit the edited text is returned and — like
/// bash — executed immediately.
#[cfg(unix)]
fn edit_in_editor(st: &mut LineState, raw: &RawMode) -> io::Result<Option<String>> {
    let path = std::env::temp_dir().join(format!("rusty-lines-edit-{}.txt", std::process::id()));
    std::fs::write(&path, &st.buffer)?;
    finish_line(st)?;
    raw.suspend();
    let editor = st
        .hooks
        .external_editor()
        .or_else(|| std::env::var("VISUAL").ok().filter(|e| !e.is_empty()))
        .or_else(|| std::env::var("EDITOR").ok().filter(|e| !e.is_empty()))
        .unwrap_or_else(|| "vi".to_string());
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} {}", path.display()))
        .status();
    raw.resume();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::remove_file(&path);
    match status {
        Ok(s) if s.success() => Ok(Some(text.trim_end_matches('\n').to_string())),
        _ => {
            // Editor failed or was declined: repaint on a fresh region.
            st.painted_rows = 1;
            st.painted_cursor_row = 0;
            Ok(None)
        }
    }
}

#[cfg(unix)]
fn history_prev(st: &mut LineState, history: &[String]) {
    let next_index = match st.hist_index {
        None if history.is_empty() => return,
        None => {
            st.draft = st.buffer.clone();
            history.len() - 1
        }
        Some(0) => 0,
        Some(i) => i - 1,
    };
    st.hist_index = Some(next_index);
    st.buffer = history[next_index].clone();
    st.cursor = st.buffer.len();
}

#[cfg(unix)]
fn history_next(st: &mut LineState, history: &[String]) {
    match st.hist_index {
        None => {}
        Some(i) if i + 1 < history.len() => {
            st.hist_index = Some(i + 1);
            st.buffer = history[i + 1].clone();
            st.cursor = st.buffer.len();
        }
        Some(_) => {
            st.hist_index = None;
            st.buffer = std::mem::take(&mut st.draft);
            st.cursor = st.buffer.len();
        }
    }
}

/// M-<: jump to the oldest history entry.
#[cfg(unix)]
fn history_first(st: &mut LineState, history: &[String]) {
    if history.is_empty() {
        return;
    }
    if st.hist_index.is_none() {
        st.draft = st.buffer.clone();
    }
    st.hist_index = Some(0);
    st.buffer = history[0].clone();
    st.cursor = st.buffer.len();
}

/// M->: back to the live (draft) line.
#[cfg(unix)]
fn history_last(st: &mut LineState) {
    if st.hist_index.is_some() {
        st.hist_index = None;
        st.buffer = std::mem::take(&mut st.draft);
        st.cursor = st.buffer.len();
    }
}

/// PageUp / M-p: previous history entry starting with the text before the
/// cursor (zsh's `history-beginning-search-backward`, fish's Up).
#[cfg(unix)]
fn history_prefix_prev(st: &mut LineState, history: &[String]) {
    if st.hist_index.is_none() {
        st.prefix = st.buffer[..st.cursor].to_string();
        st.draft = st.buffer.clone();
    }
    let below = st.hist_index.unwrap_or(history.len());
    if let Some(i) = history[..below.min(history.len())]
        .iter()
        .rposition(|h| h.starts_with(&st.prefix) && *h != st.buffer)
    {
        st.hist_index = Some(i);
        st.buffer = history[i].clone();
        st.cursor = st.buffer.len();
    }
}

/// PageDown / M-n: next prefix match, or back to the draft.
#[cfg(unix)]
fn history_prefix_next(st: &mut LineState, history: &[String]) {
    let Some(cur) = st.hist_index else { return };
    if let Some(off) = history[cur + 1..]
        .iter()
        .position(|h| h.starts_with(&st.prefix))
    {
        let i = cur + 1 + off;
        st.hist_index = Some(i);
        st.buffer = history[i].clone();
        st.cursor = st.buffer.len();
    } else {
        st.hist_index = None;
        st.buffer = std::mem::take(&mut st.draft);
        st.cursor = st.buffer.len();
    }
}

/// Tab completion: insert the longest
/// common prefix; when that makes no progress, print the columned
/// candidate list below the line and arm menu cycling, so further Tabs
/// walk the candidates in-line (zsh `AUTO_MENU`).
#[cfg(unix)]
fn complete_at_cursor(st: &mut LineState) -> io::Result<()> {
    let (start, candidates) = st.hooks.complete(&st.buffer, st.cursor);
    if candidates.is_empty() {
        return bell(st.cfg.bell);
    }
    let lcp = common_prefix(
        &candidates
            .iter()
            .map(|c| c.replacement.as_str())
            .collect::<Vec<_>>(),
        st.cfg.completion_ignore_case,
    );
    let current = &st.buffer[start..st.cursor];
    if lcp.len() > current.len() {
        st.buffer.replace_range(start..st.cursor, &lcp);
        st.cursor = start + lcp.len();
        // show-all-if-ambiguous (readline): list right away instead of
        // waiting for a second Tab after the prefix insertion.
        if !(st.cfg.show_all_if_ambiguous && candidates.len() > 1) {
            return Ok(());
        }
    }
    if candidates.len() > 1 {
        // Leave the edit region and print the columned list; the next
        // render starts a fresh region below it.
        finish_line(st)?;
        let width = candidates
            .iter()
            .map(|c| display_width(&c.display))
            .max()
            .unwrap_or(0)
            + 2;
        let cols = (term_cols() / width.max(1)).max(1);
        for chunk in candidates.chunks(cols) {
            let row: Vec<String> = chunk
                .iter()
                .map(|c| format!("{:<w$}", c.display, w = width))
                .collect();
            println!("{}", row.join("").trim_end());
        }
        st.painted_rows = 1;
        st.painted_cursor_row = 0;
        st.menu = Some(MenuState {
            start,
            inserted: st.cursor - start,
            index: None,
            candidates,
        });
    }
    Ok(())
}

/// readline `menu-complete` (also Tab under `set_menu_complete`): insert
/// the first candidate immediately and arm cycling — no LCP step, no
/// candidate list.
#[cfg(unix)]
fn menu_complete_start(st: &mut LineState) -> io::Result<()> {
    let (start, candidates) = st.hooks.complete(&st.buffer, st.cursor);
    if candidates.is_empty() {
        return bell(st.cfg.bell);
    }
    st.menu = Some(MenuState {
        start,
        inserted: st.cursor - start,
        index: None,
        candidates,
    });
    menu_next(st);
    Ok(())
}

/// A further Tab with the menu armed: replace the word with the next
/// candidate, wrapping around the list.
#[cfg(unix)]
fn menu_next(st: &mut LineState) {
    let Some(mut menu) = st.menu.take() else {
        return;
    };
    let i = match menu.index {
        None => 0,
        Some(i) => (i + 1) % menu.candidates.len(),
    };
    menu.index = Some(i);
    let replacement = &menu.candidates[i].replacement;
    st.buffer
        .replace_range(menu.start..menu.start + menu.inserted, replacement);
    st.cursor = menu.start + replacement.len();
    menu.inserted = replacement.len();
    st.menu = Some(menu);
}

fn longest_common_prefix(names: &[&str]) -> String {
    let Some(first) = names.first() else {
        return String::new();
    };
    let mut prefix = first.to_string();
    for name in &names[1..] {
        while !name.starts_with(&prefix) {
            prefix.pop();
            if prefix.is_empty() {
                return prefix;
            }
        }
    }
    prefix
}

/// `longest_common_prefix`, optionally case-insensitive (readline's
/// `completion-ignore-case`): candidates are compared ignoring case and
/// the first candidate's spelling is what gets inserted.
fn common_prefix(names: &[&str], ignore_case: bool) -> String {
    if !ignore_case {
        return longest_common_prefix(names);
    }
    let Some(first) = names.first() else {
        return String::new();
    };
    let mut end = first.len();
    for name in &names[1..] {
        let mut common = 0;
        for (a, b) in first[..end].chars().zip(name.chars()) {
            if a != b && !a.to_lowercase().eq(b.to_lowercase()) {
                break;
            }
            common += a.len_utf8();
        }
        end = common;
        if end == 0 {
            break;
        }
    }
    first[..end].to_string()
}

/// Repaint the whole edit region and reposition the cursor.
///
/// Layout math: everything is measured in display columns (ANSI-skipped,
/// wide-character-aware, control characters via `visualize`). When a
/// painted row ends exactly at the terminal width, a newline is emitted to
/// force the wrap immediately — sidestepping terminals' delayed-wrap
/// state, which would otherwise break the relative cursor movements the
/// next repaint starts with.
#[cfg(unix)]
fn render(st: &mut LineState, history: &[String]) -> io::Result<()> {
    let cols = term_cols().max(2);
    let mut out = String::new();

    // Return to the region's first row/column and clear everything below.
    out.push('\r');
    if st.painted_cursor_row > 0 {
        out.push_str(&format!("\x1b[{}A", st.painted_cursor_row));
    }
    out.push_str("\x1b[J");

    // Search mode paints its own prompt instead of PS1/buffer.
    if let Some(search) = &st.search {
        let shown = search.hit.map(|i| history[i].as_str()).unwrap_or("");
        let label = if search.forward {
            "(i-search)"
        } else {
            "(reverse-i-search)"
        };
        let line = format!("{label}`{}': {}", search.query, visualize(shown));
        out.push_str(&line);
        let w = display_width(&line);
        st.painted_rows = w / cols + 1;
        st.painted_cursor_row = w / cols;
        print!("{out}");
        return io::stdout().flush();
    }

    let vis = visualize(&st.buffer);
    let highlighted = st.hooks.highlight(&vis);
    let hint = if st.cursor == st.buffer.len() {
        visualize(&st.hooks.hint(&st.buffer, history).unwrap_or_default())
    } else {
        String::new()
    };

    let wp = display_width(st.prompt);
    let wb = display_width(&vis);
    let wh = display_width(&hint);
    let wcursor = wp + display_width(&visualize(&st.buffer[..st.cursor]));
    let wtotal = wp + wb + wh;

    out.push_str(st.prompt);
    out.push_str(&highlighted);
    if !hint.is_empty() {
        out.push_str(&format!("\x1b[2m{hint}\x1b[0m"));
    }

    // The right prompt: shown while everything fits on one row with
    // a gap; hidden (zsh-style) once the line grows into it.
    let wr = display_width(st.rprompt);
    if wr > 0 && wtotal + wr + 1 < cols {
        out.push_str(&format!("\x1b[{}G", cols - wr + 1));
        out.push_str(st.rprompt);
    }

    // Force the wrap when the content ends exactly on the boundary —
    // after which the cursor sits at (wtotal / cols, 0) either way.
    if wtotal > 0 && wtotal.is_multiple_of(cols) {
        out.push_str("\r\n");
    }

    let total_rows = wtotal / cols + 1;
    let end_row = wtotal / cols;
    let cursor_row = wcursor / cols;
    let cursor_col = wcursor % cols;

    // Reposition from the end of the paint to the cursor.
    let up = end_row.saturating_sub(cursor_row);
    if up > 0 {
        out.push_str(&format!("\x1b[{up}A"));
    }
    out.push_str(&format!("\r\x1b[{}G", cursor_col + 1));

    st.painted_rows = total_rows;
    st.painted_cursor_row = cursor_row;

    print!("{out}");
    io::stdout().flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn state(buf: &str, cursor: usize) -> LineState<'static> {
        state_hooked(buf, cursor, &NoHooks)
    }

    #[cfg(unix)]
    fn state_hooked<'a>(buf: &str, cursor: usize, hooks: &'a dyn Hooks) -> LineState<'a> {
        LineState {
            buffer: buf.to_string(),
            cursor,
            prompt: "",
            rprompt: "",
            painted_rows: 1,
            painted_cursor_row: 0,
            hist_index: None,
            draft: String::new(),
            prefix: String::new(),
            vi: false,
            vi_normal: false,
            vi_count: 0,
            vi_op: None,
            vi_find: None,
            vi_replace: false,
            last_find: None,
            search: None,
            undo: Vec::new(),
            prev_action: Action::Other,
            this_action: Action::Other,
            yank: None,
            lastarg: None,
            menu: None,
            cfg: EditorConfig::default(),
            hooks,
        }
    }

    #[test]
    fn width_skips_ansi_and_counts_wide_chars() {
        assert_eq!(display_width("plain"), 5);
        assert_eq!(display_width("\x1b[32mgreen\x1b[0m"), 5);
        assert_eq!(display_width("日本"), 4); // two double-width chars
        assert_eq!(display_width(""), 0);
    }

    #[test]
    fn common_prefix_plain() {
        assert_eq!(longest_common_prefix(&["echo", "ech", "echelon"]), "ech");
        assert_eq!(longest_common_prefix(&["abc"]), "abc");
        assert_eq!(longest_common_prefix(&["x", "y"]), "");
        assert_eq!(longest_common_prefix(&[]), "");
    }

    #[test]
    fn word_motion() {
        assert_eq!(word_back("echo hello", 10), 5);
        assert_eq!(word_back("echo hello", 5), 0);
        assert_eq!(word_back("word", 4), 0);
    }

    #[test]
    fn word_motion_alnum() {
        // M-b/M-f stop at non-alphanumerics, unlike C-w's whitespace rule.
        assert_eq!(word_back_alnum("a-b/c.txt", 9), 6); // back over "txt"
        assert_eq!(word_back_alnum("echo hello", 10), 5);
        assert_eq!(word_back_alnum("  ", 2), 0); // no word: to line start
        assert_eq!(word_forward_alnum("a-b/c.txt", 0), 1); // just "a"
        assert_eq!(word_forward_alnum("--foo bar", 0), 5); // skips the dashes
    }

    #[test]
    fn vi_small_words() {
        //           0123456789
        let s = "ab cd-ef gh";
        assert_eq!(vi_word_fwd(s, 0), 3); // ab → cd
        assert_eq!(vi_word_fwd(s, 3), 5); // cd → the "-" (its own word)
        assert_eq!(vi_word_fwd(s, 5), 6); // "-" → ef
        assert_eq!(vi_word_back(s, 6), 5); // ef → "-"
        assert_eq!(vi_word_back(s, 3), 0);
        assert_eq!(vi_word_end(s, 0), 1); // on the "b" of "ab"
        assert_eq!(vi_word_end(s, 1), 4); // b → d of "cd"
    }

    #[cfg(unix)]
    #[test]
    fn vi_find_targets() {
        //       0123456
        let s = "echo ab";
        assert_eq!(vi_find_target(s, 0, 'f', 'o'), Some((3, true)));
        assert_eq!(vi_find_target(s, 0, 't', 'o'), Some((2, true)));
        assert_eq!(vi_find_target(s, 6, 'F', 'e'), Some((0, false)));
        assert_eq!(vi_find_target(s, 6, 'T', 'e'), Some((1, false)));
        assert_eq!(vi_find_target(s, 0, 'f', 'z'), None);
    }

    #[test]
    fn visualize_control_chars() {
        #[cfg(unix)]
        {
            assert_eq!(visualize("plain"), "plain");
            assert_eq!(visualize("a\x1bb"), "a^[b");
            assert_eq!(visualize("a\tb"), "a    b");
            assert_eq!(visualize("a\nb"), "a⏎b");
            assert_eq!(visualize("\u{7f}"), "^?");
        }
    }

    #[cfg(unix)]
    #[test]
    fn csi_sequences() {
        assert_eq!(csi_key("", b'A'), Key::Up);
        assert_eq!(csi_key("", b'D'), Key::Left);
        assert_eq!(csi_key("3", b'~'), Key::Delete);
        assert_eq!(csi_key("1", b'~'), Key::Home);
        assert_eq!(csi_key("1;5", b'C'), Key::WordRight);
        assert_eq!(csi_key("1;3", b'D'), Key::WordLeft);
        assert_eq!(csi_key("5", b'~'), Key::PageUp);
        assert_eq!(csi_key("6", b'~'), Key::PageDown);
        assert_eq!(csi_key("99", b'~'), Key::Other);
    }

    #[test]
    fn history_dedups_consecutive_only() {
        let mut ed = Editor::new();
        ed.add_history_entry("a");
        ed.add_history_entry("a");
        ed.add_history_entry("b");
        ed.add_history_entry("a");
        assert_eq!(ed.history(), &["a", "b", "a"]);
    }

    #[test]
    fn history_joins_multiline_entries() {
        let mut ed = Editor::new();
        ed.add_history_entry("echo a\necho b");
        assert_eq!(ed.history(), &["echo a; echo b"]);
    }

    #[cfg(unix)]
    #[test]
    fn kill_ring_appends_consecutive_kills() {
        let mut ring = Vec::new();
        let mut st = state("one two three", 13);
        // C-w twice: "three" then "two " prepends onto the same entry.
        let (start, cur) = (word_back(&st.buffer, st.cursor), st.cursor);
        kill_span(&mut st, &mut ring, start, cur, false);
        st.prev_action = st.this_action;
        let (start, cur) = (word_back(&st.buffer, st.cursor), st.cursor);
        kill_span(&mut st, &mut ring, start, cur, false);
        assert_eq!(st.buffer, "one ");
        assert_eq!(ring, vec!["two three".to_string()]);
        // A yank restores both words at once.
        st.prev_action = Action::Other;
        yank(&mut st, &ring);
        assert_eq!(st.buffer, "one two three");
    }

    #[cfg(unix)]
    #[test]
    fn yank_pop_rotates_ring() {
        let mut ring = vec!["old".to_string(), "new".to_string()];
        let mut st = state("", 0);
        yank(&mut st, &ring);
        assert_eq!(st.buffer, "new");
        st.prev_action = st.this_action;
        yank_pop(&mut st, &ring);
        assert_eq!(st.buffer, "old");
        st.prev_action = st.this_action;
        yank_pop(&mut st, &ring);
        assert_eq!(st.buffer, "new");
        let _ = &mut ring;
    }

    #[cfg(unix)]
    #[test]
    fn transpose_words_swaps_around_cursor() {
        let mut st = state("echo one two", 12);
        transpose_words(&mut st);
        assert_eq!(st.buffer, "echo two one");
        assert_eq!(st.cursor, 12);
        // A single word: no-op.
        let mut st = state("word", 4);
        transpose_words(&mut st);
        assert_eq!(st.buffer, "word");
    }

    #[cfg(unix)]
    #[test]
    fn case_ops() {
        let mut st = state("echo word here", 5);
        case_word(&mut st, CaseOp::Upper);
        assert_eq!(st.buffer, "echo WORD here");
        assert_eq!(st.cursor, 9);
        let mut st = state("echo WORD", 5);
        case_word(&mut st, CaseOp::Lower);
        assert_eq!(st.buffer, "echo word");
        let mut st = state("echo wOrD", 5);
        case_word(&mut st, CaseOp::Capital);
        assert_eq!(st.buffer, "echo Word");
    }

    #[cfg(unix)]
    #[test]
    fn last_arg_inserts_and_cycles() {
        let history = vec!["echo first one".to_string(), "echo second two".to_string()];
        let mut st = state("echo ", 5);
        insert_last_arg(&mut st, &history);
        assert_eq!(st.buffer, "echo two");
        st.prev_action = st.this_action;
        insert_last_arg(&mut st, &history);
        assert_eq!(st.buffer, "echo one");
    }

    #[cfg(unix)]
    #[test]
    fn undo_restores_snapshots() {
        let mut st = state("hello", 5);
        st.undo.push(("hell".to_string(), 4));
        st.undo.push(("hello".to_string(), 5));
        undo_cmd(&mut st);
        assert_eq!(st.buffer, "hello");
        undo_cmd(&mut st);
        assert_eq!(st.buffer, "hell");
        assert_eq!(st.cursor, 4);
        undo_cmd(&mut st); // empty stack: no-op
        assert_eq!(st.buffer, "hell");
    }

    #[cfg(unix)]
    #[test]
    fn prefix_search_matches_start_only() {
        let history = vec![
            "git status".to_string(),
            "echo git".to_string(),
            "git push".to_string(),
        ];
        let mut st = state("git", 3);
        history_prefix_prev(&mut st, &history);
        assert_eq!(st.buffer, "git push");
        history_prefix_prev(&mut st, &history);
        assert_eq!(st.buffer, "git status"); // skipped "echo git"
        history_prefix_next(&mut st, &history);
        assert_eq!(st.buffer, "git push");
        history_prefix_next(&mut st, &history);
        assert_eq!(st.buffer, "git"); // back to the draft
    }

    #[test]
    fn history_cap_drops_oldest() {
        let mut ed = Editor::new();
        ed.set_max_history_len(2);
        ed.add_history_entry("a");
        ed.add_history_entry("b");
        ed.add_history_entry("c");
        assert_eq!(ed.history(), ["b", "c"]);
        ed.set_max_history_len(1); // shrinking trims immediately
        assert_eq!(ed.history(), ["c"]);
    }

    #[test]
    fn append_history_writes_only_new_entries() {
        let path =
            std::env::temp_dir().join(format!("rusty_lines_hist_test_{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut ed = Editor::new();
        ed.add_history_entry("one");
        ed.save_history(&path).unwrap();
        ed.add_history_entry("two");
        ed.append_history(&path).unwrap();
        ed.append_history(&path).unwrap(); // nothing new: no-op
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\n");

        // A second session appends without clobbering the first's entries.
        let mut ed2 = Editor::new();
        ed2.load_history(&path).unwrap();
        ed2.add_history_entry("three");
        ed2.append_history(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\nthree\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn key_specs_parse_readline_spellings() {
        assert_eq!(parse_key_spec("\\C-f"), Some(Key::Ctrl('f')));
        assert_eq!(parse_key_spec("\\C-F"), Some(Key::Ctrl('f')));
        assert_eq!(parse_key_spec("\\C-?"), Some(Key::Backspace));
        assert_eq!(parse_key_spec("\\C-_"), Some(Key::Ctrl('_')));
        assert_eq!(parse_key_spec("\\M-f"), Some(Key::Alt('f')));
        assert_eq!(parse_key_spec("\\ex"), Some(Key::Alt('x')));
        assert_eq!(parse_key_spec("\\M-\\C-?"), Some(Key::AltBackspace));
        assert_eq!(parse_key_spec("\\e[A"), Some(Key::Up));
        assert_eq!(parse_key_spec("\\e[1;5C"), Some(Key::WordRight));
        assert_eq!(parse_key_spec("\\e[3~"), Some(Key::Delete));
        assert_eq!(parse_key_spec("\\eOH"), Some(Key::Home));
        assert_eq!(parse_key_spec("\\e"), Some(Key::Esc));
        assert_eq!(parse_key_spec("\\C-m"), Some(Key::Enter));
        assert_eq!(parse_key_spec("\\t"), Some(Key::Tab));
        assert_eq!(parse_key_spec("\\x09"), Some(Key::Tab));
        assert_eq!(parse_key_spec("\\011"), Some(Key::Tab));
        assert_eq!(parse_key_spec("a"), Some(Key::Char('a')));
        assert_eq!(parse_key_spec("ü"), Some(Key::Char('ü')));
        // Unparseable: empty, chords, unknown sequences, junk escapes.
        assert_eq!(parse_key_spec(""), None);
        assert_eq!(parse_key_spec("\\C-x\\C-e"), None);
        assert_eq!(parse_key_spec("ab"), None);
        assert_eq!(parse_key_spec("\\e[99~"), None);
        assert_eq!(parse_key_spec("\\Z"), None);
    }

    #[test]
    fn key_specs_round_trip_through_bindings_listing() {
        for (key, _) in DEFAULT_BINDINGS {
            assert_eq!(
                parse_key_spec(&key_spec(key)).as_ref(),
                Some(key),
                "spec {:?} for {key:?} does not round-trip",
                key_spec(key)
            );
        }
    }

    #[test]
    fn bind_overrides_default_and_unbind_masks_it() {
        let mut ed = Editor::new();
        let lookup = |ed: &Editor, spec: &str| -> Option<EditorAction> {
            ed.bindings().find(|(k, _)| k == spec).map(|(_, a)| a)
        };
        assert_eq!(lookup(&ed, "\\C-f"), Some(EditorAction::ForwardChar));

        ed.bind("\\C-f", EditorAction::KillLine).unwrap();
        assert_eq!(lookup(&ed, "\\C-f"), Some(EditorAction::KillLine));
        // The default row is replaced, not duplicated.
        assert_eq!(ed.bindings().filter(|(k, _)| k == "\\C-f").count(), 1);

        ed.unbind("\\C-f").unwrap();
        assert_eq!(lookup(&ed, "\\C-f"), None);

        // Host bindings are stored but not listed as actions.
        ed.bind_host("\\C-g", "fzf".to_string()).unwrap();
        assert_eq!(lookup(&ed, "\\C-g"), None);
        assert!(
            ed.bindings
                .iter()
                .any(|(k, b)| *k == Key::Ctrl('g') && *b == Binding::Host("fzf".to_string()))
        );

        // A bad spec is an InvalidInput error.
        let err = ed.bind("\\C-x\\C-e", EditorAction::Undo).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn rebound_key_runs_the_new_action() {
        // C-f rebound to kill-line: dispatch consults the custom table.
        let mut ed = Editor::new();
        ed.bind("\\C-f", EditorAction::KillLine).unwrap();
        let (key, binding) = &ed.bindings[0];
        assert_eq!(*key, Key::Ctrl('f'));
        let Binding::Action(action) = binding else {
            panic!("expected an action binding");
        };
        let mut st = state("hello", 2);
        let mut ring = Vec::new();
        run_action(&mut st, *action, key, &[], &mut ring).unwrap();
        assert_eq!(st.buffer, "he");
        assert_eq!(ring, ["llo"]);
    }

    #[cfg(unix)]
    #[test]
    fn menu_complete_inserts_first_candidate_immediately() {
        struct H;
        impl Hooks for H {
            fn complete(&self, line: &str, pos: usize) -> (usize, Vec<Candidate>) {
                let cand = |s: &str| Candidate {
                    display: s.to_string(),
                    replacement: s.to_string(),
                };
                let _ = (line, pos);
                (4, vec![cand("alpha"), cand("alphabet")])
            }
        }
        let mut st = state_hooked("say al", 6, &H);
        st.cfg.menu_complete = true;
        let mut ring = Vec::new();
        run_action(&mut st, EditorAction::Complete, &Key::Tab, &[], &mut ring).unwrap();
        assert_eq!(st.buffer, "say alpha");
        run_action(&mut st, EditorAction::Complete, &Key::Tab, &[], &mut ring).unwrap();
        assert_eq!(st.buffer, "say alphabet");
        run_action(&mut st, EditorAction::Complete, &Key::Tab, &[], &mut ring).unwrap();
        assert_eq!(st.buffer, "say alpha"); // wraps
    }

    #[test]
    fn case_insensitive_common_prefix() {
        assert_eq!(common_prefix(&["Echo", "echelon"], true), "Ech");
        assert_eq!(common_prefix(&["Echo", "echelon"], false), "");
        assert_eq!(common_prefix(&["abc", "ABC"], true), "abc");
        assert_eq!(common_prefix(&["x", "y"], true), "");
        assert_eq!(common_prefix(&[], true), "");
    }

    #[test]
    fn history_timestamps_round_trip_both_formats() {
        let path = std::env::temp_dir().join(format!("rusty_lines_ts_test_{}", std::process::id()));
        let _ = std::fs::remove_file(&path);

        // Adding stamps entries; plain save leaves the file un-timestamped.
        let mut ed = Editor::new();
        ed.add_history_entry("one");
        assert!(ed.history_timestamps()[0].is_some());
        ed.save_history(&path).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().starts_with('#'));

        // Timestamped save writes bash's `#<epoch>` comment lines...
        ed.set_history_timestamps(true);
        ed.save_history(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with('#'), "no timestamp line:\n{text}");
        assert!(text.ends_with("one\n"));

        // ...which load back as timestamps, not entries, and append keeps
        // stamping.
        let mut ed2 = Editor::new();
        ed2.set_history_timestamps(true);
        ed2.load_history(&path).unwrap();
        assert_eq!(ed2.history(), ["one"]);
        assert_eq!(ed2.history_timestamps(), ed.history_timestamps());
        ed2.add_history_entry("two");
        ed2.append_history(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text.lines().count(),
            4,
            "expected 2 stamped entries:\n{text}"
        );

        // A plain (never-stamped) file loads with `None` timestamps.
        std::fs::write(&path, "alpha\nbeta\n").unwrap();
        let mut ed3 = Editor::new();
        ed3.load_history(&path).unwrap();
        assert_eq!(ed3.history(), ["alpha", "beta"]);
        assert_eq!(ed3.history_timestamps(), [None, None]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn replace_history_swaps_entries_and_keeps_appends_incremental() {
        let path =
            std::env::temp_dir().join(format!("rusty_lines_replace_test_{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut ed = Editor::new();
        ed.add_history_entry("a");
        ed.add_history_entry("b");
        ed.replace_history(vec!["b".to_string()]); // history -d dropped "a"
        assert_eq!(ed.history(), ["b"]);
        assert_eq!(ed.history_timestamps(), [None]);
        // Replaced entries count as persisted: only later additions append.
        ed.add_history_entry("c");
        ed.append_history(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "c\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn history_dedup_erases_earlier_duplicates() {
        let mut ed = Editor::new();
        ed.set_history_dedup(true);
        ed.add_history_entry("a");
        ed.add_history_entry("b");
        ed.add_history_entry("a");
        assert_eq!(ed.history(), ["b", "a"]);
        // Off by default: only consecutive repeats are skipped.
        let mut ed = Editor::new();
        ed.add_history_entry("a");
        ed.add_history_entry("b");
        ed.add_history_entry("a");
        assert_eq!(ed.history(), ["a", "b", "a"]);
    }

    #[cfg(unix)]
    #[test]
    fn revert_line_undoes_everything_at_once() {
        let mut st = state("hello", 5);
        st.undo = vec![("hello".to_string(), 5), ("hellox".to_string(), 6)];
        st.buffer = "helloxyz".to_string();
        st.cursor = 8;
        let mut ring = Vec::new();
        run_action(
            &mut st,
            EditorAction::RevertLine,
            &Key::Alt('r'),
            &[],
            &mut ring,
        )
        .unwrap();
        assert_eq!(st.buffer, "hello");
        assert_eq!(st.cursor, 5);
        assert!(st.undo.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn menu_cycling_walks_candidates_and_wraps() {
        let cand = |s: &str| Candidate {
            display: s.to_string(),
            replacement: s.to_string(),
        };
        let mut st = state("say al", 6);
        st.menu = Some(MenuState {
            start: 4,
            inserted: 2,
            index: None,
            candidates: vec![cand("alpha"), cand("alphabet")],
        });
        menu_next(&mut st);
        assert_eq!(st.buffer, "say alpha");
        assert_eq!(st.cursor, 9);
        menu_next(&mut st);
        assert_eq!(st.buffer, "say alphabet");
        menu_next(&mut st); // wraps back around
        assert_eq!(st.buffer, "say alpha");
    }

    #[cfg(unix)]
    #[test]
    fn alt_f_accepts_one_hint_word() {
        struct H;
        impl Hooks for H {
            fn hint(&self, line: &str, _history: &[String]) -> Option<String> {
                (line == "he").then(|| "llo world".to_string())
            }
        }
        let mut ring = Vec::new();

        // At end of line: accept exactly one word of the hint.
        let mut st = state_hooked("he", 2, &H);
        run_action(
            &mut st,
            EditorAction::ForwardWord,
            &Key::Alt('f'),
            &[],
            &mut ring,
        )
        .unwrap();
        assert_eq!(st.buffer, "hello");
        assert_eq!(st.cursor, 5);

        // Mid-line: plain word motion, no hint involvement.
        let mut st = state_hooked("he", 0, &H);
        run_action(
            &mut st,
            EditorAction::ForwardWord,
            &Key::Alt('f'),
            &[],
            &mut ring,
        )
        .unwrap();
        assert_eq!(st.buffer, "he");
        assert_eq!(st.cursor, 2);
    }
}
