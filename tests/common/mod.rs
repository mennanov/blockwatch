//! Shared helpers for integration tests.

// Each test file compiles this module on its own, and no file uses every helper.
#![allow(dead_code)]

/// The path of an empty config file. With `--config`, it makes a run use the flags alone.
///
/// The tests run inside this repository, so a run would read its `blockwatch.toml`. That file
/// ignores `tests/testdata`, so every fixture would be skipped, and a test that expects a clean run
/// would pass without checking anything.
pub(super) const EMPTY_CONFIG: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/common/empty.toml");

/// Returns a command that runs the `blockwatch` binary with `--config` set to [`EMPTY_CONFIG`].
// This is where the empty config gets added, so it has to start from the command without it.
#[allow(clippy::disallowed_macros)]
pub(super) fn command() -> assert_cmd::Command {
    let mut command = assert_cmd::cargo_bin_cmd!();
    command.args(["--config", EMPTY_CONFIG]);
    command
}

/// Returns a command that runs the `blockwatch` binary with `--config` set to [`EMPTY_CONFIG`].
///
/// It has the name of `assert_cmd::cargo_bin_cmd!` on purpose: a test file switches to it by
/// changing one import. `clippy.toml` rejects the `assert_cmd` macro, so a test can't use it by
/// mistake.
macro_rules! cargo_bin_cmd {
    () => {
        $crate::common::command()
    };
}
pub(super) use cargo_bin_cmd;

/// Runs the `blockwatch` binary with `args` and with `--config` set to [`EMPTY_CONFIG`], giving the
/// child process a real pseudo-terminal on stdin so that `stdin().is_terminal()` returns true
/// (i.e. the program behaves as if no diff is being piped in). `stdout`/`stderr` are captured as
/// pipes so callers can assert on them.
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
        .args(["--config", EMPTY_CONFIG])
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
