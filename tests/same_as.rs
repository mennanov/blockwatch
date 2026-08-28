use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo_bin_cmd;
use predicates::prelude::predicate;

#[test]
fn source_only_diff_resolves_untouched_target_and_passes() {
    // The diff touches only the source; the target file is not in the diff, forcing the validator
    // to read and parse it from disk.
    let diff = r#"
diff --git a/tests/testdata/same_as/source.rs b/tests/testdata/same_as/source.rs
index 1111111..2222222 100644
--- a/tests/testdata/same_as/source.rs
+++ b/tests/testdata/same_as/source.rs
@@ -1,3 +1,3 @@
 // <block same-as="tests/testdata/same_as/target.md:port">
-const PORT: u16 = 8000;
+const PORT: u16 = 8080;
 // </block>"#;
    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"]);
    cmd.write_stdin(diff).output().unwrap().assert().success();
}

#[test]
fn diff_touching_only_one_sibling_block_resolves_the_unmodified_one() {
    // The diff modifies only the first block; the sibling target block in the same file is not in
    // the diff. The target must still be resolved (both values agree), not reported as missing.
    let diff = r#"
diff --git a/tests/testdata/same_as/sibling.rs b/tests/testdata/same_as/sibling.rs
index 1111111..2222222 100644
--- a/tests/testdata/same_as/sibling.rs
+++ b/tests/testdata/same_as/sibling.rs
@@ -1,3 +1,3 @@
 // <block same-as=":b">
-const A: u16 = 8000;
+const A: u16 = 8080;
 // </block>"#;
    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"]);
    cmd.write_stdin(diff).output().unwrap().assert().success();
}

#[test]
fn custom_extension_mapping_applies_to_a_target_read_from_disk() {
    // The target file is outside the diff, so it is parsed from the disk rather than taken from the
    // validation context. That read has to honor `-E` too, or an extension the mapping made
    // parseable looks unsupported to `same-as` alone.
    let diff = r#"
diff --git a/tests/testdata/same_as/custom_ext_source.javascript b/tests/testdata/same_as/custom_ext_source.javascript
index 1111111..2222222 100644
--- a/tests/testdata/same_as/custom_ext_source.javascript
+++ b/tests/testdata/same_as/custom_ext_source.javascript
@@ -1,3 +1,3 @@
 // <block same-as="tests/testdata/same_as/custom_ext_target.javascript:port">
-const PORT = 8000;
+const PORT = 8080;
 // </block>"#;
    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed", "-E", "javascript=js"]);
    cmd.write_stdin(diff).output().unwrap().assert().success();
}

#[test]
fn whole_file_target_agreeing_with_the_block_passes() {
    // The target is a JSON file, a format with no grammar, so it can only be referenced whole. The
    // block's own `same-as-pattern` selects what to compare on each side.
    let diff = r#"
diff --git a/tests/testdata/same_as/whole_file_source.rs b/tests/testdata/same_as/whole_file_source.rs
index 1111111..2222222 100644
--- a/tests/testdata/same_as/whole_file_source.rs
+++ b/tests/testdata/same_as/whole_file_source.rs
@@ -1,3 +1,3 @@
 // <block same-as="tests/testdata/same_as/whole_file_target.json" same-as-pattern="\d{4}">
-const PORT: u16 = 8000;
+const PORT: u16 = 8080;
 // </block>"#;
    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"]);
    cmd.write_stdin(diff).output().unwrap().assert().success();
}

#[test]
fn whole_file_target_disagreeing_with_the_block_fails() {
    let diff = r#"
diff --git a/tests/testdata/same_as/whole_file_mismatch_source.rs b/tests/testdata/same_as/whole_file_mismatch_source.rs
index 1111111..2222222 100644
--- a/tests/testdata/same_as/whole_file_mismatch_source.rs
+++ b/tests/testdata/same_as/whole_file_mismatch_source.rs
@@ -1,3 +1,3 @@
 // <block same-as="tests/testdata/same_as/whole_file_target.json" same-as-pattern="\d{4}">
-const PORT: u16 = 8080;
+const PORT: u16 = 9000;
 // </block>"#;
    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"]);
    cmd.write_stdin(diff)
        .output()
        .unwrap()
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "disagrees with file tests/testdata/same_as/whole_file_target.json",
        ));
}
