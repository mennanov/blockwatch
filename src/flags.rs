use crate::validators;
use anyhow::Context;
use clap::{Parser, builder::ValueParser, crate_version};
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;

/// How much a run reports about what it checked.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Verbosity {
    /// Print no report.
    #[default]
    None,
    /// Print a one-line summary of the run.
    Summary,
    /// Print a JSON report of every block in scope and the validators that checked it.
    Full,
}

impl std::fmt::Display for Verbosity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        clap::ValueEnum::to_possible_value(self)
            .expect("every variant has a value")
            .get_name()
            .fmt(formatter)
    }
}

/// The parsed command line.
///
/// Fields hold the raw strings clap collected; the accessors below turn them into the compiled
/// glob sets, extension map and validator filters the rest of the program consumes. Flags are
/// `global` so they may be written before or after a subcommand.
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
    blockwatch -e keep-sorted -e line-count

    # List all found blocks
    blockwatch list 'src/**/*.rs'

    # List blocks and mark those touched by a diff (reads stdin)
    git diff --patch | blockwatch list --diff",
)]
pub struct Args {
    // <block affects="docs/cli.md:cli-docs">
    /// Additional file extension mappings, e.g. -E c++=cpp -E cxx=cpp
    #[arg(
        short = 'E',
        long = "extension",
        value_name = "KEY=VALUE",
        action = clap::ArgAction::Append,
        value_parser = ValueParser::new(parse_extensions),
        global = true,
    )]
    extensions: Vec<(String, String)>,

    /// Disable a validator, e.g. -d check-ai -d line-count
    #[arg(
        short = 'd',
        long = "disable",
        value_name = "VALIDATOR",
        action = clap::ArgAction::Append,
        value_parser = ValueParser::new(parse_validator),
        global = true,
    )]
    disabled_validators: Vec<String>,

    /// Enable a validator, e.g. -e check-ai -e line-count
    #[arg(
        short = 'e',
        long = "enable",
        value_name = "VALIDATOR",
        action = clap::ArgAction::Append,
        value_parser = ValueParser::new(parse_validator),
        global = true,
    )]
    enabled_validators: Vec<String>,

    /// Glob patterns to ignore files.
    #[arg(
        long = "ignore",
        value_name = "GLOBS",
        action = clap::ArgAction::Append,
        global = true,
    )]
    pub ignore: Vec<String>,

    /// How much to report about what the run checked. Printed to stdout.
    #[arg(
        long = "verbosity",
        value_name = "LEVEL",
        value_enum,
        default_value_t = Verbosity::None,
        global = true,
    )]
    pub verbosity: Verbosity,

    /// Glob patterns to filter files.
    #[arg(value_name = "GLOBS")]
    pub globs: Vec<String>,

    /// The subcommand to run, if any. `None` means the default action: validate.
    #[command(subcommand)]
    pub command: Option<SubCommand>,
    // </block>
}

/// A mode that inspects blocks instead of validating them.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum SubCommand {
    /// List all blocks found in the scanned files.
    List {
        /// Read a unified diff from stdin to populate `is_content_modified`.
        /// Without this flag, `list` never reads stdin.
        #[arg(long)]
        diff: bool,

        #[arg(value_name = "GLOBS")]
        globs: Vec<String>,
    },
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
    pub fn globs(&self) -> anyhow::Result<GlobSet> {
        let mut builder = GlobSetBuilder::new();
        let mut globs = self.globs.clone();
        if let Some(SubCommand::List {
            globs: list_globs, ..
        }) = &self.command
        {
            globs.extend(list_globs.clone());
        }

        for glob_str in &globs {
            let glob = Glob::new(glob_str)
                .with_context(|| format!("Invalid glob pattern: {}", glob_str))?;
            builder.add(glob);
        }
        builder.build().context("Failed to build glob set")
    }

    /// Returns a compiled GlobSet from the provided ignore glob patterns.
    pub fn ignored_globs(&self) -> anyhow::Result<GlobSet> {
        let mut builder = GlobSetBuilder::new();
        for glob_str in &self.ignore {
            let glob = Glob::new(glob_str)
                .with_context(|| format!("Invalid ignore glob pattern: {}", glob_str))?;
            builder.add(glob);
        }
        builder.build().context("Failed to build ignore glob set")
    }

    /// Validates all arguments.
    pub fn validate(&self, supported_extensions: &HashSet<&OsString>) -> anyhow::Result<()> {
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
        // `list` already prints JSON to stdout. Two JSON documents on one stream cannot be parsed.
        if self.command.is_some() && self.verbosity != Verbosity::None {
            anyhow::bail!(
                "--verbosity is not supported by the `list` subcommand; `list` already reports \
                 every block it found"
            );
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
    let validators: Vec<&str> = validators::detector_factories::<crate::fs::FileSystemImpl>()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(argv: &[&str]) -> anyhow::Result<Args> {
        Ok(Args::try_parse_from(argv)?)
    }

    #[test]
    fn verbosity_is_rejected_with_the_list_subcommand() -> anyhow::Result<()> {
        let args = parse(&["blockwatch", "list", "--verbosity", "full"])?;
        let error = args
            .validate(&HashSet::new())
            .expect_err("--verbosity must not be accepted alongside `list`");
        assert!(
            error.to_string().contains("`list` subcommand"),
            "unexpected error: {error}"
        );
        Ok(())
    }
}
