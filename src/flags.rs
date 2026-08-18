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
    about = "Validate interdependent code/doc blocks to prevent drift.",
    long_about = r"Blockwatch validates that named blocks, sorted segments, and other constraints declared in block tags remain consistent across files. It is designed for use in pre-commit hooks and CI.

By default it scans every file in the repository. Pass --diff to additionally read a unified diff from stdin, which marks the blocks the diff changed; rules that only fire on changed content, such as `affects`, need it. Add --only-changed to narrow the run down to those blocks, which is what a pre-commit hook or a per-pull-request check usually wants.",
    after_help = r"EXAMPLES:
    # Check every block in the repository
    blockwatch

    # Filter files using glob patterns
    blockwatch 'src/**/*.rs' '**/*.md'

    # Ignore files using glob patterns
    blockwatch 'src/**/*.rs' --ignore '**/generated/**'

    # Scan the whole tree, and enforce the rules that need a diff
    git diff --patch | blockwatch --diff

    # Check only the blocks the diff changed (recommended for hooks and CI)
    git diff --patch --unified=0 | blockwatch --diff --only-changed

    # The same, for staged changes only
    git diff --cached --patch --unified=0 | blockwatch --diff --only-changed

    # Narrow a diff-driven run further with glob patterns
    git diff --patch | blockwatch --diff --only-changed 'src/**/*.rs'

    # Provide extra extension mappings (map unknown extensions to supported grammars)
    blockwatch -E cxx=cpp -E c++=cpp

    # Disable specific validators
    blockwatch -d keep-sorted -d line-count

    # Enable specific validators only
    blockwatch -e keep-sorted -e line-count

    # List all found blocks
    blockwatch list 'src/**/*.rs'

    # List all blocks, marking those the diff changed
    git diff --patch | blockwatch list --diff

    # List only the blocks the diff changed
    git diff --patch | blockwatch list --diff --only-changed",
)]
pub struct Args {
    // <block affects="docs/cli.md:cli-docs">
    /// Read a unified diff from stdin to mark which blocks it changed.
    ///
    /// Without this flag stdin is never read. Rules that only fire on changed content, such as
    /// `affects`, need it.
    #[arg(long = "diff", global = true)]
    pub diff: bool,

    /// Restrict the run to the blocks the diff changed, instead of every block in the repository.
    #[arg(long = "only-changed", requires = "diff", global = true)]
    pub only_changed: bool,

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
        if let Some(SubCommand::List { globs: list_globs }) = &self.command {
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
    fn only_changed_without_diff_is_rejected() {
        let error = parse(&["blockwatch", "--only-changed"])
            .expect_err("--only-changed must not be accepted on its own");
        assert!(
            error.to_string().contains("--diff"),
            "the error must name the flag that is missing: {error}"
        );
    }

    #[test]
    fn only_changed_without_diff_is_rejected_under_the_list_subcommand() {
        let error = parse(&["blockwatch", "list", "--only-changed"])
            .expect_err("--only-changed must not be accepted on its own");
        assert!(
            error.to_string().contains("--diff"),
            "the error must name the flag that is missing: {error}"
        );
    }

    #[test]
    fn diff_and_only_changed_parse_alongside_globs() -> anyhow::Result<()> {
        let args = parse(&["blockwatch", "--diff", "--only-changed", "src/**/*.rs"])?;
        assert!(args.diff);
        assert!(args.only_changed);
        assert_eq!(args.globs, vec!["src/**/*.rs".to_string()]);
        Ok(())
    }

    #[test]
    fn diff_and_only_changed_reach_the_list_subcommand() -> anyhow::Result<()> {
        let args = parse(&["blockwatch", "list", "--diff", "--only-changed"])?;
        assert!(args.diff);
        assert!(args.only_changed);
        assert!(matches!(args.command, Some(SubCommand::List { .. })));
        Ok(())
    }

    #[test]
    fn diff_and_only_changed_are_accepted_before_the_subcommand() -> anyhow::Result<()> {
        let args = parse(&["blockwatch", "--diff", "--only-changed", "list"])?;
        assert!(args.diff);
        assert!(args.only_changed);
        assert!(matches!(args.command, Some(SubCommand::List { .. })));
        Ok(())
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
