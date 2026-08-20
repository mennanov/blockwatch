//! Repository root discovery: which directory a run treats as "the repository".

use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo_bin_cmd;
use predicates::prelude::predicate;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// A Markdown file holding one named block, ready to be found by `blockwatch list`.
fn block_file(name: &str) -> String {
    format!("# doc\n\n<!-- <block name=\"{name}\"> -->\nalpha\n<!-- </block> -->\n")
}

/// A Markdown file whose block is a `keep-sorted` violation.
fn unsorted_file() -> String {
    "# doc\n\n<!-- <block keep-sorted=\"asc\"> -->\nbanana\napple\n<!-- </block> -->\n".to_string()
}

/// Creates `<parent>/<dir>` marked as a repository the way an ordinary clone is: `marker` (`.git`
/// or `.hg`) is a directory. Returns the absolute path of the working tree.
fn repo_with_marker_dir(parent: &Path, dir: &str, marker: &str) -> PathBuf {
    let root = parent.join(dir);
    std::fs::create_dir_all(root.join(marker)).unwrap();
    root
}

/// Creates `<parent>/<dir>` marked the way Git marks a linked worktree, a submodule, or a checkout
/// made with `--separate-git-dir`: `.git` is a file holding a `gitdir:` pointer.
fn repo_with_git_file(parent: &Path, dir: &str) -> PathBuf {
    let root = parent.join(dir);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".git"), "gitdir: /somewhere/else\n").unwrap();
    root
}

/// Runs `blockwatch list` in `dir` and returns the parsed JSON report.
fn list_in(dir: &Path) -> Value {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("list").current_dir(dir);
    let output = cmd.output().expect("Failed to get command output");

    output.clone().assert().success();
    serde_json::from_slice(&output.stdout).expect("Failed to parse JSON output")
}

/// The file names a report covers, sorted so assertions do not depend on map order.
fn reported_files(report: &Value) -> Vec<String> {
    let mut files: Vec<String> = report
        .as_object()
        .expect("report should be a JSON object")
        .keys()
        .cloned()
        .collect();
    files.sort();
    files
}

#[test]
fn a_git_file_marks_a_repository_root() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = repo_with_git_file(temp.path(), "worktree");
    std::fs::write(root.join("doc.md"), block_file("own"))?;

    assert_eq!(reported_files(&list_in(&root)), ["doc.md"]);
    Ok(())
}

#[test]
fn an_hg_directory_marks_a_repository_root() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = repo_with_marker_dir(temp.path(), "repo", ".hg");
    std::fs::write(root.join("doc.md"), block_file("own"))?;

    assert_eq!(reported_files(&list_in(&root)), ["doc.md"]);
    Ok(())
}

#[test]
fn a_run_below_the_root_still_covers_the_whole_repository() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = repo_with_git_file(temp.path(), "worktree");
    let nested = root.join("src");
    std::fs::create_dir_all(&nested)?;
    std::fs::write(root.join("doc.md"), block_file("top"))?;
    std::fs::write(nested.join("nested.md"), block_file("nested"))?;

    // Paths stay relative to the repository root, not to the directory the run started in.
    assert_eq!(
        reported_files(&list_in(&nested)),
        ["doc.md", "src/nested.md"]
    );
    Ok(())
}

#[test]
fn a_run_inside_a_nested_repository_reports_only_its_own_files() -> anyhow::Result<()> {
    // The shape of a submodule: a repository whose marker sits inside another repository.
    let temp = tempfile::tempdir()?;
    let outer = repo_with_marker_dir(temp.path(), "parent", ".git");
    std::fs::write(outer.join("outer.md"), block_file("outer"))?;
    let inner = repo_with_git_file(&outer, "sub");
    std::fs::write(inner.join("doc.md"), block_file("inner"))?;

    // `doc.md`, not `sub/doc.md`: the inner repository is the root, so the parent one contributes
    // neither files nor a path prefix.
    assert_eq!(reported_files(&list_in(&inner)), ["doc.md"]);
    Ok(())
}

#[test]
fn a_violation_in_the_parent_repository_does_not_fail_a_run_inside_a_nested_one()
-> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let outer = repo_with_marker_dir(temp.path(), "parent", ".git");
    std::fs::write(outer.join("outer.md"), unsorted_file())?;
    let inner = repo_with_git_file(&outer, "sub");
    std::fs::write(inner.join("doc.md"), block_file("inner"))?;

    cargo_bin_cmd!().current_dir(&inner).assert().success();

    cargo_bin_cmd!()
        .current_dir(&outer)
        .assert()
        .failure()
        .stderr(predicate::str::contains("out-of-order"));
    Ok(())
}

#[test]
fn a_directory_outside_any_repository_fails() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let dir = temp.path().join("not-a-repo");
    std::fs::create_dir_all(&dir)?;

    cargo_bin_cmd!()
        .arg("list")
        .current_dir(&dir)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Could not find the repository root",
        ));
    Ok(())
}
