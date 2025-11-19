use crate::Position;
use crate::blocks::{Block, BlockWithContext, FileBlocks};
use crate::validators::{
    ValidationContext, ValidatorAsync, ValidatorDetector, ValidatorType, Violation, ViolationRange,
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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::task::JoinSet;

const DEFAULT_SYSTEM_PROMPT: &str = r"You are a strict validator. You are given a CONDITION and a BLOCK.
- If the BLOCK satisfies the CONDITION, reply with exactly: OK
- If the BLOCK violates the CONDITION, reply ONLY with a short, meaningful, and actionable error message describing what must be changed.
- Do not include quotes, labels, or extra text.";

// <block affects="README.md:check-ai-env-vars, tests/check_ai.rs:check-ai-env-vars">
const API_KEY_ENV_VAR_NAME: &str = "BLOCKWATCH_AI_API_KEY";
const API_URL_ENV_VAR_NAME: &str = "BLOCKWATCH_AI_API_URL";
const API_MODEL_ENV_VAR_NAME: &str = "BLOCKWATCH_AI_MODEL";
// </block>

pub(crate) struct CheckAiValidator<C: AiClient> {
    client: Arc<C>,
}

#[async_trait]
impl<C: AiClient + 'static> ValidatorAsync for CheckAiValidator<C> {
    async fn validate(
        &self,
        context: Arc<ValidationContext>,
    ) -> anyhow::Result<HashMap<PathBuf, Vec<Violation>>> {
        let mut violations = HashMap::new();
        let mut tasks = JoinSet::new();
        for (file_path, file_blocks) in &context.modified_blocks {
            for (block_idx, block_with_context) in
                file_blocks.blocks_with_context.iter().enumerate()
            {
                if let Some(condition) = block_with_context.block.attributes.get("check-ai") {
                    if condition.trim().is_empty() {
                        return Err(anyhow!(
                            "check-ai requires a non-empty condition in {}:{} at line {}",
                            file_path.display(),
                            block_with_context.block.name_display(),
                            block_with_context.block.starts_at_line
                        ));
                    };
                } else {
                    continue;
                }

                let client = Arc::clone(&self.client);
                let context = Arc::clone(&context);
                let file_path = file_path.clone();
                tasks.spawn(async move {
                    let file_blocks = &context.modified_blocks[&file_path];
                    let block_with_context = &file_blocks.blocks_with_context[block_idx];
                    let condition = &block_with_context.block.attributes["check-ai"];
                    let content = block_content(block_with_context, &file_blocks.file_content)?;

                    let result = client.check_block(condition, content).await;
                    Self::process_ai_response(file_path, file_blocks, block_with_context, result)
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
    fn detect(
        &self,
        block_with_context: &BlockWithContext,
    ) -> anyhow::Result<Option<ValidatorType>> {
        if block_with_context.block.attributes.contains_key("check-ai") {
            Ok(Some(ValidatorType::Async(Box::new(
                CheckAiValidator::with_client(OpenAiClient::new_from_env()),
            ))))
        } else {
            Ok(None)
        }
    }
}

fn block_content<'c>(
    block_with_context: &BlockWithContext,
    file_content: &'c str,
) -> anyhow::Result<&'c str> {
    let content = if let Some(pattern) = block_with_context.block.attributes.get("check-ai-pattern")
    {
        let re = regex::Regex::new(pattern).context("check-ai-pattern is not a valid regex")?;
        if let Some(c) = re.captures(block_with_context.block.content(file_content)) {
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
        block_with_context.block.content(file_content).trim()
    };
    Ok(content)
}

fn create_violation(
    file_path: &Path,
    block: &Block,
    new_line_positions: &[usize],
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
        file_path.display(),
        block.name_display(),
        block.starts_at_line,
    );
    Ok(Violation::new(
        ViolationRange::new(
            Position::from_byte_offset(block.start_tag_range.start, new_line_positions),
            Position::from_byte_offset(block.start_tag_range.end - 1, new_line_positions),
        ),
        "check-ai".to_string(),
        error_message,
        block.severity()?,
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
        file_path: PathBuf,
        file_blocks: &FileBlocks,
        block_with_context: &BlockWithContext,
        result: anyhow::Result<Option<String>>,
    ) -> anyhow::Result<Option<(PathBuf, Violation)>> {
        match result.context(format!(
            "check-ai API error in {}:{} at line {}",
            file_path.display(),
            block_with_context.block.name_display(),
            block_with_context.block.starts_at_line
        ))? {
            None => Ok(None),
            Some(msg) => {
                let violation = create_violation(
                    &file_path,
                    &block_with_context.block,
                    &file_blocks.file_content_new_lines,
                    &msg,
                )?;
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
        condition: &str,
        block_content: &str,
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
        let model = std::env::var(API_MODEL_ENV_VAR_NAME).unwrap_or("gpt-4o-mini".to_string());
        let api_base = std::env::var(API_URL_ENV_VAR_NAME).unwrap_or(OPENAI_API_BASE.to_string());
        let api_key = std::env::var(API_KEY_ENV_VAR_NAME).unwrap_or("".to_string());
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
        condition: &str,
        block_content: &str,
    ) -> anyhow::Result<Option<String>> {
        if self.client.config().api_key().expose_secret().is_empty() {
            return Err(anyhow::anyhow!(
                "API key is empty. Is {API_KEY_ENV_VAR_NAME} env variable set?"
            ));
        }
        let user =
            format!("CONDITION:\n{condition}\n\nBLOCK (formatting preserved):\n{block_content}");
        let user_msg = ChatCompletionRequestUserMessageArgs::default()
            .content(user)
            .build()
            .context("failed to build user message")?;

        let system_msg = ChatCompletionRequestSystemMessageArgs::default()
            .content(DEFAULT_SYSTEM_PROMPT)
            .build()
            .context("failed to build system message")?;

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
    use crate::test_utils::validation_context;
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
            condition: &str,
            block_content: &str,
        ) -> anyhow::Result<Option<String>> {
            let response = self
                .responses
                .get(&(condition.to_string(), block_content.to_string()))
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
        let context = validation_context(
            "example.py",
            r#"# <block check-ai="must mention banana">
I like banana
# </block>"#,
        );
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
        let context = validation_context(
            "example.py",
            r#"# <block check-ai="must mention banana" check-ai-pattern="I like \w+">
I like banana and apples
# </block>"#,
        );
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
        let context = validation_context(
            "example.py",
            r#"# <block check-ai="must mention banana" check-ai-pattern="I like (?P<value>banana and \w+)">
I like banana and apples
# </block>"#,
        );
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
        let context = validation_context(
            "example.py",
            r#"# <block check-ai="must mention banana">
I like apples
# </block>"#,
        );
        let violations = validator.validate(context).await?;
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[&PathBuf::from("example.py")].len(), 1);
        let violation = &violations[&PathBuf::from("example.py")][0];
        assert_eq!(violation.code, "check-ai");
        assert_eq!(
            violation.message,
            "Block example.py:(unnamed) defined at line 1 failed AI check: The block does not mention 'banana'. Add it."
        );
        assert_eq!(
            violation.data,
            Some(json!({
                "condition": "must mention banana",
                "ai_message": "The block does not mention 'banana'. Add it."
            }))
        );
        Ok(())
    }

    #[tokio::test]
    async fn when_ai_fails_with_error_it_is_propagated() -> anyhow::Result<()> {
        let validator = CheckAiValidator::with_client(FakeClient::new(HashMap::from([(
            ("condition".into(), "text".into()),
            FakeAiResponse::Err("API error".into()),
        )])));
        let context = validation_context(
            "example.py",
            r#"# <block check-ai="condition">
text
# </block>"#,
        );
        let err = validator.validate(context).await.unwrap_err();
        assert!(err.to_string().contains("check-ai API error"));
        Ok(())
    }

    #[tokio::test]
    async fn empty_condition_returns_error() -> anyhow::Result<()> {
        let validator = CheckAiValidator::with_client(FakeClient::default());
        let context = validation_context(
            "example.py",
            r#"# <block check-ai=" ">
text
# </block>"#,
        );
        let err = validator.validate(context).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("check-ai requires a non-empty condition")
        );
        Ok(())
    }
}
