//! Shared helpers for integration tests.

/// Runs the `blockwatch` binary with `args`, giving the child process a real
/// pseudo-terminal on stdin so that `stdin().is_terminal()` returns true (i.e. the
/// program behaves as if no diff is being piped in). `stdout`/`stderr` are captured
/// as pipes so callers can assert on them.
///
/// Unix only: it relies on `openpty(3)`.
#[cfg(unix)]
pub fn run_with_tty_stdin(args: &[&str], current_dir: Option<&str>) -> std::process::Output {
    use assert_cmd::cargo::CommandCargoExt;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::process::{Command, Stdio};

    // Open a pty pair. The slave end becomes the child's stdin (a terminal); the
    // master end is kept open by the parent until the child exits.
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut::<libc::termios>(),
            std::ptr::null_mut::<libc::winsize>(),
        )
    };
    assert_eq!(rc, 0, "openpty failed: {}", std::io::Error::last_os_error());

    // SAFETY: `master` and `slave` were just returned by `openpty` and are owned here.
    // We wrap them in `OwnedFd` immediately to manage their lifespans safely.
    let _master_fd = unsafe { OwnedFd::from_raw_fd(master) };
    let slave_fd = unsafe { OwnedFd::from_raw_fd(slave) };

    // Transfer ownership of the slave fd to the child's stdin.
    let stdin = Stdio::from(slave_fd);

    let mut command = Command::cargo_bin("blockwatch").expect("blockwatch binary should be built");
    if let Some(dir) = current_dir {
        command.current_dir(dir);
    }
    let child = command
        .args(args)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn blockwatch");

    let output = child
        .wait_with_output()
        .expect("failed to wait for blockwatch");

    // Both FDs are automatically closed here as they go out of scope.
    output
}
