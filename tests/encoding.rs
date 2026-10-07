//! Files that are not valid UTF-8.

use assert_cmd::assert::OutputAssertExt;
use common::cargo_bin_cmd;
use predicates::prelude::predicate;
use std::path::{Path, PathBuf};

mod common;

/// A Python file with a block that fails `keep-sorted`.
const UNSORTED_FILE: &str = "# <block keep-sorted=\"asc\">\nb\na\n# </block>\n";

/// The error for `legacy.py` when it holds a block.
const NOT_UTF8_ERROR: &str = "file \"legacy.py\" is not valid UTF-8";

/// Creates a repository in `parent` with two files. `legacy.py` holds `legacy`, and `unsorted.py`
/// holds a block that fails `keep-sorted`. Returns the repository root.
fn repo_with_legacy_file(parent: &Path, legacy: &[u8]) -> anyhow::Result<PathBuf> {
    let root = parent.join("repo");
    std::fs::create_dir_all(root.join(".git"))?;
    std::fs::write(root.join("legacy.py"), legacy)?;
    std::fs::write(root.join("unsorted.py"), UNSORTED_FILE)?;
    Ok(root)
}

#[test]
fn non_utf8_file_without_blocks_run_checks_the_other_files() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    // `é` in Latin-1 is the single byte 0xE9, which is not valid UTF-8.
    let root = repo_with_legacy_file(temp.path(), b"# caf\xe9\n")?;

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root);

    cmd.output()?
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("keep-sorted"));
    Ok(())
}

#[test]
fn non_utf8_file_with_a_block_run_fails_with_error() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = repo_with_legacy_file(
        temp.path(),
        b"# <block keep-sorted=\"asc\">\n# caf\xe9\n# </block>\n",
    )?;

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root);

    cmd.output()?
        .assert()
        .failure()
        .stderr(predicate::str::contains(NOT_UTF8_ERROR));
    Ok(())
}

#[test]
fn non_utf8_file_with_an_end_tag_alone_run_fails_with_error() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = repo_with_legacy_file(temp.path(), b"# caf\xe9\n# </block>\n")?;

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root);

    cmd.output()?
        .assert()
        .failure()
        .stderr(predicate::str::contains(NOT_UTF8_ERROR));
    Ok(())
}

#[test]
fn non_utf8_file_with_text_like_a_tag_run_checks_the_other_files() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = repo_with_legacy_file(temp.path(), b"# caf\xe9 <blockquote>\n")?;

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root);

    cmd.output()?
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("keep-sorted"));
    Ok(())
}

#[test]
fn diff_of_a_non_utf8_file_run_checks_the_blocks_it_changed() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = repo_with_legacy_file(temp.path(), b"# caf\xe9\n")?;
    let diff: &[u8] = b"diff --git a/legacy.py b/legacy.py
--- a/legacy.py
+++ b/legacy.py
@@ -1 +1 @@
-# cafe
+# caf\xe9
diff --git a/unsorted.py b/unsorted.py
--- a/unsorted.py
+++ b/unsorted.py
@@ -1,3 +1,4 @@
 # <block keep-sorted=\"asc\">
 b
+a
 # </block>
";

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root);
    cmd.args(["--diff", "--only-changed"]);
    cmd.write_stdin(diff);

    cmd.output()?
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("keep-sorted"));
    Ok(())
}
