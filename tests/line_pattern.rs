use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo_bin_cmd;
use predicates::prelude::predicate;
use serde_json::json;

#[test]
fn with_all_lines_matching_pattern_succeeds() {
    let diff_content = r#"
diff --git a/tests/testdata/line_pattern.py b/tests/testdata/line_pattern.py
index ca94c7e..cd73191 100644
--- a/tests/testdata/line_pattern.py
+++ b/tests/testdata/line_pattern.py
@@ -2,7 +2,7 @@ colors = [
     # <block line-pattern="0x[A-F0-9]{6,6}"> Empty lines are ignored.
     '0xFF0000',

-    '0x0000FF',
+    '0x0000FA',
     '0xAABB9F',
     # </block>
 ]"#;

    let mut cmd = cargo_bin_cmd!();
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert().success();
}

#[test]
fn with_some_lines_not_matching_pattern_fails() {
    let diff_content = r#"
diff --git a/tests/testdata/line_pattern.py b/tests/testdata/line_pattern.py
index ca94c7e..8a99694 100644
--- a/tests/testdata/line_pattern.py
+++ b/tests/testdata/line_pattern.py
@@ -10,7 +10,7 @@ colors = [
 uppercase_words = [
     # <block line-pattern="'[A-Z]+'">
     'APPLE',
-    'FAIL',
+    'FaIL',
     'ZERO'
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
              "tests/testdata/line_pattern.py": [
                {
                  "range": {
                    "start": {
                        "line": 13,
                        "character": 5
                    },
                    "end": {
                        "line": 13,
                        "character": 11
                    }
                  },
                  "code": "line-pattern",
                  "message": "Block tests/testdata/line_pattern.py:(unnamed) defined at line 11 has a non-matching line 13 (pattern: /'[A-Z]+'/)",
                  "severity": 1,
                  "data": {
                    "pattern": "'[A-Z]+'"
                  }
                }
              ]
            });
            assert_eq!(output_json, value);
            true
        }));
}
