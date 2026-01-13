use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo_bin_cmd;
use serde_json::{Value, json};

#[test]
fn list_subcommand_with_specific_file_returns_correct_json_from_that_file_only() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("list").arg("tests/testdata/list/a.py");

    let output = cmd.output().expect("Failed to get command output");

    output.clone().assert().success();

    let actual: Value =
        serde_json::from_slice(&output.stdout).expect("Failed to parse JSON output");

    let expected = json!({
        "tests/testdata/list/a.py": [
            {
                "name": "a",
                "line": 1,
                "column": 3,
                "is_content_modified": false,
                "attributes": {
                    "name": "a",
                    "attr": "val"
                }
            }
        ]
    });
    assert_eq!(actual, expected);
}

#[test]
fn list_subcommand_with_glob_returns_multiple_files() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("list").arg("tests/testdata/list/*.py");

    let output = cmd.output().expect("Failed to get command output");

    output.clone().assert().success();

    let actual: Value =
        serde_json::from_slice(&output.stdout).expect("Failed to parse JSON output");
    let report = actual.as_object().expect("Output should be a JSON object");

    assert_eq!(report.len(), 2);
    assert!(report.contains_key("tests/testdata/list/a.py"));
    assert!(report.contains_key("tests/testdata/list/b.py"));
}

#[test]
fn list_subcommand_with_ignore_excludes_files() {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("list")
        .arg("tests/testdata/list/*.py")
        .arg("--ignore")
        .arg("tests/testdata/list/b.py");

    let output = cmd.output().expect("Failed to get command output");

    output.clone().assert().success();

    let actual: Value =
        serde_json::from_slice(&output.stdout).expect("Failed to parse JSON output");
    let report = actual.as_object().expect("Output should be a JSON object");

    assert_eq!(report.len(), 1);
    assert!(report.contains_key("tests/testdata/list/a.py"));
    assert!(!report.contains_key("tests/testdata/list/b.py"));
}

#[test]
fn list_subcommand_with_no_args_checks_all_files() {
    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir("tests/testdata/list");
    cmd.arg("list");
    // BLOCKWATCH_TERMINAL_MODE is required to simulate a TTY input.
    cmd.env("BLOCKWATCH_TERMINAL_MODE", "true");

    let output = cmd.output().expect("Failed to get command output");

    output.clone().assert().success();

    let actual: Value =
        serde_json::from_slice(&output.stdout).expect("Failed to parse JSON output");
    let report = actual.as_object().expect("Output should be a JSON object");

    assert_eq!(report.len(), 2);
    assert!(report.contains_key("a.py"));
    assert!(report.contains_key("b.py"));
}

#[test]
fn list_subcommand_with_diff_input_returns_correct_is_content_modified_attribute() {
    let diff_content = r#"
diff --git a/tests/testdata/list/a.py b/tests/testdata/list/a.py
index 2fcfa70..eea9cf4 100644
--- a/tests/testdata/list/a.py
+++ b/tests/testdata/list/a.py
@@ -1,3 +1,3 @@
 # <block name="a" attr="val">
-pass old
+pass
 # </block>
"#;

    let mut cmd = cargo_bin_cmd!();
    cmd.arg("list").arg("tests/testdata/list/**");
    let output = cmd
        .write_stdin(diff_content)
        .output()
        .expect("Failed to get command output");

    output.clone().assert().success();

    let actual: Value =
        serde_json::from_slice(&output.stdout).expect("Failed to parse JSON output");
    let report = actual.as_object().expect("Output should be a JSON object");

    assert_eq!(report.len(), 2);
    assert!(
        report["tests/testdata/list/a.py"][0]["is_content_modified"]
            .as_bool()
            .unwrap()
    );
    assert!(
        !report["tests/testdata/list/b.py"][0]["is_content_modified"]
            .as_bool()
            .unwrap()
    );
}
