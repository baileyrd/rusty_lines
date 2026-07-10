//! End-to-end tests: drive `examples/demo` under a real pseudo-terminal,
//! exercising raw mode, escape-sequence decoding, and the repaint path
//! that the unit tests in `src/lib.rs` can't reach. Unix-only, like the
//! raw editor itself.
#![cfg(unix)]

use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn demo_path() -> std::path::PathBuf {
    // target/debug/deps/pty-<hash> -> target/debug/examples/demo
    let mut p = std::env::current_exe().unwrap();
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.push("examples");
    p.push("demo");
    p
}

fn spawn_demo() -> (OwnedFd, Child) {
    let mut master: libc::c_int = 0;
    let mut slave: libc::c_int = 0;
    let ws = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            &ws,
        )
    };
    assert_eq!(rc, 0, "openpty failed");
    let master = unsafe { OwnedFd::from_raw_fd(master) };
    let slave = unsafe { OwnedFd::from_raw_fd(slave) };
    let child = Command::new(demo_path())
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave))
        .spawn()
        .expect("spawn demo (is it built? cargo test builds examples)");
    (master, child)
}

/// Feed each chunk to the pty (whole, so escape sequences are never
/// split), append Ctrl-D to exit the demo, then collect everything the
/// editor wrote until the child exits.
fn run_session(chunks: &[&[u8]]) -> String {
    let (master, mut child) = spawn_demo();
    let mut m = std::fs::File::from(master.try_clone().unwrap());
    for chunk in chunks {
        m.write_all(chunk).unwrap();
        m.flush().unwrap();
        // Let the editor drain this chunk before the next one, so a
        // trailing ESC is never glued to the next chunk's bytes.
        std::thread::sleep(Duration::from_millis(100));
    }
    m.write_all(b"\x04").unwrap(); // Ctrl-D on the empty line: exit
    m.flush().unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut out = Vec::new();
    let mut exited = false;
    loop {
        let mut pfd = libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let n = unsafe { libc::poll(&mut pfd, 1, 200) };
        if n > 0 && pfd.revents & libc::POLLIN != 0 {
            let mut buf = [0u8; 4096];
            let r = unsafe { libc::read(master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if r > 0 {
                out.extend_from_slice(&buf[..r as usize]);
                continue;
            }
            break; // EOF/EIO: slave side fully closed
        }
        if exited {
            break; // child gone and the pty has drained
        }
        if child.try_wait().unwrap().is_some() {
            exited = true;
        }
        assert!(
            Instant::now() < deadline,
            "demo did not exit; output so far:\n{out:?}"
        );
    }
    let status = child.wait().unwrap();
    assert!(status.success(), "demo exited with {status}");
    strip_ansi(&out)
}

/// Drop ESC-introduced sequences (CSI, OSC, two-byte) so assertions see
/// only printable text and \r\n structure.
fn strip_ansi(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('[') => {
                // CSI: parameters, then a final byte in @..~
                for c in it.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                // OSC: terminated by BEL or ST
                while let Some(c) = it.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' && it.peek() == Some(&'\\') {
                        it.next();
                        break;
                    }
                }
            }
            _ => {} // two-byte sequence: drop the introducer + one char
        }
    }
    out
}

/// An executed line is echoed by the demo at column 0, right after the
/// editor's own final "\r\n" — a signature repaints never produce.
fn echo(line: &str) -> String {
    format!("\r\n{line}\r\n")
}

#[test]
fn types_a_line_and_gets_it_echoed() {
    let out = run_session(&[b"hello\r"]);
    assert!(out.contains("demo> "), "prompt missing:\n{out}");
    assert!(out.contains(&echo("hello")), "echo missing:\n{out}");
}

#[test]
fn emacs_editing_home_and_delete() {
    // "xhello", C-a to column 0, C-d deletes the stray 'x'.
    let out = run_session(&[b"xhello", b"\x01", b"\x04", b"\r"]);
    assert!(out.contains(&echo("hello")), "edited line wrong:\n{out}");
}

#[test]
fn kill_word_then_yank() {
    // C-w kills "bar" into the kill ring, C-y yanks it back.
    let out = run_session(&[b"foo bar", b"\x17", b"\x19", b"\r"]);
    assert!(out.contains(&echo("foo bar")), "yank result wrong:\n{out}");
}

#[test]
fn up_arrow_recalls_history() {
    let out = run_session(&[b"one\r", b"two\r", b"\x1b[A\x1b[A", b"\r"]);
    let hits = out.matches(&echo("one")).count();
    assert!(
        hits >= 2,
        "expected 'one' echoed twice (typed + recalled):\n{out}"
    );
}

#[test]
fn ctrl_c_interrupts_the_line() {
    let out = run_session(&[b"doomed", b"\x03"]);
    assert!(out.contains("^C"), "^C marker missing:\n{out}");
    assert!(
        !out.contains(&echo("doomed")),
        "interrupted line was executed:\n{out}"
    );
}

#[test]
fn bracketed_paste_inserts_literally() {
    // A pasted tab must insert, not trigger completion.
    let out = run_session(&[b"\x1b[200~a\tb\x1b[201~", b"\r"]);
    assert!(out.contains(&echo("a\tb")), "paste not literal:\n{out}");
}
