use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo::CommandCargoExt;
use assert_cmd::cargo_bin_cmd;
use predicates::prelude::{PredicateBooleanExt, predicate};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

mod common;

#[test]
fn custom_extensions_arg_provided_run_recognizes_custom_extensions() {
    let diff_content = r#"
diff --git a/tests/testdata/general/custom_file_extension.javascript b/tests/testdata/general/custom_file_extension.javascript
index 09baa87..33c9660 100644
--- a/tests/testdata/general/custom_file_extension.javascript
+++ b/tests/testdata/general/custom_file_extension.javascript
@@ -2,7 +2,7 @@
 
 function main() {
   // <block affects=":foo">
-  console.log("Hi");
+  console.log("Hi"); // Modified
   // </block>
 }
 
diff --git a/tests/testdata/general/custom_file_extension.python b/tests/testdata/general/custom_file_extension.python
index da567bd..5586a8d 100644
--- a/tests/testdata/general/custom_file_extension.python
+++ b/tests/testdata/general/custom_file_extension.python
@@ -2,7 +2,7 @@
 
 def main():
   # <block affects=":foo">
-  print("Hello world"!)
+  print("Hello world"!)  # Modified.
   # </block>
 
 def foo():
"#;

    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"]);
    cmd.arg("-E").arg("python=py");
    cmd.arg("-E").arg("javascript=js");
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("affects"));
}

#[test]
fn disabled_validator_arg_provided_run_ignores_disabled_validator_failures() {
    // Both blocks of the fixture violate their rule.
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/general/disable_enable.py");
    cmd.arg("--disable=keep-sorted");

    let output = cmd.output().expect("Failed to get command output");

    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("keep-unique"))
        .stderr(predicate::str::contains("keep-sorted").not());
}

#[test]
fn enabled_validator_arg_provided_run_returns_only_enabled_validator_failures() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/general/disable_enable.py");
    cmd.arg("--enable=keep-sorted");

    let output = cmd.output().expect("Failed to get command output");

    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("keep-sorted"))
        .stderr(predicate::str::contains("keep-unique").not());
}

#[test]
fn disable_and_enable_flags_provided_run_fails_with_error() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("--enable=keep-sorted");
    cmd.arg("--disable=keep-unique");

    let output = cmd.output().expect("Failed to get command output");

    output.assert().failure();
}

#[test]
fn severity_warning_violation_present_run_succeeds_with_exit_code_zero() {
    let diff_content = r#"
diff --git a/tests/testdata/general/severity.py b/tests/testdata/general/severity.py
index 74ff7b7..574d79a 100644
--- a/tests/testdata/general/severity.py
+++ b/tests/testdata/general/severity.py
@@ -2,6 +2,6 @@ fruits = [
     # <block keep-unique severity="warn">
     "apple",
     "banana",
-    "orange",
+    "apple",
     # </block>
 ]"#;
    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"]);
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
fn severity_error_violation_present_run_fails_with_exit_code_one() {
    // The fixture holds a warning-severity violation and an error-severity one; a single
    // error-severity violation anywhere in the run is what makes the exit code non-zero.
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/general/severity.py");

    let output = cmd.output().expect("Failed to get command output");

    output.assert().failure().code(1);
}

#[test]
fn empty_diff_provided_run_fails_with_error() {
    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff"]);
    cmd.write_stdin("");

    let output = cmd.output().expect("Failed to get command output");

    output
        .assert()
        .failure()
        .stderr(predicate::str::contains("stdin is empty"));
}

#[test]
fn valid_file_path_provided_run_succeeds() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/general/paths/valid.py");

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
fn multiple_explicit_paths_provided_run_checks_all_paths() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/general/paths/valid.py");
    cmd.arg("tests/testdata/general/paths/invalid.py");

    let output = cmd.output().expect("Failed to get command output");

    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("keep-sorted"))
        .stderr(predicate::str::contains(
            "tests/testdata/general/paths/invalid.py",
        ));
}

#[test]
fn glob_pattern_provided_run_checks_matching_files() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/general/paths/*.py");

    let output = cmd.output().expect("Failed to get command output");

    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "tests/testdata/general/paths/invalid.py",
        ));
}

#[test]
fn recursive_glob_pattern_provided_run_checks_matching_files_recursively() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/general/paths/**/*.py");

    let output = cmd.output().expect("Failed to get command output");

    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "tests/testdata/general/paths/subdir/nested_invalid.py",
        ));
}

// Emulates running `blockwatch` with no args in an interactive terminal (no diff piped in).
#[cfg(unix)]
#[test]
fn no_globs_no_diff_input_provided_run_checks_for_all_paths() {
    // A terminal stdin and no globs makes blockwatch validate the whole tree.
    // check-ai is disabled to avoid errors caused by the missing environment variables.
    let output = common::run_with_tty_stdin(&["--disable=check-ai"], None);

    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("keep-sorted"))
        .stderr(predicate::str::contains(
            "tests/testdata/general/paths/invalid.py",
        ));
}

#[test]
fn ignore_glob_provided_run_ignores_matching_files() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/general/paths/*.py");
    cmd.arg("--ignore");
    cmd.arg("tests/testdata/general/paths/invalid.py");
    // globset matches separators by default, so *.py matches subdir/nested_invalid.py
    cmd.arg("--ignore");
    cmd.arg("tests/testdata/general/paths/subdir/nested_invalid.py");

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
fn recursive_ignore_glob_provided_run_ignores_matching_files_recursively() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/general/paths/**/*.py");
    cmd.arg("--ignore");
    cmd.arg("**/invalid.py");
    cmd.arg("--ignore");
    cmd.arg("**/nested_invalid.py");

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
fn no_diff_flag_provided_run_finishes_without_waiting_for_stdin() {
    // Without `--diff` the file set comes from the working tree, so the run must never consume
    // stdin. The child is given a stdin pipe that nothing ever writes to and nothing ever closes;
    // a run that tried to read it would block forever and trip the deadline below.
    let mut child = Command::cargo_bin("blockwatch")
        .expect("blockwatch binary should be built")
        .args(["tests/testdata/general/paths/valid.py"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn blockwatch");

    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("failed to poll blockwatch") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().ok();
            panic!("blockwatch is still running; it appears to be waiting on stdin");
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    assert!(status.success());
}

#[test]
fn diff_piped_without_diff_flag_provided_run_scans_the_whole_tree() {
    // The diff contains a file that has no violations. Were it read, the run would check that file
    // alone and succeed; ignoring it means the whole tree is scanned and its violations reported.
    let diff_content = r#"
diff --git a/tests/testdata/general/paths/valid.py b/tests/testdata/general/paths/valid.py
index 0000000..1111111 100644
--- a/tests/testdata/general/paths/valid.py
+++ b/tests/testdata/general/paths/valid.py
@@ -1,4 +1,4 @@
 # <block keep-sorted="asc">
-a = 0
+a = 1
 b = 2
 # </block>
"#;

    let mut cmd = cargo_bin_cmd!();
    // check-ai is disabled to avoid errors caused by the missing environment variables.
    cmd.arg("--disable=check-ai");
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("keep-sorted"))
        .stderr(predicate::str::contains(
            "tests/testdata/general/paths/invalid.py",
        ));
}

// `--diff` promises a diff on stdin, so a terminal there is a contradiction rather than an
// empty run.
#[cfg(unix)]
#[test]
fn diff_flag_with_terminal_stdin_provided_run_fails_with_error() {
    // check-ai is disabled to avoid errors caused by the missing environment variables.
    let output = common::run_with_tty_stdin(&["--diff", "--disable=check-ai"], None);

    output
        .assert()
        .failure()
        .stderr(predicate::str::contains("stdin is a terminal"));
}

#[test]
fn diff_with_invalid_paths_provided_run_fails_with_error_in_every_mode() {
    let diff_content = r#"
diff --git a/sub/tests/testdata/general/paths/invalid.py b/sub/tests/testdata/general/paths/invalid.py
index 0000000..1111111 100644
--- a/sub/tests/testdata/general/paths/invalid.py
+++ b/sub/tests/testdata/general/paths/invalid.py
@@ -1,4 +1,4 @@
 # <block keep-sorted="asc">
 b = 2
-a = 2
+a = 1
 # </block>
"#;

    // check-ai is disabled to avoid errors caused by the missing environment variables.
    for args in [
        ["--diff", "--only-changed", "--disable=check-ai"].as_slice(),
        ["--diff", "--disable=check-ai"].as_slice(),
    ] {
        let mut cmd = cargo_bin_cmd!();
        cmd.args(args);
        cmd.write_stdin(diff_content);

        let output = cmd.output().expect("Failed to get command output");

        output
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "does not exist in the repository root",
            ))
            .stderr(predicate::str::contains(
                "sub/tests/testdata/general/paths/invalid.py",
            ));
    }
}

#[test]
fn only_changed_flag_with_globs_provided_run_checks_their_intersection() {
    let diff_content = r#"
diff --git a/tests/testdata/general/paths/invalid.py b/tests/testdata/general/paths/invalid.py
index 0000000..1111111 100644
--- a/tests/testdata/general/paths/invalid.py
+++ b/tests/testdata/general/paths/invalid.py
@@ -1,4 +1,4 @@
 # <block keep-sorted="asc">
 b = 2
-a = 2
+a = 1
 # </block>
"#;

    // The changed file alone is checked, and it has a violation.
    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"]);
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("keep-sorted"));

    // Globs narrow that set further: the changed file is outside them, so nothing is checked.
    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed", "tests/testdata/list/**"]);
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
fn diff_input_with_ignore_flag_provided_run_ignores_matching_files_in_diff() {
    let diff_content = r#"
diff --git a/tests/testdata/general/paths/invalid.py b/tests/testdata/general/paths/invalid.py
index 0000000..1111111 100644
--- a/tests/testdata/general/paths/invalid.py
+++ b/tests/testdata/general/paths/invalid.py
@@ -1,4 +1,4 @@
 # <block keep-sorted="asc">
 b = 2
-a = 2
+a = 1
 # </block>
"#;

    // First, verify that without --ignore it fails.
    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"]);
    cmd.write_stdin(diff_content);
    let output = cmd.output().expect("Failed to get command output");
    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("keep-sorted"));

    // Now verify that with --ignore it succeeds.
    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"]);
    cmd.write_stdin(diff_content);
    cmd.arg("--ignore");
    cmd.arg("tests/testdata/general/paths/invalid.py");

    let output = cmd.output().expect("Failed to get command output");
    output.assert().success();
}
