use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo_bin_cmd;
use predicates::prelude::{PredicateBooleanExt, predicate};

#[test]
fn with_custom_file_extensions_args_recognizes_files_with_given_extensions() {
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
fn with_disable_args_failures_from_disabled_validators_are_ignored() {
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
fn with_enable_args_only_the_failures_from_enabled_validators_are_returned() {
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
fn disable_and_enable_flags_used_together_fails_with_error() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("--enable=keep-sorted");
    cmd.arg("--disable=keep-unique");
    cmd.write_stdin("");

    let output = cmd.output().expect("Failed to get command output");

    output.assert().failure();
}

#[test]
fn with_severity_warning_succeeds_with_exit_code_zero() {
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
fn with_severity_error_fails_with_exit_code_one() {
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
fn empty_diff_succeeds() {
    let mut cmd = cargo_bin_cmd!();
    cmd.write_stdin("");

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
#[ignore]
fn files_mode_succeeds_on_valid_file() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/paths/valid.py");

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
#[ignore]
fn files_mode_checks_multiple_explicit_paths() {
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
#[ignore]
fn files_mode_checks_glob_patterns() {
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
#[ignore]
fn files_mode_checks_recursive_glob_patterns() {
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
