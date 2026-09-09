use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo_bin_cmd;
use predicates::prelude::{PredicateBooleanExt, predicate};
use serde_json::Value;

/// One named block whose lines are out of order: a single `keep-sorted` violation.
const SORTED: &str = "tests/testdata/suppressions/sorted.py";
/// One named block that breaks two rules at once, so a three-segment address clears only one.
const TWO_RULES: &str = "tests/testdata/suppressions/two_rules.py";
/// One named block with two dangling `affects` targets: two violations of the same validator.
const DANGLING: &str = "tests/testdata/suppressions/dangling.py";
/// The same out-of-order block as `SORTED`, but without a `name`.
const UNNAMED: &str = "tests/testdata/suppressions/unnamed.py";
/// The addresses a run prints, quoted here so a change to how they are derived, show up as a
/// failing test rather than as a suppression that silently stops matching.
const SORTED_ADDRESS: &str = "tests/testdata/suppressions/sorted.py:fruits:keep-sorted:bbd61689";
const GONE_A_ADDRESS: &str = "tests/testdata/suppressions/dangling.py:source:affects:80c47447";
const GONE_B_ADDRESS: &str = "tests/testdata/suppressions/dangling.py:source:affects:80c475fa";

/// The diagnostics a run wrote to stderr, keyed by file exactly as they were printed.
fn diagnostics(stderr: &[u8]) -> Value {
    serde_json::from_slice(stderr).expect("the diagnostics are JSON")
}

#[test]
fn suppressed_violation_exits_zero_and_is_still_reported() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(SORTED).args([
        "--suppress",
        "tests/testdata/suppressions/sorted.py:fruits:keep-sorted",
    ]);
    let output = cmd.output().unwrap();

    let violation = &diagnostics(&output.stderr)[SORTED][0];
    output.assert().success();

    // Reported, not hidden: the violation keeps its declared severity and only stops failing the
    // run.
    assert_eq!(violation["suppressed"], true);
    assert_eq!(violation["severity"], 1);
    assert_eq!(violation["code"], "keep-sorted");
    assert_eq!(violation["address"], SORTED_ADDRESS);
}

#[test]
fn unsuppressed_violation_of_another_validator_still_fails_the_run() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(TWO_RULES).args([
        "--suppress",
        "tests/testdata/suppressions/two_rules.py:both:keep-sorted",
    ]);
    let output = cmd.output().unwrap();

    let violations = diagnostics(&output.stderr)[TWO_RULES].clone();
    output.assert().failure().code(1);

    // An address names the validator as well as the block, so the block's other rule is untouched.
    let suppressed: Vec<&str> = violations
        .as_array()
        .expect("a list of violations")
        .iter()
        .filter(|violation| violation["suppressed"] == true)
        .map(|violation| violation["code"].as_str().expect("a validator name"))
        .collect();
    assert_eq!(suppressed, vec!["keep-sorted"]);
}

#[test]
fn four_segment_address_suppresses_one_violation_and_leaves_its_siblings() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(DANGLING).args(["--suppress", GONE_A_ADDRESS]);
    let output = cmd.output().unwrap();

    let violations = diagnostics(&output.stderr)[DANGLING].clone();
    output.assert().failure().code(1);

    let suppressed: Vec<&str> = violations
        .as_array()
        .expect("a list of violations")
        .iter()
        .filter(|violation| violation["suppressed"] == true)
        .map(|violation| {
            violation["address"]
                .as_str()
                .expect("a named block has an address")
        })
        .collect();
    assert_eq!(suppressed, vec![GONE_A_ADDRESS]);
}

#[test]
fn three_segment_address_suppresses_every_violation_of_that_validator() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(DANGLING).args([
        "--suppress",
        "tests/testdata/suppressions/dangling.py:source:affects",
    ]);
    let output = cmd.output().unwrap();

    let violations = diagnostics(&output.stderr)[DANGLING].clone();
    output.assert().success();

    assert!(
        violations
            .as_array()
            .expect("a list of violations")
            .iter()
            .all(|violation| violation["suppressed"] == true),
        "every violation must be suppressed: {violations}"
    );
}

#[test]
fn two_segment_address_suppresses_every_violation_of_that_block() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(TWO_RULES).args([
        "--suppress",
        "tests/testdata/suppressions/two_rules.py:both",
    ]);
    let output = cmd.output().unwrap();

    let violations = diagnostics(&output.stderr)[TWO_RULES].clone();
    output.assert().success();

    assert!(
        violations
            .as_array()
            .expect("a list of violations")
            .iter()
            .all(|violation| violation["suppressed"] == true),
        "both rules of the block must be suppressed: {violations}"
    );
}

#[test]
fn one_segment_address_suppresses_every_violation_in_the_file() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(TWO_RULES)
        .args(["--suppress", "tests/testdata/suppressions/two_rules.py"]);
    let output = cmd.output().unwrap();

    let violations = diagnostics(&output.stderr)[TWO_RULES].clone();
    output.assert().success();

    assert!(
        violations
            .as_array()
            .expect("a list of violations")
            .iter()
            .all(|violation| violation["suppressed"] == true),
        "every violation in the file must be suppressed: {violations}"
    );
}

#[test]
fn one_segment_address_leaves_other_files_alone() {
    let mut cmd = cargo_bin_cmd!();
    cmd.args([SORTED, TWO_RULES])
        .args(["--suppress", "tests/testdata/suppressions/two_rules.py"]);
    let output = cmd.output().unwrap();

    let diagnostics = diagnostics(&output.stderr);
    output.assert().failure().code(1);

    assert_eq!(diagnostics[SORTED][0].get("suppressed"), None);
}

#[test]
fn repeated_flags_suppress_several_violations_in_one_run() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(DANGLING)
        .args(["--suppress", GONE_A_ADDRESS])
        .args(["--suppress", GONE_B_ADDRESS]);
    let output = cmd.output().unwrap();

    output.assert().success();
}

#[test]
fn one_segment_address_suppresses_violations_on_unnamed_blocks() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(UNNAMED)
        .args(["--suppress", "tests/testdata/suppressions/unnamed.py"]);
    let output = cmd.output().unwrap();

    let violation = diagnostics(&output.stderr)[UNNAMED][0].clone();
    output.assert().success();

    // A file-wide address covers the file, so it must not leave the unnamed blocks behind.
    assert_eq!(violation["suppressed"], true);
    // There is still nothing narrower to point at, so no address is printed to copy.
    assert_eq!(violation.get("address"), None);
}

#[test]
fn violation_on_an_unnamed_block_cannot_be_addressed_more_narrowly() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(UNNAMED).args([
        "--suppress",
        "tests/testdata/suppressions/unnamed.py:fruits:keep-sorted",
    ]);
    let output = cmd.output().unwrap();

    let violation = diagnostics(&output.stderr)[UNNAMED][0].clone();
    output.assert().failure().code(1);

    assert_eq!(violation.get("suppressed"), None);
}

#[test]
fn address_covering_nothing_leaves_the_exit_code_alone() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(SORTED).args([
        "--suppress",
        "tests/testdata/suppressions/sorted.py:renamed:keep-sorted",
    ]);
    let output = cmd.output().unwrap();

    // The address covers nothing, so it changes nothing: the real violation still fails the run.
    output.assert().failure().code(1);
}

#[test]
fn malformed_address_fails_before_any_source_file_is_read() {
    let mut cmd = cargo_bin_cmd!();
    let address = "tests/testdata/suppressions/sorted.py:fruits:keep-sorted:bbd61689:extra";
    cmd.arg(SORTED).args(["--suppress", address]);
    let output = cmd.output().unwrap();

    // Nothing was validated, so the run fails at argument parsing rather than reporting violations.
    output
        .assert()
        .failure()
        .stderr(predicate::str::contains(address))
        .stderr(predicate::str::contains("\"code\"").not());
}

#[test]
fn suppress_from_reads_commit_message_and_suppresses_violation() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(SORTED).args([
        "--suppress-from",
        "tests/testdata/suppressions/commit_msg.txt",
    ]);
    let output = cmd.output().unwrap();

    let violation = &diagnostics(&output.stderr)[SORTED][0];
    output.assert().success();

    assert_eq!(violation["suppressed"], true);
    assert_eq!(violation["code"], "keep-sorted");
    assert_eq!(violation["address"], SORTED_ADDRESS);
}

#[test]
fn suppress_from_case_insensitive_matching() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(SORTED).args([
        "--suppress-from",
        "tests/testdata/suppressions/commit_msg_lowercase_trailer.txt",
    ]);
    let output = cmd.output().unwrap();

    output.assert().success();
}

#[test]
fn suppress_from_ignores_non_matching_lines() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(SORTED).args([
        "--suppress-from",
        "tests/testdata/suppressions/commit_msg_with_noise.txt",
    ]);
    let output = cmd.output().unwrap();

    output.assert().success();
}

#[test]
fn suppress_from_multiple_flags_collects_all_suppressions() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(DANGLING)
        .args([
            "--suppress-from",
            "tests/testdata/suppressions/commit_msg_gone_a.txt",
        ])
        .args([
            "--suppress-from",
            "tests/testdata/suppressions/commit_msg_gone_b.txt",
        ]);
    let output = cmd.output().unwrap();

    output.assert().success();
}

#[test]
fn suppress_from_combined_with_suppress_flag() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(DANGLING)
        .args(["--suppress", GONE_B_ADDRESS])
        .args([
            "--suppress-from",
            "tests/testdata/suppressions/commit_msg_gone_a.txt",
        ]);
    let output = cmd.output().unwrap();

    output.assert().success();
}

#[test]
fn suppress_from_malformed_address_fails() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(SORTED).args([
        "--suppress-from",
        "tests/testdata/suppressions/commit_msg_malformed.txt",
    ]);
    let output = cmd.output().unwrap();

    output
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid suppression address"));
}

#[test]
fn suppress_from_nonexistent_file_fails() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(SORTED)
        .args(["--suppress-from", "nonexistent_suppressions.txt"]);
    let output = cmd.output().unwrap();

    output
        .assert()
        .failure()
        .stderr(predicate::str::contains("failed to read suppression file"));
}

#[test]
fn suppress_from_file_outside_repository_fails_confinement() {
    let outside_dir = tempfile::tempdir().unwrap();
    let msg_file = outside_dir.path().join("commit_msg.txt");
    std::fs::write(&msg_file, "Blockwatch-suppress: foo:bar\n").unwrap();

    let mut cmd = cargo_bin_cmd!();
    cmd.arg(SORTED)
        .args(["--suppress-from", msg_file.to_str().unwrap()]);
    let output = cmd.output().unwrap();

    output
        .assert()
        .failure()
        .stderr(predicate::str::contains("outside the repository root"));
}
