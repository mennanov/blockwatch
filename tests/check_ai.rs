use assert_cmd::assert::OutputAssertExt;
use assert_cmd::cargo_bin_cmd;
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use std::net::SocketAddr;
use tokio::net::TcpListener;

async fn start_fake_openai() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    async fn chat_completions(Json(payload): Json<Value>) -> Json<Value> {
        let mut user_content = String::new();
        if let Some(messages) = payload.get("messages").and_then(|m| m.as_array()) {
            for msg in messages {
                if msg.get("role").and_then(|r| r.as_str()) == Some("user")
                    && let Some(content) = msg.get("content").and_then(|c| c.as_str())
                {
                    user_content = content.to_string();
                    break;
                }
            }
        }
        // Extract only the BLOCK content portion from the user message to avoid matching the CONDITION text.
        let marker = "BLOCK (preserve formatting):\n";
        let block_only = if let Some(pos) = user_content.find(marker) {
            &user_content[pos + marker.len()..]
        } else {
            user_content.as_str()
        };
        let assistant_message = if block_only.to_lowercase().contains("banana") {
            "OK".to_string()
        } else {
            "The block does not mention 'banana'. Add it.".to_string()
        };

        let resp = json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "created": 1_700_000_000u64,
            "model": payload.get("model").and_then(|v| v.as_str()).unwrap_or("gpt-4o-mini"),
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": assistant_message
                    },
                    "finish_reason": "stop"
                }
            ]
        });
        Json(resp)
    }

    let app = Router::new().route("/v1/chat/completions", post(chat_completions));

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, handle)
}

#[tokio::test(flavor = "multi_thread")]
async fn check_ai_ok_succeeds() {
    let (addr, _handle) = start_fake_openai().await;

    // Configure client to use fake server for this command only
    let mut cmd = cargo_bin_cmd!();
    cmd.env("BLOCKWATCH_AI_API_URL", format!("http://{addr}/v1"));
    cmd.env("BLOCKWATCH_AI_API_KEY", "test-key");

    let diff_content = r#"
diff --git a/tests/check_ai_test.py b/tests/check_ai_test.py
index 54d1d99..a95a452 100644
--- a/tests/check_ai_test.py
+++ b/tests/check_ai_test.py
@@ -1,5 +1,5 @@
 # AI check integration

 # <block check-ai="must mention banana">
-s = "I like mangoes"
+s = "I like bananas"
 # </block>
"#;

    let output = cmd.write_stdin(diff_content).output().unwrap();
    output.assert().success();
}

#[tokio::test(flavor = "multi_thread")]
async fn check_ai_violation_fails_and_reports_message() {
    let (addr, _handle) = start_fake_openai().await;

    let mut cmd = cargo_bin_cmd!();
    cmd.env("BLOCKWATCH_AI_API_URL", format!("http://{addr}/v1"));
    cmd.env("BLOCKWATCH_AI_API_KEY", "test-key");

    let diff_content = r#"
diff --git a/tests/check_ai_test.py b/tests/check_ai_test.py
index 1111111..2222222 100644
--- a/tests/check_ai_test.py
+++ b/tests/check_ai_test.py
@@ -5,5 +5,5 @@
 # </block>

 # <block check-ai="must mention mango">
-another_text = "I like apple"
+another_text = "I like pear"
 # </block>
"#;

    let output = cmd.write_stdin(diff_content).output().unwrap();

    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::prelude::predicate::function(|output: &str| {
            let output_json: Value =
                serde_json::from_str(output).expect("invalid json");
            let value: Value = json!({
              "tests/check_ai_test.py": [
                {
                  "range": {
                    "start": {
                        "line": 7,
                        "character": 3
                    },
                    "end": {
                        "line": 7,
                        "character": 40
                    }
                  },
                  "code": "check-ai",
                  "message": "Block tests/check_ai_test.py:(unnamed) defined at line 7 failed AI check: The block does not mention 'banana'. Add it.",
                  "severity": 1,
                  "data": {
                    "condition": "must mention banana",
                    "ai_message": "The block does not mention 'banana'. Add it."
                  }
                }
              ]
            });
            assert_eq!(output_json, value);
            true
        }));
}

#[tokio::test(flavor = "multi_thread")]
async fn when_api_key_is_empty_error_is_printed() {
    let (addr, _handle) = start_fake_openai().await;

    // Configure client to use fake server for this command only
    let mut cmd = cargo_bin_cmd!();
    cmd.env("BLOCKWATCH_AI_API_URL", format!("http://{addr}/v1"));

    let diff_content = r#"
diff --git a/tests/check_ai_test.py b/tests/check_ai_test.py
index 54d1d99..a95a452 100644
--- a/tests/check_ai_test.py
+++ b/tests/check_ai_test.py
@@ -1,5 +1,5 @@
 # AI check integration

 # <block check-ai="must mention banana">
-s = "I like mangoes"
+s = "I like bananas"
 # </block>
"#;

    let output = cmd.write_stdin(diff_content).output().unwrap();
    output
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::prelude::predicate::function(|output: &str| {
            output.contains("API key is empty.")
        }));
}
