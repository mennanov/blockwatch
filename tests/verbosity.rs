use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo_bin_cmd;
use predicates::prelude::predicate;
use serde_json::Value;

/// Matches three files with one block each, plus one file whose extension has no parser.
const CLEAN_GLOB: &str = "tests/testdata/verbosity/clean/**";
/// Matches one file with a block that fails validation.
const FAILING_GLOB: &str = "tests/testdata/verbosity/failing/**";
/// Matches one file whose single block carries both a synchronous and an asynchronous validator.
const MIXED_GLOB: &str = "tests/testdata/verbosity/mixed/**";
/// Matches one file holding an `affects` source and the block it points at.
const AFFECTS_GLOB: &str = "tests/testdata/verbosity/affects/**";

/// Touches the content of both blocks in `verbosity_affects/pair.py`, so the `affects` obligation
/// the source declares is met.
const DIFF_TOUCHING_THE_AFFECTS_PAIR: &str = r#"
diff --git a/tests/testdata/verbosity/affects/pair.py b/tests/testdata/verbosity/affects/pair.py
index 1111111..2222222 100644
--- a/tests/testdata/verbosity/affects/pair.py
+++ b/tests/testdata/verbosity/affects/pair.py
@@ -1,7 +1,7 @@
 # <block name="source" affects=":target">
-SOURCE = "old"
+SOURCE = "value"
 # </block>

 # <block name="target">
-TARGET = "old"
+TARGET = "value"
 # </block>"#;

/// Touches only the `target` block in `verbosity_affects/pair.py`, leaving the `affects` source
/// block untouched — so the rule that block carries never gets a chance to run.
const DIFF_TOUCHING_ONLY_THE_AFFECTS_TARGET: &str = r#"
diff --git a/tests/testdata/verbosity/affects/pair.py b/tests/testdata/verbosity/affects/pair.py
index 1111111..2222222 100644
--- a/tests/testdata/verbosity/affects/pair.py
+++ b/tests/testdata/verbosity/affects/pair.py
@@ -4,4 +4,4 @@

 # <block name="target">
-TARGET = "old"
+TARGET = "value"
 # </block>"#;

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
        .stdout(
            "blockwatch: mode=all, 3/3 files, 3 blocks (1 unchecked, 0 needs --diff), 2 checks, 0 violations\n",
        );
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
        .stdout(
            "blockwatch: mode=all, 0/0 files, 0 blocks (0 unchecked, 0 needs --diff), 0 checks, 0 violations\n",
        );
}

#[test]
fn summary_level_without_a_diff_counts_the_rules_it_could_not_check() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(AFFECTS_GLOB).arg("--verbosity").arg("summary");
    let output = cmd.output().unwrap();

    output.assert().success().stdout(
        "blockwatch: mode=all, 1/1 files, 2 blocks (2 unchecked, 1 needs --diff), 0 checks, 0 violations\n",
    );
}

#[test]
fn summary_level_with_the_diff_gated_validator_disabled_counts_no_rules_as_unchecked() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(AFFECTS_GLOB)
        .arg("--disable=affects")
        .arg("--verbosity")
        .arg("summary");
    let output = cmd.output().unwrap();

    // A rule the user switched off is not a rule that went unchecked.
    output.assert().success().stdout(
        "blockwatch: mode=all, 1/1 files, 2 blocks (2 unchecked, 0 needs --diff), 0 checks, 0 violations\n",
    );
}

#[test]
fn summary_level_with_a_diff_reports_the_all_plus_diff_mode() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("--diff")
        .arg(AFFECTS_GLOB)
        .arg("--verbosity")
        .arg("summary")
        .write_stdin(DIFF_TOUCHING_THE_AFFECTS_PAIR);
    let output = cmd.output().unwrap();

    // The whole tree is still scanned, but the diff says which blocks changed, so `affects` runs
    // and nothing is left needing a diff.
    output.assert().success().stdout(
        "blockwatch: mode=all+diff, 1/1 files, 2 blocks (1 unchecked), 1 checks, 0 violations\n",
    );
}

#[test]
fn summary_level_with_a_diff_that_misses_the_rule_omits_the_needs_diff_clause() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("--diff")
        .arg(AFFECTS_GLOB)
        .arg("--verbosity")
        .arg("summary")
        .write_stdin(DIFF_TOUCHING_ONLY_THE_AFFECTS_TARGET);
    let output = cmd.output().unwrap();

    // The diff never reached the `affects` block, so its rule did not run — but a block the diff
    // simply did not touch is the normal state of an incremental check, not something to report.
    // Counting it would say nothing a reader could act on, so the clause stays out of diff runs.
    output.assert().success().stdout(
        "blockwatch: mode=all+diff, 1/1 files, 2 blocks (2 unchecked), 0 checks, 0 violations\n",
    );
}

#[test]
fn summary_level_with_only_changed_reports_the_only_changed_mode() {
    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"])
        .arg("--verbosity")
        .arg("summary")
        .write_stdin(DIFF_TOUCHING_THE_AFFECTS_PAIR);
    let output = cmd.output().unwrap();

    output.assert().success().stdout(
        "blockwatch: mode=only-changed, 1/1 files, 2 blocks (1 unchecked), 1 checks, 0 violations\n",
    );
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
        report["files"]["tests/testdata/verbosity/clean/sorted.py"][0]["checks"],
        serde_json::json!(["keep-sorted"])
    );
}

#[test]
fn full_level_names_the_count_of_blocks_that_needed_a_diff() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(AFFECTS_GLOB).arg("--verbosity").arg("full");
    let output = cmd.output().unwrap();

    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    output.assert().success();

    // Whatever parses the report reads this key by name, so it is asserted directly rather than
    // only through the summary line: a missing key reads as null here and fails.
    assert_eq!(report["summary"]["blocks_needing_diff"], 1);
}

#[test]
fn full_level_under_a_diff_omits_the_needs_diff_key() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("--diff")
        .arg(AFFECTS_GLOB)
        .arg("--verbosity")
        .arg("full")
        .write_stdin(DIFF_TOUCHING_ONLY_THE_AFFECTS_TARGET);
    let output = cmd.output().unwrap();

    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    output.assert().success();

    // Absent, not zero: a run given a diff has no opinion on how many blocks need one.
    assert_eq!(report["summary"].get("blocks_needing_diff"), None);
    assert_eq!(report["summary"]["blocks_unchecked"], 2);
}

#[test]
fn full_level_shows_a_block_no_validator_examined() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(CLEAN_GLOB).arg("--verbosity").arg("full");
    let output = cmd.output().unwrap();

    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let block = &report["files"]["tests/testdata/verbosity/clean/noop.py"][0];

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
        report["files"]["tests/testdata/verbosity/failing/unsorted.py"][0]["checks"],
        serde_json::json!(["keep-sorted"])
    );
    assert!(
        violations["tests/testdata/verbosity/failing/unsorted.py"]
            .as_array()
            .is_some_and(|entries| !entries.is_empty())
    );
}

/// Touches the first of the two blocks in `verbosity_diff/two_blocks.py`, leaving the second alone.
const DIFF_TOUCHING_ONE_BLOCK: &str = r#"
diff --git a/tests/testdata/verbosity/diff/two_blocks.py b/tests/testdata/verbosity/diff/two_blocks.py
index 1111111..2222222 100644
--- a/tests/testdata/verbosity/diff/two_blocks.py
+++ b/tests/testdata/verbosity/diff/two_blocks.py
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
    let blocks = report["files"]["tests/testdata/verbosity/diff/two_blocks.py"]
        .as_array()
        .expect("the touched file is reported");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["name"], "fruits");
    assert_eq!(blocks[0]["is_content_modified"], true);
    assert_eq!(blocks[0]["checks"], serde_json::json!(["keep-sorted"]));

    // The mode name matches the flag that selects it, and carries no space, so the summary line
    // it also appears on stays tokenizable on whitespace.
    assert_eq!(report["summary"]["mode"], "only-changed");

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
        .stdout(
            "blockwatch: mode=only-changed, 1/1 files, 1 blocks (0 unchecked), 1 checks, 0 violations\n",
        );
}

#[test]
fn diff_mode_with_a_violation_writes_the_report_and_the_violations() {
    let diff = r#"
diff --git a/tests/testdata/verbosity/failing/unsorted.py b/tests/testdata/verbosity/failing/unsorted.py
index 1111111..2222222 100644
--- a/tests/testdata/verbosity/failing/unsorted.py
+++ b/tests/testdata/verbosity/failing/unsorted.py
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
        report["files"]["tests/testdata/verbosity/failing/unsorted.py"][0]["checks"],
        serde_json::json!(["keep-sorted"])
    );
    assert!(
        violations["tests/testdata/verbosity/failing/unsorted.py"]
            .as_array()
            .is_some_and(|entries| !entries.is_empty())
    );
}

#[test]
fn full_level_under_a_diff_omits_a_reference_target_the_diff_did_not_touch() {
    // Only the source side is in the diff. The target lives in another file and is resolved from
    // disk to run the comparison.
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
    cmd.args(["--diff", "--only-changed"])
        .arg("--verbosity")
        .arg("full")
        .write_stdin(diff);
    let output = cmd.output().unwrap();

    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    output.assert().success();

    // Reading a block to compare against it is not checking it, so the target file is absent from
    // the report and uncounted, even though the run had to parse it.
    assert!(report["files"]["tests/testdata/same_as/target.md"].is_null());
    assert_eq!(report["summary"]["files_scanned"], 1);
    assert_eq!(report["summary"]["blocks"], 1);
    assert_eq!(
        report["files"]["tests/testdata/same_as/source.rs"][0]["checks"],
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
        report["files"]["tests/testdata/verbosity/mixed/both_kinds.py"][0]["checks"],
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
