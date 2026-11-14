//! Tests that the config file is found and that its settings are used.

// These tests check how a run reads its config file, so they can't use `common::cargo_bin_cmd!`,
// which replaces that file with an empty one.
#![allow(clippy::disallowed_macros)]

use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo_bin_cmd;
use predicates::prelude::{PredicateBooleanExt, predicate};
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
        "Block package.json:(unnamed) at line 1 of \"blockwatch.toml\" has a \
         non-matching line 2 (pattern: /^\\d+\\.\\d+\\.\\d+$/)"
    );
    Ok(())
}

#[test]
fn list_attribute_checks_the_block_against_each_reference() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    std::fs::create_dir_all(root.join(".git"))?;
    std::fs::write(root.join("colors.txt"), "red\nblue\n")?;
    std::fs::write(root.join("red.txt"), "red\n")?;
    std::fs::write(root.join("blue,green.txt"), "blue\n")?;
    std::fs::write(
        root.join("blockwatch.toml"),
        r"[[block]]
target = 'colors.txt'
same-as = ['red.txt', 'blue,green.txt']
",
    )?;

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root);
    let output = cmd.output()?;

    output.clone().assert().failure();
    let violations: serde_json::Value = serde_json::from_slice(&output.stderr)?;
    let mut targets: Vec<&str> = violations["colors.txt"]
        .as_array()
        .expect("violations of colors.txt")
        .iter()
        .map(|violation| violation["data"]["target_file"].as_str().expect("a file"))
        .collect();
    // The violations are not in the order of the references.
    targets.sort_unstable();
    assert_eq!(targets, ["blue,green.txt", "red.txt"]);
    Ok(())
}

#[test]
fn pattern_block_in_a_file_without_a_grammar_reports_its_violation_there() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    std::fs::create_dir_all(root.join(".git"))?;
    std::fs::write(
        root.join("prompt.txt"),
        "Answer briefly.
Categories: billing, Refunds
",
    )?;
    std::fs::write(
        root.join("blockwatch.toml"),
        r"[[block]]
target = 'prompt.txt'
target-pattern = '^Categories: (?P<value>.+)$'
line-pattern = '^[a-z, ]+$'
",
    )?;

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root);
    let output = cmd.output()?;

    output.clone().assert().failure();
    let violations: serde_json::Value = serde_json::from_slice(&output.stderr)?;
    let violation = &violations["prompt.txt"][0];
    assert_eq!(violation["code"], "line-pattern");
    // The value group starts after `Categories: `, at column 13 of line 2.
    assert_eq!(
        violation["range"]["start"],
        serde_json::json!({"line": 2, "character": 13})
    );
    Ok(())
}

/// The config file of the `glob_target` fixture. Its block wraps the version of each plugin, which
/// must be the root's. `plugins/red` keeps the rule, and `plugins/blue` and `plugins/green` break
/// it.
const GLOB_TARGET_CONFIG: &str = "tests/testdata/config/glob_target/config.toml";

#[test]
fn glob_target_violation_is_reported_in_each_matching_file_that_breaks_the_rule()
-> anyhow::Result<()> {
    let mut cmd = cargo_bin_cmd!();
    cmd.args([
        "--config",
        GLOB_TARGET_CONFIG,
        "tests/testdata/config/glob_target",
    ]);
    let output = cmd.output()?;

    output.clone().assert().failure();
    let violations: serde_json::Value = serde_json::from_slice(&output.stderr)?;
    let files: Vec<&String> = violations
        .as_object()
        .expect("violations by file")
        .keys()
        .collect();
    assert_eq!(
        files,
        [
            "tests/testdata/config/glob_target/plugins/blue/package.json",
            "tests/testdata/config/glob_target/plugins/green/package.json"
        ]
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
fn diff_adding_a_file_that_a_glob_target_matches_checks_it_under_only_changed() -> anyhow::Result<()>
{
    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--config", GLOB_TARGET_CONFIG, "--diff", "--only-changed"]);
    cmd.write_stdin(
        r#"diff --git a/tests/testdata/config/glob_target/plugins/green/package.json b/tests/testdata/config/glob_target/plugins/green/package.json
new file mode 100644
--- /dev/null
+++ b/tests/testdata/config/glob_target/plugins/green/package.json
@@ -0,0 +1,3 @@
+{
+  "version": "1.4.0"
+}
"#,
    );
    let output = cmd.output()?;

    output.clone().assert().failure();
    // `plugins/blue` breaks the rule too, but the diff doesn't touch it.
    let violations: serde_json::Value = serde_json::from_slice(&output.stderr)?;
    let files: Vec<&String> = violations
        .as_object()
        .expect("violations by file")
        .keys()
        .collect();
    assert_eq!(
        files,
        ["tests/testdata/config/glob_target/plugins/green/package.json"]
    );
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
fn only_changed_with_an_argument_for_an_unchanged_virtual_block_file_passes() -> anyhow::Result<()>
{
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    std::fs::create_dir_all(root.join(".git"))?;
    std::fs::write(root.join("a.py"), "x = 1\n")?;
    // blockwatch has no grammar for `.txt`, so only its virtual block makes it a file to check.
    std::fs::write(root.join("prompt.txt"), "Answer briefly.\n")?;
    std::fs::write(
        root.join("blockwatch.toml"),
        "[[block]]
target = 'prompt.txt'
",
    )?;

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root);
    cmd.args(["--diff", "--only-changed", "prompt.txt"]);
    cmd.write_stdin(
        "diff --git a/a.py b/a.py
--- a/a.py
+++ b/a.py
@@ -1 +1 @@
-x = 0
+x = 1
",
    );

    cmd.output()?.assert().success();
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

/// Creates a repository with `a.yml` (uncovered) and `b.yml` (has `flag: true` and `x`).
/// Returns the repository root.
fn repo_with_yaml_files(parent: &Path, b_x_yaml: &str) -> anyhow::Result<PathBuf> {
    let root = parent.join("repo");
    std::fs::create_dir_all(root.join(".git"))?;
    std::fs::write(root.join("a.yml"), "other: true\n")?;
    std::fs::write(root.join("b.yml"), format!("flag: true\nx:\n{b_x_yaml}"))?;
    Ok(root)
}

#[test]
fn glob_target_if_checks_only_covered_files_and_skips_uncovered_ones() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = repo_with_yaml_files(temp.path(), "  one: 1\n  two: 2\n")?;
    std::fs::write(root.join("empty.yml"), "")?;
    std::fs::write(
        root.join("blockwatch.toml"),
        r"[[block]]
target = '*.yml#/x'
line-count = '<=1'
target-if = { symbols = ['#/flag'] }
",
    )?;

    let output = cargo_bin_cmd!().current_dir(&root).output()?;
    output.clone().assert().failure();
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("b.yml"), "{stderr}");
    assert!(!stderr.contains("a.yml"), "{stderr}");
    assert!(!stderr.contains("empty.yml"), "{stderr}");

    // A covered file whose `target` symbol is missing still fails the run.
    std::fs::write(root.join("b.yml"), "flag: true\n")?;
    cargo_bin_cmd!()
        .current_dir(&root)
        .output()?
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("target b.yml#/x does not resolve")
                .and(predicate::str::contains("symbol not found")),
        );
    Ok(())
}

#[test]
fn list_omits_uncovered_files_and_referencing_an_uncovered_block_fails() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = repo_with_yaml_files(temp.path(), "  one: 1\n")?;
    std::fs::write(
        root.join("blockwatch.toml"),
        r"[[block]]
target = '*.yml#/x'
name = 'x'
target-if = { symbols = ['#/flag'] }
",
    )?;
    std::fs::write(
        root.join("README.md"),
        "<!-- <block same-as=\"a.yml:x\"> -->\ny\n<!-- </block> -->\n",
    )?;

    let list_output = cargo_bin_cmd!().current_dir(&root).arg("list").output()?;
    list_output.clone().assert().success();
    let listing: serde_json::Value = serde_json::from_slice(&list_output.stdout)?;
    assert!(listing.get("b.yml").is_some());
    assert!(listing.get("a.yml").is_none());

    cargo_bin_cmd!()
        .current_dir(&root)
        .args(["--only-block", "a.yml:x"])
        .output()?
        .assert()
        .failure()
        .stderr(predicate::str::contains(r#"no block matches "a.yml:x""#));

    cargo_bin_cmd!()
        .current_dir(&root)
        .arg("README.md")
        .output()?
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "disagrees with a.yml:x: target block not found",
        ));
    Ok(())
}

#[test]
fn full_runs_fail_when_no_file_matches_target_if() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    std::fs::create_dir_all(root.join(".git"))?;
    std::fs::write(root.join("a.yml"), "other: true\n")?;
    let config = r"[[block]]
target = '*.yml#/x'
target-if = { symbols = ['#/flag'] }
";
    std::fs::write(root.join("blockwatch.toml"), config)?;
    std::fs::write(root.join("other.toml"), config)?;

    let has_no_covered_error = || {
        predicate::str::contains("invalid block at line 1 of \"blockwatch.toml\"").and(
            predicate::str::contains("no file the glob matches passes target-if"),
        )
    };
    let file_diff = "\
diff --git a/a.yml b/a.yml
index 1111111..2222222 100644
--- a/a.yml
+++ b/a.yml
@@ -1 +1,2 @@
 other: true
+more: true
";
    let entry_diff = "\
diff --git a/blockwatch.toml b/blockwatch.toml
index 1111111..2222222 100644
--- a/blockwatch.toml
+++ b/blockwatch.toml
@@ -1,3 +1,3 @@
 [[block]]
 target = '*.yml#/x'
-target-if = { symbols = ['#/old'] }
+target-if = { symbols = ['#/flag'] }
";

    for args in [
        vec![],
        vec!["list"],
        vec!["--only-block", "a.yml:x"],
        vec!["--skip-block", "a.yml:x"],
        vec!["-E", "foo=yaml"],
        vec!["-d", "line-pattern"],
        vec!["-e", "line-count"],
    ] {
        cargo_bin_cmd!()
            .current_dir(&root)
            .args(&args)
            .output()?
            .assert()
            .failure()
            .stderr(has_no_covered_error());
    }

    cargo_bin_cmd!()
        .current_dir(&root)
        .args(["--config", "other.toml"])
        .output()?
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no file the glob matches passes target-if",
        ));

    cargo_bin_cmd!()
        .current_dir(&root)
        .arg("--diff")
        .write_stdin(file_diff)
        .output()?
        .assert()
        .failure()
        .stderr(has_no_covered_error());

    cargo_bin_cmd!()
        .current_dir(&root)
        .args(["--diff", "--only-changed"])
        .write_stdin(entry_diff)
        .output()?
        .assert()
        .failure()
        .stderr(has_no_covered_error());

    // Config `ignore` skipping the only covered file must still fail a full run.
    std::fs::write(root.join("b.yml"), "flag: true\nx: 1\n")?;
    std::fs::write(
        root.join("blockwatch.toml"),
        format!("ignore = ['b.yml']\n{config}"),
    )?;
    cargo_bin_cmd!()
        .current_dir(&root)
        .output()?
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no file the glob matches passes target-if",
        ));
    Ok(())
}

#[test]
fn full_runs_fail_when_one_of_two_target_if_entries_covers_no_file() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = repo_with_yaml_files(temp.path(), "  one: 1\n")?;
    let separate_entries = r"[[block]]
target = '*.yml#/x'
target-if = { symbols = ['#/flag'] }

[[block]]
target = '*.yml#/flag'
target-if = { symbols = ['#/missing'] }
";
    // Both entries start on line 1, so the line alone doesn't tell them apart.
    let entries_on_one_line = r"block = [
  { target = '*.yml#/x', target-if = { symbols = ['#/flag'] } }, { target = '*.yml#/flag', target-if = { symbols = ['#/missing'] } },
]
";

    for (config, entry_line) in [(separate_entries, 5), (entries_on_one_line, 2)] {
        std::fs::write(root.join("blockwatch.toml"), config)?;
        cargo_bin_cmd!()
            .current_dir(&root)
            .output()?
            .assert()
            .failure()
            .stderr(predicate::str::contains(format!(
                "Error: invalid block at line {entry_line} of \"blockwatch.toml\"

Caused by:
    no file the glob matches passes target-if"
            )));
    }
    Ok(())
}

/// The config file of the `target_if_symbol_change` fixture. Its block wraps `x` in each YAML
/// file that has `flag`, and `x` in `a.yml` breaks its `line-count`.
const TARGET_IF_SYMBOL_CHANGE_CONFIG: &str =
    "tests/testdata/config/target_if_symbol_change/config.toml";

#[test]
fn only_changed_runs_check_a_block_whose_target_if_symbol_the_diff_touches() -> anyhow::Result<()> {
    // The diff adds the `target-if` symbol, and leaves the target as it was.
    let target_if_symbol_diff = "\
diff --git a/tests/testdata/config/target_if_symbol_change/a.yml b/tests/testdata/config/target_if_symbol_change/a.yml
index 1111111..2222222 100644
--- a/tests/testdata/config/target_if_symbol_change/a.yml
+++ b/tests/testdata/config/target_if_symbol_change/a.yml
@@ -1,2 +1,3 @@
+flag: true
 x:
   one: 1
";

    cargo_bin_cmd!()
        .args([
            "--config",
            TARGET_IF_SYMBOL_CHANGE_CONFIG,
            "--diff",
            "--only-changed",
        ])
        .write_stdin(target_if_symbol_diff)
        .output()?
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            r#"Block tests/testdata/config/target_if_symbol_change/a.yml:(unnamed) at line 1 of \"tests/testdata/config/target_if_symbol_change/config.toml\" has 3 lines, which does not satisfy <=1"#,
        ));
    Ok(())
}

/// The config file of the `target_if_patterns` fixture. Its block wraps `DB_URL` in each
/// service's `.env` file that sets `DB_DRIVER=postgres`, and requires a `postgres://` URL.
/// `services/a` breaks the rule. `services/b` fails `target-if` and has no `DB_URL`, so without
/// `target-if` its `target-pattern` would match no text and stop the run.
const TARGET_IF_PATTERNS_CONFIG: &str = "tests/testdata/config/target_if_patterns/config.toml";

#[test]
fn glob_target_if_patterns_check_only_matching_files() -> anyhow::Result<()> {
    let output = cargo_bin_cmd!()
        .args([
            "--config",
            TARGET_IF_PATTERNS_CONFIG,
            "tests/testdata/config/target_if_patterns",
        ])
        .output()?;

    output.clone().assert().failure();
    let violations: serde_json::Value = serde_json::from_slice(&output.stderr)?;
    let files: Vec<&String> = violations
        .as_object()
        .expect("violations by file")
        .keys()
        .collect();
    assert_eq!(
        files,
        ["tests/testdata/config/target_if_patterns/services/a/.env"]
    );
    Ok(())
}

#[test]
fn scoped_runs_succeed_when_no_checked_file_matches_target_if() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    std::fs::create_dir_all(root.join(".git"))?;
    std::fs::write(root.join("a.yml"), "other: true\n")?;
    std::fs::write(
        root.join("README.md"),
        "<!-- <block name=\"doc\"> -->\nhello\n<!-- </block> -->\n",
    )?;
    std::fs::write(
        root.join("blockwatch.toml"),
        r"[[block]]
target = '*.yml#/x'
target-if = { symbols = ['#/flag'] }
",
    )?;

    // Path arguments skip the check even when no file in the repository passes `target-if`.
    for arg in ["README.md", "a.yml"] {
        cargo_bin_cmd!()
            .current_dir(&root)
            .arg(arg)
            .output()?
            .assert()
            .success();
    }

    cargo_bin_cmd!()
        .current_dir(&root)
        .args(["--ignore", "a.yml"])
        .output()?
        .assert()
        .success();

    cargo_bin_cmd!()
        .current_dir(&root)
        .args(["explain", "README.md:doc"])
        .output()?
        .assert()
        .success();

    let file_diff = "\
diff --git a/a.yml b/a.yml
index 1111111..2222222 100644
--- a/a.yml
+++ b/a.yml
@@ -1 +1,2 @@
 other: true
+more: true
";
    cargo_bin_cmd!()
        .current_dir(&root)
        .args(["--diff", "--only-changed"])
        .write_stdin(file_diff)
        .output()?
        .assert()
        .success();
    Ok(())
}
