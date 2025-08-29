use assert_cmd::Command;
use assert_cmd::assert::OutputAssertExt;
use predicates::prelude::predicate;
use serde_json::json;

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
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert()
        .failure()
        .code(1)
        .stderr(predicate::function(|output: &str| {
            let output_json: serde_json::Value = serde_json::from_str(output).unwrap();
            let value: serde_json::Value  = json!({
              "tests/testing_data.md": [
                {
                  "violation": "affects",
                  "error": "Block tests/testing_data.md:(unnamed) at line 3 is modified, but tests/testing_data.md:foo is not",
                  "details": {
                    "modified_block": {
                      "attributes": {
                        "affects": ":foo"
                      },
                      "ends_at_line": 6,
                      "starts_at_line": 3
                    }
                  }
                }
              ]
            });
            assert_eq!(output_json, value);
            true
        }));
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

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
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

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
fn with_custom_file_extensions_args() {
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

    let output = cmd.output().expect("Failed to get command output");

    output.assert().failure().code(1).stderr(predicate::function(|output: &str| {
        let output_json: serde_json::Value =
            serde_json::from_str(output).expect("invalid json");
        let value: serde_json::Value  = json!({
              "tests/another_testing_file.javascript": [
                {
                  "violation": "affects",
                  "error": "Block tests/another_testing_file.javascript:(unnamed) at line 4 is modified, but tests/another_testing_file.javascript:foo is not",
                  "details": {
                    "modified_block": {
                      "attributes": {
                        "affects": ":foo"
                      },
                      "ends_at_line": 6,
                      "starts_at_line": 4
                    }
                  }
                }
              ],
              "tests/testing_file.python": [
                {
                  "violation": "affects",
                  "error": "Block tests/testing_file.python:(unnamed) at line 4 is modified, but tests/testing_file.python:foo is not",
                  "details": {
                    "modified_block": {
                      "attributes": {
                        "affects": ":foo"
                      },
                      "ends_at_line": 6,
                      "starts_at_line": 4
                    }
                  }
                }
              ]
            });
        assert_eq!(output_json, value);
        true
    }));
}

#[test]
fn empty_diff_succeeds() {
    let mut cmd = get_cmd();
    cmd.write_stdin("");

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}
