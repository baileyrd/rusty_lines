//! End-to-end tests: drive `examples/demo` under a real pseudo-terminal,
//! exercising raw mode, escape-sequence decoding, and the repaint path
//! that the unit tests in `src/lib.rs` can't reach. Unix-only, like the
//! raw editor itself.
#![cfg(unix)]

use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn example_path(name: &str) -> std::path::PathBuf {
    // target/debug/deps/pty-<hash> -> target/debug/examples/<name>
    let mut p = std::env::current_exe().unwrap();
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.push("examples");
    p.push(name);
    p
}

fn spawn_example(name: &str) -> (OwnedFd, Child) {
    let mut master: libc::c_int = 0;
    let mut slave: libc::c_int = 0;
    let mut ws = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let rc = unsafe {
        // *mut pointers throughout: macOS's binding takes *mut termios /
        // *mut winsize where Linux takes *const, and *mut coerces to both.
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut ws,
        )
    };
    assert_eq!(rc, 0, "openpty failed");
    let master = unsafe { OwnedFd::from_raw_fd(master) };
    let slave = unsafe { OwnedFd::from_raw_fd(slave) };
    let child = Command::new(example_path(name))
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave))
        .spawn()
        .expect("spawn demo (is it built? cargo test builds examples)");
    (master, child)
}

/// Read whatever the pty has within `timeout_ms`, appending to `out`.
/// Returns false on EOF/EIO (slave side fully closed).
fn drain(master: &OwnedFd, out: &mut Vec<u8>, timeout_ms: i32) -> bool {
    let mut pfd = libc::pollfd {
        fd: master.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let n = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    if n <= 0 || pfd.revents & libc::POLLIN == 0 {
        return true; // nothing available right now
    }
    let mut buf = [0u8; 4096];
    let r = unsafe { libc::read(master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
    if r <= 0 {
        return false;
    }
    out.extend_from_slice(&buf[..r as usize]);
    true
}

/// Wait until the editor has painted a fresh, empty prompt. This is the
/// raw-mode synchronization point: the prompt is only painted once
/// read_line has the terminal set up, so keystrokes sent after it can't
/// be swallowed by the cooked-mode tty driver (on macOS the driver owns
/// C-w/C-y itself — VWERASE/VDSUSP — which corrupted unsynchronized runs).
fn wait_for_prompt(master: &OwnedFd, out: &mut Vec<u8>, deadline: Instant, prompt: &str) {
    loop {
        // The paint is `\r ESC[J <prompt> <buffer> \r ESC[<col>G`: with an
        // empty buffer, the text ends with the prompt + \r after
        // ANSI-stripping.
        if strip_ansi(out).trim_end_matches('\r').ends_with(prompt) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "no empty prompt; output so far:\n{}",
            strip_ansi(out)
        );
        drain(master, out, 100);
    }
}

/// Feed each chunk to the pty (whole, so escape sequences are never
/// split), waiting for the prompt before each chunk that starts a new
/// line; append Ctrl-D to exit the demo, and collect everything the
/// editor wrote until the child exits.
fn run_session(chunks: &[&[u8]]) -> String {
    run_session_in("demo", "demo> ", chunks)
}

fn run_session_in(example: &str, prompt: &str, chunks: &[&[u8]]) -> String {
    let (master, mut child) = spawn_example(example);
    let mut m = std::fs::File::from(master.try_clone().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut out = Vec::new();
    let mut at_line_start = true;
    for chunk in chunks {
        if at_line_start {
            wait_for_prompt(&master, &mut out, deadline, prompt);
        }
        m.write_all(chunk).unwrap();
        m.flush().unwrap();
        // Let the editor drain this chunk before the next one, so a
        // trailing ESC is never glued to the next chunk's bytes.
        std::thread::sleep(Duration::from_millis(100));
        drain(&master, &mut out, 0);
        // Enter and Ctrl-C both end the line and repaint a fresh prompt.
        at_line_start = matches!(chunk.last(), Some(b'\r' | b'\x03'));
    }
    wait_for_prompt(&master, &mut out, deadline, prompt);
    m.write_all(b"\x04").unwrap(); // Ctrl-D on the empty line: exit
    m.flush().unwrap();

    let mut exited = false;
    loop {
        if !drain(&master, &mut out, 200) {
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
fn ctrl_o_operate_and_get_next_replays_history() {
    // Recall "one" with two Ups, then C-o: it executes "one" and
    // pre-loads "two" on the next prompt, so a bare Enter replays it.
    let out = run_session(&[b"one\r", b"two\r", b"\x1b[A\x1b[A", b"\x0f", b"\r"]);
    assert!(
        out.matches(&echo("one")).count() >= 2,
        "C-o did not execute the recalled line:\n{out}"
    );
    assert!(
        out.matches(&echo("two")).count() >= 2,
        "next history entry not pre-loaded after C-o:\n{out}"
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
fn ctrl_l_clears_screen_and_keeps_the_line() {
    // C-l mid-line: the screen clears but the buffer and cursor survive.
    let out = run_session(&[b"abc", b"\x0c", b"def", b"\r"]);
    assert!(
        out.contains(&echo("abcdef")),
        "line lost across C-l:\n{out}"
    );
}

#[test]
fn bracketed_paste_inserts_literally() {
    // A pasted tab must insert, not trigger completion.
    let out = run_session(&[b"\x1b[200~a\tb\x1b[201~", b"\r"]);
    assert!(out.contains(&echo("a\tb")), "paste not literal:\n{out}");
}

#[test]
fn tab_menu_cycles_candidates() {
    // "al" → Tab inserts the LCP "alpha"; Tab again lists and arms the
    // menu; further Tabs cycle: alpha (candidate 0), then alphabet.
    let out = run_session_in(
        "hooked",
        "hooked> ",
        &[b"al", b"\t", b"\t", b"\t", b"\t", b"\r"],
    );
    assert!(
        out.contains("alphanumeric"),
        "candidate list missing:\n{out}"
    );
    assert!(out.contains(&echo("alphabet")), "menu cycle wrong:\n{out}");
}

#[test]
fn right_arrow_accepts_full_hint() {
    let out = run_session_in(
        "hooked",
        "hooked> ",
        &[b"alpha beta\r", b"alp", b"\x1b[C", b"\r"],
    );
    let hits = out.matches(&echo("alpha beta")).count();
    assert!(hits >= 2, "hint not accepted (typed + hinted):\n{out}");
}

#[test]
fn alt_f_accepts_one_word_of_hint() {
    let out = run_session_in(
        "hooked",
        "hooked> ",
        &[b"alpha beta\r", b"al", b"\x1bf", b"\r"],
    );
    assert!(
        out.contains(&echo("alpha")),
        "partial hint accept wrong:\n{out}"
    );
}

#[test]
fn rebound_key_runs_the_new_action() {
    // hooked rebinds C-o to unix-line-discard: "abc" C-o "def" leaves
    // only "def".
    let out = run_session_in("hooked", "hooked> ", &[b"abc", b"\x0f", b"def", b"\r"]);
    assert!(out.contains(&echo("def")), "rebinding ignored:\n{out}");
    assert!(
        !out.contains(&echo("abcdef")),
        "rebound key did nothing:\n{out}"
    );
}

#[test]
fn host_binding_rewrites_the_line() {
    // hooked binds C-g to a host command that uppercases the line
    // (bash `bind -x`): the edited buffer must survive the raw-mode
    // suspend/resume round trip.
    let out = run_session_in("hooked", "hooked> ", &[b"abc", b"\x07", b"\r"]);
    assert!(out.contains(&echo("ABC")), "host binding missing:\n{out}");
}

#[test]
fn read_line_timeout_expires_at_the_prompt() {
    // Sit at the timeout example's prompt without typing: the 2s deadline
    // must fire, print bash's message, and exit cleanly.
    let (master, mut child) = spawn_example("timeout");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut out = Vec::new();
    wait_for_prompt(&master, &mut out, deadline, "timeout> ");
    loop {
        if !drain(&master, &mut out, 200) || child.try_wait().unwrap().is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timeout demo did not exit:\n{}",
            strip_ansi(&out)
        );
    }
    let status = child.wait().unwrap();
    assert!(status.success(), "timeout demo exited with {status}");
    drain(&master, &mut out, 200);
    let out = strip_ansi(&out);
    assert!(
        out.contains("timed out waiting for input"),
        "timeout message missing:\n{out}"
    );
}

#[test]
fn resize_while_idle_repaints_and_keeps_the_line() {
    // Type half a line, shrink the terminal while the editor idles at
    // the prompt (no keystroke pending), then finish the line: the idle
    // poll tick must notice the new width and nothing may be lost.
    let (master, mut child) = spawn_example("demo");
    let mut m = std::fs::File::from(master.try_clone().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut out = Vec::new();
    wait_for_prompt(&master, &mut out, deadline, "demo> ");
    m.write_all(b"abc").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let ws = libc::winsize {
        ws_row: 24,
        ws_col: 40,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let rc = unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &ws) };
    assert_eq!(rc, 0, "TIOCSWINSZ failed");
    std::thread::sleep(Duration::from_millis(500)); // > one idle tick
    m.write_all(b"def\r").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    wait_for_prompt(&master, &mut out, deadline, "demo> ");
    m.write_all(b"\x04").unwrap();
    loop {
        if !drain(&master, &mut out, 200) || child.try_wait().unwrap().is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "demo did not exit");
    }
    child.wait().unwrap();
    let out = strip_ansi(&out);
    assert!(
        out.contains(&echo("abcdef")),
        "line lost across resize:\n{out}"
    );
}
