use anyhow::Context;
use blockwatch::blocks;
use blockwatch::blocks::BlockSeverity;
use blockwatch::diff_parser;
use blockwatch::flags;
use blockwatch::language_parsers;
use blockwatch::repo_path::RepoPath;
use blockwatch::report;
use blockwatch::validators;

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
        Some(flags::SubCommand::List { diff, .. }) => run_list(&args, *diff),
        None => run_validators(&args),
    }
}

/// Runs the `list` subcommand: parses every block in scope and writes a JSON report to stdout.
///
/// A diff is read from stdin only when `--diff` is set (and stdin is not a terminal), to
/// populate `is_content_modified`. Otherwise `list` never touches stdin, so it is safe to run
/// non-interactively — piped to `jq`, in CI, or when spawned by another program such as an AI agent.
fn run_list(args: &flags::Args, read_diff_flag: bool) -> anyhow::Result<()> {
    let read_diff = read_diff_flag && !stdin_is_terminal();
    let file_system = blockwatch::fs::FileSystemImpl::new(&repository_root()?)?;
    let (context, _scan_stats) = build_context(args, read_diff, &file_system)?;
    let report = context.to_serializable_report();
    serde_json::to_writer_pretty(std::io::stdout(), &report).context("Failed to list blocks")
}

/// Runs the default command: validates every block in scope and reports any violations.
///
/// The diff to validate is read from stdin whenever stdin is not a terminal (i.e. when a
/// `git diff` is piped in); otherwise the whole working tree is checked.
fn run_validators(args: &flags::Args) -> anyhow::Result<()> {
    let file_system = Arc::new(blockwatch::fs::FileSystemImpl::new(&repository_root()?)?);
    let (context, scan_stats) = build_context(args, !stdin_is_terminal(), file_system.as_ref())?;
    let (sync_validators, async_validators) = validators::detect_validators(
        &context,
        &validators::detector_factories::<blockwatch::fs::FileSystemImpl>(),
        &args.disabled_validators(),
        &args.enabled_validators(),
        &file_system,
    )?;
    let context = Arc::new(context);
    let log = validators::run(Arc::clone(&context), sync_validators, async_validators)?;

    // Violations are what the run is for; the report only describes it. Writing them first keeps a
    // failure to write the report from discarding them.
    let has_error_severity = !log.violations.is_empty() && process_violations(&log.violations)?;

    write_report(args.verbosity, scan_stats, &context, &log)?;

    if has_error_severity {
        process::exit(1);
    }
    Ok(())
}

/// Writes the run report to stdout at the given verbosity, and flushes it.
///
/// Flushing matters because an error-severity violation exits the process without unwinding, which
/// would drop anything still held in the buffer.
fn write_report(
    verbosity: flags::Verbosity,
    scan_stats: blocks::ScanStats,
    context: &validators::ValidationContext,
    log: &validators::ValidationLog,
) -> anyhow::Result<()> {
    if verbosity == flags::Verbosity::None {
        // Building the report walks every block, so skip it when nothing will be printed.
        return Ok(());
    }
    let report = report::RunReport::new(scan_stats, context, log)?;
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

/// Parses every block the run should consider into a `ValidationContext`.
///
/// When `read_diff` is set, a unified diff is read from stdin and used to mark which blocks
/// changed. With neither globs nor a diff to scope the run, the whole tree is scanned.
fn build_context(
    args: &flags::Args,
    should_read_diff: bool,
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
    let has_explicit_globs = !glob_set.is_empty();
    if !has_explicit_globs {
        glob_set = GlobSet::new([globset::Glob::new("**")?])?;
    }
    let scan_mode = if should_read_diff && !has_explicit_globs {
        blocks::ScanMode::DiffTargets
    } else {
        blocks::ScanMode::Walk
    };

    let path_checker = blockwatch::fs::PathCheckerImpl::new(glob_set, args.ignored_globs()?);

    let modified_lines_by_file = if should_read_diff {
        read_diff_from_stdin(file_system)?
    } else {
        HashMap::new()
    };

    let parsed = blocks::parse_blocks(
        modified_lines_by_file,
        scan_mode,
        file_system,
        &path_checker,
        &language_parsers,
        extra_file_extensions,
    )?;
    Ok((
        validators::ValidationContext::new(parsed.blocks, language_parsers),
        parsed.stats,
    ))
}

/// Whether stdin is connected to an interactive terminal, i.e. no diff is piped in.
fn stdin_is_terminal() -> bool {
    std::io::stdin().is_terminal()
}

/// Reads a unified diff from stdin and parses it into per-file line changes.
fn read_diff_from_stdin(
    file_system: &impl FileSystem,
) -> anyhow::Result<HashMap<RepoPath, Vec<diff_parser::LineChange>>> {
    let mut diff = String::new();
    std::io::stdin().read_to_string(&mut diff)?;
    diff_parser::line_changes_from_diff(&diff, file_system)
}

/// Writes the violations to stderr as JSON. Returns true if any violation has error severity.
fn process_violations(violations: &HashMap<RepoPath, Vec<Violation>>) -> anyhow::Result<bool> {
    let mut has_error_severity = false;
    let mut diagnostics: HashMap<&RepoPath, Vec<serde_json::Value>> =
        HashMap::with_capacity(violations.len());
    for (file_path, file_violations) in violations {
        let mut file_diagnostics = Vec::with_capacity(file_violations.len());
        for violation in file_violations {
            let diagnostic = violation.as_simple_diagnostic();
            if diagnostic.severity() == BlockSeverity::Error {
                has_error_severity = true;
            }
            file_diagnostics.push(serde_json::to_value(diagnostic)?);
        }
        diagnostics.insert(file_path, file_diagnostics);
    }

    let mut stderr = std::io::stderr().lock();
    serde_json::to_writer_pretty(&mut stderr, &diagnostics)?;
    writeln!(&mut stderr)?;
    Ok(has_error_severity)
}

fn repository_root_path(current_path: PathBuf) -> anyhow::Result<PathBuf> {
    current_path
        .ancestors()
        .find(|path| path.join(".git").is_dir() || path.join(".hg").is_dir())
        .map(|path| path.to_path_buf())
        .ok_or_else(|| anyhow::anyhow!("Could not find the repository root directory"))
}

/// Resolves the repository root from the current working directory.
fn repository_root() -> anyhow::Result<PathBuf> {
    repository_root_path(fs::canonicalize(env::current_dir()?)?)
}
