use crate::blocks::{Block, BlockWithContext};
use crate::validators::parse_affects_attribute;
use crate::validators::{
    ValidationContext, ValidatorAsync, ValidatorDetector, ValidatorType, Violation, ViolationRange,
};
use anyhow::{Context, anyhow};
use async_trait::async_trait;
use mlua::{Lua, StdLib};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::task::JoinSet;

const LUA_STDLIB_ENV_VAR: &str = "BLOCKWATCH_LUA_MODE";

/// Returns the Lua standard library set based on the `BLOCKWATCH_LUA_MODE` environment variable.
///
/// - `sandboxed` (default): Most restrictive, blocks file/OS access.
/// - `safe`: Memory-safe but includes IO/OS (useful for trusted scripts).
/// - `unsafe`: Fully unsafe, allows C module loading.
fn lua_from_env() -> Lua {
    // <block affects="README.md:lua-safety-modes">
    match std::env::var(LUA_STDLIB_ENV_VAR)
        .as_deref()
        .unwrap_or("sandboxed")
    {
        "unsafe" => unsafe { Lua::unsafe_new() },
        "safe" => Lua::new(),
        _ => Lua::new_with(
            StdLib::COROUTINE | StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH,
            Default::default(),
        )
        .expect("failed to start Lua"),
    }
    // </block>
}

pub(crate) struct CheckLuaValidator;

impl CheckLuaValidator {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ValidatorAsync for CheckLuaValidator {
    async fn validate(
        &self,
        context: Arc<ValidationContext>,
    ) -> anyhow::Result<HashMap<PathBuf, Vec<Violation>>> {
        let mut violations = HashMap::new();
        let mut tasks = JoinSet::new();
        for (file_path, file_blocks) in &context.blocks {
            for (block_idx, block_with_context) in
                file_blocks.blocks_with_context.iter().enumerate()
            {
                if let Some(script_path) = block_with_context.block.attributes.get("check-lua") {
                    if script_path.trim().is_empty() {
                        return Err(anyhow!(
                            "check-lua requires a non-empty script path in {}:{} at line {}",
                            file_path.display(),
                            block_with_context.block.name_display(),
                            block_with_context
                                .block
                                .start_tag_position_range
                                .start()
                                .line
                        ));
                    };
                } else {
                    continue;
                }

                let context = Arc::clone(&context);
                let file_path = file_path.clone();
                tasks.spawn(async move {
                    let file_blocks = &context.blocks[&file_path];
                    let block_with_context = &file_blocks.blocks_with_context[block_idx];
                    let script_path = &block_with_context.block.attributes["check-lua"];
                    let content = block_content(block_with_context, &file_blocks.file_content)?;
                    let affected_blocks =
                        resolve_affected_blocks(&context, &file_path, &block_with_context.block)?;

                    let result = run_lua_script(
                        script_path,
                        &file_path,
                        block_with_context,
                        content,
                        &affected_blocks,
                    )
                    .await;

                    match result.context(format!(
                        "check-lua script error in {}:{} at line {}",
                        file_path.display(),
                        block_with_context.block.name_display(),
                        block_with_context
                            .block
                            .start_tag_position_range
                            .start()
                            .line
                    ))? {
                        None => Ok(None),
                        Some(msg) => {
                            let violation = create_violation(
                                &file_path,
                                &block_with_context.block,
                                script_path,
                                &msg,
                            )?;
                            Ok(Some((file_path, violation)))
                        }
                    }
                });
            }
        }
        while let Some(task_result) = tasks.join_next().await {
            match task_result.context("check-lua task failed")? {
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

async fn run_lua_script(
    script_path: &str,
    file_path: &Path,
    block_with_context: &BlockWithContext,
    content: &str,
    affected_blocks: &[AffectedBlock],
) -> anyhow::Result<Option<String>> {
    let lua = lua_from_env();

    let script_content = std::fs::read_to_string(script_path)
        .with_context(|| format!("failed to read Lua script: {script_path}"))?;

    lua.load(&script_content)
        .exec_async()
        .await
        .with_context(|| format!("failed to execute Lua script: {script_path}"))?;

    let validate_fn: mlua::Function = lua
        .globals()
        .get("validate")
        .context("Lua script must define a global 'validate' function")?;

    let ctx_table = lua.create_table().context("failed to create ctx table")?;
    ctx_table
        .set("file", file_path.to_string_lossy().as_ref())
        .context("failed to set ctx.file")?;
    ctx_table
        .set(
            "line",
            block_with_context
                .block
                .start_tag_position_range
                .start()
                .line,
        )
        .context("failed to set ctx.line")?;

    let attrs_table = lua.create_table().context("failed to create attrs table")?;
    for (key, value) in &block_with_context.block.attributes {
        attrs_table
            .set(key.as_str(), value.as_str())
            .with_context(|| format!("failed to set attr {key}"))?;
    }
    ctx_table
        .set("attrs", attrs_table)
        .context("failed to set ctx.attrs")?;

    // When the block carries an `affects` attribute, expose the affected blocks as
    // `ctx.affects = [{ file, name, content }, …]` so scripts can inspect them in sandboxed mode.
    if block_with_context.block.attributes.contains_key("affects") {
        let affects_table = lua
            .create_table()
            .context("failed to create affects table")?;
        for (i, affected) in affected_blocks.iter().enumerate() {
            let entry = lua
                .create_table()
                .context("failed to create affects entry table")?;
            entry
                .set("file", affected.file.to_string_lossy().as_ref())
                .context("failed to set ctx.affects[].file")?;
            entry
                .set("name", affected.name.as_str())
                .context("failed to set ctx.affects[].name")?;
            entry
                .set("content", affected.content.as_str())
                .context("failed to set ctx.affects[].content")?;
            affects_table
                .set(i + 1, entry)
                .context("failed to set ctx.affects entry")?;
        }
        ctx_table
            .set("affects", affects_table)
            .context("failed to set ctx.affects")?;
    }

    let result: mlua::Value = validate_fn
        .call_async((ctx_table, content.to_string()))
        .await
        .with_context(|| format!("failed to call validate() in {script_path}"))?;

    match result {
        mlua::Value::Nil => Ok(None),
        mlua::Value::String(s) => Ok(Some(s.to_str()?.to_string())),
        other => Err(anyhow!(
            "validate() must return nil or a string, got: {:?}",
            other.type_name()
        )),
    }
}

fn create_violation(
    file_path: &Path,
    block: &Block,
    script_path: &str,
    error_message: &str,
) -> anyhow::Result<Violation> {
    let details = serde_json::to_value(CheckLuaViolation {
        script: script_path,
        lua_error: error_message,
    })
    .context("failed to serialize CheckLuaDetails")?;
    let message = format!(
        "Block {}:{} defined at line {} failed Lua check: {error_message}",
        file_path.display(),
        block.name_display(),
        block.start_tag_position_range.start().line,
    );
    Ok(Violation::new(
        ViolationRange::new(
            block.start_tag_position_range.start().clone(),
            block.start_tag_position_range.end().clone(),
        ),
        "check-lua".to_string(),
        message,
        block.severity()?,
        Some(details),
    ))
}

/// A block referenced by the validated block's `affects` attribute, exposed to Lua scripts.
struct AffectedBlock {
    file: PathBuf,
    name: String,
    content: String,
}

/// Resolves the blocks referenced by the `affects` attribute of `block` to their `(file, name,
/// content)` so they can be exposed to the Lua script.
///
/// References to blocks that don't exist in the validation context are skipped (the `affects`
/// validator is responsible for reporting those). The content is trimmed to mirror how the
/// validated block's own content is presented.
fn resolve_affected_blocks(
    context: &ValidationContext,
    current_file_path: &Path,
    block: &Block,
) -> anyhow::Result<Vec<AffectedBlock>> {
    let mut result = Vec::new();
    let Some(affects) = block.attributes.get("affects") else {
        return Ok(result);
    };
    for (file, name) in parse_affects_attribute(affects)? {
        let file = file.unwrap_or_else(|| current_file_path.to_path_buf());
        let Some(file_blocks) = context.blocks.get(&file) else {
            continue;
        };
        for block_with_context in &file_blocks.blocks_with_context {
            if block_with_context.block.name() == Some(name.as_str()) {
                result.push(AffectedBlock {
                    file: file.clone(),
                    name: name.clone(),
                    content: block_with_context
                        .block
                        .content(&file_blocks.file_content)
                        .trim()
                        .to_string(),
                });
            }
        }
    }
    Ok(result)
}

fn block_content<'c>(
    block_with_context: &BlockWithContext,
    file_content: &'c str,
) -> anyhow::Result<&'c str> {
    let content = if let Some(pattern) =
        block_with_context.block.attributes.get("check-lua-pattern")
    {
        let re = regex::Regex::new(pattern).context("check-lua-pattern is not a valid regex")?;
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

pub(crate) struct CheckLuaValidatorDetector;

impl CheckLuaValidatorDetector {
    pub fn new() -> Self {
        Self
    }
}

impl ValidatorDetector for CheckLuaValidatorDetector {
    fn detect(
        &self,
        block_with_context: &BlockWithContext,
    ) -> anyhow::Result<Option<ValidatorType>> {
        if block_with_context
            .block
            .attributes
            .contains_key("check-lua")
        {
            Ok(Some(ValidatorType::Async(Box::new(
                CheckLuaValidator::new(),
            ))))
        } else {
            Ok(None)
        }
    }
}

#[derive(Serialize)]
struct CheckLuaViolation<'a> {
    script: &'a str,
    lua_error: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{merge_validation_contexts, validation_context};
    use serde_json::json;
    use std::io::Write;

    fn write_temp_lua_script(content: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(content.as_bytes()).unwrap();
        file.flush().unwrap();
        file
    }

    #[tokio::test]
    async fn when_lua_returns_nil_returns_no_violations() -> anyhow::Result<()> {
        let script = write_temp_lua_script(
            r#"
function validate(ctx, content)
    return nil
end
"#,
        );
        let script_path = script.path().to_str().unwrap();
        let context = validation_context(
            "example.py",
            &format!(
                r#"# <block check-lua="{script_path}">
some content
# </block>"#,
            ),
        );
        let validator = CheckLuaValidator::new();

        let violations = validator.validate(context).await?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn when_lua_returns_error_message_returns_violation() -> anyhow::Result<()> {
        let script = write_temp_lua_script(
            r#"
function validate(ctx, content)
    return "block content is invalid"
end
"#,
        );
        let script_path = script.path().to_str().unwrap();
        let context = validation_context(
            "example.py",
            &format!(
                r#"# <block check-lua="{script_path}">
some content
# </block>"#,
            ),
        );
        let validator = CheckLuaValidator::new();

        let violations = validator.validate(context).await?;

        assert_eq!(violations.len(), 1);
        assert_eq!(violations[&PathBuf::from("example.py")].len(), 1);
        let violation = &violations[&PathBuf::from("example.py")][0];
        assert_eq!(violation.code, "check-lua");
        assert_eq!(
            violation.message,
            "Block example.py:(unnamed) defined at line 1 failed Lua check: block content is invalid"
        );
        assert_eq!(
            violation.data,
            Some(json!({
                "script": script_path,
                "lua_error": "block content is invalid"
            }))
        );
        Ok(())
    }

    #[tokio::test]
    async fn empty_script_path_returns_error() -> anyhow::Result<()> {
        let validator = CheckLuaValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block check-lua=" ">
text
# </block>"#,
        );
        let err = validator.validate(context).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("check-lua requires a non-empty script path")
        );
        Ok(())
    }

    #[tokio::test]
    async fn missing_script_file_returns_error() -> anyhow::Result<()> {
        let validator = CheckLuaValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="/nonexistent/path/script.lua">
text
# </block>"#,
        );
        let err = validator.validate(context).await.unwrap_err();
        let err_chain = format!("{err:#}");
        assert!(
            err_chain.contains("failed to read Lua script"),
            "unexpected error: {err_chain}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn pattern_match_is_used_as_block_content() -> anyhow::Result<()> {
        let script = write_temp_lua_script(
            r#"
function validate(ctx, content)
    if content ~= "id: 42" then
        return "expected 'id: 42' but got '" .. content .. "'"
    end
    return nil
end
"#,
        );
        let script_path = script.path().to_str().unwrap();
        let context = validation_context(
            "example.py",
            &format!(
                r#"# <block check-lua="{script_path}" check-lua-pattern="id: \d+">
name: Alice, id: 42
# </block>"#,
            ),
        );
        let validator = CheckLuaValidator::new();
        let violations = validator.validate(context).await?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn pattern_group_match_is_used_as_block_content() -> anyhow::Result<()> {
        let script = write_temp_lua_script(
            r#"
function validate(ctx, content)
    if content ~= "42" then
        return "expected '42' but got '" .. content .. "'"
    end
    return nil
end
"#,
        );
        let script_path = script.path().to_str().unwrap();
        let context = validation_context(
            "example.py",
            &format!(
                r#"# <block check-lua="{script_path}" check-lua-pattern="id: (?P<value>\d+)">
name: Alice, id: 42
# </block>"#,
            ),
        );
        let validator = CheckLuaValidator::new();
        let violations = validator.validate(context).await?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn invalid_pattern_returns_error() -> anyhow::Result<()> {
        let script = write_temp_lua_script(
            r#"
function validate(ctx, content)
    return nil
end
"#,
        );
        let script_path = script.path().to_str().unwrap();
        let context = validation_context(
            "example.py",
            &format!(
                r#"# <block check-lua="{script_path}" check-lua-pattern="[invalid">
some content
# </block>"#,
            ),
        );
        let validator = CheckLuaValidator::new();
        let err = validator.validate(context).await.unwrap_err();
        let err_chain = format!("{err:#}");
        assert!(
            err_chain.contains("check-lua-pattern is not a valid regex"),
            "unexpected error: {err_chain}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn ctx_fields_are_accessible() -> anyhow::Result<()> {
        let script = write_temp_lua_script(
            r#"
function validate(ctx, content)
    if ctx.file ~= "example.py" then
        return "ctx.file is not 'example.py'"
    end
    if ctx.line ~= 1 then
        return "ctx.line is not 1"
    end
    if ctx.attrs == nil then
        return "ctx.attrs is nil"
    end
    if ctx.attrs["check-lua"] == nil then
        return "ctx.attrs['check-lua'] is nil"
    end
    return nil
end
"#,
        );
        let script_path = script.path().to_str().unwrap();
        let context = validation_context(
            "example.py",
            &format!(
                r#"# <block check-lua="{script_path}">
some content
# </block>"#,
            ),
        );
        let validator = CheckLuaValidator::new();
        let violations = validator.validate(context).await?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn ctx_affects_exposes_affected_blocks() -> anyhow::Result<()> {
        let script = write_temp_lua_script(
            r#"
function validate(ctx, content)
    if ctx.affects == nil then
        return "ctx.affects is nil"
    end
    if #ctx.affects ~= 2 then
        return "expected 2 affected blocks, got " .. tostring(#ctx.affects)
    end
    if ctx.affects[1].file ~= "example.py" then
        return "ctx.affects[1].file is '" .. tostring(ctx.affects[1].file) .. "'"
    end
    if ctx.affects[1].name ~= "local-block" then
        return "ctx.affects[1].name is '" .. tostring(ctx.affects[1].name) .. "'"
    end
    if ctx.affects[1].content ~= "local content" then
        return "ctx.affects[1].content is '" .. tostring(ctx.affects[1].content) .. "'"
    end
    if ctx.affects[2].file ~= "other.py" then
        return "ctx.affects[2].file is '" .. tostring(ctx.affects[2].file) .. "'"
    end
    if ctx.affects[2].name ~= "remote-block" then
        return "ctx.affects[2].name is '" .. tostring(ctx.affects[2].name) .. "'"
    end
    if ctx.affects[2].content ~= "remote content" then
        return "ctx.affects[2].content is '" .. tostring(ctx.affects[2].content) .. "'"
    end
    return nil
end
"#,
        );
        let script_path = script.path().to_str().unwrap();
        let context = merge_validation_contexts(vec![
            validation_context(
                "example.py",
                &format!(
                    r#"# <block check-lua="{script_path}" affects=":local-block, other.py:remote-block">
some content
# </block>

# <block name="local-block">
local content
# </block>"#,
                ),
            ),
            validation_context(
                "other.py",
                r#"# <block name="remote-block">
remote content
# </block>"#,
            ),
        ]);
        let validator = CheckLuaValidator::new();
        let violations = validator.validate(context).await?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn ctx_affects_skips_unresolved_references() -> anyhow::Result<()> {
        let script = write_temp_lua_script(
            r#"
function validate(ctx, content)
    if ctx.affects == nil then
        return "ctx.affects is nil"
    end
    if #ctx.affects ~= 0 then
        return "expected 0 affected blocks, got " .. tostring(#ctx.affects)
    end
    return nil
end
"#,
        );
        let script_path = script.path().to_str().unwrap();
        let context = validation_context(
            "example.py",
            &format!(
                r#"# <block check-lua="{script_path}" affects=":does-not-exist">
some content
# </block>"#,
            ),
        );
        let validator = CheckLuaValidator::new();
        let violations = validator.validate(context).await?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn ctx_affects_is_nil_without_affects_attribute() -> anyhow::Result<()> {
        let script = write_temp_lua_script(
            r#"
function validate(ctx, content)
    if ctx.affects ~= nil then
        return "ctx.affects should be nil"
    end
    return nil
end
"#,
        );
        let script_path = script.path().to_str().unwrap();
        let context = validation_context(
            "example.py",
            &format!(
                r#"# <block check-lua="{script_path}">
some content
# </block>"#,
            ),
        );
        let validator = CheckLuaValidator::new();
        let violations = validator.validate(context).await?;
        assert!(violations.is_empty());
        Ok(())
    }
}
