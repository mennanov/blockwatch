use assert_cmd::Command;
use assert_cmd::assert::OutputAssertExt;
use predicates::prelude::predicate;
use serde_json::json;

fn get_cmd() -> Command {
    Command::cargo_bin(assert_cmd::crate_name!()).expect("Failed to find binary")
}

#[test]
fn with_all_lines_matching_pattern_succeeds() {
    let diff_content = r#"
diff --git a/tests/line_pattern_test.py b/tests/line_pattern_test.py
index ca94c7e..cd73191 100644
--- a/tests/line_pattern_test.py
+++ b/tests/line_pattern_test.py
@@ -2,7 +2,7 @@ colors = [
     # <block line-pattern="0x[A-F0-9]{6,6}"> Empty lines are ignored.
     '0xFF0000',

-    '0x0000FF',
+    '0x0000FA',
     '0xAABB9F',
     # </block>
 ]"#;

    let mut cmd = get_cmd();
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert().success();
}

#[test]
fn with_some_lines_not_matching_pattern_fails() {
    let diff_content = r#"
diff --git a/test/line_pattern_test.py b/test/line_pattern_test.py
index ca94c7e..8a99694 100644
--- a/tests/line_pattern_test.py
+++ b/tests/line_pattern_test.py
@@ -10,7 +10,7 @@ colors = [
 uppercase_words = [
     # <block line-pattern="'[A-Z]+'">
     'APPLE',
-    'FAIL',
+    'FaIL',
     'ZERO'
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
              "tests/line_pattern_test.py": [
                {
                  "violation": "line-pattern",
                  "error": "Block tests/line_pattern_test.py:(unnamed) defined at line 11 has a non-matching line 13 (pattern: /'[A-Z]+'/)",
                  "details": {
                    "line_number_not_matching": 13,
                    "pattern": "'[A-Z]+'"
                  }
                }
              ]
            });
            assert_eq!(output_json, value);
            true
        }));
}
