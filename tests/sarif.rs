use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo_bin_cmd;
use assert_json_diff::assert_json_include;
use serde_json::{Value, json};

/// One named block whose lines are out of order: a single error-severity `keep-sorted` violation.
const SORTED: &str = "tests/testdata/sarif/sorted.py";
/// The same block, declared advisory, so its violation must not fail the run.
const ADVISORY: &str = "tests/testdata/sarif/advisory.py";
/// The same out-of-order block, but without a `name`.
const UNNAMED: &str = "tests/testdata/sarif/unnamed.py";
/// A block that breaks no rule.
const CLEAN: &str = "tests/testdata/sarif/clean.py";
/// The address a run prints for the `SORTED` block, quoted here so a change to how addresses are
/// derived shows up as a failing test rather than as a fingerprint that silently stops matching.
const SORTED_ADDRESS: &str = "tests/testdata/sarif/sorted.py:fruits:keep-sorted:bbd61689";

/// The SARIF log a run wrote to stderr.
fn sarif_log(stderr: &[u8]) -> Value {
    serde_json::from_slice(stderr).expect("the log is JSON")
}

/// The only result of a run over `file`, plus the run's exit status.
fn only_result(file: &str, extra_args: &[&str]) -> (Value, std::process::Output) {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(file).args(["--format", "sarif"]).args(extra_args);
    let output = cmd.output().unwrap();
    let log = sarif_log(&output.stderr);
    let results = log["runs"][0]["results"]
        .as_array()
        .expect("a list of results")
        .clone();
    assert_eq!(results.len(), 1, "expected one result, got: {log}");
    (results[0].clone(), output)
}

#[test]
fn violation_is_written_as_a_sarif_result() {
    let (result, output) = only_result(SORTED, &[]);
    output.assert().failure().code(1);

    assert_json_include!(
        actual: result,
        expected: json!({
            "ruleId": "keep-sorted",
            "level": "error",
            "locations": [{
                "physicalLocation": {
                    "artifactLocation": {"uri": SORTED},
                    "region": {"startLine": 4},
                },
            }],
            "partialFingerprints": {"blockwatchAddress/v1": SORTED_ADDRESS},
        })
    );
}

#[test]
fn log_mentions_the_tool_and_describes_the_rules_that_fired() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(SORTED).args(["--format", "sarif"]);
    let output = cmd.output().unwrap();

    let log = sarif_log(&output.stderr);
    let driver = log["runs"][0]["tool"]["driver"].clone();
    output.assert().failure().code(1);

    assert_json_include!(
        actual: log,
        expected: json!({"runs": [{"columnKind": "unicodeCodePoints"}]})
    );
    assert_json_include!(
        actual: driver.clone(),
        expected: json!({
            "name": "blockwatch",
            "version": env!("CARGO_PKG_VERSION"),
            "semanticVersion": env!("CARGO_PKG_VERSION"),
            "rules": [{"id": "keep-sorted"}],
        })
    );
    // An expected array only has to be a prefix of the actual one, so the count is asserted on its
    // own.
    let rules = driver["rules"].as_array().expect("a list of rules");
    assert_eq!(rules.len(), 1, "only the rule that fired: {driver}");
    assert!(
        rules[0]["helpUri"]
            .as_str()
            .expect("a help URI")
            .ends_with("/docs/validators/keep-sorted.md"),
        "the rule must link to its documentation: {driver}"
    );
}

#[test]
fn suppressed_violation_is_reported_as_suppressed_and_does_not_fail_the_run() {
    let (result, output) = only_result(SORTED, &["--suppress", SORTED_ADDRESS]);
    output.assert().success();

    // Reported, not hidden: the finding keeps the level its author declared and carries the verdict
    // that takes it out of what a consumer treats as outstanding.
    assert_json_include!(
        actual: result,
        expected: json!({"level": "error", "suppressions": [{"kind": "external"}]})
    );
}

#[test]
fn run_that_found_nothing_still_writes_a_log() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(CLEAN).args(["--format", "sarif"]);
    let output = cmd.output().unwrap();

    let log = sarif_log(&output.stderr);
    output.assert().success();

    // A code-scanning service reads a missing log as a run that never happened, so a clean run has
    // to say so rather than write nothing.
    // Compared exactly, because an expected array only has to be a prefix of the actual one: an
    // inclusive match against an empty list would pass whatever the run reported.
    assert_eq!(log["version"], "2.1.0");
    assert_eq!(log["runs"][0]["results"], json!([]), "{log}");
}

#[test]
fn advisory_violation_is_reported_as_a_note_without_failing_the_run() {
    let (result, output) = only_result(ADVISORY, &[]);
    output.assert().success();

    assert_eq!(result["level"], "note");
}

#[test]
fn violation_on_an_unnamed_block_carries_no_fingerprint() {
    let (result, output) = only_result(UNNAMED, &[]);
    output.assert().failure().code(1);

    // An unnamed block has no address, so there is nothing to fingerprint the finding by.
    assert_eq!(result["ruleId"], "keep-sorted");
    assert!(
        result.get("partialFingerprints").is_none(),
        "unexpected fingerprint: {result}"
    );
}

#[test]
fn json_diagnostics_are_what_a_run_writes_without_the_flag() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg(SORTED);
    let output = cmd.output().unwrap();

    let diagnostics: Value = serde_json::from_slice(&output.stderr).expect("the diagnostics");
    output.assert().failure().code(1);

    assert_eq!(diagnostics[SORTED][0]["code"], "keep-sorted");
}
