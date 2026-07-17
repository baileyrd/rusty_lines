//! Terminal syscall facade.
//!
//! The line editor needs a small, fixed slice of the Unix terminal API:
//! `isatty`, `tcgetattr`/`tcsetattr`, a raw-mode flag flip, `poll`, a raw
//! byte `read`, and the window size. Two backends provide it behind one
//! interface:
//!
//! - **`rusty_libc`** — the hand-rolled raw-syscall crate; the default on
//!   Linux (zero third-party deps).
//! - **`libc` crate** — the backend on other Unix, and on Linux when forced
//!   with `--no-default-features --features libc-backend`.
//!
//! All functions target the streams the editor uses directly: stdin (fd 0)
//! for input and raw mode, stdout (fd 1) for the window size.
//!
//! ## errno note
//!
//! `rusty_libc` does not write glibc's TLS `errno`, so `Error::last_os_error`
//! is meaningless after its calls. This facade therefore builds every
//! `io::Error` from the syscall's own return/`Errno`, so callers keep getting
//! a correct `ErrorKind` (notably `Interrupted` for `EINTR`) in both backends.

pub use imp::*;

// Backend selection is target-driven: `rusty_libc` on Linux (the default),
// `libc` on other Unix. `libc-backend` forces libc on Linux too.
#[cfg(all(
    target_os = "linux",
    not(feature = "rusty-libc"),
    not(feature = "libc-backend")
))]
compile_error!("no backend: enable the default `rusty-libc`, or `libc-backend`");

// ---- libc backend: other Unix, or Linux with `libc-backend` --------------
#[cfg(any(
    all(unix, not(target_os = "linux")),
    all(target_os = "linux", feature = "libc-backend")
))]
mod imp {
    use std::io;

    /// Opaque terminal-attributes value, sized and laid out by the backend.
    pub type Termios = libc::termios;

    /// Is stdin a terminal?
    pub fn isatty_stdin() -> bool {
        // SAFETY: isatty takes an fd and touches no memory.
        unsafe { libc::isatty(0) != 0 }
    }

    /// Is stdout a terminal? The editor paints on stdout, so raw-mode
    /// editing needs it to be one just as much as stdin.
    pub fn isatty_stdout() -> bool {
        // SAFETY: isatty takes an fd and touches no memory.
        unsafe { libc::isatty(1) != 0 }
    }

    /// Read stdin's terminal attributes.
    pub fn tcgetattr_stdin() -> io::Result<Termios> {
        // SAFETY: `t` is a valid, zeroed termios the kernel fills.
        unsafe {
            let mut t: Termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut t) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(t)
        }
    }

    /// Apply attributes to stdin, draining pending output first (`TCSADRAIN`).
    pub fn tcsetattr_stdin_drain(t: &Termios) -> io::Result<()> {
        // SAFETY: `t` is a valid termios the kernel only reads.
        unsafe {
            if libc::tcsetattr(0, libc::TCSADRAIN, t) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    /// The editor's raw-mode recipe: no flow control or CR/NL mangling
    /// (`IGNCR` too — with it set, the `\r` that Enter sends would be
    /// discarded), no 7-bit stripping (`ISTRIP` would corrupt every
    /// UTF-8 high byte), no canonical mode, echo, signals, or
    /// literal-next; output stays cooked so ordinary `println!` still
    /// works. One byte at a time, no read timeout.
    pub fn apply_raw_flags(t: &mut Termios) {
        t.c_iflag &= !(libc::IXON | libc::ICRNL | libc::INLCR | libc::IGNCR | libc::ISTRIP);
        t.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
        t.c_cc[libc::VMIN] = 1;
        t.c_cc[libc::VTIME] = 0;
    }

    /// Does an attribute set still match the raw-mode recipe? What the
    /// idle tick checks to heal the terminal after an external
    /// SIGTSTP/SIGCONT or a host command that ran `stty`.
    pub fn is_raw(t: &Termios) -> bool {
        t.c_iflag & (libc::IXON | libc::ICRNL | libc::INLCR | libc::IGNCR | libc::ISTRIP) == 0
            && t.c_lflag & (libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN) == 0
    }

    /// Is a byte ready on stdin within `ms` milliseconds? A signal
    /// interrupting the poll retries it — EINTR must not read as "no
    /// input", which upstream would misdecode as a lone ESC or a
    /// timed-out escape sequence.
    pub fn poll_stdin(ms: i32) -> bool {
        loop {
            let mut pfd = libc::pollfd {
                fd: 0,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: single valid pollfd, count 1.
            let n = unsafe { libc::poll(&mut pfd, 1, ms) };
            if n >= 0 {
                return n > 0;
            }
            if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                return false;
            }
        }
    }

    /// Read up to `buf.len()` bytes from stdin in one syscall: `Ok(0)` at
    /// EOF, `Ok(n)` for `n` bytes read (may be less than `buf.len()` —
    /// a single `read` on a tty returns whatever the kernel line
    /// discipline currently has queued, which is exactly what the
    /// caller's userspace buffer should hold). Lets a caller collecting
    /// many bytes (a paste, a burst of escape sequences) pay one syscall
    /// instead of one per byte.
    pub fn read_stdin_chunk(buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: writing at most `buf.len()` bytes into `buf`.
        let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as usize)
    }

    /// Terminal width in columns from stdout, if available and non-zero.
    pub fn term_cols_stdout() -> Option<usize> {
        term_size_stdout().map(|(cols, _)| cols as usize)
    }

    /// Terminal size from stdout as (columns, rows), if available and
    /// non-zero in both dimensions.
    pub fn term_size_stdout() -> Option<(u16, u16)> {
        // SAFETY: `ws` is a valid, zeroed winsize the kernel fills.
        unsafe {
            let mut ws: libc::winsize = std::mem::zeroed();
            if libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
                return Some((ws.ws_col, ws.ws_row));
            }
        }
        None
    }

    /// Turn terminal echo off in an attribute set (`stty -echo`'s bit).
    pub fn clear_echo_flag(t: &mut Termios) {
        t.c_lflag &= !libc::ECHO;
    }
}

// ---- rusty_libc backend: Linux default -----------------------------------
#[cfg(all(
    target_os = "linux",
    feature = "rusty-libc",
    not(feature = "libc-backend")
))]
mod imp {
    use rusty_libc::{Errno, fd, termios, tty};
    use std::io;

    /// Opaque terminal-attributes value, sized and laid out by the backend.
    pub type Termios = termios::Termios;

    /// Build an `io::Error` from a raw-syscall `Errno` (see the module note on
    /// why `last_os_error` cannot be used here).
    fn to_io(e: Errno) -> io::Error {
        io::Error::from_raw_os_error(e.code())
    }

    /// Is stdin a terminal?
    pub fn isatty_stdin() -> bool {
        termios::isatty(0)
    }

    /// Is stdout a terminal? The editor paints on stdout, so raw-mode
    /// editing needs it to be one just as much as stdin.
    pub fn isatty_stdout() -> bool {
        termios::isatty(1)
    }

    /// Read stdin's terminal attributes.
    pub fn tcgetattr_stdin() -> io::Result<Termios> {
        termios::tcgetattr(0).map_err(to_io)
    }

    /// Apply attributes to stdin, draining pending output first (`TCSADRAIN`,
    /// which `rusty_libc::termios::tcsetattr` always uses).
    pub fn tcsetattr_stdin_drain(t: &Termios) -> io::Result<()> {
        termios::tcsetattr(0, t).map_err(to_io)
    }

    /// The editor's raw-mode recipe (identical to the libc backend's, using
    /// the same kernel flag bits — see that backend for the `IGNCR`/
    /// `ISTRIP` rationale). Deliberately lighter than
    /// `Termios::make_raw`: output processing is left on.
    pub fn apply_raw_flags(t: &mut Termios) {
        t.c_iflag &=
            !(termios::IXON | termios::ICRNL | termios::INLCR | termios::IGNCR | termios::ISTRIP);
        t.c_lflag &= !(termios::ICANON | termios::ECHO | termios::ISIG | termios::IEXTEN);
        t.c_cc[termios::VMIN] = 1;
        t.c_cc[termios::VTIME] = 0;
    }

    /// Does an attribute set still match the raw-mode recipe? What the
    /// idle tick checks to heal the terminal after an external
    /// SIGTSTP/SIGCONT or a host command that ran `stty`.
    pub fn is_raw(t: &Termios) -> bool {
        t.c_iflag
            & (termios::IXON | termios::ICRNL | termios::INLCR | termios::IGNCR | termios::ISTRIP)
            == 0
            && t.c_lflag & (termios::ICANON | termios::ECHO | termios::ISIG | termios::IEXTEN) == 0
    }

    /// Is a byte ready on stdin within `ms` milliseconds? A signal
    /// interrupting the poll retries it — EINTR must not read as "no
    /// input" (see the libc backend).
    pub fn poll_stdin(ms: i32) -> bool {
        loop {
            let mut fds = [fd::PollFd {
                fd: 0,
                events: fd::POLLIN,
                revents: 0,
            }];
            match fd::poll(&mut fds, ms) {
                Ok(n) => return n > 0,
                Err(e) if to_io(e).kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return false,
            }
        }
    }

    /// Read up to `buf.len()` bytes from stdin in one syscall (see the
    /// libc backend for the rationale — one read call instead of one per
    /// byte when collecting a paste or a burst of escape sequences).
    pub fn read_stdin_chunk(buf: &mut [u8]) -> io::Result<usize> {
        fd::read(0, buf).map_err(to_io)
    }

    /// Terminal width in columns from stdout, if available and non-zero.
    pub fn term_cols_stdout() -> Option<usize> {
        term_size_stdout().map(|(cols, _)| cols as usize)
    }

    /// Terminal size from stdout as (columns, rows), if available and
    /// non-zero in both dimensions.
    pub fn term_size_stdout() -> Option<(u16, u16)> {
        match tty::window_size(1) {
            Ok(ws) if ws.ws_col > 0 && ws.ws_row > 0 => Some((ws.ws_col, ws.ws_row)),
            _ => None,
        }
    }

    /// Turn terminal echo off in an attribute set (`stty -echo`'s bit).
    pub fn clear_echo_flag(t: &mut Termios) {
        t.c_lflag &= !termios::ECHO;
    }
}
