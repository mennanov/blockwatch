use crate::settings::{RawSettings, parse_validator};
use crate::violation_address::ViolationAddress;
use anyhow::Context;
use clap::{Parser, builder::ValueParser, crate_version};
use std::path::{Path, PathBuf};

/// How much a run reports about what it checked.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Verbosity {
    /// Print no report.
    #[default]
    None,
    /// Print a one-line summary of the run.
    Summary,
    /// Print a JSON report of every block in scope and the validators that checked it.
    Full,
}

/// Violations output format.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum OutputFormat {
    /// One JSON object of diagnostics, grouped by file.
    #[default]
    Json,
    /// A SARIF 2.1.0 log, the interchange format code-scanning services read.
    Sarif,
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
/// The fields hold the values as clap parsed them. The methods below turn them into what the rest
/// of the program uses, such as [`RawSettings`] and suppression addresses. The flags here are
/// `global`, so they can go before or after `list`. The flags that only validation uses are in
/// [`ValidationFlags`].
#[derive(Parser, Debug)]
#[command(
    author,
    version = crate_version!(),
    about = "Validate interdependent code/doc blocks to prevent drift.",
    // <block name="help-text">
    long_about =r"Blockwatch validates that named blocks, sorted segments, and other constraints declared in block tags remain consistent across files. It is designed for use in pre-commit hooks and CI.

By default it scans every file in the repository. Pass --diff to also read a unified diff from stdin. The diff marks the blocks it changed. Rules that only fire on changed content, such as `affects`, need it.
Hooks and CI should pass --diff and still scan every file. A change can break a block in a file it never touched, such as the other side of a `same-as`. Only a full scan sees that.
Add --only-changed to check only the blocks the diff changed, when a full scan costs too much.

You can put project-wide settings (--ignore, -E, --enable, --disable) in blockwatch.toml at the repository root. It can also declare blocks around one value of a JSON, TOML or YAML file, instead of tags.",
    after_help = r"EXAMPLES:
    # Check every block in the repository
    blockwatch

    # Check one file and one directory
    blockwatch src/main.rs docs

    # Filter files using glob patterns
    blockwatch 'src/**/*.rs' '**/*.md'

    # Ignore files using glob patterns
    blockwatch 'src/**/*.rs' --ignore '**/generated/**'

    # Scan the whole tree, and enforce the rules that need a diff (recommended for hooks and CI)
    git diff --patch | blockwatch --diff

    # Check only the blocks the diff changed, when a full scan costs too much
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

    # Read settings from a different file instead of blockwatch.toml
    blockwatch --config ci/blockwatch.toml

    # Suppress a reported violation without editing the source
    blockwatch --suppress docs/cli.md:cli-docs:keep-sorted

    # Suppress violations from a commit message or text file
    blockwatch --suppress-from commit_msg.txt

    # Write the violations as a SARIF log for a code-scanning service
    blockwatch --format sarif 2> blockwatch.sarif

    # List all found blocks
    blockwatch list 'src/**/*.rs'

    # List all blocks, marking those the diff changed
    git diff --patch | blockwatch list --diff

    # List only the blocks the diff changed
    git diff --patch | blockwatch list --diff --only-changed",
    // </block>
)]
pub(crate) struct Args {
    /* <block name="cli-flags" affects="docs/cli.md:cli-docs"
    same-as-pattern='long = "(?P<value>[a-z-]+)"'> */
    /// Read a unified diff from stdin to mark which blocks it changed.
    ///
    /// Without this flag stdin is never read. Rules that only fire on changed content, such as
    /// `affects`, need it.
    #[arg(long = "diff", global = true)]
    pub(crate) diff: bool,

    /// Restrict the run to the blocks the diff changed, instead of every block in the repository.
    #[arg(long = "only-changed", requires = "diff", global = true)]
    pub(crate) only_changed: bool,

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

    /// Glob patterns to ignore files.
    #[arg(
        long = "ignore",
        value_name = "GLOBS",
        action = clap::ArgAction::Append,
        global = true,
    )]
    ignore: Vec<String>,

    /// Read settings from FILE instead of blockwatch.toml in the repository root.
    ///
    /// A relative path starts from the current directory, not from the repository root.
    /// blockwatch.toml is optional, but FILE must exist.
    #[arg(long = "config", value_name = "FILE", global = true)]
    pub(crate) config: Option<PathBuf>,

    // Not `global`, so that `list --help` doesn't show these flags and `list` rejects them after
    // it. clap still accepts them before `list`, so `validate` rejects them there.
    #[command(flatten)]
    pub(crate) validation: ValidationFlags,

    /// Files, directories or glob patterns to check, starting from the repository root.
    ///
    /// An argument that selects no file to check fails the run.
    #[arg(value_name = "PATHS")]
    paths: Vec<String>,

    /// The subcommand to run, if any. `None` means the default action: validate.
    #[command(subcommand)]
    pub(crate) command: Option<SubCommand>,
}

/// The flags that only the default command takes. They choose the validators, report what they
/// checked, or suppress what they found, and `list` runs no validators.
///
/// The default value is what a command line without any of these flags parses to.
#[derive(clap::Args, Debug, Default, PartialEq)]
pub(crate) struct ValidationFlags {
    /// Disable a validator, e.g. -d check-ai -d line-count
    #[arg(
        short = 'd',
        long = "disable",
        value_name = "VALIDATOR",
        action = clap::ArgAction::Append,
        value_parser = ValueParser::new(parse_validator),
    )]
    disabled_validators: Vec<&'static str>,

    /// Enable a validator, e.g. -e check-ai -e line-count
    #[arg(
        short = 'e',
        long = "enable",
        value_name = "VALIDATOR",
        action = clap::ArgAction::Append,
        value_parser = ValueParser::new(parse_validator),
    )]
    enabled_validators: Vec<&'static str>,

    /// How much to report about what the run checked. Printed to stdout.
    #[arg(
        long = "verbosity",
        value_name = "LEVEL",
        value_enum,
        default_value_t = Verbosity::None,
    )]
    pub(crate) verbosity: Verbosity,

    /// The format the violations are written in. Printed to stderr.
    ///
    /// `sarif` writes a SARIF 2.1.0 log in place of the JSON diagnostics, for code-scanning
    /// services that read it. Unlike the JSON diagnostics, a SARIF log is written even when the run
    /// found nothing, because such a service expects a log from every run.
    #[arg(long = "format", value_name = "FORMAT", value_enum)]
    format: Option<OutputFormat>,

    /// Suppress reported violations so they no longer fail the run, e.g.
    /// --suppress docs/cli.md:cli-docs:keep-sorted
    ///
    /// The address is FILE[:BLOCK_NAME[:VALIDATOR[:HASH]]]: the shorter it is, the more it covers,
    /// from a single violation down to every violation in a file. The violations are still
    /// reported. Repeat the flag to suppress more.
    #[arg(
        long = "suppress",
        value_name = "ADDRESS",
        action = clap::ArgAction::Append,
        value_parser = ValueParser::new(ViolationAddress::parse),
    )]
    suppressed_addresses: Vec<ViolationAddress>,

    /// Suppress reported violations loaded from a text file, matching lines with format:
    /// Blockwatch-suppress: ADDRESS
    ///
    /// Any other line is ignored, so an ordinary commit message is a valid input.
    /// Repeat the flag to read from multiple files.
    #[arg(
        long = "suppress-from",
        value_name = "FILE",
        action = clap::ArgAction::Append,
    )]
    suppress_from: Vec<PathBuf>,
    // </block>
}

/// A mode that inspects blocks instead of validating them.
#[derive(clap::Subcommand, Debug, Clone)]
pub(crate) enum SubCommand {
    /// List all blocks found in the scanned files.
    List {
        /// Files, directories or glob patterns to list the blocks of, starting from the repository
        /// root.
        ///
        /// An argument that selects no file to check fails the run.
        #[arg(value_name = "PATHS")]
        paths: Vec<String>,
    },
}

impl Args {
    /// Returns the settings given on the command line. They are not validated yet.
    pub(crate) fn raw_settings(&self) -> RawSettings {
        RawSettings {
            ignore: self.ignore.clone(),
            extensions: self.extensions.iter().cloned().collect(),
            enable: self
                .validation
                .enabled_validators
                .iter()
                .map(|name| name.to_string())
                .collect(),
            disable: self
                .validation
                .disabled_validators
                .iter()
                .map(|name| name.to_string())
                .collect(),
        }
    }

    /// The format to write the violations in.
    pub(crate) fn output_format(&self) -> OutputFormat {
        self.validation.format.unwrap_or_default()
    }

    /// Where the violations the run was told to suppress sit.
    ///
    /// Combines addresses passed directly via `--suppress` and those read from files
    /// passed via `--suppress-from`. Errors when such a file cannot be read or holds an address
    /// that does not parse.
    pub(crate) fn suppressed_addresses(&self) -> anyhow::Result<Vec<ViolationAddress>> {
        let mut addresses = self.validation.suppressed_addresses.clone();
        for path in &self.validation.suppress_from {
            addresses.extend(parse_suppressions_from_file(path)?);
        }
        Ok(addresses)
    }

    /// The path and glob arguments as written: those before `list`, then those after it.
    pub(crate) fn path_arguments(&self) -> Vec<String> {
        let mut arguments = self.paths.clone();
        match &self.command {
            Some(SubCommand::List { paths }) => arguments.extend(paths.iter().cloned()),
            None => {}
        }
        arguments
    }

    /// Validates the flags that are not settings, such as `--format` and `--suppress`.
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        match self.command {
            Some(SubCommand::List { .. }) => {
                if self.validation != ValidationFlags::default() {
                    anyhow::bail!("the `list` subcommand doesn't take flags meant for validation");
                }
            }
            None => {
                self.suppressed_addresses()?;
            }
        }
        Ok(())
    }
}

const SUPPRESS_TRAILER_PREFIX: &str = "blockwatch-suppress:";

/// Reads the suppression addresses written as trailers in the file at `path`.
///
/// `path` may name any file the process can read, inside the repository or not. Errors when the
/// file cannot be read, or when a trailer holds an address that does not parse.
fn parse_suppressions_from_file(path: &Path) -> anyhow::Result<Vec<ViolationAddress>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read suppression file \"{}\"", path.display()))?;
    parse_suppressions(&content, path)
}

/// Extracts the suppression addresses from the trailers in `content`.
fn parse_suppressions(content: &str, path: &Path) -> anyhow::Result<Vec<ViolationAddress>> {
    let mut addresses = Vec::new();
    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.len() <= SUPPRESS_TRAILER_PREFIX.len()
            || !trimmed[..SUPPRESS_TRAILER_PREFIX.len()]
                .eq_ignore_ascii_case(SUPPRESS_TRAILER_PREFIX)
        {
            // Non-matching line.
            continue;
        }
        let address_str = trimmed[SUPPRESS_TRAILER_PREFIX.len()..].trim();
        let address = ViolationAddress::parse(address_str).with_context(|| {
            format!(
                "invalid suppression address \"{address_str}\" in \"{}\" at line {}",
                path.display(),
                line_idx + 1
            )
        })?;
        addresses.push(address);
    }
    Ok(addresses)
}

fn parse_extensions(s: &str) -> anyhow::Result<(String, String)> {
    s.split_once('=')
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .with_context(|| format!("Invalid KEY=VALUE format: {s}"))
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
        assert_eq!(args.paths, vec!["src/**/*.rs".to_string()]);
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
    fn list_help_shows_only_the_flags_list_takes() {
        let help = parse(&["blockwatch", "list", "--help"])
            .expect_err("--help prints the help instead of returning the flags")
            .to_string();
        for flag in [
            "--diff",
            "--only-changed",
            "--extension",
            "--ignore",
            "--config",
        ] {
            assert!(
                help.contains(flag),
                "`list --help` must show {flag}: {help}"
            );
        }
        for flag in [
            "--enable",
            "--disable",
            "--verbosity",
            "--format",
            "--suppress",
            "--suppress-from",
        ] {
            assert!(
                !help.contains(flag),
                "`list --help` must not show {flag}: {help}"
            );
        }
    }

    #[test]
    fn validation_flag_before_list_is_rejected() -> anyhow::Result<()> {
        for argv in [
            ["blockwatch", "--enable", "check-ai", "list"],
            ["blockwatch", "--disable", "check-ai", "list"],
            ["blockwatch", "--verbosity", "full", "list"],
            ["blockwatch", "--format", "sarif", "list"],
            ["blockwatch", "--suppress", "a.md:n:keep-sorted", "list"],
            ["blockwatch", "--suppress-from", "msg.txt", "list"],
        ] {
            let error = parse(&argv)?
                .validate()
                .expect_err("`list` must reject a flag it doesn't take");
            assert!(
                error.to_string().contains("`list` subcommand"),
                "unexpected error for {argv:?}: {error}"
            );
        }
        Ok(())
    }

    #[test]
    fn unknown_validator_is_rejected() {
        let error = parse(&["blockwatch", "--disable", "keep-tidy"])
            .expect_err("a validator that does not exist must be rejected");
        assert!(
            error.to_string().contains("keep-tidy"),
            "the error must quote the offending name: {error}"
        );
    }

    #[test]
    fn violations_format_is_json_by_default() -> anyhow::Result<()> {
        assert_eq!(parse(&["blockwatch"])?.output_format(), OutputFormat::Json);
        assert_eq!(
            parse(&["blockwatch", "--format", "sarif"])?.output_format(),
            OutputFormat::Sarif
        );
        Ok(())
    }

    #[test]
    fn unknown_format_value_is_rejected() {
        let error = parse(&["blockwatch", "--format", "xml"])
            .expect_err("only the formats the program writes are accepted");
        assert!(
            error.to_string().contains("xml"),
            "the error must quote the offending value: {error}"
        );
    }

    #[test]
    fn repeated_suppress_flags_collect_every_address() -> anyhow::Result<()> {
        let args = parse(&[
            "blockwatch",
            "--suppress",
            "docs/cli.md:cli-docs:keep-sorted",
            "--suppress",
            "src/lib.rs:languages:line-count",
        ])?;
        assert_eq!(
            args.suppressed_addresses()?
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec![
                "docs/cli.md:cli-docs:keep-sorted".to_string(),
                "src/lib.rs:languages:line-count".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn malformed_suppression_address_fails_at_parse_time() {
        let error = parse(&["blockwatch", "--suppress", "docs/cli.md:cli-docs:keep-tidy"])
            .expect_err("an address naming an unknown validator must be rejected");
        assert!(
            error.to_string().contains("docs/cli.md:cli-docs:keep-tidy"),
            "the error must quote the offending address: {error}"
        );
    }

    #[test]
    fn trailers_are_collected_and_every_other_line_is_ignored() -> anyhow::Result<()> {
        let content = "feat: update documentation\n\
             \n\
             This commit updates the docs without updating code.\n\
             \n\
             Blockwatch-suppress: docs/cli.md:cli-docs:keep-sorted\n\
             blockwatch-suppress:  src/lib.rs:languages:line-count  \n\
             Other-trailer: value\n";

        assert_eq!(
            parse_suppressions(content, Path::new("commit_msg.txt"))?
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec![
                "docs/cli.md:cli-docs:keep-sorted".to_string(),
                "src/lib.rs:languages:line-count".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn suppress_from_with_nonexistent_file_fails_validation() -> anyhow::Result<()> {
        let args = parse(&["blockwatch", "--suppress-from", "nonexistent_file.txt"])?;
        let error = args
            .validate()
            .expect_err("nonexistent file must fail validation");
        assert!(
            error
                .to_string()
                .contains("failed to read suppression file"),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[test]
    fn suppress_from_with_malformed_address_fails_validation() {
        let error = parse_suppressions(
            "Blockwatch-suppress: bad:::address\n",
            Path::new("bad_msg.txt"),
        )
        .expect_err("a malformed address must be rejected");
        assert!(
            error.to_string().contains("invalid suppression address"),
            "unexpected error: {error}"
        );
    }
}
