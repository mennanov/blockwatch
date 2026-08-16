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
    cmd.args(["--diff", "--only-changed"]);
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
    cmd.args(["--diff", "--only-changed"]);
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
fn with_pattern_extracts_matching_content() {
    let diff_content = r#"
diff --git a/tests/testdata/check_lua_pattern.py b/tests/testdata/check_lua_pattern.py
index 1111111..2222222 100644
--- a/tests/testdata/check_lua_pattern.py
+++ b/tests/testdata/check_lua_pattern.py
@@ -1,5 +1,5 @@
 data = [
     # <block check-lua="tests/testdata/check_lua_echo.lua" check-lua-pattern="id: (?P<value>\d+)" expected="42">
-    name: Alice, id: 42
+    name: Bob, id: 42
     # </block>
 ]
"#;

    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"]);
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert().success();
}

#[test]
fn with_pattern_no_match_passes_empty_content() {
    let diff_content = r#"
diff --git a/tests/testdata/check_lua_pattern.py b/tests/testdata/check_lua_pattern.py
index 1111111..2222222 100644
--- a/tests/testdata/check_lua_pattern.py
+++ b/tests/testdata/check_lua_pattern.py
@@ -1,5 +1,5 @@
 data = [
     # <block check-lua="tests/testdata/check_lua_echo.lua" check-lua-pattern="zzz_no_match" name="">
-    name: Alice, id: 42
+    name: Bob, id: 42
     # </block>
 ]
"#;

    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"]);
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert().success();
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
    cmd.args(["--diff", "--only-changed"]);
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
    cmd.args(["--diff", "--only-changed"]);
    cmd.env(LUA_STDLIB_ENV_VAR, "safe");
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert().success();
}

#[test]
fn ctx_affects_exposes_in_sync_affected_blocks_succeeds() {
    // The diff touches both the check-lua block and the affected docs block so the `affects`
    // validator is satisfied and the check-lua script runs against an in-sync pair.
    let diff_content = r#"
diff --git a/tests/testdata/check_lua_affects.py b/tests/testdata/check_lua_affects.py
index 1111111..2222222 100644
--- a/tests/testdata/check_lua_affects.py
+++ b/tests/testdata/check_lua_affects.py
@@ -3,3 +3,3 @@
     'blue',
-    'yellow',
+    'green',
     'red',
@@ -12,3 +12,3 @@
     'blue',
-    'yellow',
+    'green',
     'red',
"#;

    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"]);
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert().success();
}

#[test]
fn ctx_affects_detects_out_of_sync_affected_block_fails() {
    // Both blocks are touched (so the `affects` validator passes), but their content has drifted
    // apart on disk, so the check-lua script reports the mismatch via ctx.affects.
    let diff_content = r#"
diff --git a/tests/testdata/check_lua_affects_drift.py b/tests/testdata/check_lua_affects_drift.py
index 1111111..2222222 100644
--- a/tests/testdata/check_lua_affects_drift.py
+++ b/tests/testdata/check_lua_affects_drift.py
@@ -3,3 +3,3 @@
     'blue',
-    'yellow',
+    'green',
     'red',
@@ -12,3 +12,3 @@
     'blue',
-    'orange',
+    'purple',
     'red',
"#;

    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--diff", "--only-changed"]);
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "block 'allowed-colors-docs' in tests/testdata/check_lua_affects_drift.py is out of sync",
        ));
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
    cmd.args(["--diff", "--only-changed"]);
    cmd.env(LUA_STDLIB_ENV_VAR, "unsafe");
    let output = cmd.write_stdin(diff_content).output().unwrap();

    output.assert().success();
}

/// A Lua checker sees the same repository-relative, forward-slash path however the run was scoped.
/// Without this, a checker that branches on paths changes verdict between a glob scan (which uses
/// native separators) and a diff scan (which uses the diff's forward slashes) on Windows.
#[test]
fn ctx_file_is_identical_in_glob_and_diff_modes() {
    let expected = "ctx.file=tests/testdata/pathfmt/source.py";

    let mut glob_command = cargo_bin_cmd!();
    let glob_stderr = String::from_utf8(
        glob_command
            .arg("tests/testdata/pathfmt/source.py")
            .output()
            .unwrap()
            .stderr,
    )
    .unwrap();

    let diff = "\
diff --git a/tests/testdata/pathfmt/source.py b/tests/testdata/pathfmt/source.py
--- a/tests/testdata/pathfmt/source.py
+++ b/tests/testdata/pathfmt/source.py
@@ -1,3 +1,3 @@
 # <block check-lua=\"tests/testdata/pathfmt/report_path.lua\">
-value = 1
+value = 2
 # </block>
";
    let mut diff_command = cargo_bin_cmd!();
    diff_command.args(["--diff", "--only-changed"]);
    let diff_stderr =
        String::from_utf8(diff_command.write_stdin(diff).output().unwrap().stderr).unwrap();

    assert!(glob_stderr.contains(expected), "glob mode: {glob_stderr}");
    assert!(diff_stderr.contains(expected), "diff mode: {diff_stderr}");
}
