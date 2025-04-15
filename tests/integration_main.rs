use assert_cmd::Command;
use predicates::boolean::PredicateBooleanExt;
use predicates::prelude::predicate;

fn get_cmd() -> Command {
    Command::cargo_bin(assert_cmd::crate_name!()).expect("Failed to find binary")
}

#[test]
fn diff_with_unsatisfied_blocks_fails() {
    let diff_content = r#"
diff --git a/tests/testing_data.md b/tests/testing_data
index abc123..def456 100644
--- a/tests/testing_data.md
+++ b/tests/testing_data.md
@@ -1,6 +1,5 @@
 # Testing data for integration tests
 
 [//]: # (<block affects=":foo">)
-First block.
 
 [//]: # (</block>)
"#;

    let mut cmd = get_cmd();
    cmd.write_stdin(diff_content);

    cmd.assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "Error: Blocks need to be updated:
tests/testing_data.md:foo referenced by tests/testing_data.md:(unnamed) at line 3",
        ));
}

#[test]
fn diff_with_satisfied_blocks_succeeds() {
    let diff_content = r#"
diff --git a/tests/testing_data.md b/tests/testing_data
index abc123..def456 100644
--- a/tests/testing_data.md
+++ b/tests/testing_data.md
@@ -1,11 +1,9 @@
 # Testing data for integration tests
 
 [//]: # (<block affects=":foo">)
-First block.
 
 [//]: # (</block>)
 
 [//]: # (<block name="foo">)
-Second block.
 
 [//]: # (</block>)
"#;

    let mut cmd = get_cmd();
    cmd.write_stdin(diff_content);

    cmd.assert().success();
}

#[test]
fn diff_with_satisfied_blocks_non_root_dir_succeeds() {
    let diff_content = r#"
diff --git a/tests/testing_data.md b/tests/testing_data
index abc123..def456 100644
--- a/tests/testing_data.md
+++ b/tests/testing_data.md
@@ -1,11 +1,9 @@
 # Testing data for integration tests
 
 [//]: # (<block affects=":foo">)
-First block.
 
 [//]: # (</block>)
 
 [//]: # (<block name="foo">)
-Second block.
 
 [//]: # (</block>)
"#;

    let mut cmd = get_cmd();
    cmd.current_dir("./tests");
    cmd.write_stdin(diff_content);

    cmd.assert().success();
}

#[test]
fn with_custom_file_extensions_args_handles_files_with_custom_extensions() {
    let diff_content = r#"
diff --git a/tests/another_testing_file.javascript b/tests/another_testing_file.javascript
index 09baa87..33c9660 100644
--- a/tests/another_testing_file.javascript
+++ b/tests/another_testing_file.javascript
@@ -2,7 +2,7 @@
 
 function main() {
   // <block affects=":foo">
-  console.log("Hi");
+  console.log("Hi"); // Modified
   // </block>
 }
 
diff --git a/tests/testing_file.python b/tests/testing_file.python
index da567bd..5586a8d 100644
--- a/tests/testing_file.python
+++ b/tests/testing_file.python
@@ -2,7 +2,7 @@
 
 def main():
   # <block affects=":foo">
-  print("Hello world"!)
+  print("Hello world"!)  # Modified.
   # </block>
 
 def foo():
"#;

    let mut cmd = get_cmd();
    cmd.arg("-E python=py");
    cmd.arg("-E javascript=js");
    cmd.write_stdin(diff_content);

    cmd.assert().failure().code(1).stderr(
        predicate::str::contains("Blocks need to be updated")
            .and(predicate::str::contains("tests/testing_file.python:foo"))
            .and(predicate::str::contains(
                "tests/another_testing_file.javascript:foo",
            )),
    );
}

#[test]
fn empty_diff_succeeds() {
    let mut cmd = get_cmd();
    cmd.write_stdin("");

    cmd.assert().success();
}
