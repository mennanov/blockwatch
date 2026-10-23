//! Tests of the `explain` subcommand that only the binary can show: the exit codes, what goes to
//! stdout and to stderr, and how the settings change the result.

use assert_cmd::assert::OutputAssertExt;
use common::cargo_bin_cmd;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

mod common;

/// `colors` in this file fails `keep-sorted`, and `fruits` passes. `colors-copy` has the same
/// content as `colors`, which `same-as` checks. `checked-fruits` passes `keep-sorted` and a
/// `check-lua` script.
const FIXTURE: &str = "tests/testdata/explain/a.py";

/// The text of [`FIXTURE`], for a repository that a test creates.
const FIXTURE_TEXT: &str = include_str!("testdata/explain/a.py");

/// Creates a repository in `parent` with [`FIXTURE_TEXT`] in `a.py`, and `config` in
/// `blockwatch.toml`. Returns the repository root.
fn repo_with_config(parent: &Path, config: &str) -> anyhow::Result<PathBuf> {
    let root = parent.join("repo");
    std::fs::create_dir_all(root.join(".git"))?;
    std::fs::write(root.join("a.py"), FIXTURE_TEXT)?;
    std::fs::write(root.join("blockwatch.toml"), config)?;
    Ok(root)
}

fn json_from(bytes: &[u8]) -> anyhow::Result<Value> {
    Ok(serde_json::from_slice(bytes)?)
}

#[test]
fn named_block_writes_its_explanation_to_stdout() -> anyhow::Result<()> {
    let output = cargo_bin_cmd!()
        .args(["explain", &format!("{FIXTURE}:fruits")])
        .output()?;

    output.clone().assert().success().stderr("");
    assert_eq!(
        json_from(&output.stdout)?,
        json!({
            "block": {
                "attributes": { "keep-sorted": "asc", "name": "fruits" },
                "column": 3,
                "is_content_modified": false,
                "line": 1,
                "name": "fruits"
            },
            "checks": {
                "keep-sorted": {
                    "format": "lexicographic",
                    "lines": [
                        { "line": 2, "text": "apple", "value": "apple" },
                        { "line": 3, "text": "banana", "value": "banana" }
                    ],
                    "order": "asc"
                }
            },
            "content": {
                "start": { "line": 1, "character": 42 },
                "end": { "line": 4, "character": 1 },
                "text": "\napple\nbanana\n"
            },
            "disabled": [],
            "file": FIXTURE,
            "with_diff": false
        })
    );
    Ok(())
}

#[test]
fn block_with_same_as_writes_its_trace_under_checks() -> anyhow::Result<()> {
    let output = cargo_bin_cmd!()
        .args(["explain", &format!("{FIXTURE}:colors-copy")])
        .output()?;

    output.clone().assert().success().stderr("");
    let records = |line| {
        json!([
            { "groups": { "value": "red" }, "line": line, "match": "red" },
            { "groups": { "value": "blue" }, "line": line + 1, "match": "blue" }
        ])
    };
    assert_eq!(
        json_from(&output.stdout)?["checks"],
        json!({
            "same-as": {
                "mode": "sequence",
                "records": records(10),
                "targets": [
                    {
                        "records": records(6),
                        "reason": null,
                        "target": format!("{FIXTURE}:colors")
                    }
                ]
            }
        })
    );
    Ok(())
}

#[test]
fn block_with_sync_and_async_validators_writes_both_traces() -> anyhow::Result<()> {
    let output = cargo_bin_cmd!()
        .args(["explain", &format!("{FIXTURE}:checked-fruits")])
        .output()?;

    output.clone().assert().success().stderr("");
    let checks = json_from(&output.stdout)?["checks"].clone();
    let validators: Vec<&String> = checks
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, _)| key)
        .collect();
    assert_eq!(validators, ["check-lua", "keep-sorted"]);
    Ok(())
}

#[test]
fn block_with_a_violation_exits_with_1_and_writes_the_violation_to_stderr() -> anyhow::Result<()> {
    let output = cargo_bin_cmd!()
        .args(["explain", &format!("{FIXTURE}:colors")])
        .output()?;

    output.clone().assert().code(1);
    assert_eq!(json_from(&output.stdout)?["block"]["name"], "colors");
    let violations = json_from(&output.stderr)?;
    assert_eq!(violations[FIXTURE][0]["code"], "keep-sorted");
    Ok(())
}

#[test]
fn named_block_gets_the_same_violations_as_only_block() -> anyhow::Result<()> {
    let address = format!("{FIXTURE}:colors");

    let explained = cargo_bin_cmd!().args(["explain", &address]).output()?;
    let validated = cargo_bin_cmd!()
        .args([FIXTURE, "--only-block", &address])
        .output()?;

    validated.clone().assert().code(1);
    explained.clone().assert().code(1);
    assert_eq!(json_from(&explained.stderr)?, json_from(&validated.stderr)?);
    Ok(())
}

#[test]
fn address_that_matches_no_block_exits_with_2() -> anyhow::Result<()> {
    let output = cargo_bin_cmd!()
        .args(["explain", &format!("{FIXTURE}:missing")])
        .output()?;

    output.clone().assert().code(2).stdout("");
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.contains(&format!("no block matches \"{FIXTURE}:missing\"")),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn with_diff_is_recorded_and_marks_the_block_as_changed() -> anyhow::Result<()> {
    let diff = format!(
        r#"diff --git a/{FIXTURE} b/{FIXTURE}
--- a/{FIXTURE}
+++ b/{FIXTURE}
@@ -1,3 +1,3 @@
 # <block name="fruits" keep-sorted="asc">
-apricot
+apple
 banana
"#
    );

    let output = cargo_bin_cmd!()
        .args(["explain", "--diff", &format!("{FIXTURE}:fruits")])
        .write_stdin(diff)
        .output()?;

    output.clone().assert().success();
    let explanation = json_from(&output.stdout)?;
    assert_eq!(explanation["with_diff"], true);
    assert_eq!(explanation["block"]["is_content_modified"], true);
    Ok(())
}

// The next tests read their own config file, so they can't use `common::cargo_bin_cmd!`, which
// replaces that file with an empty one.

#[test]
#[allow(clippy::disallowed_macros)]
fn validator_that_the_config_disables_is_listed_as_disabled_and_not_run() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    // `line-count` is off too, but the block doesn't ask for it, so it isn't listed.
    let root = repo_with_config(temp.path(), "disable = ['keep-sorted', 'line-count']\n")?;

    let output = assert_cmd::cargo_bin_cmd!()
        .current_dir(&root)
        .args(["explain", "a.py:colors"])
        .output()?;

    output.clone().assert().success().stderr("");
    assert_eq!(
        json_from(&output.stdout)?["disabled"],
        json!(["keep-sorted"])
    );
    Ok(())
}

#[test]
#[allow(clippy::disallowed_macros)]
fn block_that_the_config_skips_is_still_checked() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = repo_with_config(temp.path(), "skip-blocks = ['a.py:colors']\n")?;

    let output = assert_cmd::cargo_bin_cmd!()
        .current_dir(&root)
        .args(["explain", "a.py:colors"])
        .output()?;

    output.clone().assert().code(1);
    assert_eq!(json_from(&output.stdout)?["block"]["name"], "colors");
    Ok(())
}
