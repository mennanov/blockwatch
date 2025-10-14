use blockwatch::blocks;
use blockwatch::blocks::BlockSeverity;
use blockwatch::differ;
use blockwatch::differ::HunksExtractor;
use blockwatch::flags;
use blockwatch::parsers;
use blockwatch::validators;
use clap::Parser;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::{env, fs, process};

fn main() -> anyhow::Result<()> {
    let args = flags::Args::parse();
    let languages = parsers::language_parsers()?;
    args.validate(languages.keys().cloned().collect())?;

    let mut diff = String::new();
    std::io::stdin().read_to_string(&mut diff)?;
    let extractor = differ::UnidiffExtractor::new();
    let modified_ranges_by_file = extractor.extract(diff.as_str())?;

    let file_reader = blocks::FsReader::new(repository_root_path(fs::canonicalize(
        env::current_dir()?,
    )?)?);
    let modified_blocks = blocks::parse_blocks(
        &modified_ranges_by_file,
        &file_reader,
        languages,
        args.extensions(),
    )?;
    let context = validators::ValidationContext::new(modified_blocks);
    let (sync_validators, async_validators) = validators::detect_validators(
        &context,
        validators::DETECTOR_FACTORIES,
        &args.disabled_validators(),
    )?;
    let violations = validators::run(Arc::new(context), sync_validators, async_validators)?;
    if !violations.is_empty() {
        let mut has_error_severity = false;
        let mut diagnostics: HashMap<String, Vec<serde_json::Value>> =
            HashMap::with_capacity(violations.len());
        for (file_path, file_violations) in violations {
            let mut file_diagnostics = Vec::with_capacity(file_violations.len());
            for violation in file_violations {
                let diagnostic = violation.as_simple_diagnostic()?;
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
