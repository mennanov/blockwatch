use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo_bin_cmd;
use predicates::prelude::predicate;
use serde_json::json;

#[test]
fn with_correct_number_of_lines_succeeds() {
    let diff_content = r#"
diff --git a/tests/line_count_test.py b/tests/line_count_test.py
index 6781fec..1a59757 100644
--- a/tests/line_count_test.py
+++ b/tests/line_count_test.py
@@ -2,7 +2,7 @@ colors = [
     # <block line-count="==4">
     'red',
     'green',
-    'yellow',
+    'black',
     'blue',
     # </block>
 ]"#;

    let mut cmd = cargo_bin_cmd!();
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert().success();
}

#[test]
fn with_incorrect_number_of_lines_fails() {
    let diff_content = r#"
diff --git a/tests/line_count_test.py b/tests/line_count_test.py
index 6781fec..4ce6a3b 100644
--- a/tests/line_count_test.py
+++ b/tests/line_count_test.py
@@ -11,6 +11,6 @@ fruits = [
     # <block line-count=">3">
     'apple',
     'banana',
-    'orange',
+    'grape',
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
              "tests/line_count_test.py": [
                {
                  "range": {
                    "start": {
                        "line": 12,
                        "character": 7
                    },
                    "end": {
                        "line": 12,
                        "character": 29
                    }
                  },
                  "code": "line-count",
                  "message": "Block tests/line_count_test.py:(unnamed) defined at line 12 has 3 lines, which does not satisfy >3",
                  "severity": 1,
                  "data": {
                    "actual": 3,
                    "expected": 3,
                    "op": ">"
                  }
                }
              ]
            });
            assert_eq!(output_json, value);
            true
        }));
}
