use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo_bin_cmd;
use predicates::prelude::predicate;
use serde_json::json;

#[test]
fn with_all_lines_in_order_succeeds() {
    let diff_content = r#"
diff --git a/tests/testdata/keep_sorted.py b/tests/testdata/keep_sorted.py
index 83205ee..d0cce11 100644
--- a/tests/testdata/keep_sorted.py
+++ b/tests/testdata/keep_sorted.py
@@ -2,6 +2,7 @@ fruits = [
     # <block keep-sorted="asc">
     'apple',
     'banana',
+    'grape',
     'orange',
     # </block>
 ]"#;

    let mut cmd = cargo_bin_cmd!();
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert().success();
}

#[test]
fn with_some_lines_out_of_order_fails() {
    let diff_content = r#"
diff --git a/tests/testdata/keep_sorted.py b/tests/testdata/keep_sorted.py
index 366590e..82c1f16 100644
--- a/tests/testdata/keep_sorted.py
+++ b/tests/testdata/keep_sorted.py
@@ -9,7 +9,7 @@ fruits = [
 vegetables = [
     # <block keep-sorted="desc">
     'tomato',
-    'spinach',
+    'lettuce',
     'potato',
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
              "tests/testdata/keep_sorted.py": [
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
                  "message": "Block tests/testdata/keep_sorted.py:(unnamed) defined at line 10 has an out-of-order line 13 (desc)",
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

#[test]
fn with_keep_sorted_pattern_in_order_succeeds() {
    let diff_content = r#"
diff --git a/tests/testdata/keep_sorted.py b/tests/testdata/keep_sorted.py
index 1111111..2222222 100644
--- a/tests/testdata/keep_sorted.py
+++ b/tests/testdata/keep_sorted.py
@@ -17,6 +17,7 @@ items = [
     # <block keep-sorted="asc" keep-sorted-pattern="id: (?P<value>\d+)">
     "id: 1 apple",
     "id: 3 cherry",
+    "id: 4 orange",
     # </block>
 ]"#;

    let mut cmd = cargo_bin_cmd!();
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert().success();
}

#[test]
fn with_keep_sorted_pattern_out_of_order_fails() {
    let diff_content = r#"
diff --git a/tests/testdata/keep_sorted.py b/tests/testdata/keep_sorted.py
index 1111111..2222222 100644
--- a/tests/testdata/keep_sorted.py
+++ b/tests/testdata/keep_sorted.py
@@ -25,6 +25,7 @@ items = [
     # <block keep-sorted="asc" keep-sorted-pattern="id: (?P<value>\d+)">
     "id: 1 apple",
     "id: 3 cherry",
+    "id: 10 orange",
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
              "tests/testdata/keep_sorted.py": [
                {
                  "range": {
                    "start": {
                        "line": 29,
                        "character": 10
                    },
                    "end": {
                        "line": 29,
                        "character": 11
                    }
                  },
                  "code": "keep-sorted",
                  "message": "Block tests/testdata/keep_sorted.py:(unnamed) defined at line 26 has an out-of-order line 29 (asc)",
                  "severity": 1,
                  "data": {
                    "order_by": "asc",
                  }
                }
              ]
            });
            assert_eq!(output_json, value);
            true
        }));
}

#[test]
fn with_empty_keep_sorted_value_defaults_to_asc() {
    let diff_content = r#"
diff --git a/tests/testdata/keep_sorted.py b/tests/testdata/keep_sorted.py
index 1111111..2222222 100644
--- a/tests/testdata/keep_sorted.py
+++ b/tests/testdata/keep_sorted.py
@@ -33,6 +33,7 @@ defaults_unsorted = [
     # <block keep-sorted>
     'b',
     'a',
+    'c',
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
              "tests/testdata/keep_sorted.py": [
                {
                  "range": {
                    "start": {
                        "line": 36,
                        "character": 5
                    },
                    "end": {
                        "line": 36,
                        "character": 8
                    }
                  },
                  "code": "keep-sorted",
                  "message": "Block tests/testdata/keep_sorted.py:(unnamed) defined at line 34 has an out-of-order line 36 (asc)",
                  "severity": 1,
                  "data": {
                    "order_by": "asc",
                  }
                }
              ]
            });
            assert_eq!(output_json, value);
            true
        }));
}
