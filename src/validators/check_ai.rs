use crate::blocks::{Block, BlockWithContext};
use crate::repo_path::RepoPath;
use crate::validators::{
    AsyncValidator, PatternContent, ValidationContext, ValidationReport, Validator, ValidatorTrace,
    ValidatorType, Violation, block_content_for_pattern, require_main,
};
use anyhow::{Context, anyhow};
use async_openai::Client;
use async_openai::config::{Config, OPENAI_API_BASE, OpenAIConfig};
use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
    ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs,
};
use async_trait::async_trait;
use secrecy::ExposeSecret;
use serde::Serialize;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::task::JoinSet;

const DEFAULT_SYSTEM_PROMPT: &str = r"You are a strict validator. You are given a CONDITION and a BLOCK.
- If the BLOCK satisfies the CONDITION, reply with exactly: OK
- If the BLOCK violates the CONDITION, reply ONLY with a short, meaningful, and actionable error message describing what must be changed.
- Do not include quotes, labels, or extra text.";

/* <block name="latest-model" check-lua="scripts/check_latest_gpt_nano_model.lua"
   check-lua-pattern='str = "(?P<value>[^"]+)"' same-as-pattern='str = "(?P<value>[^"]+)"'>
*/
const DEFAULT_MODEL_NAME: &str = "gpt-5.4-nano";
// </block>

/* <block name="check-ai-env-vars"
   same-as="docs/validators/check-ai.md:check-ai-env-vars" same-as-pattern="BLOCKWATCH_AI_[A-Z_]+">
*/
const API_KEY_ENV_VAR_NAME: &str = "BLOCKWATCH_AI_API_KEY";
const API_URL_ENV_VAR_NAME: &str = "BLOCKWATCH_AI_API_URL";
const API_MODEL_ENV_VAR_NAME: &str = "BLOCKWATCH_AI_MODEL";
// </block>

/// Enforces `check-ai="<condition>"`: asks a language model whether the block's content satisfies a
/// condition stated in prose, for rules too fuzzy to express as a regex.
///
/// Async because each block costs a network round trip; blocks are checked concurrently.
struct CheckAiValidator<C: AiClient> {
    client: Arc<C>,
}

#[async_trait]
impl<C: AiClient + 'static> AsyncValidator for CheckAiValidator<C> {
    async fn validate(&self, context: Arc<ValidationContext>) -> anyhow::Result<ValidationReport> {
        let mut report = ValidationReport::default();
        let mut tasks = JoinSet::new();
        for (file_path, file_blocks) in &context.blocks {
            for (block_idx, block_with_context) in
                file_blocks.blocks_with_context.iter().enumerate()
            {
                let Some(condition) = block_with_context.block.attributes.get("check-ai") else {
                    continue;
                };
                if condition.trim().is_empty() {
                    return Err(anyhow!(
                        "check-ai requires a non-empty condition in {}",
                        block_with_context.block.label(file_path)
                    ));
                }

                // The block is checked from here on, whatever the API answers, so add it
                // before the borrowed path is shadowed by the owned copy the task takes.
                report.add_checked_block(file_path, &block_with_context.block);

                let client = Arc::clone(&self.client);
                let context = Arc::clone(&context);
                let file_path = file_path.clone();
                tasks.spawn(async move {
                    let (violations, trace) =
                        check_block(client.as_ref(), &context, &file_path, block_idx).await?;
                    anyhow::Ok((file_path, block_idx, violations, trace))
                });
            }
        }
        while let Some(task_result) = tasks.join_next().await {
            let (file_path, block_idx, violations, trace) =
                task_result.context("check-ai task failed")??;
            if let Some(trace) = trace {
                let block = &context.blocks[&file_path].blocks_with_context[block_idx].block;
                report.add_trace(&file_path, block, trace);
            }
            report.add_violations(&file_path, violations);
        }
        Ok(report)
    }
}

/// Asks the model whether block `block_idx` of `file_path` meets its `check-ai` condition.
/// Returns the block's violations, and its trace when `context` asks for traces.
///
/// # Errors
/// Returns an error if `check-ai-pattern` is not a valid regex, or the request to the model fails.
async fn check_block<C: AiClient>(
    client: &C,
    context: &ValidationContext,
    file_path: &RepoPath,
    block_idx: usize,
) -> anyhow::Result<(Vec<Violation>, Option<ValidatorTrace>)> {
    let file_blocks = &context.blocks[file_path];
    let block_with_context = &file_blocks.blocks_with_context[block_idx];
    let block = &block_with_context.block;
    let trace = |prompt, response| {
        context.traces_requested.then(|| {
            ValidatorTrace::CheckAi(CheckAiTrace {
                model: client.model().to_string(),
                prompt,
                response,
            })
        })
    };
    // A prompt can only carry text, so the extracted values are flattened into one
    // newline-separated blob for the model to read.
    let content = match block_content_for_pattern(
        block_with_context,
        &file_blocks.file_content,
        "check-ai-pattern",
    )? {
        PatternContent::Whole(content) => Cow::Borrowed(content),
        PatternContent::Matches(matches) if matches.is_empty() => {
            let violation = create_pattern_no_match_violation(
                file_path,
                block,
                &block.attributes["check-ai-pattern"],
            )?;
            return Ok((vec![violation], trace(None, None)));
        }
        PatternContent::Matches(matches) => Cow::Owned(matches.join("\n")),
    };

    let prompt = user_prompt(&block.attributes["check-ai"], &content);
    let response = client
        .ask(&prompt)
        .await
        .with_context(|| format!("check-ai API error in {}", block.label(file_path)))?;
    let violation = (!is_pass(&response))
        .then(|| create_violation(file_path, block, &response))
        .transpose()?;
    Ok((
        Vec::from_iter(violation),
        trace(Some(prompt), Some(response)),
    ))
}

/// Checks that `check-ai-pattern` comes with `check-ai`.
///
/// # Errors
/// Returns an error that quotes `check-ai-pattern` when it comes without `check-ai`.
pub(super) fn check_attributes(attributes: &HashMap<String, String>) -> anyhow::Result<()> {
    require_main(attributes, &["check-ai"], &["check-ai-pattern"])
}

/// Selects [`CheckAiValidator`] for blocks carrying a `check-ai` attribute, building a client from
/// the `BLOCKWATCH_AI_*` environment variables.
pub(super) fn detect(block_with_context: &BlockWithContext) -> Option<ValidatorType> {
    block_with_context
        .block
        .attributes
        .contains_key("check-ai")
        .then(|| {
            ValidatorType::Async(Box::new(CheckAiValidator::with_client(
                OpenAiClient::from_env(),
            )))
        })
}

fn create_violation(
    file_path: &RepoPath,
    block: &Block,
    ai_message: &str,
) -> anyhow::Result<Violation> {
    let error_message = format!(
        "Block {} failed AI check: {ai_message}",
        block.label(file_path),
    );
    block_violation(
        file_path,
        block,
        error_message,
        CheckAiViolation {
            condition: condition_of(block),
            ai_message: Some(ai_message),
            pattern: None,
        },
    )
}

/// Reports a `check-ai-pattern` that matches nothing from its block.
fn create_pattern_no_match_violation(
    file_path: &RepoPath,
    block: &Block,
    pattern: &str,
) -> anyhow::Result<Violation> {
    let error_message = format!(
        "Block {} was not checked: check-ai-pattern \"{pattern}\" matched nothing in the block",
        block.label(file_path),
    );
    block_violation(
        file_path,
        block,
        error_message,
        CheckAiViolation {
            condition: condition_of(block),
            ai_message: None,
            pattern: Some(pattern),
        },
    )
}

/// The block's `check-ai` condition.
fn condition_of(block: &Block) -> &str {
    block
        .attributes
        .get("check-ai")
        .expect("check-ai attribute must be present")
        .trim()
}

/// Builds a `check-ai` violation spanning the block's start tag, shared by everything the validator
/// reports so the range and severity are decided in one place.
fn block_violation(
    file_path: &RepoPath,
    block: &Block,
    error_message: String,
    details: CheckAiViolation,
) -> anyhow::Result<Violation> {
    let details = serde_json::to_value(details)?;
    Ok(Violation::new(
        block.start_tag_position_range.clone().into(),
        file_path,
        block,
        Validator::CheckAi,
        error_message,
        // The model answers about the whole block, so there is never a sibling to tell apart.
        None,
        Some(details),
    ))
}

impl<C: AiClient> CheckAiValidator<C> {
    /// Creates the validator over a given client, which is what makes it testable without a
    /// network call.
    fn with_client(client: C) -> Self {
        Self {
            client: Arc::new(client),
        }
    }
}

/// The user message that asks the model whether `content` meets `condition`.
fn user_prompt(condition: &str, content: &str) -> String {
    format!("CONDITION:\n{condition}\n\nBLOCK (formatting preserved):\n{content}")
}

/// Whether the model's `response` says that the block meets its condition. The system prompt asks
/// for exactly `OK`, but models often add a period or change the case.
fn is_pass(response: &str) -> bool {
    response.eq_ignore_ascii_case("OK") || response.eq_ignore_ascii_case("OK.")
}

/// What `check-ai` sent to the model, and what it answered.
///
/// serde writes the keys in the order the fields are declared, and the output keeps its keys
/// sorted. So declare the fields in alphabetical order.
#[derive(Debug, Serialize)]
pub(crate) struct CheckAiTrace {
    model: String,
    /// The user message. The system prompt is the same for every block, so it is left out. `None`
    /// when `check-ai-pattern` matches nothing, so no request is sent.
    prompt: Option<String>,
    /// The model's answer as it is. `None` when no request is sent.
    response: Option<String>,
}

#[derive(Serialize)]
struct CheckAiViolation<'a> {
    condition: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    ai_message: Option<&'a str>,
    /// Present only when the block's `check-ai-pattern` matches nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pattern: Option<&'a str>,
}

/// The single call [`CheckAiValidator`] makes against a model provider.
///
/// Narrow on purpose, so the validator's own logic, which builds the prompt and reads the answer,
/// can be tested against a canned client.
#[async_trait]
trait AiClient: Send + Sync {
    /// The model that answers.
    fn model(&self) -> &str;

    /// Sends `prompt` as the user message, and returns the model's answer as it is.
    async fn ask(&self, prompt: &str) -> anyhow::Result<String>;
}

/// Default OpenAI-based implementation. Uses async-openai crate.
struct OpenAiClient {
    client: Client<OpenAIConfig>,
    model: String,
}

impl OpenAiClient {
    /// Creates a new OpenAI client from environment variables (BLOCKWATCH_AI_*),
    /// falling back to `async-openai` crate defaults when not provided.
    fn from_env() -> Self {
        let model = std::env::var(API_MODEL_ENV_VAR_NAME).unwrap_or(DEFAULT_MODEL_NAME.into());
        let api_base = std::env::var(API_URL_ENV_VAR_NAME).unwrap_or(OPENAI_API_BASE.into());
        let api_key = std::env::var(API_KEY_ENV_VAR_NAME).unwrap_or("".into());
        let config = OpenAIConfig::new()
            .with_api_base(api_base)
            .with_api_key(api_key);
        let client = Client::with_config(config);
        Self { model, client }
    }
}

#[async_trait]
impl AiClient for OpenAiClient {
    fn model(&self) -> &str {
        &self.model
    }

    async fn ask(&self, prompt: &str) -> anyhow::Result<String> {
        if self.client.config().api_key().expose_secret().is_empty() {
            return Err(anyhow::anyhow!(
                "API key is empty. Is {API_KEY_ENV_VAR_NAME} env variable set?"
            ));
        }
        let user_msg = ChatCompletionRequestUserMessageArgs::default()
            .content(prompt)
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

        resp.choices
            .into_iter()
            .next()
            .and_then(|chat_choice| chat_choice.message.content)
            .ok_or_else(|| anyhow!("empty response from AI"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo_path::RepoPath;
    use crate::test_utils::validation_context;
    use crate::validators::test_utils::{checked_lines, violation_count};
    use serde_json::json;
    use std::collections::HashMap;

    #[derive(Clone)]
    pub(super) enum FakeAiResponse {
        // No violations.
        None,
        // Violation.
        Some(String),
        // Error message for a failed request.
        Err(String),
    }

    #[derive(Default)]
    pub(super) struct FakeClient {
        // A map from the prompt to a response: None = OK, Some(msg) = violation.
        responses: HashMap<String, FakeAiResponse>,
    }

    impl FakeClient {
        /// A client that answers the prompt of each `(condition, content)` with its response.
        pub(super) fn new(responses: HashMap<(String, String), FakeAiResponse>) -> Self {
            let responses = responses
                .into_iter()
                .map(|((condition, content), response)| {
                    (user_prompt(&condition, &content), response)
                })
                .collect();
            Self { responses }
        }
    }

    #[async_trait]
    impl AiClient for FakeClient {
        fn model(&self) -> &str {
            "fake-model"
        }

        async fn ask(&self, prompt: &str) -> anyhow::Result<String> {
            let response = self
                .responses
                .get(prompt)
                .cloned()
                .unwrap_or_else(|| panic!("Unexpected AiClient call: {prompt:?}"));
            match response {
                FakeAiResponse::None => Ok("OK".to_string()),
                FakeAiResponse::Some(validation_error) => Ok(validation_error),
                FakeAiResponse::Err(error_message) => Err(anyhow!(error_message)),
            }
        }
    }

    #[tokio::test]
    async fn block_with_a_failing_ai_check_returns_a_violation() -> anyhow::Result<()> {
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
        let violations = validator.validate(context).await?.violations;
        assert_eq!(violations.len(), 1);
        assert_eq!(
            violations[&RepoPath::from_reference("example.py")?].len(),
            1
        );
        let violation = &violations[&RepoPath::from_reference("example.py")?][0];
        assert_eq!(violation.code, Validator::CheckAi);
        assert_eq!(
            violation.message,
            "Block example.py:(unnamed) at line 1 failed AI check: The block does not mention 'banana'. Add it."
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
    async fn block_with_a_passing_ai_check_returns_no_violations() -> anyhow::Result<()> {
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
        let violations = validator.validate(context).await?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn ai_answer_ok_in_lowercase_with_a_period_returns_no_violations() -> anyhow::Result<()> {
        let validator = CheckAiValidator::with_client(FakeClient::new(HashMap::from([(
            ("must mention banana".into(), "I like banana".into()),
            FakeAiResponse::Some("ok.".into()),
        )])));
        let context = validation_context(
            "example.py",
            r#"# <block check-ai="must mention banana">
I like banana
# </block>"#,
        );
        let violations = validator.validate(context).await?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn failing_ai_request_returns_an_error() -> anyhow::Result<()> {
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
    async fn block_with_a_check_ai_pattern_sends_only_the_matched_text() -> anyhow::Result<()> {
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
        let report = validator.validate(context).await?;

        assert!(report.violations.is_empty());
        // The block was checked and passed. That is different from never being checked at all.
        assert_eq!(checked_lines(&report), vec![1]);
        Ok(())
    }

    #[tokio::test]
    async fn block_with_a_check_ai_pattern_group_sends_only_the_captured_group()
    -> anyhow::Result<()> {
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
        let violations = validator.validate(context).await?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn block_with_a_check_ai_pattern_sends_every_match_in_the_block() -> anyhow::Result<()> {
        let validator = CheckAiValidator::with_client(FakeClient::new(HashMap::from([(
            ("Prices must be under $100".into(), "50\n150".into()),
            FakeAiResponse::Some("Item B costs $150.".into()),
        )])));
        let context = validation_context(
            "example.py",
            r#"prices = [
    # <block check-ai="Prices must be under $100" check-ai-pattern="\$(?P<value>\d+)">
    "Item A: $50",
    "Item B: $150",
    # </block>
]"#,
        );
        let violations = validator.validate(context).await?.violations;

        assert_eq!(
            violations[&RepoPath::from_reference("example.py")?].len(),
            1
        );
        Ok(())
    }

    #[tokio::test]
    async fn check_ai_pattern_matches_that_are_empty_are_skipped() -> anyhow::Result<()> {
        let validator = CheckAiValidator::with_client(FakeClient::new(HashMap::from([(
            ("must list the ids".into(), "1\n22".into()),
            FakeAiResponse::None,
        )])));
        // "\d*" matches the empty string at every position that does not start a digit run. Those
        // carry no value, so they must not pad the extracted content with blank lines.
        let context = validation_context(
            "example.py",
            r#"# <block check-ai="must list the ids" check-ai-pattern="\d*">
a1
b22
# </block>"#,
        );
        let violations = validator.validate(context).await?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn check_ai_pattern_with_a_value_group_not_in_the_match_skips_the_match()
    -> anyhow::Result<()> {
        let validator = CheckAiValidator::with_client(FakeClient::new(HashMap::from([(
            ("must list colors".into(), "red".into()),
            FakeAiResponse::None,
        )])));
        // The `none` line matches the branch without the `value` group.
        let context = validation_context(
            "example.py",
            r#"# <block check-ai="must list colors" check-ai-pattern="(?m)^(?:none|(?P<value>\w+))$">
none
red
# </block>"#,
        );
        let violations = validator.validate(context).await?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn block_with_a_check_ai_pattern_that_matches_nothing_returns_a_violation()
    -> anyhow::Result<()> {
        let validator = CheckAiValidator::with_client(FakeClient::default());
        let context = validation_context(
            "example.py",
            r#"# <block check-ai="must mention banana" check-ai-pattern="zzz_no_match">
I like apples
# </block>"#,
        );
        let report = validator.validate(context).await?;

        let file_violations = &report.violations[&RepoPath::from_reference("example.py")?];
        assert_eq!(file_violations.len(), 1);
        assert_eq!(file_violations[0].code, Validator::CheckAi);
        assert_eq!(
            file_violations[0].message,
            "Block example.py:(unnamed) at line 1 was not checked: check-ai-pattern \"zzz_no_match\" matched nothing in the block"
        );
        assert_eq!(
            file_violations[0].data,
            Some(json!({
                "condition": "must mention banana",
                "pattern": "zzz_no_match"
            }))
        );
        Ok(())
    }

    #[tokio::test]
    async fn block_with_an_empty_check_ai_condition_returns_an_error() -> anyhow::Result<()> {
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

    #[tokio::test]
    async fn blocks_with_and_without_check_ai_records_a_check_for_the_examined_ones_only()
    -> anyhow::Result<()> {
        let validator = CheckAiValidator::with_client(FakeClient::new(HashMap::from([
            (
                ("must mention banana".into(), "I like banana".into()),
                FakeAiResponse::None,
            ),
            (
                ("must mention apple".into(), "I like banana".into()),
                FakeAiResponse::Some("no apple".into()),
            ),
        ])));
        let context = validation_context(
            "example.py",
            r#"# <block name="passing" check-ai="must mention banana">
I like banana
# </block>
# <block name="failing" check-ai="must mention apple">
I like banana
# </block>
# <block name="unrelated">
I like banana
# </block>"#,
        );

        let report = validator.validate(context).await?;

        // Both blocks are recorded whatever the answer comes back as, and the block without a
        // check-ai attribute is not checked, so it records nothing.
        assert_eq!(checked_lines(&report), vec![1, 4]);
        assert_eq!(violation_count(&report), 1);
        Ok(())
    }
}

#[cfg(test)]
mod trace_tests {
    use super::tests::{FakeAiResponse, FakeClient};
    use super::*;
    use crate::test_utils::validation_context;
    use crate::validators::test_utils::{trace_json, with_traces};
    use serde_json::json;
    use std::collections::HashMap;

    #[tokio::test]
    async fn block_records_the_prompt_and_the_response() -> anyhow::Result<()> {
        let validator = CheckAiValidator::with_client(FakeClient::new(HashMap::from([(
            ("must mention banana".into(), "I like apples".into()),
            FakeAiResponse::Some("Mention banana.".into()),
        )])));
        let context = with_traces(validation_context(
            "a.py",
            "# <block check-ai=\"must mention banana\">\nI like apples\n# </block>\n",
        ));

        let report = validator.validate(context).await?;

        assert_eq!(
            trace_json(&report),
            json!({
                "model": "fake-model",
                "prompt": "CONDITION:\nmust mention banana\n\nBLOCK (formatting preserved):\nI like apples",
                "response": "Mention banana."
            })
        );
        Ok(())
    }

    #[tokio::test]
    async fn block_whose_pattern_matches_nothing_records_that_no_prompt_was_sent()
    -> anyhow::Result<()> {
        let validator = CheckAiValidator::with_client(FakeClient::default());
        let context = with_traces(validation_context(
            "a.py",
            "# <block check-ai=\"must mention banana\" check-ai-pattern=\"zzz\">\nI like apples\n# </block>\n",
        ));

        let report = validator.validate(context).await?;

        assert_eq!(
            trace_json(&report),
            json!({ "model": "fake-model", "prompt": null, "response": null })
        );
        Ok(())
    }

    #[tokio::test]
    async fn context_without_traces_requested_records_no_trace() -> anyhow::Result<()> {
        let validator = CheckAiValidator::with_client(FakeClient::new(HashMap::from([(
            ("must mention banana".into(), "I like apples".into()),
            FakeAiResponse::None,
        )])));
        let context = validation_context(
            "a.py",
            "# <block check-ai=\"must mention banana\">\nI like apples\n# </block>\n",
        );

        let report = validator.validate(context).await?;

        assert!(report.traces.is_empty());
        Ok(())
    }
}

#[cfg(test)]
mod check_attributes_tests {
    use crate::validators::Validator;
    use std::collections::HashMap;

    #[test]
    fn pattern_without_check_ai_returns_an_error() {
        let attributes = HashMap::from([("check-ai-pattern".to_string(), "\\d+".to_string())]);

        let error = Validator::CheckAi
            .check_attributes(&attributes)
            .unwrap_err();

        assert_eq!(error.to_string(), "`check-ai-pattern` without `check-ai`");
    }
}
