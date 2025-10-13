use assert_cmd::Command;
use assert_cmd::assert::OutputAssertExt;
use predicates::prelude::predicate;
use serde_json::json;

fn get_cmd() -> Command {
    Command::cargo_bin(assert_cmd::crate_name!()).expect("Failed to find binary")
}

#[test]
fn with_all_lines_in_order_succeeds() {
    let diff_content = r#"
diff --git a/tests/keep_sorted_test.py b/tests/keep_sorted_test.py
index 83205ee..d0cce11 100644
--- a/tests/keep_sorted_test.py
+++ b/tests/keep_sorted_test.py
@@ -2,6 +2,7 @@ fruits = [
     # <block keep-sorted="asc">
     'apple',
     'banana',
+    'grape',
     'orange',
     # </block>
 ]"#;

    let mut cmd = get_cmd();
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert().success();
}

#[test]
fn with_some_lines_out_of_order_fails() {
    let diff_content = r#"
diff --git a/tests/keep_sorted_test.py b/tests/keep_sorted_test.py
index 366590e..82c1f16 100644
--- a/tests/keep_sorted_test.py
+++ b/tests/keep_sorted_test.py
@@ -9,7 +9,7 @@ fruits = [
 vegetables = [
     # <block keep-sorted="desc">
     'tomato',
-    'spinach',
+    'lettuce',
     'potato',
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
              "tests/keep_sorted_test.py": [
                {
                  "range": {
                    "start": {
                        "line": 13,
                        "character": 5
                    },
                    "end": {
                        "line": 13,
                        "character": 13
                    }
                  },
                  "code": "keep-sorted",
                  "message": "Block tests/keep_sorted_test.py:(unnamed) defined at line 10 has an out-of-order line 13 (desc)",
                  "severity": 1,
                  "data": {
                    "order_by": "desc",
                  }
                }
              ]
            });
            assert_eq!(output_json, value);
            true
        }));
}
