use crate::validators;
use anyhow::Context;
use clap::{Parser, builder::ValueParser, crate_version};
use std::collections::{HashMap, HashSet};

#[derive(Parser, Debug)]
#[command(
    author,
    version = crate_version!(),
    about = "Validate interdependent code/doc blocks in diffs to prevent drift.",
    long_about = r"Blockwatch reads a unified git diff from stdin and validates that named blocks, sorted segments, and other constraints remain consistent across files. It is designed for use in pre-commit hooks and CI. Pipe `git diff --patch` to blockwatch.",
    after_help = r"EXAMPLES:
    # Validate current unstaged changes
    git diff --patch | blockwatch

    # Validate staged changes only
    git diff --cached --patch | blockwatch

    # Provide extra extension mappings (map unknown extensions to supported grammars)
    git diff --patch | blockwatch -E cxx=cpp -E c++=cpp

    # With zero context for tighter diffs (recommended for hooks)
    git diff --patch --unified=0 | blockwatch",
)]
pub struct Args {
    /// Additional file extension mappings, e.g. -E c++=cpp -E cxx=cpp
    #[arg(
        short = 'E',
        long = "extension",
        value_name = "KEY=VALUE",
        action = clap::ArgAction::Append,
        value_parser = ValueParser::new(parse_extensions),
    )]
    extensions: Vec<(String, String)>,

    /// Disable a validator, e.g. -D check-ai -D line-count
    #[arg(
        short = 'D',
        long = "disable",
        value_name = "VALIDATOR",
        action = clap::ArgAction::Append,
        value_parser = ValueParser::new(parse_disable),
    )]
    disabled_validators: Vec<String>,
}

impl Args {
    /// Returns a map of user-provided extension remappings: KEY -> VALUE.
    pub fn extensions(&self) -> HashMap<String, String> {
        self.extensions
            .iter()
            .map(|(key, val)| (key.clone(), val.clone()))
            .collect()
    }

    /// Disabled validator names.
    pub fn disabled_validators(&self) -> HashSet<&str> {
        self.disabled_validators.iter().map(AsRef::as_ref).collect()
    }

    /// Validates that all user-provided extension values are supported by available parsers.
    pub fn validate(&self, supported_extensions: HashSet<String>) -> anyhow::Result<()> {
        for (key, val) in &self.extensions {
            if !supported_extensions.contains(val) {
                anyhow::bail!("Unsupported extension mapping: {key}={val}");
            }
        }

        Ok(())
    }
}

fn parse_extensions(s: &str) -> anyhow::Result<(String, String)> {
    s.split_once('=')
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .with_context(|| format!("Invalid KEY=VALUE format: {s}"))
}

fn parse_disable(value: &str) -> anyhow::Result<String> {
    let validators: Vec<&str> = validators::DETECTOR_FACTORIES
        .iter()
        .map(|(validator_name, _)| *validator_name)
        .collect();

    validators
        .contains(&value)
        .then(|| value.trim().to_string())
        .with_context(|| {
            format!(
                "Unknown validator: {value}. Available validators: {}",
                validators.join(", ")
            )
        })
}
