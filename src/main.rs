use anyhow::Context;
use blockwatch::blocks;
use blockwatch::blocks::BlockSeverity;
use blockwatch::diff_parser;
use blockwatch::flags;
use blockwatch::language_parsers;
use blockwatch::repo_path::RepoPath;
use blockwatch::report;
use blockwatch::sarif;
use blockwatch::validators;
use blockwatch::violation_address::ViolationAddress;

use blockwatch::fs::FileSystem;
use blockwatch::validators::Violation;
use clap::Parser;
use globset::GlobSet;
use std::collections::HashMap;
use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::{env, fs, process};

fn main() -> anyhow::Result<()> {
    let args = flags::Args::parse();
    match &args.command {
        Some(flags::SubCommand::List { .. }) => run_list(&args),
        None => run_validators(&args),
    }
}

/// Runs the `list` subcommand: parses every block in scope and writes a JSON report to stdout.
fn run_list(args: &flags::Args) -> anyhow::Result<()> {
    let file_system = blockwatch::fs::FileSystemImpl::new(&repository_root()?)?;
    let (scan_mode, line_changes) = run_inputs(args, &file_system)?;
    let (context, _scan_stats) = build_context(args, scan_mode, line_changes, &file_system)?;
    let report = context.to_serializable_report();
    serde_json::to_writer_pretty(std::io::stdout(), &report).context("Failed to list blocks")
}

/// Runs the default command: validates every block in scope and reports any violations.
fn run_validators(args: &flags::Args) -> anyhow::Result<()> {
    let file_system = Arc::new(blockwatch::fs::FileSystemImpl::new(&repository_root()?)?);
    let (scan_mode, line_changes) = run_inputs(args, file_system.as_ref())?;
    let (context, scan_stats) = build_context(args, scan_mode, line_changes, file_system.as_ref())?;
    let (sync_validators, async_validators) = validators::detect_validators(
        &context,
        &validators::detector_factories::<blockwatch::fs::FileSystemImpl>(),
        &args.disabled_validators(),
        &args.enabled_validators(),
        &file_system,
    )?;
    let context = Arc::new(context);
    let mut log = validators::run(Arc::clone(&context), sync_validators, async_validators)?;
    apply_suppressions(args.suppressed_addresses(), &mut log.violations);

    // Violations are what the run is for; the report only describes it. Writing them first keeps a
    // failure to write the report from discarding them.
    let has_error_severity = process_violations(&log.violations, args.output_format())?;

    let blocks_needing_diff = (!args.diff).then(|| {
        validators::diff_gated_block_count(
            &context,
            &args.disabled_validators(),
            &args.enabled_validators(),
        )
    });
    write_report(
        args.verbosity,
        report::RunMode::new(args.diff, args.only_changed),
        blocks_needing_diff,
        scan_stats,
        &context,
        &log,
    )?;

    if has_error_severity {
        process::exit(1);
    }
    Ok(())
}

/// Marks every violation covered by a `--suppress` address, so it no longer fails the run.
///
/// An address covering nothing is not an error: a renamed block or a file outside the run's globs
/// both leave one behind, and neither says anything about the code being checked.
fn apply_suppressions(
    suppressed_addresses: &[ViolationAddress],
    violations: &mut HashMap<RepoPath, Vec<Violation>>,
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
///
/// Flushing matters because an error-severity violation exits the process without unwinding, which
/// would drop anything still held in the buffer.
fn write_report(
    verbosity: flags::Verbosity,
    mode: report::RunMode,
    blocks_needing_diff: Option<usize>,
    scan_stats: blocks::ScanStats,
    context: &validators::ValidationContext,
    log: &validators::ValidationLog,
) -> anyhow::Result<()> {
    if verbosity == flags::Verbosity::None {
        // Building the report walks every block, so skip it when nothing will be printed.
        return Ok(());
    }
    let report = report::RunReport::new(mode, blocks_needing_diff, scan_stats, context, log)?;
    let mut stdout = std::io::stdout().lock();
    match verbosity {
        flags::Verbosity::None => {}
        flags::Verbosity::Summary => writeln!(&mut stdout, "{}", report.summary_line())?,
        flags::Verbosity::Full => {
            serde_json::to_writer_pretty(&mut stdout, &report)?;
            writeln!(&mut stdout)?;
        }
    }
    stdout.flush()?;
    Ok(())
}

/// Decides the [`ScanMode`] and extracts the [`diff_parser::LineChange`]s from the diff (if any).
fn run_inputs(
    args: &flags::Args,
    file_system: &impl FileSystem,
) -> anyhow::Result<(
    blocks::ScanMode,
    HashMap<RepoPath, Vec<diff_parser::LineChange>>,
)> {
    let scan_mode = if args.only_changed {
        blocks::ScanMode::OnlyChanged
    } else {
        blocks::ScanMode::All
    };
    if !args.diff {
        return Ok((scan_mode, HashMap::new()));
    }
    if stdin_is_terminal() {
        return Err(anyhow::anyhow!(
            "--diff was given but stdin is a terminal, so there is no diff to read. \
             Pipe a unified diff in, or drop --diff to check every block."
        ));
    }
    Ok((scan_mode, read_diff_from_stdin(file_system)?))
}

/// Parses every block the run should consider into a `ValidationContext`.
fn build_context(
    args: &flags::Args,
    scan_mode: blocks::ScanMode,
    modified_lines_by_file: HashMap<RepoPath, Vec<diff_parser::LineChange>>,
    file_system: &impl FileSystem,
) -> anyhow::Result<(validators::ValidationContext, blocks::ScanStats)> {
    let language_parsers = language_parsers::language_parsers()?;
    let supported_extensions = language_parsers.keys().collect();
    args.validate(&supported_extensions)?;

    let extra_file_extensions = args.extensions();

    let mut glob_set = args.globs()?;
    // An empty glob set matches nothing, so "the caller named no files" has to be spelled out as
    // "every file". It applies in every mode, because the globs narrow whichever set of files the
    // scan mode selected — including the files in a diff.
    if glob_set.is_empty() {
        glob_set = GlobSet::new([globset::Glob::new("**")?])?;
    }

    let path_checker = blockwatch::fs::PathCheckerImpl::new(glob_set, args.ignored_globs()?);

    let parsed = blocks::parse_blocks(
        &modified_lines_by_file,
        scan_mode,
        file_system,
        &path_checker,
        &language_parsers,
        &extra_file_extensions,
    )?;
    Ok((
        validators::ValidationContext::new(
            parsed.blocks,
            language_parsers,
            modified_lines_by_file,
            extra_file_extensions,
        ),
        parsed.stats,
    ))
}

/// Whether stdin is connected to an interactive terminal, i.e. nothing is piped in.
fn stdin_is_terminal() -> bool {
    std::io::stdin().is_terminal()
}

/// Reads a unified diff from stdin and parses it into per-file line changes.
fn read_diff_from_stdin(
    file_system: &impl FileSystem,
) -> anyhow::Result<HashMap<RepoPath, Vec<diff_parser::LineChange>>> {
    let mut diff = String::new();
    std::io::stdin().read_to_string(&mut diff)?;
    diff_parser::validate_diff_input(&diff)?;
    diff_parser::line_changes_from_diff(&diff, file_system)
}

/// Writes the violations to stderr in the requested format. Returns true if any violation has error
/// severity.
fn process_violations(
    violations: &HashMap<RepoPath, Vec<Violation>>,
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

/// Writes the violations to stderr as one JSON object of diagnostics grouped by file.
fn write_json_violations(violations: &HashMap<RepoPath, Vec<Violation>>) -> anyhow::Result<()> {
    let diagnostics: HashMap<&RepoPath, Vec<_>> = violations
        .iter()
        .map(|(file_path, file_violations)| {
            let file_diagnostics = file_violations
                .iter()
                .map(Violation::as_simple_diagnostic)
                .collect();
            (file_path, file_diagnostics)
        })
        .collect();
    write_to_stderr(&diagnostics)
}

/// Writes the violations to stderr as a SARIF log.
fn write_sarif_violations(violations: &HashMap<RepoPath, Vec<Violation>>) -> anyhow::Result<()> {
    write_to_stderr(&sarif::SarifLog::new(violations))
}

/// Writes `document` to stderr as pretty-printed JSON, followed by a newline.
fn write_to_stderr(document: &impl serde::Serialize) -> anyhow::Result<()> {
    let mut stderr = std::io::stderr().lock();
    serde_json::to_writer_pretty(&mut stderr, document)?;
    writeln!(&mut stderr)?;
    Ok(())
}

/// Finds the repository root by walking up from `current_path` to the nearest ancestor carrying a
/// repository marker.
///
/// The search stops at the first marker it meets, so a run started inside a nested repository (a
/// submodule) stays within that repository instead of escaping into the parent one.
fn repository_root_path(current_path: PathBuf) -> anyhow::Result<PathBuf> {
    current_path
        .ancestors()
        // <block affects="src/fs.rs:vcs-metadata-directories">
        .find(|path| path.join(".git").exists() || path.join(".hg").exists())
        // </block>
        .map(|path| path.to_path_buf())
        .ok_or_else(|| anyhow::anyhow!("Could not find the repository root directory"))
}

/// Resolves the repository root from the current working directory.
fn repository_root() -> anyhow::Result<PathBuf> {
    repository_root_path(fs::canonicalize(env::current_dir()?)?)
}
