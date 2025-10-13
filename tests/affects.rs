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
diff --git a/tests/affects_test.md b/tests/affects_test.md
index abc123..def456 100644
--- a/tests/affects_test.md
+++ b/tests/affects_test.md
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
              "tests/affects_test.md": [
                {
                  "range": {
                    "start": {
                        "line": 3,
                        "character": 0
                    },
                    "end": {
                        "line": 6,
                        "character": 0
                    }
                  },
                  "code": "affects",
                  "message": "Block tests/affects_test.md:(unnamed) at line 3 is modified, but tests/affects_test.md:foo is not",
                  "severity": 1,
                  "data": {
                    "affected_block_file_path": "tests/affects_test.md",
                    "affected_block_name": "foo",
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
diff --git a/tests/affects_test.md b/tests/testing_data
index abc123..def456 100644
--- a/tests/affects_test.md
+++ b/tests/affects_test.md
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
diff --git a/tests/affects_test.md b/tests/testing_data
index abc123..def456 100644
--- a/tests/affects_test.md
+++ b/tests/affects_test.md
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
