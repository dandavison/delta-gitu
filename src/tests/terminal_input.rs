//! As git's pager, gitu's stdin is the pipe git writes to, and gitu reads keys
//! from the terminal. A program gitu runs in the foreground, such as the
//! editor, must read the terminal too: vim reading the pipe takes the patch as
//! keys and exits at its end, and emacs refuses to start.
//!
//! This needs a process whose terminal is a pseudo-terminal and whose stdin is
//! a pipe, so the test runs the test binary again as that process.

use super::*;
use std::io::{self, Read};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::{fs::PermissionsExt, process::CommandExt};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Where the editor writes whether its stdin is a terminal.
const EDITOR_LOG: &str = "GITU_TEST_EDITOR_LOG";

#[test]
fn the_editor_reads_the_terminal_when_stdin_is_a_pipe() {
    let dir = temp_dir::TempDir::new().unwrap();
    let log = dir.path().join("editor.log");
    let (master, slave) = pty(80, 20);

    let mut child = unsafe {
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "--ignored",
                "--nocapture",
                "tests::terminal_input::open_the_editor_from_the_pager",
            ])
            .env(EDITOR_LOG, &log)
            .stdin(Stdio::piped())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave)
            .pre_exec(|| {
                // A session of its own, whose terminal is the pseudo-terminal on
                // its stdout.
                if libc::setsid() == -1 || libc::ioctl(1, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            })
            .spawn()
            .unwrap()
    };

    // The child blocks once the terminal's buffer is full, so read it while
    // the child runs.
    let (output_tx, output_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut output = vec![];
        let _ = std::fs::File::from(master).read_to_end(&mut output);
        let _ = output_tx.send(output);
    });

    let status = child.wait().unwrap();
    let output = output_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_default();
    assert!(
        status.success(),
        "{}",
        String::from_utf8_lossy(&strip_ansi_escapes::strip(output))
    );
    assert_eq!(fs::read_to_string(&log).unwrap(), "stdin: terminal\n");
}

/// Run by the test above, as gitu run by git as its pager: `enter` on a file
/// opens the editor.
#[test]
#[ignore = "run by the_editor_reads_the_terminal_when_stdin_is_a_pipe"]
fn open_the_editor_from_the_pager() {
    let Ok(log) = std::env::var(EDITOR_LOG) else {
        return;
    };
    let mut ctx = setup_clone!();
    commit(&ctx.dir, "file", "a\n");
    fs::write(ctx.dir.join("file"), "b\n").unwrap();
    let editor = std::path::Path::new(&log).with_file_name("editor");
    fs::write(
        &editor,
        format!(
            "#!/bin/sh\n\
             if [ -t 0 ]; then echo 'stdin: terminal'; else echo 'stdin: not a terminal'; fi > '{log}'\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&editor, fs::Permissions::from_mode(0o755)).unwrap();

    let mut app = ctx.init_app_as_pager_of(&["git", "diff"]);
    unsafe { std::env::set_var("GITU_SHOW_EDITOR", &editor) };
    let mut term = crate::term::backend();
    for event in keys("<enter>") {
        app.handle_event(&mut term, event).unwrap();
    }
}

fn pty(cols: u16, rows: u16) -> (OwnedFd, OwnedFd) {
    let (mut master, mut slave) = (0, 0);
    let mut size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    assert_eq!(opened, 0);
    unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) }
}
