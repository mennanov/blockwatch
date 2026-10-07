//! Tests that the config file is found and that its settings are used.

// These tests check how a run reads its config file, so they can't use `common::cargo_bin_cmd!`,
// which replaces that file with an empty one.
#![allow(clippy::disallowed_macros)]

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
fn virtual_block_in_a_non_utf8_file_fails_the_run() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    std::fs::create_dir_all(root.join(".git"))?;
    // `é` in Latin-1 is the single byte 0xE9, which is not valid UTF-8.
    std::fs::write(
        root.join("package.json"),
        b"{\n  \"name\": \"caf\xe9\"\n}\n",
    )?;
    std::fs::write(
        root.join("blockwatch.toml"),
        "[[block]]\ntarget = 'package.json#/name'\nline-pattern = '.'\n",
    )?;

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root);

    cmd.output()?
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "file \"package.json\" is not valid UTF-8",
        ));
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

/// Creates a repository in `parent` whose `package.json` breaks the rule of the block
/// `package.json:version`, which the config file `config_file` declares. Returns the repository
/// root.
fn repo_with_a_virtual_block_violation(
    parent: &Path,
    config_file: &str,
) -> anyhow::Result<PathBuf> {
    let root = parent.join("repo");
    std::fs::create_dir_all(root.join(".git"))?;
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(
        root.join("package.json"),
        r#"{
  "version": "1.2"
}
"#,
    )?;
    std::fs::write(
        root.join(config_file),
        r"[[block]]
target = 'package.json#/version'
line-pattern = '^\d+\.\d+\.\d+$'
name = 'version'
",
    )?;
    Ok(root)
}

/// A diff that adds the last line of the `[[block]]` entry in `config_file`.
fn diff_adding_the_rule(config_file: &str) -> String {
    format!(
        r"diff --git a/{config_file} b/{config_file}
--- a/{config_file}
+++ b/{config_file}
@@ -1,2 +1,3 @@
 [[block]]
 target = 'package.json#/version'
+line-pattern = '^\d+\.\d+\.\d+$'
"
    )
}

#[test]
fn diff_touching_a_config_entry_rechecks_its_block_under_only_changed() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = repo_with_a_virtual_block_violation(temp.path(), "blockwatch.toml")?;

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root);
    cmd.args(["--diff", "--only-changed"]);
    cmd.write_stdin(diff_adding_the_rule("blockwatch.toml"));

    cmd.output()?
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"package.json\""));
    Ok(())
}

#[test]
fn diff_touching_an_entry_of_the_config_flag_file_rechecks_its_block() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = repo_with_a_virtual_block_violation(temp.path(), "src/settings.toml")?;

    let mut cmd = cargo_bin_cmd!();
    // The flag's path starts at `src/`, and the diff's path at the repository root.
    cmd.current_dir(root.join("src"));
    cmd.args(["--config", "settings.toml", "--diff", "--only-changed"]);
    cmd.write_stdin(diff_adding_the_rule("src/settings.toml"));

    cmd.output()?
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"package.json\""));
    Ok(())
}

#[test]
fn config_flag_file_outside_the_repository_is_read_with_a_diff() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = repo_with_a_virtual_block_violation(temp.path(), "src/settings.toml")?;
    std::fs::rename(
        root.join("src/settings.toml"),
        temp.path().join("shared.toml"),
    )?;

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root);
    cmd.args(["--config", "../shared.toml", "--diff", "--only-changed"]);
    cmd.write_stdin(
        r#"diff --git a/package.json b/package.json
--- a/package.json
+++ b/package.json
@@ -2 +2 @@
-  "version": "1.1"
+  "version": "1.2"
"#,
    );

    cmd.output()?
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"package.json\""));
    Ok(())
}

#[test]
fn missing_config_flag_file_fails_the_run_with_its_path() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = repo_with_a_generated_violation(temp.path())?;

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root);
    cmd.args(["--config", "missing.toml"]);

    cmd.output()?
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "failed to read config file \"missing.toml\"",
        ));
    Ok(())
}

#[test]
fn block_referencing_a_virtual_block_reads_it_from_a_file_outside_the_globs() -> anyhow::Result<()>
{
    let temp = tempfile::tempdir()?;
    let root = repo_with_a_virtual_block_violation(temp.path(), "blockwatch.toml")?;
    std::fs::write(
        root.join("README.md"),
        r#"<!-- <block same-as="package.json:version"> -->
1.2
<!-- </block> -->
"#,
    )?;

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root);
    // The glob leaves `package.json` out. So the rule of its block is not checked, and `same-as`
    // reads the file itself.
    cmd.arg("*.md");

    cmd.output()?.assert().success();
    Ok(())
}

#[test]
fn list_shows_a_block_that_the_config_file_skips() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    std::fs::create_dir_all(root.join(".git"))?;
    std::fs::write(
        root.join("doc.md"),
        "<!-- <block name=\"fruits\"> -->\napple\n<!-- </block> -->\n",
    )?;
    std::fs::write(
        root.join("blockwatch.toml"),
        "skip-blocks = ['doc.md:fruits']\n",
    )?;

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root).arg("list");
    let output = cmd.output()?;

    output.clone().assert().success();
    let listing: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(listing["doc.md"][0]["name"], "fruits");
    Ok(())
}
