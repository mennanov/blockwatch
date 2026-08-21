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
    cmd.args(["--diff", "--only-changed"]);
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert()
        .failure()
        .code(1)
        .stderr(predicate::function(|output: &str| {
            let output_json: serde_json::Value = serde_json::from_str(output).unwrap();
            let value: serde_json::Value = json!({
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
    // The diff deletes one line inside each block, so its post-image matches the on-disk fixture
    // and both blocks' contents count as modified.
    let diff_content = r#"
diff --git a/tests/testdata/affects.md b/tests/testing_data
index abc123..def456 100644
--- a/tests/testdata/affects.md
+++ b/tests/testdata/affects.md
@@ -1,13 +1,11 @@
 # Testing data for integration tests

 [//]: # (<block affects=":foo">)
-Deleted first line.
 First block.

 [//]: # (</block>)

 [//]: # (<block name="foo">)
-Deleted second line.
 Second block.

 [//]: # (</block>)
"#;

    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"]);
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
fn diff_with_satisfied_blocks_non_root_dir_succeeds() {
    // Same post-image-consistent diff as `diff_with_satisfied_blocks_succeeds`.
    let diff_content = r#"
diff --git a/tests/testdata/affects.md b/tests/testing_data
index abc123..def456 100644
--- a/tests/testdata/affects.md
+++ b/tests/testdata/affects.md
@@ -1,13 +1,11 @@
 # Testing data for integration tests

 [//]: # (<block affects=":foo">)
-Deleted first line.
 First block.

 [//]: # (</block>)

 [//]: # (<block name="foo">)
-Deleted second line.
 Second block.

 [//]: # (</block>)
"#;

    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"]);
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
    cmd.args(["--diff", "--only-changed"]);
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
fn diff_with_both_the_tag_and_the_content_modified_succeeds() {
    // The target's edit adds a comment above the block, changes its start tag and changes its
    // content, all in one change group. The content change is the one that satisfies `affects`, and
    // the two changes above it must not hide it.
    let diff_content = r#"
diff --git a/tests/testdata/affects_tag_and_content_source.ts b/tests/testdata/affects_tag_and_content_source.ts
index abc123..def456 100644
--- a/tests/testdata/affects_tag_and_content_source.ts
+++ b/tests/testdata/affects_tag_and_content_source.ts
@@ -1,3 +1,3 @@
 // <block name="source" affects="tests/testdata/affects_tag_and_content_target.ts:target">
-const value = 1;
+const value = 2;
 // </block>
diff --git a/tests/testdata/affects_tag_and_content_target.ts b/tests/testdata/affects_tag_and_content_target.ts
index abc123..def456 100644
--- a/tests/testdata/affects_tag_and_content_target.ts
+++ b/tests/testdata/affects_tag_and_content_target.ts
@@ -1,3 +1,4 @@
-// <block name="target" affects="tests/testdata/affects_tag_and_content_source.ts:source">
-const value = 1;
+// Added explanation.
+// <block name="target" affects="tests/testdata/affects_tag_and_content_source.ts:source" severity="warning">
+const value = 2;
 // </block>
"#;

    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"]);
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
fn diff_with_only_tag_modified_in_hunk_with_more_added_than_deleted_lines_succeeds() {
    let diff_content = r#"
diff --git a/tests/testdata/affects.py b/tests/testdata/affects.py
index abc123..def456 100644
--- a/tests/testdata/affects.py
+++ b/tests/testdata/affects.py
@@ -1,2 +1,3 @@
-# Project dependencies.
-# <block name="deps" affects=":deps-docs">
+# Project dependencies, kept sorted
+# and unique.
+# <block name="deps" affects=":deps-docs" keep-sorted="asc">
"#;

    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"]);
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
    cmd.args(["--diff", "--only-changed"]);
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output.assert()
        .failure()
        .code(1)
        .stderr(predicate::function(|output: &str| {
            let output_json: serde_json::Value = serde_json::from_str(output).unwrap();
            let value: serde_json::Value = json!({
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

/// The diff that both cross-file tests below feed in. It changes the source block and its `affects`
/// target together, which is exactly what the rule asks for, so no invocation over it may fail.
const CROSS_FILE_DIFF: &str = r#"
diff --git a/tests/testdata/affects_cross_file_source.rs b/tests/testdata/affects_cross_file_source.rs
index abc123..def456 100644
--- a/tests/testdata/affects_cross_file_source.rs
+++ b/tests/testdata/affects_cross_file_source.rs
@@ -1,3 +1,3 @@
 // <block name="limits" affects="tests/testdata/affects_cross_file_target.md:limits">
-pub const MAX: usize = 10;
+pub const MAX: usize = 20;
 // </block>
diff --git a/tests/testdata/affects_cross_file_target.md b/tests/testdata/affects_cross_file_target.md
index abc123..def456 100644
--- a/tests/testdata/affects_cross_file_target.md
+++ b/tests/testdata/affects_cross_file_target.md
@@ -1,5 +1,5 @@
 [//]: # (<block name="limits">)

-Max is 10.
+Max is 20.

 [//]: # (</block>)
"#;

#[test]
fn only_changed_with_globs_excluding_a_modified_affects_target_succeeds() {
    // Globs choose what a run validates; they must not shrink the set of files `affects` resolves
    // its targets against, or narrowing a run to one language would fail every cross-language rule.
    let mut cmd = cargo_bin_cmd!();
    cmd.args([
        "--diff",
        "--only-changed",
        "tests/testdata/affects_cross_file_source.rs",
    ]);
    cmd.write_stdin(CROSS_FILE_DIFF);

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
fn diff_with_globs_excluding_a_modified_affects_target_succeeds() {
    // The same guarantee on a full-tree run, where the globs filter the walk rather than the diff.
    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "tests/testdata/affects_cross_file_source.rs"]);
    cmd.write_stdin(CROSS_FILE_DIFF);

    let output = cmd.output().expect("Failed to get command output");

    output.assert().success();
}

#[test]
fn only_changed_with_globs_excluding_an_unmodified_affects_target_fails() {
    // The counterpart of the two tests above: resolving targets outside the validated set must
    // still report the ones the diff left alone, rather than assuming any excluded target is fine.
    let diff_content = r#"
diff --git a/tests/testdata/affects_cross_file_source.rs b/tests/testdata/affects_cross_file_source.rs
index abc123..def456 100644
--- a/tests/testdata/affects_cross_file_source.rs
+++ b/tests/testdata/affects_cross_file_source.rs
@@ -1,3 +1,3 @@
 // <block name="limits" affects="tests/testdata/affects_cross_file_target.md:limits">
-pub const MAX: usize = 10;
+pub const MAX: usize = 20;
 // </block>
"#;

    let mut cmd = cargo_bin_cmd!();
    cmd.args([
        "--diff",
        "--only-changed",
        "tests/testdata/affects_cross_file_source.rs",
    ]);
    cmd.write_stdin(diff_content);

    let output = cmd.output().expect("Failed to get command output");

    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "tests/testdata/affects_cross_file_target.md:limits is not",
        ));
}

#[test]
fn custom_extension_mapping_applies_to_a_target_outside_the_globs() {
    // The globs keep the target file out of the validation context, so `affects` reads it from disk
    // to see whether the diff changed it. That read has to honor `-E`: without the mapping the
    // file looks unparseable and its modified block is reported as unmodified.
    let diff = r#"
diff --git a/tests/testdata/affects_custom_ext_source.javascript b/tests/testdata/affects_custom_ext_source.javascript
index 1111111..2222222 100644
--- a/tests/testdata/affects_custom_ext_source.javascript
+++ b/tests/testdata/affects_custom_ext_source.javascript
@@ -1,3 +1,3 @@
 // <block affects="tests/testdata/affects_custom_ext_target.javascript:port">
-const PORT = 8000;
+const PORT = 8080;
 // </block>
diff --git a/tests/testdata/affects_custom_ext_target.javascript b/tests/testdata/affects_custom_ext_target.javascript
index 1111111..2222222 100644
--- a/tests/testdata/affects_custom_ext_target.javascript
+++ b/tests/testdata/affects_custom_ext_target.javascript
@@ -1,3 +1,3 @@
 // <block name="port">
-const PORT = 8000;
+const PORT = 8080;
 // </block>"#;
    let mut cmd = cargo_bin_cmd!();
    cmd.args([
        "--diff",
        "--only-changed",
        "-E",
        "javascript=js",
        "tests/testdata/affects_custom_ext_source.javascript",
    ]);
    cmd.write_stdin(diff).output().unwrap().assert().success();
}
