//! Tests that the config file is found and that its settings are used.

use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo_bin_cmd;
use predicates::prelude::predicate;
use std::path::{Path, PathBuf};

/// A Markdown file with a block that fails `keep-sorted`.
const UNSORTED_FILE: &str =
    "# doc\n\n<!-- <block keep-sorted=\"asc\"> -->\nbanana\napple\n<!-- </block> -->\n";

/// Creates a repository in `parent` with an empty `src/` directory and a failing block in
/// `generated/doc.md`. Returns the repository root.
fn repo_with_a_generated_violation(parent: &Path) -> anyhow::Result<PathBuf> {
    let root = parent.join("repo");
    std::fs::create_dir_all(root.join(".git"))?;
    std::fs::create_dir_all(root.join("generated"))?;
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(root.join("generated/doc.md"), UNSORTED_FILE)?;
    Ok(root)
}

#[test]
fn config_at_repository_root_applies_to_a_run_from_a_subdirectory() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = repo_with_a_generated_violation(temp.path())?;
    std::fs::write(root.join("blockwatch.toml"), "ignore = ['generated/**']\n")?;

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(root.join("src"));

    cmd.output()?.assert().success();
    Ok(())
}

#[test]
fn config_flag_reads_a_path_relative_to_the_working_directory() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = repo_with_a_generated_violation(temp.path())?;
    // The repository root also has a `settings.toml`, but it ignores nothing. If the run read that
    // file instead, it would report the violation.
    std::fs::write(
        root.join("src/settings.toml"),
        "ignore = ['generated/**']\n",
    )?;
    std::fs::write(root.join("settings.toml"), "")?;

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(root.join("src"));
    cmd.args(["--config", "settings.toml"]);

    cmd.output()?.assert().success();
    Ok(())
}

#[test]
fn virtual_block_violation_is_reported_in_the_file_it_wraps() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    std::fs::create_dir_all(root.join(".git"))?;
    std::fs::write(root.join("package.json"), "{\n  \"version\": \"1.2\"\n}\n")?;
    std::fs::write(
        root.join("blockwatch.toml"),
        r#"[[block]]
target = 'package.json#/version'
line-pattern = '^\d+\.\d+\.\d+$'
"#,
    )?;

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root);
    let output = cmd.output()?;

    output.clone().assert().failure();
    let violations: serde_json::Value = serde_json::from_slice(&output.stderr)?;
    let violation = &violations["package.json"][0];
    assert_eq!(violation["code"], "line-pattern");
    // The value starts after the key, at column 14 of line 2.
    assert_eq!(
        violation["range"]["start"],
        serde_json::json!({"line": 2, "character": 14})
    );
    // The rule is written in the config file, so the message shows the line there.
    assert_eq!(
        violation["message"],
        "Block package.json:(unnamed) defined at line 1 of \"blockwatch.toml\" has a \
         non-matching line 2 (pattern: /^\\d+\\.\\d+\\.\\d+$/)"
    );
    Ok(())
}

#[test]
fn config_with_an_unknown_key_fails_the_run_at_its_position() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = repo_with_a_generated_violation(temp.path())?;
    std::fs::write(root.join("blockwatch.toml"), "ignore = []\nignor = ['x']\n")?;

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root);

    cmd.output()?
        .assert()
        .failure()
        .stderr(predicate::str::contains("blockwatch.toml"))
        .stderr(predicate::str::contains("line 2, column 1"))
        .stderr(predicate::str::contains("unknown field `ignor`"));
    Ok(())
}
