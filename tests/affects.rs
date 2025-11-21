use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo_bin_cmd;
use predicates::prelude::predicate;
use serde_json::json;

#[test]
fn diff_with_unsatisfied_blocks_fails() {
    let diff_content = r#"
diff --git a/tests/testdata/affects.md b/tests/testdata/affects.md
index abc123..def456 100644
--- a/tests/testdata/affects.md
+++ b/tests/testdata/affects.md
@@ -1,6 +1,5 @@
 # Testing data for integration tests

 [//]: # (<block affects=":foo">)
-First block.

 [//]: # (</block>)
"#;

    let mut cmd = cargo_bin_cmd!();
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert()
        .failure()
        .code(1)
        .stderr(predicate::function(|output: &str| {
            let output_json: serde_json::Value = serde_json::from_str(output).unwrap();
            let value: serde_json::Value  = json!({
              "tests/testdata/affects.md": [
                {
                  "range": {
                    "start": {
                        "line": 3,
                        "character": 10
                    },
                    "end": {
                        "line": 3,
                        "character": 31
                    }
                  },
                  "code": "affects",
                  "message": "Block tests/testdata/affects.md:(unnamed) at line 3 is modified, but tests/testdata/affects.md:foo is not",
                  "severity": 1,
                  "data": {
                    "affected_block_file_path": "tests/testdata/affects.md",
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
diff --git a/tests/testdata/affects.md b/tests/testing_data
index abc123..def456 100644
--- a/tests/testdata/affects.md
+++ b/tests/testdata/affects.md
@@ -1,11 +1,9 @@
 # Testing data for integration tests

 [//]: # (<block affects=":foo">)
-First block.

 [//]: # (</block>)

 [//]: # (<block name="foo">)
-Second block.

 [//]: # (</block>)
"#;

    let mut cmd = cargo_bin_cmd!();
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
fn diff_with_satisfied_blocks_non_root_dir_succeeds() {
    let diff_content = r#"
diff --git a/tests/testdata/affects.md b/tests/testing_data
index abc123..def456 100644
--- a/tests/testdata/affects.md
+++ b/tests/testdata/affects.md
@@ -1,11 +1,9 @@
 # Testing data for integration tests

 [//]: # (<block affects=":foo">)
-First block.

 [//]: # (</block>)

 [//]: # (<block name="foo">)
-Second block.

 [//]: # (</block>)
"#;

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir("./tests");
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
fn diff_with_only_tag_modified_succeeds() {
    let diff_content = r#"
diff --git a/tests/testdata/affects.md b/tests/testdata/affects.md
index abc123..def456 100644
--- a/tests/testdata/affects.md
+++ b/tests/testdata/affects.md
@@ -1,6 +1,6 @@
 # Testing data for integration tests

-[//]: # (<block affects=":foo" name="first">)
+[//]: # (<block affects=":foo">)
 First block.

 [//]: # (</block>)
"#;

    let mut cmd = cargo_bin_cmd!();
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
fn diff_dependent_block_with_only_tag_modified_fails() {
    let diff_content = r#"
diff --git a/tests/testdata/affects.md b/tests/testing_data
index abc123..def456 100644
--- a/tests/testdata/affects.md
+++ b/tests/testdata/affects.md
@@ -1,11 +1,9 @@
 # Testing data for integration tests

 [//]: # (<block affects=":foo">)
-First block.

 [//]: # (</block>)

- [//]: # (<block name="foo" test="value">)
+ [//]: # (<block name="foo">)
 Second block.

 [//]: # (</block>)
"#;

    let mut cmd = cargo_bin_cmd!();
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output.assert()
        .failure()
        .code(1)
        .stderr(predicate::function(|output: &str| {
            let output_json: serde_json::Value = serde_json::from_str(output).unwrap();
            let value: serde_json::Value  = json!({
              "tests/testdata/affects.md": [
                {
                  "range": {
                    "start": {
                        "line": 3,
                        "character": 10
                    },
                    "end": {
                        "line": 3,
                        "character": 31
                    }
                  },
                  "code": "affects",
                  "message": "Block tests/testdata/affects.md:(unnamed) at line 3 is modified, but tests/testdata/affects.md:foo is not",
                  "severity": 1,
                  "data": {
                    "affected_block_file_path": "tests/testdata/affects.md",
                    "affected_block_name": "foo",
                  }
                }
              ]
            });
            assert_eq!(output_json, value);
            true
        }));
}
