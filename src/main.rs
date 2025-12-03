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
    let languages = language_parsers::language_parsers()?;
    args.validate(languages.keys().cloned().collect())?;

    let root_path = repository_root_path(fs::canonicalize(env::current_dir()?)?)?;
    let mut glob_set = args.globs(&root_path)?;
    let is_terminal =
        std::io::stdin().is_terminal() || env::var("BLOCKWATCH_TERMINAL_MODE").is_ok();
    if glob_set.is_empty() && is_terminal {
        // Match all files when there is no diff input in stdin and no globs in args.
        // This allows running `blockwatch` with no args and input.
        glob_set = GlobSet::new([globset::Glob::new("**")?])?
    }
    let ignored_glob_set = args.ignored_globs(&root_path)?;
    let file_system = blocks::FileSystemImpl::new(root_path, glob_set, ignored_glob_set);
    let modified_lines_by_file = if !is_terminal {
        let mut diff = String::new();
        std::io::stdin().read_to_string(&mut diff)?;
        diff_parser::line_changes_from_diff(diff.as_str())?
    } else {
        HashMap::new()
    };

    let modified_blocks = blocks::parse_blocks(
        modified_lines_by_file,
        &file_system,
        languages,
        args.extensions(),
    )?;
    let context = validators::ValidationContext::new(modified_blocks);
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
        .find(|path| path.join(".git").is_dir())
        .map(|path| path.to_path_buf())
        .ok_or_else(|| anyhow::anyhow!("Could not find the repository root directory"))
}
