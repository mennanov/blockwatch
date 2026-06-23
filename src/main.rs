use anyhow::Context;
use blockwatch::blocks;
use blockwatch::blocks::BlockSeverity;
use blockwatch::diff_parser;
use blockwatch::flags;
use blockwatch::language_parsers;
use blockwatch::validators;

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
    let context = build_context(args, read_diff)?;
    let report = context.to_serializable_report();
    serde_json::to_writer_pretty(std::io::stdout(), &report).context("Failed to list blocks")
}

/// Runs the default command: validates every block in scope and reports any violations.
///
/// The diff to validate is read from stdin whenever stdin is not a terminal (i.e. when a
/// `git diff` is piped in); otherwise the whole working tree is checked.
fn run_validators(args: &flags::Args) -> anyhow::Result<()> {
    let context = build_context(args, !stdin_is_terminal())?;
    let (sync_validators, async_validators) = validators::detect_validators(
        &context,
        validators::DETECTOR_FACTORIES,
        &args.disabled_validators(),
        &args.enabled_validators(),
    )?;
    let violations = validators::run(Arc::new(context), sync_validators, async_validators)?;
    if !violations.is_empty() {
        process_violations(violations)?;
    }
    Ok(())
}

/// Parses every block the run should consider into a `ValidationContext`.
///
/// When `read_diff` is set, a unified diff is read from stdin and used to mark which blocks
/// changed. With neither globs nor a diff to scope the run, the whole tree is scanned.
fn build_context(
    args: &flags::Args,
    read_diff: bool,
) -> anyhow::Result<validators::ValidationContext> {
    let languages = language_parsers::language_parsers()?;
    let supported_extensions = languages.keys().collect();
    args.validate(&supported_extensions)?;

    let modified_lines_by_file = if read_diff {
        read_diff_from_stdin()?
    } else {
        HashMap::new()
    };

    let mut glob_set = args.globs()?;
    if glob_set.is_empty() && !read_diff {
        // Nothing scopes the run, so match every file.
        glob_set = GlobSet::new([globset::Glob::new("**")?])?;
    }
    let should_scan_files = !glob_set.is_empty();

    let path_checker = blocks::PathCheckerImpl::new(glob_set, args.ignored_globs()?);
    let root_path = repository_root_path(fs::canonicalize(env::current_dir()?)?)?;
    let file_system = blocks::FileSystemImpl::new(root_path.clone());

    let blocks = blocks::parse_blocks(
        modified_lines_by_file,
        should_scan_files,
        &file_system,
        &path_checker,
        languages,
        args.extensions(),
    )?;
    Ok(validators::ValidationContext::new(root_path, blocks))
}

/// Whether stdin is connected to an interactive terminal, i.e. no diff is piped in.
fn stdin_is_terminal() -> bool {
    std::io::stdin().is_terminal()
}

/// Reads a unified diff from stdin and parses it into per-file line changes.
fn read_diff_from_stdin() -> anyhow::Result<HashMap<PathBuf, Vec<diff_parser::LineChange>>> {
    let mut diff = String::new();
    std::io::stdin().read_to_string(&mut diff)?;
    diff_parser::line_changes_from_diff(&diff)
}

fn process_violations(violations: HashMap<PathBuf, Vec<Violation>>) -> anyhow::Result<()> {
    let mut has_error_severity = false;
    let mut diagnostics: HashMap<PathBuf, Vec<serde_json::Value>> =
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
    if has_error_severity {
        process::exit(1);
    }
    Ok(())
}

fn repository_root_path(current_path: PathBuf) -> anyhow::Result<PathBuf> {
    current_path
        .ancestors()
        .find(|path| path.join(".git").is_dir() || path.join(".hg").is_dir())
        .map(|path| path.to_path_buf())
        .ok_or_else(|| anyhow::anyhow!("Could not find the repository root directory"))
}
