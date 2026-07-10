//! Terminal syscall facade.
//!
//! The line editor needs a small, fixed slice of the Unix terminal API:
//! `isatty`, `tcgetattr`/`tcsetattr`, a raw-mode flag flip, `poll`, a raw
//! byte `read`, and the window size. Two backends provide it behind one
//! interface:
//!
//! - **default** — the `libc` crate (portable across every Unix).
//! - **`rusty-libc` feature** — the hand-rolled raw-syscall crate (Linux
//!   only; zero third-party deps).
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

// ---- libc backend (default) ----------------------------------------------
#[cfg(not(feature = "rusty-libc"))]
mod imp {
    use std::io;

    /// Opaque terminal-attributes value, sized and laid out by the backend.
    pub type Termios = libc::termios;

    /// Is stdin a terminal?
    pub fn isatty_stdin() -> bool {
        // SAFETY: isatty takes an fd and touches no memory.
        unsafe { libc::isatty(0) != 0 }
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

    /// The editor's raw-mode recipe: no flow control or CR/NL mangling, no
    /// canonical mode, echo, signals, or literal-next; output stays cooked so
    /// ordinary `println!` still works. One byte at a time, no read timeout.
    pub fn apply_raw_flags(t: &mut Termios) {
        t.c_iflag &= !(libc::IXON | libc::ICRNL | libc::INLCR);
        t.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
        t.c_cc[libc::VMIN] = 1;
        t.c_cc[libc::VTIME] = 0;
    }

    /// Is a byte ready on stdin within `ms` milliseconds?
    pub fn poll_stdin(ms: i32) -> bool {
        let mut pfd = libc::pollfd {
            fd: 0,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: single valid pollfd, count 1.
        unsafe { libc::poll(&mut pfd, 1, ms) > 0 }
    }

    /// Read one byte from stdin: `Ok(Some(b))`, `Ok(None)` at EOF, or an
    /// error (whose `ErrorKind` is correct, e.g. `Interrupted` on `EINTR`).
    pub fn read_stdin_byte() -> io::Result<Option<u8>> {
        let mut b = [0u8; 1];
        // SAFETY: writing at most one byte into a one-byte buffer.
        let n = unsafe { libc::read(0, b.as_mut_ptr().cast(), 1) };
        match n {
            0 => Ok(None),
            1 => Ok(Some(b[0])),
            _ => Err(io::Error::last_os_error()),
        }
    }

    /// Terminal width in columns from stdout, if available and non-zero.
    pub fn term_cols_stdout() -> Option<usize> {
        // SAFETY: `ws` is a valid, zeroed winsize the kernel fills.
        unsafe {
            let mut ws: libc::winsize = std::mem::zeroed();
            if libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
                return Some(ws.ws_col as usize);
            }
        }
        None
    }
}

// ---- rusty_libc backend (feature = "rusty-libc") -------------------------
#[cfg(feature = "rusty-libc")]
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
    /// the same kernel flag bits). Deliberately lighter than
    /// `Termios::make_raw`: output processing is left on.
    pub fn apply_raw_flags(t: &mut Termios) {
        t.c_iflag &= !(termios::IXON | termios::ICRNL | termios::INLCR);
        t.c_lflag &= !(termios::ICANON | termios::ECHO | termios::ISIG | termios::IEXTEN);
        t.c_cc[termios::VMIN] = 1;
        t.c_cc[termios::VTIME] = 0;
    }

    /// Is a byte ready on stdin within `ms` milliseconds?
    pub fn poll_stdin(ms: i32) -> bool {
        let mut fds = [fd::PollFd {
            fd: 0,
            events: fd::POLLIN,
            revents: 0,
        }];
        matches!(fd::poll(&mut fds, ms), Ok(n) if n > 0)
    }

    /// Read one byte from stdin: `Ok(Some(b))`, `Ok(None)` at EOF, or an
    /// error (whose `ErrorKind` is correct, e.g. `Interrupted` on `EINTR`).
    pub fn read_stdin_byte() -> io::Result<Option<u8>> {
        let mut b = [0u8; 1];
        match fd::read(0, &mut b) {
            Ok(0) => Ok(None),
            Ok(_) => Ok(Some(b[0])),
            Err(e) => Err(to_io(e)),
        }
    }

    /// Terminal width in columns from stdout, if available and non-zero.
    pub fn term_cols_stdout() -> Option<usize> {
        match tty::window_size(1) {
            Ok(ws) if ws.ws_col > 0 => Some(ws.ws_col as usize),
            _ => None,
        }
    }
}
