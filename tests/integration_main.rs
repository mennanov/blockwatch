use assert_cmd::Command;
use assert_cmd::assert::OutputAssertExt;
use predicates::prelude::predicate;
use serde_json::json;

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
    cmd.arg("-E python=py");
    cmd.arg("-E javascript=js");
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output.assert().failure().code(1).stderr(predicate::function(|output: &str| {
        let output_json: serde_json::Value =
            serde_json::from_str(output).expect("invalid json");
        let value: serde_json::Value  = json!({
              "tests/custom_file_extension_test.javascript": [
                {
                  "violation": "affects",
                  "error": "Block tests/custom_file_extension_test.javascript:(unnamed) at line 4 is modified, but tests/custom_file_extension_test.javascript:foo is not",
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
              "tests/custom_file_extension_test.python": [
                {
                  "violation": "affects",
                  "error": "Block tests/custom_file_extension_test.python:(unnamed) at line 4 is modified, but tests/custom_file_extension_test.python:foo is not",
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
