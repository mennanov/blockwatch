use assert_cmd::Command;
use predicates::prelude::predicate;

fn get_cmd() -> Command {
    Command::cargo_bin(assert_cmd::crate_name!()).expect("Failed to find binary")
}

#[test]
fn diff_with_unsatisfied_blocks_fails() {
    let diff_content = r#"
diff --git a/tests/testing_data.md b/tests/testing_data
index abc123..def456 100644
--- a/tests/testing_data.md
+++ b/tests/testing_data.md
@@ -1,6 +1,5 @@
 # Testing data for integration tests
 
 [//]: # (<block affects=":foo">)
-First block.
 
 [//]: # (</block>)
"#;

    let mut cmd = get_cmd();
    cmd.write_stdin(diff_content);

    cmd.assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "Error: Blocks need to be updated:
tests/testing_data.md:foo referenced by tests/testing_data.md:(unnamed) at line 3",
        ));
}

#[test]
fn diff_with_satisfied_blocks_succeeds() {
    let diff_content = r#"
diff --git a/tests/testing_data.md b/tests/testing_data
index abc123..def456 100644
--- a/tests/testing_data.md
+++ b/tests/testing_data.md
@@ -1,11 +1,9 @@
 # Testing data for integration tests
 
 [//]: # (<block affects=":foo">)
-First block.
 
 [//]: # (</block>)
 
 [//]: # (<block name="foo">)
-Second block.
 
 [//]: # (</block>)
"#;

    let mut cmd = get_cmd();
    cmd.write_stdin(diff_content);

    cmd.assert().success();
}

#[test]
fn diff_with_satisfied_blocks_non_root_dir_succeeds() {
    let diff_content = r#"
diff --git a/tests/testing_data.md b/tests/testing_data
index abc123..def456 100644
--- a/tests/testing_data.md
+++ b/tests/testing_data.md
@@ -1,11 +1,9 @@
 # Testing data for integration tests
 
 [//]: # (<block affects=":foo">)
-First block.
 
 [//]: # (</block>)
 
 [//]: # (<block name="foo">)
-Second block.
 
 [//]: # (</block>)
"#;

    let mut cmd = get_cmd();
    cmd.current_dir("./tests");
    cmd.write_stdin(diff_content);

    cmd.assert().success();
}

#[test]
fn empty_diff_succeeds() {
    let mut cmd = get_cmd();
    cmd.write_stdin("");

    cmd.assert().success();
}
