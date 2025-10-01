use assert_cmd::Command;
use assert_cmd::assert::OutputAssertExt;
use predicates::prelude::predicate;
use serde_json::json;

fn get_cmd() -> Command {
    Command::cargo_bin(assert_cmd::crate_name!()).expect("Failed to find binary")
}

#[test]
fn with_all_lines_unique_succeeds() {
    let diff_content = r#"
diff --git a/tests/keep_unique_test.py b/tests/keep_unique_test.py
index d69398d..c5cbb7f 100644
--- a/tests/keep_unique_test.py
+++ b/tests/keep_unique_test.py
@@ -2,7 +2,7 @@ fruits = [
     # <block keep-unique=>
     'apple',
     'banana',
-    'orange',
+    'cherry',
     # </block>
 ]"#;

    let mut cmd = get_cmd();
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert().success();
}

#[test]
fn with_non_unique_lines_fails() {
    let diff_content = r#"
diff --git a/tests/keep_unique_test.py b/tests/keep_unique_test.py
index d69398d..9b29f11 100644
--- a/tests/keep_unique_test.py
+++ b/tests/keep_unique_test.py
@@ -10,6 +10,6 @@ unique_prefixes = [
     # <block keep-unique="ID:(?P<value>\d+)">
     'ID:1 A',
     'ID:2 B',
-    'ID:3 C',
+    'ID:1 C',
     # </block>
 ]"#;

    let mut cmd = get_cmd();
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert()
        .failure()
        .code(1)
        .stderr(predicate::function(|output: &str| {
            let output_json: serde_json::Value = serde_json::from_str(output).unwrap();
            let value: serde_json::Value  = json!({
              "tests/keep_unique_test.py": [
                {
                  "violation": "keep-unique",
                  "error": "Block tests/keep_unique_test.py:(unnamed) defined at line 10 has a duplicated line 13",
                  "details": {
                    "line_number_duplicated": 13,
                  }
                }
              ]
            });
            assert_eq!(output_json, value);
            true
        }));
}
