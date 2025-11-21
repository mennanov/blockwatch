use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo_bin_cmd;
use predicates::prelude::predicate;
use serde_json::json;

#[test]
fn with_all_lines_unique_succeeds() {
    let diff_content = r#"
diff --git a/tests/testdata/keep_unique.py b/tests/testdata/keep_unique.py
index d69398d..c5cbb7f 100644
--- a/tests/testdata/keep_unique.py
+++ b/tests/testdata/keep_unique.py
@@ -2,7 +2,7 @@ fruits = [
     # <block keep-unique=>
     'apple',
     'banana',
-    'orange',
+    'cherry',
     # </block>
 ]"#;

    let mut cmd = cargo_bin_cmd!();
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert().success();
}

#[test]
fn with_non_unique_lines_fails() {
    let diff_content = r#"
diff --git a/tests/testdata/keep_unique.py b/tests/testdata/keep_unique.py
index d69398d..9b29f11 100644
--- a/tests/testdata/keep_unique.py
+++ b/tests/testdata/keep_unique.py
@@ -10,6 +10,6 @@ unique_prefixes = [
     # <block keep-unique="ID:(?P<value>\d+)">
     'ID:1 A',
     'ID:2 B',
-    'ID:3 C',
+    'ID:1 C',
     # </block>
 ]"#;

    let mut cmd = cargo_bin_cmd!();
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert()
        .failure()
        .code(1)
        .stderr(predicate::function(|output: &str| {
            let output_json: serde_json::Value = serde_json::from_str(output).unwrap();
            let value: serde_json::Value  = json!({
              "tests/testdata/keep_unique.py": [
                {
                  "range": {
                    "start": {
                        "line": 13,
                        "character": 9
                    },
                    "end": {
                        "line": 13,
                        "character": 9
                    }
                  },
                  "code": "keep-unique",
                  "message": "Block tests/testdata/keep_unique.py:(unnamed) defined at line 10 has a duplicated line 13",
                  "severity": 1,
                }
              ]
            });
            assert_eq!(output_json, value);
            true
        }));
}
