use crate::validators;
use anyhow::Context;
use clap::{Parser, builder::ValueParser, crate_version};
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::Path;

#[derive(Parser, Debug)]
#[command(
    author,
    version = crate_version!(),
    about = "Validate interdependent code/doc blocks in diffs to prevent drift.",
    long_about = r"Blockwatch reads a unified git diff from stdin and validates that named blocks, sorted segments, and other constraints remain consistent across files. It is designed for use in pre-commit hooks and CI. Pipe `git diff --patch` to blockwatch.",
    after_help = r"EXAMPLES:
    # Filter files using glob patterns
    blockwatch 'src/**/*.rs'

    # Ignore files using glob patterns
    blockwatch 'src/**/*.rs' --ignore '**/generated/**'
    
    # Filter files with the diff input
    git diff --patch | blockwatch 'src/**/*.rs'

    # Validate current unstaged changes
    git diff --patch | blockwatch

    # Validate staged changes only
    git diff --cached --patch | blockwatch

    # With zero context for tighter diffs (recommended for hooks)
    git diff --patch --unified=0 | blockwatch

    # Provide extra extension mappings (map unknown extensions to supported grammars)
    blockwatch -E cxx=cpp -E c++=cpp

    # Disable specific validators
    blockwatch -d keep-sorted -d line-count

    # Enable specific validators only
    blockwatch -e keep-sorted -e line-count",
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

    /// Glob patterns to ignore files.
    #[arg(
        long = "ignore",
        value_name = "GLOBS",
        action = clap::ArgAction::Append,
    )]
    pub ignore: Vec<String>,

    /// Glob patterns to filter files.
    #[arg(value_name = "GLOBS")]
    pub globs: Vec<String>,
    // </block>
}

impl Args {
    /// Returns a map of user-provided extension remappings: KEY -> VALUE.
    pub fn extensions(&self) -> HashMap<OsString, OsString> {
        self.extensions
            .iter()
            .map(|(key, val)| (OsString::from(key), OsString::from(val)))
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

    /// Returns a compiled GlobSet from the provided glob patterns.
    pub fn globs(&self, root_path: &Path) -> anyhow::Result<GlobSet> {
        let mut builder = GlobSetBuilder::new();
        for glob_str in &self.globs {
            let path = root_path.join(glob_str);
            let glob = Glob::new(
                path.to_str()
                    .context(format!("Invalid path: {}", path.display()))?,
            )
            .with_context(|| format!("Invalid glob pattern: {}", path.display()))?;
            builder.add(glob);
        }
        builder.build().context("Failed to build glob set")
    }

    /// Returns a compiled GlobSet from the provided ignore glob patterns.
    pub fn ignored_globs(&self, root_path: &Path) -> anyhow::Result<GlobSet> {
        let mut builder = GlobSetBuilder::new();
        for glob_str in &self.ignore {
            let path = root_path.join(glob_str);
            let glob = Glob::new(
                path.to_str()
                    .context(format!("Invalid ignore path: {}", path.display()))?,
            )
            .with_context(|| format!("Invalid ignore glob pattern: {}", path.display()))?;
            builder.add(glob);
        }
        builder.build().context("Failed to build ignore glob set")
    }

    /// Validates all arguments.
    pub fn validate(&self, supported_extensions: HashSet<OsString>) -> anyhow::Result<()> {
        // Check custom extensions.
        for (key, val) in &self.extensions {
            if !supported_extensions.contains(&OsString::from(val)) {
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
