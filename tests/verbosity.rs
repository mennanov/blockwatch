use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo_bin_cmd;
use predicates::prelude::predicate;
use serde_json::Value;

/// Matches three files with one block each, plus one file whose extension has no parser.
const CLEAN_GLOB: &str = "tests/testdata/verbosity/**";
/// Matches one file with a block that fails validation.
const FAILING_GLOB: &str = "tests/testdata/verbosity_failing/**";
/// Matches one file whose single block carries both a synchronous and an asynchronous validator.
const MIXED_GLOB: &str = "tests/testdata/verbosity_mixed/**";

#[test]
fn without_the_flag_stdout_stays_empty() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(CLEAN_GLOB);
    let output = cmd.output().unwrap();

    output.assert().success().stdout(predicate::str::is_empty());
}

#[test]
fn summary_level_prints_one_line_of_counts() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(CLEAN_GLOB).arg("--verbosity").arg("summary");
    let output = cmd.output().unwrap();

    output
        .assert()
        .success()
        .stdout("blockwatch: 3/3 files, 3 blocks (1 unchecked), 2 checks, 0 violations\n");
}

#[test]
fn summary_level_with_no_matching_files_reports_zeroes() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("tests/testdata/no_such_directory/**")
        .arg("--verbosity")
        .arg("summary");
    let output = cmd.output().unwrap();

    output
        .assert()
        .success()
        .stdout("blockwatch: 0/0 files, 0 blocks (0 unchecked), 0 checks, 0 violations\n");
}

#[test]
fn full_level_lists_every_block_and_the_validators_that_examined_it() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(CLEAN_GLOB).arg("--verbosity").arg("full");
    let output = cmd.output().unwrap();

    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    output.assert().success();

    assert_eq!(report["summary"]["blocks"], 3);
    assert_eq!(report["summary"]["checks"], 2);
    assert_eq!(report["summary"]["files_skipped"], 1);
    assert_eq!(
        report["files"]["tests/testdata/verbosity/sorted.py"][0]["checks"],
        serde_json::json!(["keep-sorted"])
    );
}

#[test]
fn full_level_shows_a_block_no_validator_examined() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(CLEAN_GLOB).arg("--verbosity").arg("full");
    let output = cmd.output().unwrap();

    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let block = &report["files"]["tests/testdata/verbosity/noop.py"][0];

    // A block carrying only a `name` declares no rule, so no validator claims it. It is still
    // listed, with an empty check list, and it counts towards `blocks_unchecked`.
    assert_eq!(block["name"], "noop");
    assert_eq!(block["checks"], serde_json::json!([]));
    assert_eq!(report["summary"]["blocks_unchecked"], 1);
}

#[test]
fn full_level_with_violations_writes_two_parseable_documents() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(FAILING_GLOB).arg("--verbosity").arg("full");
    let output = cmd.output().unwrap();

    // The report goes to stdout and the violations go to stderr, so each one parses on its own.
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let violations: Value = serde_json::from_slice(&output.stderr).unwrap();
    output.assert().failure().code(1);

    assert_eq!(report["summary"]["violations"], 1);
    assert_eq!(
        report["files"]["tests/testdata/verbosity_failing/unsorted.py"][0]["checks"],
        serde_json::json!(["keep-sorted"])
    );
    assert!(
        violations["tests/testdata/verbosity_failing/unsorted.py"]
            .as_array()
            .is_some_and(|entries| !entries.is_empty())
    );
}

/// Touches the first of the two blocks in `verbosity_diff/two_blocks.py`, leaving the second alone.
const DIFF_TOUCHING_ONE_BLOCK: &str = r#"
diff --git a/tests/testdata/verbosity_diff/two_blocks.py b/tests/testdata/verbosity_diff/two_blocks.py
index 1111111..2222222 100644
--- a/tests/testdata/verbosity_diff/two_blocks.py
+++ b/tests/testdata/verbosity_diff/two_blocks.py
@@ -1,6 +1,6 @@
 fruits = [
     # <block name="fruits" keep-sorted="asc">
     'apple',
-    'blueberry',
+    'banana',
     # </block>
 ]"#;

#[test]
fn full_level_under_a_diff_describes_only_the_blocks_in_scope() {
    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"])
        .arg("--verbosity")
        .arg("full")
        .write_stdin(DIFF_TOUCHING_ONE_BLOCK);
    let output = cmd.output().unwrap();

    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    output.assert().success();

    // A diff puts only the blocks it touches in scope, so the untouched `vegetables` block in the
    // same file is absent from the report rather than listed as unchecked.
    let blocks = report["files"]["tests/testdata/verbosity_diff/two_blocks.py"]
        .as_array()
        .expect("the touched file is reported");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["name"], "fruits");
    assert_eq!(blocks[0]["is_content_modified"], true);
    assert_eq!(blocks[0]["checks"], serde_json::json!(["keep-sorted"]));

    // Only the file named by the diff is read, so the wider testdata tree is never scanned.
    assert_eq!(report["summary"]["files_scanned"], 1);
    assert_eq!(report["summary"]["blocks"], 1);
    assert_eq!(report["summary"]["blocks_unchecked"], 0);
    assert_eq!(report["summary"]["checks"], 1);
}

#[test]
fn summary_level_under_a_diff_counts_only_the_blocks_in_scope() {
    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"])
        .arg("--verbosity")
        .arg("summary")
        .write_stdin(DIFF_TOUCHING_ONE_BLOCK);
    let output = cmd.output().unwrap();

    output
        .assert()
        .success()
        .stdout("blockwatch: 1/1 files, 1 blocks (0 unchecked), 1 checks, 0 violations\n");
}

#[test]
fn diff_mode_with_a_violation_writes_the_report_and_the_violations() {
    let diff = r#"
diff --git a/tests/testdata/verbosity_failing/unsorted.py b/tests/testdata/verbosity_failing/unsorted.py
index 1111111..2222222 100644
--- a/tests/testdata/verbosity_failing/unsorted.py
+++ b/tests/testdata/verbosity_failing/unsorted.py
@@ -1,6 +1,6 @@
 vegetables = [
     # <block name="vegetables" keep-sorted="asc">
     'tomato',
-    'squash',
+    'potato',
     # </block>
 ]"#;
    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"])
        .arg("--verbosity")
        .arg("full")
        .write_stdin(diff);
    let output = cmd.output().unwrap();

    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let violations: Value = serde_json::from_slice(&output.stderr).unwrap();
    output.assert().failure().code(1);

    assert_eq!(report["summary"]["violations"], 1);
    assert_eq!(
        report["files"]["tests/testdata/verbosity_failing/unsorted.py"][0]["checks"],
        serde_json::json!(["keep-sorted"])
    );
    assert!(
        violations["tests/testdata/verbosity_failing/unsorted.py"]
            .as_array()
            .is_some_and(|entries| !entries.is_empty())
    );
}

#[test]
fn full_level_under_a_diff_omits_a_reference_target_the_diff_did_not_touch() {
    // Only the source side is in the diff. The target lives in another file and is resolved from
    // disk to run the comparison.
    let diff = r#"
diff --git a/tests/testdata/same_as_source.rs b/tests/testdata/same_as_source.rs
index 1111111..2222222 100644
--- a/tests/testdata/same_as_source.rs
+++ b/tests/testdata/same_as_source.rs
@@ -1,3 +1,3 @@
 // <block same-as="tests/testdata/same_as_target.md:port">
-const PORT: u16 = 8000;
+const PORT: u16 = 8080;
 // </block>"#;
    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"])
        .arg("--verbosity")
        .arg("full")
        .write_stdin(diff);
    let output = cmd.output().unwrap();

    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    output.assert().success();

    // Reading a block to compare against it is not checking it, so the target file is absent from
    // the report and uncounted, even though the run had to parse it.
    assert!(report["files"]["tests/testdata/same_as_target.md"].is_null());
    assert_eq!(report["summary"]["files_scanned"], 1);
    assert_eq!(report["summary"]["blocks"], 1);
    assert_eq!(
        report["files"]["tests/testdata/same_as_source.rs"][0]["checks"],
        serde_json::json!(["same-as"])
    );
}

#[test]
fn full_level_names_both_a_sync_and_an_async_validator_of_one_block() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(MIXED_GLOB).arg("--verbosity").arg("full");
    let output = cmd.output().unwrap();

    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    output.assert().success();

    // Synchronous and asynchronous validators run apart from each other and their results are
    // combined afterwards, so a block checked by one of each has to end up naming both.
    assert_eq!(
        report["files"]["tests/testdata/verbosity_mixed/both_kinds.py"][0]["checks"],
        serde_json::json!(["check-lua", "keep-sorted"])
    );
    assert_eq!(report["summary"]["checks"], 2);
    assert_eq!(
        report["summary"]["validators"],
        serde_json::json!({ "check-lua": 1, "keep-sorted": 1 })
    );
}

#[test]
fn verbosity_with_the_list_subcommand_fails() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("list").arg("--verbosity").arg("full");
    let output = cmd.output().unwrap();

    output
        .assert()
        .failure()
        .stderr(predicate::str::contains("`list` subcommand"));
}
