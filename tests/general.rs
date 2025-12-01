use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo_bin_cmd;
use predicates::prelude::{PredicateBooleanExt, predicate};

#[test]
fn custom_extensions_arg_provided_run_recognizes_custom_extensions() {
    let diff_content = r#"
diff --git a/tests/testdata/custom_file_extension.javascript b/tests/testdata/custom_file_extension.javascript
index 09baa87..33c9660 100644
--- a/tests/testdata/custom_file_extension.javascript
+++ b/tests/testdata/custom_file_extension.javascript
@@ -2,7 +2,7 @@
 
 function main() {
   // <block affects=":foo">
-  console.log("Hi");
+  console.log("Hi"); // Modified
   // </block>
 }
 
diff --git a/tests/testdata/custom_file_extension.python b/tests/testdata/custom_file_extension.python
index da567bd..5586a8d 100644
--- a/tests/testdata/custom_file_extension.python
+++ b/tests/testdata/custom_file_extension.python
@@ -2,7 +2,7 @@
 
 def main():
   # <block affects=":foo">
-  print("Hello world"!)
+  print("Hello world"!)  # Modified.
   # </block>
 
 def foo():
"#;

    let mut cmd = cargo_bin_cmd!();
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
    let diff_content = r#"
diff --git a/tests/testdata/disable_enable.py b/tests/testdata/disable_enable.py
index 6739b09..a8464fb 100644
--- a/tests/testdata/disable_enable.py
+++ b/tests/testdata/disable_enable.py
@@ -2,7 +2,7 @@ fruits = [
     # <block keep-unique>
     "apple",
     "banana",
-    "pear",
+    "apple",
     # </block>
 ]

@@ -10,6 +10,6 @@ colors = [
     # <block keep-sorted>
     "blue",
     "red",
-    "yellow",
+    "green",
     # </block>
 ]
"#;

    let mut cmd = cargo_bin_cmd!();
    cmd.arg("--disable=keep-sorted");
    cmd.write_stdin(diff_content);

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
    let diff_content = r#"
diff --git a/tests/testdata/disable_enable.py b/tests/testdata/disable_enable.py
index 6739b09..a8464fb 100644
--- a/tests/testdata/disable_enable.py
+++ b/tests/testdata/disable_enable.py
@@ -2,7 +2,7 @@ fruits = [
     # <block keep-unique>
     "apple",
     "banana",
-    "pear",
+    "apple",
     # </block>
 ]

@@ -10,6 +10,6 @@ colors = [
     # <block keep-sorted>
     "blue",
     "red",
-    "yellow",
+    "green",
     # </block>
 ]
"#;

    let mut cmd = cargo_bin_cmd!();
    cmd.arg("--enable=keep-sorted");
    cmd.write_stdin(diff_content);

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
    cmd.write_stdin("");

    let output = cmd.output().expect("Failed to get command output");

    output.assert().failure();
}

#[test]
fn severity_warning_violation_present_run_succeeds_with_exit_code_zero() {
    let diff_content = r#"
diff --git a/tests/testdata/severity.py b/tests/testdata/severity.py
index 74ff7b7..574d79a 100644
--- a/tests/testdata/severity.py
+++ b/tests/testdata/severity.py
@@ -2,6 +2,6 @@ fruits = [
     # <block keep-unique severity="warn">
     "apple",
     "banana",
-    "orange",
+    "apple",
     # </block>
 ]"#;
    let mut cmd = cargo_bin_cmd!();
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
fn severity_error_violation_present_run_fails_with_exit_code_one() {
    let diff_content = r#"
diff --git a/tests/testdata/severity.py b/tests/testdata/severity.py
index a01afcd..74c68a3 100644
--- a/tests/testdata/severity.py
+++ b/tests/testdata/severity.py
@@ -2,7 +2,7 @@ fruits = [
     # <block keep-unique severity="warning">
     "apple",
     "banana",
-    "orange",
+    "apple",
     # </block>
 ]

@@ -10,6 +10,6 @@ colors = [
     # <block keep-unique>
     "red",
     "green",
-    "red"
+    "green",
     # </block>
 ]"#;
    let mut cmd = cargo_bin_cmd!();
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output.assert().failure().code(1);
}

#[test]
fn empty_diff_provided_run_succeeds() {
    let mut cmd = cargo_bin_cmd!();
    cmd.write_stdin("");

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
fn valid_file_path_provided_run_succeeds() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/paths/valid.py");

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
fn multiple_explicit_paths_provided_run_checks_all_paths() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/paths/valid.py");
    cmd.arg("tests/testdata/paths/invalid.py");

    let output = cmd.output().expect("Failed to get command output");

    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("keep-sorted"))
        .stderr(predicate::str::contains("tests/testdata/paths/invalid.py"));
}

#[test]
fn glob_pattern_provided_run_checks_matching_files() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/paths/*.py");

    let output = cmd.output().expect("Failed to get command output");

    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("tests/testdata/paths/invalid.py"));
}

#[test]
fn recursive_glob_pattern_provided_run_checks_matching_files_recursively() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/paths/**/*.py");

    let output = cmd.output().expect("Failed to get command output");

    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "tests/testdata/paths/subdir/nested_invalid.py",
        ));
}

// Emulates running `blockwatch` with no args and input.
#[test]
fn no_globs_no_diff_input_provided_run_checks_for_all_paths() {
    let mut cmd = cargo_bin_cmd!();
    // BLOCKWATCH_TERMINAL_MODE is required to simulate a TTY input.
    cmd.env("BLOCKWATCH_TERMINAL_MODE", "true");
    // check-ai is disabled to avoid errors caused by the missing environment variables.
    cmd.arg("--disable=check-ai");

    let output = cmd.output().expect("Failed to get command output");

    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("keep-sorted"))
        .stderr(predicate::str::contains("tests/testdata/paths/invalid.py"));
}

#[test]
fn ignore_glob_provided_run_ignores_matching_files() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/paths/*.py");
    cmd.arg("--ignore");
    cmd.arg("tests/testdata/paths/invalid.py");
    // globset matches separators by default, so *.py matches subdir/nested_invalid.py
    cmd.arg("--ignore");
    cmd.arg("tests/testdata/paths/subdir/nested_invalid.py");

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
fn recursive_ignore_glob_provided_run_ignores_matching_files_recursively() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/paths/**/*.py");
    cmd.arg("--ignore");
    cmd.arg("**/invalid.py");
    cmd.arg("--ignore");
    cmd.arg("**/nested_invalid.py");

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}
