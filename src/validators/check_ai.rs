use crate::blocks::Block;
use crate::validators;
use crate::validators::{
    ValidatorAsync, ValidatorDetector, ValidatorType, Violation, ViolationRange,
};
use anyhow::{Context, anyhow};
use async_openai::Client;
use async_openai::config::{Config, OPENAI_API_BASE, OpenAIConfig};
use async_openai::types::{
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
    ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs,
};
use async_trait::async_trait;
use secrecy::ExposeSecret;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::task::JoinSet;

const DEFAULT_SYSTEM_PROMPT: &str = r"You are a strict validator. You are given a CONDITION and a BLOCK.
- If the BLOCK satisfies the CONDITION, reply with exactly: OK
- If the BLOCK violates the CONDITION, reply ONLY with a short, meaningful, and actionable error message describing what must be changed.
- Do not include quotes, labels, or extra text.";

pub(crate) struct CheckAiValidator<C: AiClient> {
    client: Arc<C>,
}

#[async_trait]
impl<C: AiClient + 'static> ValidatorAsync for CheckAiValidator<C> {
    async fn validate(
        &self,
        context: Arc<validators::ValidationContext>,
    ) -> anyhow::Result<HashMap<String, Vec<Violation>>> {
        let mut violations = HashMap::new();
        let mut tasks = JoinSet::new();
        for (file_path, file_blocks) in &context.modified_blocks {
            for block in &file_blocks.blocks {
                let condition = match block.attributes.get("check-ai") {
                    None => continue,
                    Some(v) => v,
                };
                let condition_trimmed = condition.trim();
                if condition_trimmed.is_empty() {
                    return Err(anyhow!(
                        "check-ai requires a non-empty condition in {}:{} at line {}",
                        file_path,
                        block.name_display(),
                        block.starts_at_line
                    ));
                }
                let block_content = if let Some(pattern) = block.attributes.get("check-ai-pattern")
                {
                    let re = regex::Regex::new(pattern)
                        .context("check-ai-pattern is not a valid regex")?;
                    if let Some(c) = re.captures(block.content(&file_blocks.file_contents)) {
                        // If named group "value" exists use it, otherwise use the whole match
                        if let Some(m) = c.name("value") {
                            m.as_str()
                        } else {
                            c.get(0).map_or("", |m| m.as_str())
                        }
                    } else {
                        ""
                    }
                } else {
                    block.content(&file_blocks.file_contents).trim()
                };
                // Skip blocks with empty content.
                if block_content.is_empty() {
                    continue;
                }

                let client = Arc::clone(&self.client);
                let condition = condition_trimmed.to_string();
                let block_content = block_content.to_string();
                let file_path = file_path.clone();
                let block = Arc::clone(block);
                tasks.spawn(async move {
                    let result = client.check_block(condition, block_content).await;
                    Self::process_ai_response(file_path, block, result)
                });
            }
        }
        while let Some(task_result) = tasks.join_next().await {
            match task_result.context("check-ai task failed")? {
                Ok(None) => continue,
                Ok(Some((file_path, violation))) => {
                    violations
                        .entry(file_path)
                        .or_insert_with(Vec::new)
                        .push(violation);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(violations)
    }
}

pub(crate) struct CheckAiValidatorDetector();

impl CheckAiValidatorDetector {
    pub fn new() -> Self {
        Self {}
    }
}

impl ValidatorDetector for CheckAiValidatorDetector {
    fn detect(&self, block: &Block) -> anyhow::Result<Option<ValidatorType>> {
        if block.attributes.contains_key("check-ai") {
            Ok(Some(ValidatorType::Async(Box::new(
                CheckAiValidator::with_client(OpenAiClient::new_from_env()),
            ))))
        } else {
            Ok(None)
        }
    }
}

fn create_violation(
    block_file_path: &str,
    block: Arc<Block>,
    ai_message: &str,
) -> anyhow::Result<Violation> {
    let details = serde_json::to_value(CheckAiViolation {
        condition: block
            .attributes
            .get("check-ai")
            .expect("check-ai attribute must be present")
            .trim(),
        ai_message: Some(ai_message),
    })
    .context("failed to serialize CheckAiDetails")?;
    let error_message = format!(
        "Block {}:{} defined at line {} failed AI check: {ai_message}",
        block_file_path,
        block.name_display(),
        block.starts_at_line,
    );
    Ok(Violation::new(
        // TODO: The block's start & end character position is not known. See issue #46.
        ViolationRange::new(block.starts_at_line, 0, block.ends_at_line, 0),
        "check-ai".to_string(),
        error_message,
        block,
        Some(details),
    ))
}

impl<C: AiClient> CheckAiValidator<C> {
    pub(super) fn with_client(client: C) -> Self {
        Self {
            client: Arc::new(client),
        }
    }

    fn process_ai_response(
        file_path: String,
        block: Arc<Block>,
        result: anyhow::Result<Option<String>>,
    ) -> anyhow::Result<Option<(String, Violation)>> {
        match result.context(format!(
            "check-ai API error in {}:{} at line {}",
            file_path,
            block.name_display(),
            block.starts_at_line
        ))? {
            None => Ok(None),
            Some(msg) => {
                let violation = create_violation(&file_path, block, &msg)?;
                Ok(Some((file_path, violation)))
            }
        }
    }
}

#[derive(Serialize)]
struct CheckAiViolation<'a> {
    condition: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    ai_message: Option<&'a str>,
}

#[async_trait]
pub(crate) trait AiClient: Send + Sync {
    /// Returns Ok(None) if the block satisfies the condition, Ok(Some(error_message)) otherwise.
    async fn check_block(
        &self,
        condition: String,
        block_content: String,
    ) -> anyhow::Result<Option<String>>;
}

/// Default OpenAI-based implementation. Uses async-openai crate.
pub(super) struct OpenAiClient {
    client: Client<OpenAIConfig>,
    model: String,
}

impl OpenAiClient {
    /// Creates a new OpenAI client from environment variables (BLOCKWATCH_AI_*),
    /// falling back to `async-openai` crate defaults when not provided.
    pub(crate) fn new_from_env() -> Self {
        let model = std::env::var("BLOCKWATCH_AI_MODEL").unwrap_or("gpt-4o-mini".to_string());
        let api_base =
            std::env::var("BLOCKWATCH_AI_API_URL").unwrap_or(OPENAI_API_BASE.to_string());
        let api_key = std::env::var("BLOCKWATCH_AI_API_KEY").unwrap_or("".to_string());
        let config = OpenAIConfig::new()
            .with_api_base(api_base)
            .with_api_key(api_key);
        let client = Client::with_config(config);
        Self { model, client }
    }
}

#[async_trait]
impl AiClient for OpenAiClient {
    async fn check_block(
        &self,
        condition: String,
        block_content: String,
    ) -> anyhow::Result<Option<String>> {
        if self.client.config().api_key().expose_secret().is_empty() {
            return Err(anyhow::anyhow!(
                "API key is empty. Is BLOCKWATCH_AI_API_KEY env variable set?"
            ));
        }
        let system_msg = ChatCompletionRequestSystemMessageArgs::default()
            .content(DEFAULT_SYSTEM_PROMPT)
            .build()
            .context("failed to build system message")?;

        let user =
            format!("CONDITION:\n{condition}\n\nBLOCK (preserve formatting):\n{block_content}");
        let user_msg = ChatCompletionRequestUserMessageArgs::default()
            .content(user)
            .build()
            .context("failed to build user message")?;

        let req = CreateChatCompletionRequestArgs::default()
            .model(self.model.clone())
            .messages([
                ChatCompletionRequestMessage::System(system_msg),
                ChatCompletionRequestMessage::User(user_msg),
            ])
            .build()
            .context("failed to build OpenAI request")?;

        let resp = self
            .client
            .chat()
            .create(req)
            .await
            .context("OpenAI API request failed")?;

        if let Some(chat_choice) = resp.choices.into_iter().next()
            && let Some(message) = chat_choice.message.content
        {
            return if message.eq_ignore_ascii_case("OK") || message.eq_ignore_ascii_case("OK.") {
                Ok(None)
            } else {
                Ok(Some(message))
            };
        }
        Err(anyhow!("empty response from AI"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::{Block, FileBlocks};
    use crate::test_utils;
    use serde_json::json;

    #[derive(Clone)]
    enum FakeAiResponse {
        // No violations.
        None,
        // Violation.
        Some(String),
        // Error message for a failed request.
        Err(String),
    }

    #[derive(Default)]
    struct FakeClient {
        // A map from the block's condition to a response: None = OK, Some(msg) = violation.
        responses: HashMap<(String, String), FakeAiResponse>,
    }

    impl FakeClient {
        fn new(responses: HashMap<(String, String), FakeAiResponse>) -> Self {
            Self { responses }
        }
    }

    #[async_trait]
    impl AiClient for FakeClient {
        async fn check_block(
            &self,
            condition: String,
            block_content: String,
        ) -> anyhow::Result<Option<String>> {
            let response = self
                .responses
                .get(&(condition.clone(), block_content.clone()))
                .cloned()
                .unwrap_or_else(|| {
                    panic!("Unexpected AiClient call: {condition:?}, {block_content:?}")
                });
            match response {
                FakeAiResponse::None => Ok(None),
                FakeAiResponse::Some(validation_error) => Ok(Some(validation_error)),
                FakeAiResponse::Err(error_message) => Err(anyhow!(error_message)),
            }
        }
    }

    #[tokio::test]
    async fn when_ai_returns_ok_returns_no_violations() -> anyhow::Result<()> {
        let validator = CheckAiValidator::with_client(FakeClient::new(HashMap::from([(
            ("must mention banana".into(), "I like banana".into()),
            FakeAiResponse::None,
        )])));
        let file1_contents = "block content goes here: I like banana";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    3,
                    HashMap::from([("check-ai".to_string(), "must mention banana".to_string())]),
                    test_utils::substr_range(file1_contents, "I like banana"),
                ))],
            },
        )])));
        let violations = validator.validate(context).await?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn pattern_match_is_used_as_block_content() -> anyhow::Result<()> {
        let validator = CheckAiValidator::with_client(FakeClient::new(HashMap::from([(
            ("must mention banana".into(), "I like banana".into()),
            FakeAiResponse::None,
        )])));
        let file1_contents = "block content goes here: I like banana and apples";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    3,
                    HashMap::from([
                        ("check-ai".to_string(), "must mention banana".to_string()),
                        // Entire RegExp match is used as the block's content.
                        ("check-ai-pattern".to_string(), r"I like \w+".to_string()),
                    ]),
                    test_utils::substr_range(file1_contents, "I like banana and apples"),
                ))],
            },
        )])));
        let violations = validator.validate(context).await?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn pattern_group_match_is_used_as_block_content() -> anyhow::Result<()> {
        let validator = CheckAiValidator::with_client(FakeClient::new(HashMap::from([(
            ("must mention banana".into(), "banana and apples".into()),
            FakeAiResponse::None,
        )])));
        let file1_contents = "block content goes here: I like banana and apples";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    3,
                    HashMap::from([
                        ("check-ai".to_string(), "must mention banana".to_string()),
                        // When a RegExp named group "value" is present it is used as the block's content.
                        (
                            "check-ai-pattern".to_string(),
                            r"I like (?P<value>banana and \w+)".to_string(),
                        ),
                    ]),
                    test_utils::substr_range(file1_contents, "I like banana and apples"),
                ))],
            },
        )])));
        let violations = validator.validate(context).await?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn when_ai_returns_violation_message_returns_violation() -> anyhow::Result<()> {
        let validator = CheckAiValidator::with_client(FakeClient::new(HashMap::from([(
            ("must mention banana".into(), "I like apples".into()),
            FakeAiResponse::Some("The block does not mention 'banana'. Add it.".into()),
        )])));
        let file1_contents = "block content goes here: I like apples";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    10,
                    14,
                    HashMap::from([("check-ai".to_string(), "must mention banana".to_string())]),
                    test_utils::substr_range(file1_contents, "I like apples"),
                ))],
            },
        )])));
        let violations = validator.validate(context).await?;
        assert_eq!(violations.len(), 1);
        assert_eq!(violations["file1"].len(), 1);
        let violation = &violations["file1"][0];
        assert_eq!(violation.code, "check-ai");
        assert_eq!(
            violation.message,
            "Block file1:(unnamed) defined at line 10 failed AI check: The block does not mention 'banana'. Add it."
        );
        assert_eq!(
            violation.data,
            Some(json!({
                "condition": "must mention banana",
                "ai_message": "The block does not mention 'banana'. Add it."
            }))
        );
        assert_eq!(violation.range, ViolationRange::new(10, 0, 14, 0));
        Ok(())
    }

    #[tokio::test]
    async fn when_ai_fails_with_error_it_is_propagated() -> anyhow::Result<()> {
        let validator = CheckAiValidator::with_client(FakeClient::new(HashMap::from([(
            ("condition".into(), "text".into()),
            FakeAiResponse::Err("API error".into()),
        )])));
        let file1_contents = "block content goes here: text";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    2,
                    4,
                    HashMap::from([("check-ai".to_string(), "condition".to_string())]),
                    test_utils::substr_range(file1_contents, "text"),
                ))],
            },
        )])));
        let err = validator.validate(context).await.unwrap_err();
        assert!(err.to_string().contains("check-ai API error"));
        Ok(())
    }

    #[tokio::test]
    async fn empty_condition_returns_error() -> anyhow::Result<()> {
        let validator = CheckAiValidator::with_client(FakeClient::default());
        let file1_contents = "block content goes here: text";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    5,
                    7,
                    HashMap::from([("check-ai".to_string(), " ".to_string())]),
                    test_utils::substr_range(file1_contents, "text"),
                ))],
            },
        )])));
        let err = validator.validate(context).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("check-ai requires a non-empty condition")
        );
        Ok(())
    }

    #[tokio::test]
    async fn empty_content_skips_api_call_returns_no_violations() -> anyhow::Result<()> {
        let validator = CheckAiValidator::with_client(FakeClient::default());
        let file1_contents = "block content goes here:  \n\n";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    1,
                    HashMap::from([("check-ai".to_string(), "condition".to_string())]),
                    test_utils::substr_range(file1_contents, "  \n\n"),
                ))],
            },
        )])));
        let violations = validator.validate(context).await?;
        assert!(violations.is_empty());
        Ok(())
    }
}
