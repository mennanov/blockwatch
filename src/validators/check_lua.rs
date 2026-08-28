use crate::blocks::{Block, BlockWithContext};
use crate::fs::FileSystem;
use crate::repo_path::RepoPath;
use crate::validators::parse_block_references;
use crate::validators::{
    BlockReference, PatternContent, ValidationContext, ValidationReport, ValidatorAsync,
    ValidatorDetector, ValidatorType, Violation, ViolationRange, block_content_for_pattern,
};
use anyhow::{Context, anyhow};
use async_trait::async_trait;
use mlua::{HookTriggers, Lua, StdLib, VmState};
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::{JoinSet, spawn_blocking};

const LUA_STDLIB_ENV_VAR: &str = "BLOCKWATCH_LUA_MODE";

/// The wall-clock budget a `check-lua` script gets when the block does not set `check-lua-timeout`.
const DEFAULT_CHECK_LUA_TIMEOUT_SECS: u64 = 30;

/// How many Lua VM instructions run between two checks of the timeout deadline. Small enough that a
/// runaway script is stopped promptly, large enough that the hook stays negligible for scripts that
/// terminate on their own.
const CHECK_LUA_INSTRUCTION_HOOK_INTERVAL: u32 = 1_000;

/// Extra wall-clock time the out-of-VM backstop waits beyond the configured budget. The instruction
/// hook stops any script it can reach right at the budget; this window lets that graceful, in-VM
/// stop win the race, so the backstop only ever fires for a script wedged in a native call the hook
/// cannot interrupt (a blocking system call, say).
const CHECK_LUA_TIMEOUT_BACKSTOP_GRACE: Duration = Duration::from_secs(1);

/// Returns the Lua standard library set based on the `BLOCKWATCH_LUA_MODE` environment variable.
///
/// - `sandboxed` (default): Most restrictive, blocks file/OS access.
/// - `safe`: Memory-safe but includes IO/OS (useful for trusted scripts).
/// - `unsafe`: Fully unsafe, allows C module loading.
fn lua_from_env() -> Lua {
    // <block affects="docs/validators/check-lua.md:lua-safety-modes">
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

/// Enforces `check-lua="path/to/script.lua"`: runs a user-supplied Lua script over the block's
/// content, for project-specific rules the built-in validators cannot express.
///
/// Needs a filesystem to read the script, which is resolved inside the repository like any other
/// referenced file. How much of the Lua standard library the script may use is set by
/// `BLOCKWATCH_LUA_MODE`.
pub(crate) struct CheckLuaValidator<Fs: FileSystem> {
    file_system: Arc<Fs>,
}

impl<Fs: FileSystem + 'static> CheckLuaValidator<Fs> {
    /// Creates the validator over the filesystem it will read scripts from.
    pub(super) fn new(file_system: Arc<Fs>) -> Self {
        Self { file_system }
    }
}

#[async_trait]
impl<Fs: FileSystem + 'static> ValidatorAsync for CheckLuaValidator<Fs> {
    async fn validate(&self, context: Arc<ValidationContext>) -> anyhow::Result<ValidationReport> {
        let mut report = ValidationReport::default();
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

                // The block is checked from here on, whatever the script returns, so add it
                // before the borrowed path is shadowed by the owned copy the task takes.
                report.add_checked_block(file_path, &block_with_context.block);

                let context = Arc::clone(&context);
                let file_path = file_path.clone();
                let file_system = Arc::clone(&self.file_system);
                tasks.spawn(async move {
                    let file_blocks = &context.blocks[&file_path];
                    let block_with_context = &file_blocks.blocks_with_context[block_idx];
                    let script_path = &block_with_context.block.attributes["check-lua"];
                    let content = match block_content_for_pattern(
                        block_with_context,
                        &file_blocks.file_content,
                        "check-lua-pattern",
                    )? {
                        PatternContent::Whole(content) => LuaContent::Text(content.to_string()),
                        PatternContent::Matches(matches) => {
                            LuaContent::Matches(matches.into_iter().map(str::to_string).collect())
                        }
                    };
                    let affected_blocks = resolve_affected_blocks(
                        &context,
                        file_system.as_ref(),
                        &file_path,
                        &block_with_context.block,
                    )?;

                    let result = run_lua_script(
                        script_path,
                        file_system.as_ref(),
                        &file_path,
                        block_with_context,
                        content,
                        &affected_blocks,
                    )
                    .await;

                    let block_violations = match result.context(format!(
                        "check-lua script error in {}:{} at line {}",
                        file_path.display(),
                        block_with_context.block.name_display(),
                        block_with_context
                            .block
                            .start_tag_position_range
                            .start()
                            .line
                    ))? {
                        None => Vec::new(),
                        Some(msg) => vec![create_violation(
                            &file_path,
                            &block_with_context.block,
                            script_path,
                            &msg,
                        )?],
                    };
                    anyhow::Ok((file_path, block_violations))
                });
            }
        }
        while let Some(task_result) = tasks.join_next().await {
            let (file_path, violations) = task_result.context("check-lua task failed")??;
            report.add_violations(&file_path, violations);
        }
        Ok(report)
    }
}

async fn run_lua_script<Fs: FileSystem>(
    script_path: &str,
    file_system: &Fs,
    file_path: &RepoPath,
    block_with_context: &BlockWithContext,
    content: LuaContent,
    affected_blocks: &[AffectedBlock],
) -> anyhow::Result<Option<String>> {
    let timeout = parse_check_lua_timeout(&block_with_context.block)?;
    let script_content = file_system
        .read_to_string(Path::new(script_path))
        .with_context(|| format!("failed to read Lua script: {script_path}"))?;

    let inputs = LuaScriptInputs::new(
        script_path,
        script_content,
        timeout,
        file_path,
        block_with_context,
        content,
        affected_blocks,
    );

    // Run the Lua script in a blocking thread for CPU-heavy scripts that never yield control back.
    // I/O-heavy scripts won't invoke the hook, in this case the Tokio's timeout will fire.
    let worker = spawn_blocking(move || run_lua_script_sync(inputs));
    match tokio::time::timeout(timeout + CHECK_LUA_TIMEOUT_BACKSTOP_GRACE, worker).await {
        Ok(worker_result) => worker_result.context("the check-lua worker thread panicked")?,
        Err(_elapsed) => Err(anyhow!(timeout_error_message(timeout))),
    }
}

/// All the context needed to run the Lua script.
///
/// See [`run_lua_script`] for why the work runs off the async thread.
struct LuaScriptInputs {
    script_path: String,
    script_content: String,
    timeout: Duration,
    file: String,
    line: usize,
    attributes: Vec<(String, String)>,
    /// `Some` exactly when the block carries an `affects` attribute, so `ctx.affects` is present in
    /// the script precisely when the attribute is.
    affects: Option<Vec<LuaAffected>>,
    content: LuaContent,
}

/// The `content` argument handed to the `validate()` function in the Lua script.
enum LuaContent {
    /// The block's whole trimmed content, passed to the script as a string.
    Text(String),
    /// The values `check-lua-pattern` extracted, passed to the script as a 1-based array.
    Matches(Vec<String>),
}

/// One affected target exposed to a script as an entry of `ctx.affects`.
struct LuaAffected {
    file: String,
    /// `None` for a whole-file reference; the entry's `name` key is then absent, so a script sees
    /// `nil` and can distinguish a whole file from a named block.
    name: Option<String>,
    content: String,
}

impl LuaScriptInputs {
    /// Copies out of the borrowed validation context everything the script needs, so the result can
    /// outlive the borrow and move onto the blocking thread.
    fn new(
        script_path: &str,
        script_content: String,
        timeout: Duration,
        file_path: &RepoPath,
        block_with_context: &BlockWithContext,
        content: LuaContent,
        affected_blocks: &[AffectedBlock],
    ) -> Self {
        let block = &block_with_context.block;
        let affects = block.attributes.contains_key("affects").then(|| {
            affected_blocks
                .iter()
                .map(|affected| LuaAffected {
                    file: affected.file.as_str().to_string(),
                    name: affected.name.clone(),
                    content: affected.content.clone(),
                })
                .collect()
        });
        Self {
            script_path: script_path.to_string(),
            script_content,
            timeout,
            file: file_path.as_str().to_string(),
            line: block.start_tag_position_range.start().line,
            attributes: block
                .attributes
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            affects,
            content,
        }
    }
}

/// Runs the script synchronously so its whole Lua lifetime stays on one thread, which lets
/// [`run_lua_script`] host it on a blocking thread and bound it with a wall-clock timeout.
fn run_lua_script_sync(inputs: LuaScriptInputs) -> anyhow::Result<Option<String>> {
    let lua = lua_from_env();
    // Install the hook that stops the script from inside the VM once `timeout` elapses.
    // Needed for the CPU-heavy Lua scripts that never yield control back to the runtime.
    install_timeout_hook(&lua, inputs.timeout)?;

    lua.load(lua_chunk(&inputs.script_content))
        .exec()
        .with_context(|| format!("failed to execute Lua script: {}", inputs.script_path))?;

    let validate_fn: mlua::Function = lua
        .globals()
        .get("validate")
        .context("Lua script must define a global 'validate' function")?;

    let ctx_table = lua.create_table().context("failed to create ctx table")?;
    ctx_table
        .set("file", inputs.file.as_str())
        .context("failed to set ctx.file")?;
    ctx_table
        .set("line", inputs.line)
        .context("failed to set ctx.line")?;

    let attrs_table = lua.create_table().context("failed to create attrs table")?;
    for (key, value) in &inputs.attributes {
        attrs_table
            .set(key.as_str(), value.as_str())
            .with_context(|| format!("failed to set attr {key}"))?;
    }
    ctx_table
        .set("attrs", attrs_table)
        .context("failed to set ctx.attrs")?;

    // When the block carries an `affects` attribute, expose the affected blocks as
    // `ctx.affects = [{ file, name, content }, …]` so scripts can inspect them in sandboxed mode.
    if let Some(affected_blocks) = &inputs.affects {
        let affects_table = lua
            .create_table()
            .context("failed to create affects table")?;
        for (i, affected) in affected_blocks.iter().enumerate() {
            let entry = lua
                .create_table()
                .context("failed to create affects entry table")?;
            entry
                .set("file", affected.file.as_str())
                .context("failed to set ctx.affects[].file")?;
            if let Some(name) = &affected.name {
                entry
                    .set("name", name.as_str())
                    .context("failed to set ctx.affects[].name")?;
            }
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

    let content: mlua::Value = match &inputs.content {
        LuaContent::Text(text) => mlua::Value::String(
            lua.create_string(text.as_str())
                .context("failed to build the content string")?,
        ),
        LuaContent::Matches(matches) => mlua::Value::Table(
            lua.create_sequence_from(matches.iter().map(String::as_str))
                .context("failed to build the content array")?,
        ),
    };

    let result: mlua::Value = validate_fn
        .call((ctx_table, content))
        .with_context(|| format!("failed to call validate() in {}", inputs.script_path))?;

    match result {
        mlua::Value::Nil => Ok(None),
        mlua::Value::String(s) => Ok(Some(s.to_str()?.to_string())),
        other => Err(anyhow!(
            "validate() must return nil or a string, got: {:?}",
            other.type_name()
        )),
    }
}

/// Reads the block's `check-lua-timeout` as a wall-clock budget in whole seconds, defaulting to
/// [`DEFAULT_CHECK_LUA_TIMEOUT_SECS`] when it is absent.
///
/// A value below one second is rejected: a timeout that short cannot tell a runaway script from a
/// slow-but-terminating one, which is the false-violation trap the timeout exists to avoid.
fn parse_check_lua_timeout(block: &Block) -> anyhow::Result<Duration> {
    let Some(raw) = block.attributes.get("check-lua-timeout") else {
        return Ok(Duration::from_secs(DEFAULT_CHECK_LUA_TIMEOUT_SECS));
    };
    let seconds: u64 = raw
        .trim()
        .parse()
        .ok()
        .filter(|&seconds| seconds >= 1)
        .ok_or_else(|| {
            anyhow!("check-lua-timeout must be a whole number of seconds >= 1, got {raw:?}")
        })?;
    Ok(Duration::from_secs(seconds))
}

/// The message both timeout layers report, so a script stopped inside the VM by the hook and one
/// stopped from outside by the wall-clock backstop read identically. `timeout` is the configured
/// budget, not the slightly longer window the backstop actually waits.
fn timeout_error_message(timeout: Duration) -> String {
    let seconds = timeout.as_secs();
    format!(
        "check-lua script timed out after {seconds} second{}",
        if seconds == 1 { "" } else { "s" }
    )
}

/// Installs a hook that stops the script from inside the VM once `timeout` elapses.
fn install_timeout_hook(lua: &Lua, timeout: Duration) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    lua.set_global_hook(
        HookTriggers::new().every_nth_instruction(CHECK_LUA_INSTRUCTION_HOOK_INTERVAL),
        move |_lua, _debug| {
            if Instant::now() >= deadline {
                Err(mlua::Error::runtime(timeout_error_message(timeout)))
            } else {
                Ok(VmState::Continue)
            }
        },
    )
    .context("failed to install the check-lua timeout hook")
}

/// Returns the part of a Lua script that is an actual chunk, skipping a UTF-8 BOM and a leading
/// `#!` line.
fn lua_chunk(script: &str) -> &str {
    let script = script.strip_prefix('\u{feff}').unwrap_or(script);
    if !script.starts_with('#') {
        return script;
    }
    // The shebang line is emptied rather than dropped, so the line numbers Lua reports in syntax
    // and runtime errors keep matching the lines of the file on disk.
    match script.find('\n') {
        Some(line_end) => &script[line_end..],
        None => "",
    }
}

fn create_violation(
    file_path: &RepoPath,
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

/// A target referenced by the validated block's `affects` attribute, exposed to Lua scripts.
struct AffectedBlock {
    file: RepoPath,
    /// `None` for a whole-file reference, which names no block.
    name: Option<String>,
    content: String,
}

/// Resolves the targets referenced by the `affects` attribute of `block` to their `(file, name,
/// content)` so they can be exposed to the Lua script.
///
/// References that don't resolve are skipped (the `affects` validator is responsible for reporting
/// those). The content is trimmed to mirror how the validated block's own content is presented.
fn resolve_affected_blocks<Fs: FileSystem>(
    context: &ValidationContext,
    file_system: &Fs,
    current_file_path: &RepoPath,
    block: &Block,
) -> anyhow::Result<Vec<AffectedBlock>> {
    let mut result = Vec::new();
    let Some(affects) = block.attributes.get("affects") else {
        return Ok(result);
    };
    let references = parse_block_references(affects).with_context(|| {
        format!(
            "invalid affects reference on block {}:{} at line {}",
            current_file_path,
            block.name_display(),
            block.start_tag_position_range.start().line,
        )
    })?;
    for reference in references {
        match reference {
            BlockReference::Block { file, name } => {
                let file = file.unwrap_or_else(|| current_file_path.clone());
                let Some(file_blocks) = context.blocks.get(&file) else {
                    continue;
                };
                for block_with_context in &file_blocks.blocks_with_context {
                    if block_with_context.block.name() == Some(name.as_str()) {
                        result.push(AffectedBlock {
                            file: file.clone(),
                            name: Some(name.clone()),
                            content: block_with_context
                                .block
                                .content(&file_blocks.file_content)
                                .trim()
                                .to_string(),
                        });
                    }
                }
            }
            BlockReference::File(file) => {
                let content = match context.blocks.get(&file) {
                    Some(file_blocks) => file_blocks.file_content.clone(),
                    None => match file_system.read_to_string(file.as_path()) {
                        Ok(content) => content,
                        Err(_) => continue,
                    },
                };
                result.push(AffectedBlock {
                    file,
                    name: None,
                    content: content.trim().to_string(),
                });
            }
        }
    }
    Ok(result)
}

/// Selects [`CheckLuaValidator`] for blocks carrying a `check-lua` attribute.
pub(crate) struct CheckLuaValidatorDetector;

impl CheckLuaValidatorDetector {
    /// Creates the detector. Registered in [`crate::validators::detector_factories`].
    pub fn new() -> Self {
        Self
    }
}

impl<Fs: FileSystem + 'static> ValidatorDetector<Fs> for CheckLuaValidatorDetector {
    fn detect(
        &self,
        block_with_context: &BlockWithContext,
        file_system: &Arc<Fs>,
    ) -> anyhow::Result<Option<ValidatorType>> {
        if block_with_context
            .block
            .attributes
            .contains_key("check-lua")
        {
            Ok(Some(ValidatorType::Async(Box::new(
                CheckLuaValidator::new(Arc::clone(file_system)),
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
    use crate::fs::test_utils::FakeFileSystem;
    use crate::repo_path::RepoPath;
    use crate::test_utils::{
        checked_lines, merge_validation_contexts, validation_context,
        validation_context_with_changes, violation_count,
    };
    use serde_json::json;

    /// Builds a `CheckLuaValidator` backed by a fake filesystem seeded with `scripts`.
    fn validator(scripts: &[(&str, &str)]) -> CheckLuaValidator<FakeFileSystem> {
        let files = scripts
            .iter()
            .map(|(path, contents)| (path.to_string(), contents.to_string()))
            .collect();
        CheckLuaValidator::new(Arc::new(FakeFileSystem::new(files)))
    }

    #[tokio::test]
    async fn block_whose_script_fails_returns_a_violation() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua">
some content
# </block>"#,
        );

        let violations = validator(&[(
            "check.lua",
            r#"
function validate(ctx, content)
    return "block content is invalid"
end
"#,
        )])
        .validate(context)
        .await?
        .violations;

        assert_eq!(violations.len(), 1);
        assert_eq!(
            violations[&RepoPath::from_reference("example.py")?].len(),
            1
        );
        let violation = &violations[&RepoPath::from_reference("example.py")?][0];
        assert_eq!(violation.code, "check-lua");
        assert_eq!(
            violation.message,
            "Block example.py:(unnamed) defined at line 1 failed Lua check: block content is invalid"
        );
        assert_eq!(
            violation.data,
            Some(json!({
                "script": "check.lua",
                "lua_error": "block content is invalid"
            }))
        );
        Ok(())
    }

    #[tokio::test]
    async fn block_whose_script_passes_returns_no_violations() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua">
some content
# </block>"#,
        );

        let report = validator(&[(
            "check.lua",
            r#"
function validate(ctx, content)
    return nil
end
"#,
        )])
        .validate(context)
        .await?;

        assert!(report.violations.is_empty());
        // The block was checked and passed. That is different from never being checked at all.
        assert_eq!(checked_lines(&report), vec![1]);
        Ok(())
    }

    #[tokio::test]
    async fn block_without_a_check_lua_pattern_passes_the_content_as_a_string() -> anyhow::Result<()>
    {
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua">
name: Alice, id: 42
# </block>"#,
        );

        let violations = validator(&[(
            "check.lua",
            r#"
function validate(ctx, content)
    if content ~= "name: Alice, id: 42" then
        return "expected the whole block as a string, got " .. type(content)
    end
    return nil
end
"#,
        )])
        .validate(context)
        .await?
        .violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn check_lua_pattern_without_a_value_group_passes_the_whole_match() -> anyhow::Result<()>
    {
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua" check-lua-pattern="id: \d+">
name: Alice, id: 42
# </block>"#,
        );

        let violations = validator(&[(
            "check.lua",
            r#"
function validate(ctx, content)
    if content[1] ~= "id: 42" then
        return "expected 'id: 42' but got '" .. tostring(content[1]) .. "'"
    end
    return nil
end
"#,
        )])
        .validate(context)
        .await?
        .violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn check_lua_pattern_with_a_value_group_passes_the_captured_value() -> anyhow::Result<()>
    {
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua" check-lua-pattern="id: (?P<value>\d+)">
name: Alice, id: 42
# </block>"#,
        );

        let violations = validator(&[(
            "check.lua",
            r#"
function validate(ctx, content)
    if content[1] ~= "42" then
        return "expected '42' but got '" .. tostring(content[1]) .. "'"
    end
    return nil
end
"#,
        )])
        .validate(context)
        .await?
        .violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn check_lua_pattern_with_several_matches_passes_all_of_them() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua" check-lua-pattern="id: (?P<value>\d+)">
name: Alice, id: 42
name: Bob, id: 7
# </block>"#,
        );

        // A pattern narrows what the script sees; it must never hide part of the block from it.
        let violations = validator(&[(
            "check.lua",
            r#"
function validate(ctx, content)
    if #content ~= 2 then
        return "expected 2 matches, got " .. #content
    end
    if content[1] ~= "42" or content[2] ~= "7" then
        return "unexpected matches: " .. table.concat(content, ",")
    end
    return nil
end
"#,
        )])
        .validate(context)
        .await?
        .violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn check_lua_pattern_matching_nothing_passes_an_empty_array() -> anyhow::Result<()> {
        // A pattern that matches nothing is not a violation (as in `check-ai`).
        // Lua script is responsible for checking the content.
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua" check-lua-pattern="zzz_no_match">
name: Alice, id: 42
# </block>"#,
        );

        let violations = validator(&[(
            "check.lua",
            r#"
function validate(ctx, content)
    if type(content) ~= "table" then
        return "expected a table, got " .. type(content)
    end
    if #content ~= 0 then
        return "expected no matches, got " .. #content
    end
    return nil
end
"#,
        )])
        .validate(context)
        .await?
        .violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn check_lua_pattern_matching_empty_values_skips_them() -> anyhow::Result<()> {
        // "\d*" produces 6 matches, 4 of which are empty.
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua" check-lua-pattern="\d*">
a1
b22
# </block>"#,
        );

        let violations = validator(&[(
            "check.lua",
            r#"
function validate(ctx, content)
    if #content ~= 2 then
        return "expected 2 matches, got " .. #content
    end
    if content[1] ~= "1" or content[2] ~= "22" then
        return "unexpected matches: " .. table.concat(content, ",")
    end
    return nil
end
"#,
        )])
        .validate(context)
        .await?
        .violations;

        assert!(violations.is_empty(), "{violations:?}");
        Ok(())
    }

    #[tokio::test]
    async fn check_lua_pattern_spanning_several_lines_matches_across_them() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua" check-lua-pattern="(?s)BEGIN(?P<value>.*?)END">
BEGIN
middle
END
# </block>"#,
        );

        let violations = validator(&[(
            "check.lua",
            r#"
function validate(ctx, content)
    if content[1] ~= "\nmiddle\n" then
        return "expected '\nmiddle\n' but got '" .. tostring(content[1]) .. "'"
    end
    return nil
end
"#,
        )])
        .validate(context)
        .await?
        .violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn invalid_check_lua_pattern_returns_an_error() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua" check-lua-pattern="[invalid">
some content
# </block>"#,
        );
        // The invalid pattern fails before the script is read, so no script needs seeding.
        let err = validator(&[]).validate(context).await.unwrap_err();
        let err_chain = format!("{err:#}");
        assert!(
            err_chain.contains("check-lua-pattern is not a valid regex"),
            "unexpected error: {err_chain}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn lua_context_exposes_the_block_fields() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua">
some content
# </block>"#,
        );

        let violations = validator(&[(
            "check.lua",
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
        )])
        .validate(context)
        .await?
        .violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn lua_context_exposes_the_affected_blocks() -> anyhow::Result<()> {
        let script = r#"
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
"#;
        let context = merge_validation_contexts(vec![
            validation_context(
                "example.py",
                r#"# <block check-lua="check.lua" affects=":local-block, other.py:remote-block">
some content
# </block>

# <block name="local-block">
local content
# </block>"#,
            ),
            validation_context(
                "other.py",
                r#"# <block name="remote-block">
remote content
# </block>"#,
            ),
        ]);

        let violations = validator(&[("check.lua", script)])
            .validate(context)
            .await?
            .violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn lua_context_exposes_a_whole_file_affects_target() -> anyhow::Result<()> {
        // A whole-file target carries the file's text and no block name, which is how a script
        // distinguishes it from a named block.
        let script = r#"
function validate(ctx, content)
    if #ctx.affects ~= 1 then
        return "expected 1 affected target, got " .. tostring(#ctx.affects)
    end
    if ctx.affects[1].file ~= "config.json" then
        return "ctx.affects[1].file is '" .. tostring(ctx.affects[1].file) .. "'"
    end
    if ctx.affects[1].name ~= nil then
        return "ctx.affects[1].name is '" .. tostring(ctx.affects[1].name) .. "'"
    end
    if ctx.affects[1].content ~= '{"value": 2}' then
        return "ctx.affects[1].content is '" .. tostring(ctx.affects[1].content) .. "'"
    end
    return nil
end
"#;
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua" affects="config.json">
some content
# </block>"#,
        );

        let violations = validator(&[("check.lua", script), ("config.json", "{\"value\": 2}\n")])
            .validate(context)
            .await?
            .violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn lua_context_skips_unresolved_affects_references() -> anyhow::Result<()> {
        let script = r#"
function validate(ctx, content)
    if ctx.affects == nil then
        return "ctx.affects is nil"
    end
    if #ctx.affects ~= 0 then
        return "expected 0 affected blocks, got " .. tostring(#ctx.affects)
    end
    return nil
end
"#;
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua" affects=":does-not-exist">
some content
# </block>"#,
        );

        let violations = validator(&[("check.lua", script)])
            .validate(context)
            .await?
            .violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn lua_context_skips_an_unreadable_whole_file_affects_target() -> anyhow::Result<()> {
        // Reporting a missing target is the `affects` validator's job, not the script's.
        let script = r#"
function validate(ctx, content)
    if #ctx.affects ~= 0 then
        return "expected 0 affected targets, got " .. tostring(#ctx.affects)
    end
    return nil
end
"#;
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua" affects="gone.json">
some content
# </block>"#,
        );

        let violations = validator(&[("check.lua", script)])
            .validate(context)
            .await?
            .violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn lua_context_excludes_affected_blocks_absent_from_the_diff() -> anyhow::Result<()> {
        // The affected block lives in a file that has no diff changes, so it is filtered out of the
        // validation context entirely. It must therefore NOT appear in ctx.affects.
        let script = r#"
function validate(ctx, content)
    if ctx.affects == nil then
        return "ctx.affects is nil"
    end
    if #ctx.affects ~= 0 then
        return "expected 0 affected blocks, got " .. tostring(#ctx.affects)
    end
    return nil
end
"#;
        let context = merge_validation_contexts(vec![
            validation_context(
                "example.py",
                r#"# <block check-lua="check.lua" affects="other.py:remote-block">
some content
# </block>"#,
            ),
            validation_context_with_changes(
                "other.py",
                r#"# <block name="remote-block">
remote content
# </block>"#,
                vec![], // No changes: this block is absent from the diff.
            ),
        ]);

        let violations = validator(&[("check.lua", script)])
            .validate(context)
            .await?
            .violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn block_without_an_affects_attribute_has_no_lua_context_affects() -> anyhow::Result<()> {
        let script = r#"
function validate(ctx, content)
    if ctx.affects ~= nil then
        return "ctx.affects should be nil"
    end
    return nil
end
"#;
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua">
some content
# </block>"#,
        );

        let violations = validator(&[("check.lua", script)])
            .validate(context)
            .await?
            .violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn blocks_with_and_without_check_lua_record_a_check_only_for_the_examined_ones()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block name="passing" check-lua="ok.lua">
some content
# </block>
# <block name="failing" check-lua="fail.lua">
some content
# </block>
# <block name="unrelated">
some content
# </block>"#,
        );

        let report = validator(&[
            (
                "ok.lua",
                r#"
function validate(ctx, content)
    return nil
end
"#,
            ),
            (
                "fail.lua",
                r#"
function validate(ctx, content)
    return "bad content"
end
"#,
            ),
        ])
        .validate(context)
        .await?;

        // Both blocks are recorded whatever the script returns, and the block without a check-lua
        // attribute is not checked, so it records nothing.
        assert_eq!(checked_lines(&report), vec![1, 4]);
        assert_eq!(violation_count(&report), 1);
        Ok(())
    }

    // An endless loop is stopped by the in-VM instruction hook, which needs no help from the
    // runtime: the runaway is interrupted on its own blocking thread, so the flavor is irrelevant.
    #[tokio::test]
    async fn block_exceeding_its_check_lua_timeout_fails_the_run() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="loop.lua" check-lua-timeout="1">
some content
# </block>"#,
        );

        let err = validator(&[(
            "loop.lua",
            r#"
function validate(ctx, content)
    while true do end
end
"#,
        )])
        .validate(context)
        .await
        .unwrap_err();

        let err_chain = format!("{err:#}");
        assert!(
            err_chain.contains("timed out") && err_chain.contains("1 second"),
            "unexpected error: {err_chain}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn block_within_its_check_lua_timeout_passes() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua" check-lua-timeout="5">
some content
# </block>"#,
        );

        let report = validator(&[(
            "check.lua",
            r#"
function validate(ctx, content)
    return nil
end
"#,
        )])
        .validate(context)
        .await?;

        assert!(report.violations.is_empty());
        assert_eq!(checked_lines(&report), vec![1]);
        Ok(())
    }

    #[tokio::test]
    async fn non_numeric_check_lua_timeout_returns_an_error() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua" check-lua-timeout="soon">
some content
# </block>"#,
        );
        // The invalid timeout fails before the script is read, so no script needs seeding.
        let err = validator(&[]).validate(context).await.unwrap_err();
        let err_chain = format!("{err:#}");
        assert!(
            err_chain.contains("check-lua-timeout must be a whole number of seconds"),
            "unexpected error: {err_chain}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn zero_check_lua_timeout_returns_an_error() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua" check-lua-timeout="0">
some content
# </block>"#,
        );
        // Zero is rejected as a value error, not silently treated as an instant deadline.
        let err = validator(&[]).validate(context).await.unwrap_err();
        let err_chain = format!("{err:#}");
        assert!(
            err_chain.contains("check-lua-timeout must be a whole number of seconds"),
            "unexpected error: {err_chain}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn script_with_a_shebang_line_runs() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua">
some content
# </block>"#,
        );

        let report = validator(&[(
            "check.lua",
            r#"#!/usr/bin/env lua
function validate(ctx, content)
    return nil
end
"#,
        )])
        .validate(context)
        .await?;

        assert!(report.violations.is_empty());
        assert_eq!(checked_lines(&report), vec![1]);
        Ok(())
    }

    #[tokio::test]
    async fn script_with_a_utf8_bom_runs() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua">
some content
# </block>"#,
        );

        let report = validator(&[(
            "check.lua",
            "\u{feff}function validate(ctx, content)\n    return nil\nend\n",
        )])
        .validate(context)
        .await?;

        assert!(report.violations.is_empty());
        assert_eq!(checked_lines(&report), vec![1]);
        Ok(())
    }

    #[tokio::test]
    async fn script_with_a_utf8_bom_before_a_shebang_line_runs() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua">
some content
# </block>"#,
        );

        let report = validator(&[(
            "check.lua",
            "\u{feff}#!/usr/bin/env lua\nfunction validate(ctx, content)\n    return nil\nend\n",
        )])
        .validate(context)
        .await?;

        assert!(report.violations.is_empty());
        assert_eq!(checked_lines(&report), vec![1]);
        Ok(())
    }

    #[tokio::test]
    async fn script_with_a_shebang_line_reports_lua_errors_at_their_original_line()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="check.lua">
some content
# </block>"#,
        );

        // The error is on line 3 of the file. Dropping the shebang line instead of blanking it
        // would shift every line the script reports by one.
        let err = validator(&[(
            "check.lua",
            r#"#!/usr/bin/env lua
function validate(ctx, content)
    this is not lua
end
"#,
        )])
        .validate(context)
        .await
        .unwrap_err();

        let err_chain = format!("{err:#}");
        assert!(
            err_chain.contains(":3:"),
            "expected the error to name line 3: {err_chain}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn missing_script_returns_an_error() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block check-lua="missing.lua">
text
# </block>"#,
        );
        // The fake filesystem has no "missing.lua", so the read fails and check-lua wraps the error.
        let err = validator(&[]).validate(context).await.unwrap_err();
        let err_chain = format!("{err:#}");
        assert!(
            err_chain.contains("failed to read Lua script"),
            "unexpected error: {err_chain}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn empty_script_path_returns_an_error() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block check-lua=" ">
text
# </block>"#,
        );
        let err = validator(&[]).validate(context).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("check-lua requires a non-empty script path")
        );
        Ok(())
    }
}
