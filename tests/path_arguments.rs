//! Path and glob arguments: which files a run checks for each one, and when one fails the run.

use assert_cmd::assert::OutputAssertExt;
use common::cargo_bin_cmd;
use predicates::prelude::predicate;
use serde_json::Value;
use std::path::{Path, PathBuf};

mod common;

/// A Python file whose `keep-sorted` block is out of order, so a run that checks it reports it.
const UNSORTED_PYTHON: &str = "# <block keep-sorted>\nb\na\n# </block>\n";

/// A diff that changes `src/x.py`, written against [`UNSORTED_PYTHON`].
const DIFF_CHANGING_SRC_X: &str = r#"
diff --git a/src/x.py b/src/x.py
index 0000000..1111111 100644
--- a/src/x.py
+++ b/src/x.py
@@ -1,4 +1,4 @@
 # <block keep-sorted>
-a
+b
 a
 # </block>
"#;

/// Creates a repository in a temporary directory. Returns the directory, which deletes the
/// repository when dropped, and the path of the repository root.
///
/// Every Python file in it has an unsorted block, so the files a run reports are the files it
/// checked:
///
/// - `src/x.py`, `src/i.py` and `vendor/lib.py`.
/// - `src/[id].py`. Read as a glob, its name matches `src/i.py` instead.
/// - `build/gen.py`, which `.gitignore` leaves out.
///
/// `notes.txt` has an extension that blockwatch does not support.
fn repository() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    // An empty `.git` directory is enough for the walk to read `.gitignore`.
    std::fs::create_dir_all(root.join(".git")).unwrap();
    for file in [
        "src/x.py",
        "src/i.py",
        "src/[id].py",
        "vendor/lib.py",
        "build/gen.py",
    ] {
        write(&root, file, UNSORTED_PYTHON);
    }
    write(&root, ".gitignore", "build/\n");
    write(&root, "notes.txt", "notes\n");
    (temp, root)
}

fn write(root: &Path, file: &str, contents: &str) {
    let path = root.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// The files that have a violation in the JSON diagnostics written to `stderr`, sorted.
fn files_with_violations(stderr: &[u8]) -> Vec<String> {
    // The diagnostics are written only when there is a violation.
    if stderr.is_empty() {
        return Vec::new();
    }
    let diagnostics: Value = serde_json::from_slice(stderr).unwrap_or_else(|error| {
        panic!(
            "stderr should hold JSON diagnostics ({error}): {}",
            String::from_utf8_lossy(stderr)
        )
    });
    let mut files: Vec<String> = diagnostics
        .as_object()
        .expect("the diagnostics should be a JSON object")
        .keys()
        .cloned()
        .collect();
    files.sort();
    files
}

#[test]
fn argument_that_selects_no_checked_file_fails_the_run() {
    let (_temp, root) = repository();
    // The last argument of each case is the one that selects no file the run checks.
    for args in [
        &["scr/**/*.py"][..],                        // a typo
        &["notes.txt"],                              // an unsupported extension
        &["build/gen.py"],                           // left out by .gitignore
        &["--ignore", "vendor/**", "vendor/lib.py"], // left out by --ignore
    ] {
        let argument = args.last().unwrap();
        let mut cmd = cargo_bin_cmd!();
        cmd.current_dir(&root).args(args);

        cmd.output()
            .unwrap()
            .assert()
            .failure()
            .stderr(predicate::str::contains(format!(
                "no file to check matches \"{argument}\""
            )));
    }
}

#[test]
fn arguments_that_select_no_checked_file_are_listed_in_one_error() {
    let (_temp, root) = repository();
    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root)
        .args(["scr/**/*.py", "src/x.py", "notes.txt"]);

    cmd.output()
        .unwrap()
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no file to check matches \"scr/**/*.py\" or \"notes.txt\"",
        ));
}

#[test]
fn path_argument_checks_the_files_it_points_at() {
    let (_temp, root) = repository();
    // On macOS the temporary directory is reached through the `/var` symlink, so this path is
    // not the one the repository root resolves to.
    let absolute = root.join("src/x.py");
    let every_file_in_src = ["src/[id].py", "src/i.py", "src/x.py"];
    for (argument, expected) in [
        ("./src/x.py", &["src/x.py"][..]),
        (absolute.to_str().unwrap(), &["src/x.py"]),
        ("src/[id].py", &["src/[id].py"]),
        ("src", &every_file_in_src),
        ("src/", &every_file_in_src),
        (
            ".",
            &["src/[id].py", "src/i.py", "src/x.py", "vendor/lib.py"],
        ),
    ] {
        let mut cmd = cargo_bin_cmd!();
        cmd.current_dir(&root).arg(argument);
        let output = cmd.output().unwrap();

        assert_eq!(
            files_with_violations(&output.stderr),
            expected,
            "files checked for the argument {argument}"
        );
    }
}

#[test]
fn argument_from_a_subdirectory_starts_from_the_repository_root() {
    let (_temp, root) = repository();

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(root.join("src")).arg("src/x.py");
    let output = cmd.output().unwrap();
    assert_eq!(files_with_violations(&output.stderr), ["src/x.py"]);

    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(root.join("src")).arg("x.py");
    cmd.output()
        .unwrap()
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no file to check matches \"x.py\"",
        ))
        .stderr(predicate::str::contains(
            "start from the repository root, not from the current directory",
        ));
}

#[test]
fn only_changed_with_an_argument_that_matches_no_file_fails_the_run() {
    let (_temp, root) = repository();
    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root)
        .args(["--diff", "--only-changed", "scr/**/*.py"])
        .write_stdin(DIFF_CHANGING_SRC_X);

    cmd.output()
        .unwrap()
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no file to check matches \"scr/**/*.py\"",
        ));
}

#[test]
fn list_with_a_path_argument_lists_the_blocks_of_its_file() {
    let (_temp, root) = repository();
    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root).args(["list", "./src/x.py"]);
    let output = cmd.output().unwrap();

    output.clone().assert().success();
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let files: Vec<&String> = report.as_object().unwrap().keys().collect();
    assert_eq!(files, ["src/x.py"]);
}

#[test]
fn list_with_an_argument_that_selects_no_checked_file_fails() {
    let (_temp, root) = repository();
    let mut cmd = cargo_bin_cmd!();
    cmd.current_dir(&root).args(["list", "scr/**/*.py"]);

    cmd.output()
        .unwrap()
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no file to check matches \"scr/**/*.py\"",
        ));
}
