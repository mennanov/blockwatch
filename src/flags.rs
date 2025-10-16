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

    # Disable specific validators
    git diff --patch | blockwatch -d keep-sorted -d line-count

    # With zero context for tighter diffs (recommended for hooks)
    git diff --patch --unified=0 | blockwatch",
)]
pub struct Args {
    // <block affects="README.md:cli-docs">
    /// Additional file extension mappings, e.g. -E c++=cpp -E cxx=cpp
    #[arg(
        short = 'E',
        long = "extension",
        value_name = "KEY=VALUE",
        action = clap::ArgAction::Append,
        value_parser = ValueParser::new(parse_extensions),
    )]
    extensions: Vec<(String, String)>,

    /// Disable a validator, e.g. -d check-ai -d line-count
    #[arg(
        short = 'd',
        long = "disable",
        value_name = "VALIDATOR",
        action = clap::ArgAction::Append,
        value_parser = ValueParser::new(parse_validator),
    )]
    disabled_validators: Vec<String>,

    /// Enable a validator, e.g. -e check-ai -e line-count
    #[arg(
        short = 'e',
        long = "enable",
        value_name = "VALIDATOR",
        action = clap::ArgAction::Append,
        value_parser = ValueParser::new(parse_validator),
    )]
    enabled_validators: Vec<String>,
    // </block>
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

    /// Enabled validator names.
    pub fn enabled_validators(&self) -> HashSet<&str> {
        self.enabled_validators.iter().map(AsRef::as_ref).collect()
    }

    /// Validates all arguments.
    pub fn validate(&self, supported_extensions: HashSet<String>) -> anyhow::Result<()> {
        // Check custom extensions.
        for (key, val) in &self.extensions {
            if !supported_extensions.contains(val) {
                anyhow::bail!("Unsupported extension mapping: {key}={val}");
            }
        }
        // Check that "--enable" and "--disable" flags are not used together.
        if !self.disabled_validators.is_empty() && !self.enabled_validators.is_empty() {
            anyhow::bail!("--enable and --disable flags must not be set at the same time");
        }

        Ok(())
    }
}

fn parse_extensions(s: &str) -> anyhow::Result<(String, String)> {
    s.split_once('=')
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .with_context(|| format!("Invalid KEY=VALUE format: {s}"))
}

fn parse_validator(value: &str) -> anyhow::Result<String> {
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
