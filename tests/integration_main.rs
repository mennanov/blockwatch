use assert_cmd::Command;
use assert_cmd::assert::OutputAssertExt;
use predicates::prelude::{PredicateBooleanExt, predicate};

fn get_cmd() -> Command {
    Command::cargo_bin(assert_cmd::crate_name!()).expect("Failed to find binary")
}

#[test]
fn with_custom_file_extensions_args() {
    let diff_content = r#"
diff --git a/tests/custom_file_extension_test.javascript b/tests/custom_file_extension_test.javascript
index 09baa87..33c9660 100644
--- a/tests/custom_file_extension_test.javascript
+++ b/tests/custom_file_extension_test.javascript
@@ -2,7 +2,7 @@
 
 function main() {
   // <block affects=":foo">
-  console.log("Hi");
+  console.log("Hi"); // Modified
   // </block>
 }
 
diff --git a/tests/custom_file_extension_test.python b/tests/custom_file_extension_test.python
index da567bd..5586a8d 100644
--- a/tests/custom_file_extension_test.python
+++ b/tests/custom_file_extension_test.python
@@ -2,7 +2,7 @@
 
 def main():
   # <block affects=":foo">
-  print("Hello world"!)
+  print("Hello world"!)  # Modified.
   # </block>
 
 def foo():
"#;

    let mut cmd = get_cmd();
    cmd.arg("-E").arg("python=py");
    cmd.arg("-E").arg("javascript=js");
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output.assert().failure().code(1);
}

#[test]
fn with_disable_args() {
    let diff_content = r#"
diff --git a/tests/disabled_test.py b/tests/disabled_test.py
index 6739b09..a8464fb 100644
--- a/tests/isabled_test.py
+++ b/tests/disabled_test.py
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

    let mut cmd = get_cmd();
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
fn with_severity_warning_for_all_violations_exit_code_is_zero() {
    let diff_content = r#"
diff --git a/tests/severity_test.py b/tests/severity_test.py
index 74ff7b7..574d79a 100644
--- a/tests/severity_test.py
+++ b/tests/severity_test.py
@@ -2,6 +2,6 @@ fruits = [
     # <block keep-unique severity="warn">
     "apple",
     "banana",
-    "orange",
+    "apple",
     # </block>
 ]"#;
    let mut cmd = get_cmd();
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
fn with_severity_error_for_some_violations_fails_with_exit_code_one() {
    let diff_content = r#"
diff --git a/tests/severity_test.py b/tests/severity_test.py
index a01afcd..74c68a3 100644
--- a/tests/severity_test.py
+++ b/tests/severity_test.py
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
    let mut cmd = get_cmd();
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output.assert().failure().code(1);
}

#[test]
fn empty_diff_succeeds() {
    let mut cmd = get_cmd();
    cmd.write_stdin("");

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}
