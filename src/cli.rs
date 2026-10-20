//! The whole run: reads the flags and the input, runs the validators, `list` or `explain`, and
//! writes the output.

use crate::blocks::{BlockAddress, BlockSeverity};
use crate::config;
use crate::diff_parser::{self, DiffChanges};
use crate::explain::Explanation;
use crate::flags;
use crate::fs::FileSystem;
use crate::language_parsers;
use crate::path_arguments::PathArguments;
use crate::repo_path::RepoPath;
use crate::report;
use crate::sarif;
use crate::scan;
use crate::settings::{BlockSelection, Settings};
use crate::validators;
use crate::validators::Violation;
use crate::violation_address::ViolationAddress;
use crate::virtual_blocks::VirtualBlock;
use anyhow::Context;
use clap::Parser;
use std::collections::BTreeMap;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::{env, fs};

/// Runs the program with the command-line flags of the process: validates the blocks, lists them
/// with the `list` subcommand, checks one block with the `explain` subcommand, or prints the agent
/// skill with the `skill` subcommand.
///
/// Returns the exit code of the process:
/// - `0` when no violation fails the run.
/// - `1` when a violation of error severity is not suppressed.
/// - `2` when the run can't finish, such as for an invalid flag or an unreadable file. The error
///   is written to stderr.
///
/// For a flag that clap can't parse, clap exits the process with 2 itself.
pub fn run() -> ExitCode {
    let args = flags::Args::parse();
    let result = args.validate().and_then(|()| match &args.command {
        Some(flags::Subcommand::List { .. }) => run_list(&args).map(|()| ExitCode::SUCCESS),
        Some(flags::Subcommand::Skill) => run_skill().map(|()| ExitCode::SUCCESS),
        Some(flags::Subcommand::Explain { address }) => run_explain(&args, address),
        None => run_validators(&args),
    });
    result.unwrap_or_else(|error| {
        // `{error:?}` adds the causes, as Rust does for an error returned from `main`. The result
        // is ignored because there is nowhere left to report a closed stderr, and `eprintln!`
        // would panic on it instead.
        let _ = writeln!(std::io::stderr(), "Error: {error:?}");
        // Not 1, so a script can tell a broken rule from a run that could not check the code.
        // clap already uses 2 for a flag it can't parse.
        ExitCode::from(2)
    })
}

/// Runs the `skill` subcommand: writes the agent skill to stdout, as a complete `SKILL.md`.
///
/// It works in any directory, even outside a repository.
fn run_skill() -> anyhow::Result<()> {
    // The skill is built into the binary, so the text always matches this version. The version is
    // filled in here rather than written in the file, so it can't fall behind `Cargo.toml`.
    let skill = include_str!("skill.md").replace("{{version}}", env!("CARGO_PKG_VERSION"));
    // `write_line` adds the newline that ends the output, so the file's own one is dropped.
    write_line(std::io::stdout(), skill.trim_end())
}

/// Runs the `explain` subcommand: checks the one block that `address` selects, writes what its
/// validators saw to stdout as JSON, and writes its violations to stderr.
///
/// Returns the exit code of a validation run: 1 when a violation of error severity is found, and 0
/// otherwise.
fn run_explain(args: &flags::Args, address: &BlockAddress) -> anyhow::Result<ExitCode> {
    let Prepared {
        file_system,
        settings,
        mut context,
        ..
    } = prepare(args)?;
    // The address picks the block, so the config's `only-blocks` and `skip-blocks` play no part.
    context.apply_block_selection(
        &BlockSelection::Only(vec![address.clone()]),
        file_system.as_ref(),
    )?;
    let (sync_validators, async_validators) = validators::detect_validators(
        &context,
        validators::Validator::detect,
        settings.validator_selection(),
        &file_system,
    )?;
    context.request_traces();
    let context = Arc::new(context);
    let log = validators::run(Arc::clone(&context), sync_validators, async_validators)?;

    // Violations are what the run is for, so they are written first, as in `run_validators`. A
    // failure to write the explanation then can't discard them.
    let has_error_severity = process_violations(&log.violations, flags::OutputFormat::Json)?;
    write_json(
        std::io::stdout(),
        &Explanation::new(&context, &settings, &log, args.diff),
    )?;
    Ok(if has_error_severity {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

/// Runs the `list` subcommand: parses every block in scope and writes a JSON report to stdout.
fn run_list(args: &flags::Args) -> anyhow::Result<()> {
    let Prepared { context, .. } = prepare(args)?;
    let report = context.listings();
    write_json(std::io::stdout(), &report).context("failed to list blocks")
}

/// Runs the default command: validates every block in scope and reports any violations.
///
/// Returns exit code 1 when a violation of error severity is not suppressed, and 0 otherwise.
fn run_validators(args: &flags::Args) -> anyhow::Result<ExitCode> {
    // Read first, so that a bad suppression file is reported before the diff or the config file is
    // read.
    let suppressed_addresses = args.suppressed_addresses()?;
    let Prepared {
        file_system,
        settings,
        mut context,
        scan_stats,
    } = prepare(args)?;
    context.apply_block_selection(settings.block_selection(), file_system.as_ref())?;
    let (sync_validators, async_validators) = validators::detect_validators(
        &context,
        validators::Validator::detect,
        settings.validator_selection(),
        &file_system,
    )?;
    let context = Arc::new(context);
    let mut log = validators::run(Arc::clone(&context), sync_validators, async_validators)?;
    apply_suppressions(&suppressed_addresses, &mut log.violations);

    // Violations are what the run is for; the report only describes it. Writing them first keeps a
    // failure to write the report from discarding them.
    let has_error_severity = process_violations(&log.violations, args.output_format())?;

    let blocks_needing_diff = (!args.diff)
        .then(|| validators::diff_gated_block_count(&context, settings.validator_selection()));
    write_report(
        args.validation.verbosity,
        args.run_mode(),
        blocks_needing_diff,
        scan_stats,
        &context,
        &log,
    )?;

    Ok(if has_error_severity {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

/// The result of [`prepare`].
struct Prepared {
    file_system: Arc<crate::fs::FileSystemImpl>,
    settings: Settings,
    context: validators::ValidationContext,
    scan_stats: scan::ScanStats,
}

/// Finds the repository root, reads the diff and the config file, and parses every block in scope.
fn prepare(args: &flags::Args) -> anyhow::Result<Prepared> {
    let file_system = Arc::new(crate::fs::FileSystemImpl::new(&repository_root()?)?);
    let (scan_mode, line_changes) = run_inputs(args, file_system.as_ref())?;
    let language_parsers = language_parsers::language_parsers()?;
    let (settings, virtual_blocks) =
        read_config(args, file_system.as_ref(), &line_changes, &language_parsers)?;
    let (context, scan_stats) = build_context(
        args,
        &settings,
        virtual_blocks,
        scan_mode,
        line_changes,
        language_parsers,
        file_system.as_ref(),
    )?;
    Ok(Prepared {
        file_system,
        settings,
        context,
        scan_stats,
    })
}

/// Marks every violation covered by a `--suppress` address, so it no longer fails the run.
///
/// An address covering nothing is not an error: a renamed block or a file outside the run's globs
/// both leave one behind, and neither says anything about the code being checked.
fn apply_suppressions(
    suppressed_addresses: &[ViolationAddress],
    violations: &mut BTreeMap<RepoPath, Vec<Violation>>,
) {
    if suppressed_addresses.is_empty() {
        return;
    }
    for (file, violations) in violations.iter_mut() {
        for violation in violations {
            let covered = suppressed_addresses.iter().any(|suppressed| {
                match violation.address() {
                    Some(violation_address) => suppressed.matches(violation_address),
                    // A violation on an unnamed block has no address to point at, so only an
                    // address that covers the whole file reaches it.
                    None => suppressed.covers_whole_file(file),
                }
            });
            if covered {
                violation.suppress();
            }
        }
    }
}

/// Writes the run report to stdout at the given verbosity, and flushes it.
fn write_report(
    verbosity: flags::Verbosity,
    mode: report::RunMode,
    blocks_needing_diff: Option<usize>,
    scan_stats: scan::ScanStats,
    context: &validators::ValidationContext,
    log: &validators::ValidationLog,
) -> anyhow::Result<()> {
    if verbosity == flags::Verbosity::None {
        // Building the report walks every block, so skip it when nothing will be printed.
        return Ok(());
    }
    let report = report::RunReport::new(mode, blocks_needing_diff, scan_stats, context, log);
    match verbosity {
        flags::Verbosity::None => Ok(()),
        flags::Verbosity::Summary => write_line(std::io::stdout(), &report.summary_line()),
        flags::Verbosity::Full => write_json(std::io::stdout(), &report),
    }
}

/// Writes `document` to `output` as pretty-printed JSON, followed by a newline, and flushes it.
///
/// Like [`write_line`], returns `Ok` when the reader has closed the pipe.
fn write_json(output: impl Write, document: &impl serde::Serialize) -> anyhow::Result<()> {
    write_line(output, &serde_json::to_string_pretty(document)?)
}

/// Writes `text` to `output`, followed by a newline, and flushes it.
///
/// Returns `Ok` without writing the rest when the reader has closed the pipe, as `head` does after
/// reading enough. Returns an error for any other failed write.
fn write_line(mut output: impl Write, text: &str) -> anyhow::Result<()> {
    match writeln!(output, "{text}").and_then(|()| output.flush()) {
        // Rust ignores SIGPIPE, so a write to a closed pipe returns this error instead of ending
        // the process. A reader that closed the pipe wants no more output, so it is not a failure.
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        result => Ok(result?),
    }
}

/// Decides the [`scan::ScanMode`] and extracts the [`DiffChanges`] from the diff (if any).
fn run_inputs(
    args: &flags::Args,
    file_system: &impl FileSystem,
) -> anyhow::Result<(scan::ScanMode, DiffChanges)> {
    let scan_mode = if args.only_changed {
        scan::ScanMode::OnlyChanged
    } else {
        scan::ScanMode::All
    };
    if !args.diff {
        return Ok((scan_mode, DiffChanges::default()));
    }
    if stdin_is_terminal() {
        return Err(anyhow::anyhow!(
            "--diff was given but stdin is a terminal, so there is no diff to read. \
             Pipe a unified diff in, or drop --diff to check every block"
        ));
    }
    Ok((scan_mode, read_diff_from_stdin(file_system)?))
}

/// Reads the config file. Returns the settings for this run, which merge the file's with the
/// flags, and the file's virtual blocks.
///
/// `line_changes` are the diff's changes, by file.
fn read_config(
    args: &flags::Args,
    file_system: &impl FileSystem,
    line_changes: &DiffChanges,
    language_parsers: &language_parsers::LanguageParsers,
) -> anyhow::Result<(Settings, Vec<VirtualBlock>)> {
    let config = config::read(args.config.as_deref(), file_system, line_changes)?;
    let settings = Settings::resolve(
        args.raw_settings(),
        config.settings,
        &language_parsers.extensions().collect(),
    )?;
    Ok((settings, config.blocks))
}

/// Parses every block the run should consider into a `ValidationContext`.
fn build_context(
    args: &flags::Args,
    settings: &Settings,
    virtual_blocks: Vec<VirtualBlock>,
    scan_mode: scan::ScanMode,
    modified_lines_by_file: DiffChanges,
    language_parsers: language_parsers::LanguageParsers,
    file_system: &impl FileSystem,
) -> anyhow::Result<(validators::ValidationContext, scan::ScanStats)> {
    let path_arguments = PathArguments::resolve(args.path_arguments(), file_system)?;
    let path_filter = crate::fs::PathFilter::new(
        path_arguments.allowed_globs()?,
        settings.ignored_globs().clone(),
    );
    let language_parsers = language_parsers.with_extension_mappings(settings.extensions().clone());

    let scan = scan::Scan::new(
        file_system,
        &path_filter,
        &language_parsers,
        &virtual_blocks,
    );
    let parsed = scan.parse_blocks(&modified_lines_by_file, scan_mode)?;
    let scanned_files = parsed.scanned_files.iter().cloned().map(Ok);
    match scan_mode {
        scan::ScanMode::All => path_arguments.ensure_each_matches(scanned_files)?,
        // The scan read only the files in the diff. An argument that selects none of them still
        // passes when it selects a file that the diff doesn't touch.
        scan::ScanMode::OnlyChanged => {
            path_arguments.ensure_each_matches(scanned_files.chain(scan.checkable_files()))?
        }
    }
    Ok((
        validators::ValidationContext::new(
            parsed.blocks,
            language_parsers,
            modified_lines_by_file,
            virtual_blocks,
        ),
        parsed.stats,
    ))
}

/// Whether stdin is connected to an interactive terminal, i.e. nothing is piped in.
fn stdin_is_terminal() -> bool {
    std::io::stdin().is_terminal()
}

/// Reads a unified diff from stdin and parses it into per-file line changes.
fn read_diff_from_stdin(file_system: &impl FileSystem) -> anyhow::Result<DiffChanges> {
    let mut diff = Vec::new();
    std::io::stdin().read_to_end(&mut diff)?;
    // A diff of a file that is not UTF-8 carries the file's bytes as they are. Only line numbers
    // are taken from a diff, so those bytes can be replaced.
    let diff = String::from_utf8_lossy(&diff);
    diff_parser::validate_diff_input(&diff)?;
    diff_parser::line_changes_from_diff(&diff, file_system)
}

/// Writes the violations to stderr in the requested format. Returns true if any violation has error
/// severity.
fn process_violations(
    violations: &BTreeMap<RepoPath, Vec<Violation>>,
    format: flags::OutputFormat,
) -> anyhow::Result<bool> {
    let has_error_severity = violations.values().flatten().any(|violation| {
        let diagnostic = violation.as_simple_diagnostic();
        // A suppressed violation is still reported, at the severity its author declared; only its
        // effect on the exit code goes away.
        diagnostic.severity() == BlockSeverity::Error && !diagnostic.is_suppressed()
    });
    match format {
        // JSON is written to stderr only when there are violations.
        flags::OutputFormat::Json if !violations.is_empty() => write_json_violations(violations)?,
        flags::OutputFormat::Json => {}
        // SARIF is written to stderr even when there are no violations.
        flags::OutputFormat::Sarif => write_sarif_violations(violations)?,
    }
    Ok(has_error_severity)
}

/// Writes the violations to stderr as one JSON object of diagnostics grouped by file, with the
/// files sorted by path.
fn write_json_violations(violations: &BTreeMap<RepoPath, Vec<Violation>>) -> anyhow::Result<()> {
    let diagnostics: BTreeMap<&RepoPath, Vec<_>> = violations
        .iter()
        .map(|(file_path, file_violations)| {
            let file_diagnostics = file_violations
                .iter()
                .map(Violation::as_simple_diagnostic)
                .collect();
            (file_path, file_diagnostics)
        })
        .collect();
    write_json(std::io::stderr(), &diagnostics)
}

/// Writes the violations to stderr as a SARIF log.
fn write_sarif_violations(violations: &BTreeMap<RepoPath, Vec<Violation>>) -> anyhow::Result<()> {
    write_json(std::io::stderr(), &sarif::SarifLog::new(violations))
}

/// Finds the repository root by walking up from `current_path` to the nearest ancestor carrying a
/// repository marker.
///
/// The search stops at the first marker it meets, so a run started inside a nested repository (a
/// submodule) stays within that repository instead of escaping into the parent one.
fn repository_root_path(current_path: &Path) -> anyhow::Result<PathBuf> {
    current_path
        .ancestors()
        /* <block same-as="src/fs.rs:vcs-metadata-directories" same-as-mode="subset"
        same-as-pattern='"(?P<value>\.[a-z]+)"'> */
        .find(|path| path.join(".git").exists() || path.join(".hg").exists())
        // </block>
        .map(|path| path.to_path_buf())
        .ok_or_else(|| anyhow::anyhow!("could not find the repository root directory"))
}

/// Resolves the repository root from the current working directory.
fn repository_root() -> anyhow::Result<PathBuf> {
    repository_root_path(&fs::canonicalize(env::current_dir()?)?)
}
