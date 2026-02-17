use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo_bin_cmd;
use predicates::prelude::predicate;
use serde_json::json;

const LUA_STDLIB_ENV_VAR: &str = "BLOCKWATCH_LUA_MODE";

#[test]
fn with_valid_lua_script_succeeds() {
    let diff_content = r#"
diff --git a/tests/testdata/check_lua.py b/tests/testdata/check_lua.py
index 1111111..2222222 100644
--- a/tests/testdata/check_lua.py
+++ b/tests/testdata/check_lua.py
@@ -1,7 +1,7 @@
 colors = [
     # <block check-lua="tests/testdata/check_lua_success.lua">
     'red',
-    'green',
+    'yellow',
     'blue',
     # </block>
 ]
"#;

    let mut cmd = cargo_bin_cmd!();
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert().success();
}

#[test]
fn with_failing_lua_script_fails() {
    let diff_content = r#"
diff --git a/tests/testdata/check_lua.py b/tests/testdata/check_lua.py
index 1111111..2222222 100644
--- a/tests/testdata/check_lua.py
+++ b/tests/testdata/check_lua.py
@@ -9,7 +9,7 @@
 numbers = [
     # <block check-lua="tests/testdata/check_lua_fail.lua">
     '1',
-    '2',
+    '4',
     '3',
     # </block>
 ]
"#;

    let mut cmd = cargo_bin_cmd!();
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::function(|output: &str| {
            let output_json: serde_json::Value =
                serde_json::from_str(output).expect("invalid json");
            let value: serde_json::Value = json!({
              "tests/testdata/check_lua.py": [
                {
                  "range": {
                    "start": {
                        "line": 10,
                        "character": 7
                    },
                    "end": {
                        "line": 10,
                        "character": 59
                    }
                  },
                  "code": "check-lua",
                  "message": "Block tests/testdata/check_lua.py:(unnamed) defined at line 10 failed Lua check: block content is invalid",
                  "severity": 1,
                  "data": {
                    "script": "tests/testdata/check_lua_fail.lua",
                    "lua_error": "block content is invalid"
                  }
                }
              ]
            });
            assert_eq!(output_json, value);
            true
        }));
}

#[test]
fn lua_script_using_os_fails_in_sandboxed_mode() {
    let diff_content = r#"
diff --git a/tests/testdata/check_lua_mode.py b/tests/testdata/check_lua_mode.py
index 1111111..2222222 100644
--- a/tests/testdata/check_lua_mode.py
+++ b/tests/testdata/check_lua_mode.py
@@ -1,7 +1,7 @@
 times = [
     # <block check-lua="tests/testdata/check_lua_os.lua">
-    'morning',
+    'evening',
     'afternoon',
     # </block>
 ]
"#;

    let mut cmd = cargo_bin_cmd!();
    // Default (sandboxed) mode: os library is not available.
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("os library is not available"));
}

#[test]
fn lua_script_using_os_succeeds_in_safe_mode() {
    let diff_content = r#"
diff --git a/tests/testdata/check_lua_mode.py b/tests/testdata/check_lua_mode.py
index 1111111..2222222 100644
--- a/tests/testdata/check_lua_mode.py
+++ b/tests/testdata/check_lua_mode.py
@@ -1,7 +1,7 @@
 times = [
     # <block check-lua="tests/testdata/check_lua_os.lua">
-    'morning',
+    'evening',
     'afternoon',
     # </block>
 ]
"#;

    let mut cmd = cargo_bin_cmd!();
    cmd.env(LUA_STDLIB_ENV_VAR, "safe");
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert().success();
}

#[test]
fn lua_script_using_os_succeeds_in_unsafe_mode() {
    let diff_content = r#"
diff --git a/tests/testdata/check_lua_mode.py b/tests/testdata/check_lua_mode.py
index 1111111..2222222 100644
--- a/tests/testdata/check_lua_mode.py
+++ b/tests/testdata/check_lua_mode.py
@@ -1,7 +1,7 @@
 times = [
     # <block check-lua="tests/testdata/check_lua_os.lua">
-    'morning',
+    'evening',
     'afternoon',
     # </block>
 ]
"#;

    let mut cmd = cargo_bin_cmd!();
    cmd.env(LUA_STDLIB_ENV_VAR, "unsafe");
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert().success();
}
