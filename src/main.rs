use blockwatch::blocks;
use blockwatch::differ;
use blockwatch::differ::HunksExtractor;
use blockwatch::flags;
use blockwatch::parsers;
use blockwatch::validators;
use clap::Parser;
use std::io::Read;
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
    let (sync_validators, async_validators) =
        validators::detect_validators(&context, validators::DETECTOR_FACTORIES)?;
    let violations = validators::run(Arc::new(context), sync_validators, async_validators)?;
    if !violations.is_empty() {
        let json = serde_json::to_string_pretty(&violations)?;
        eprintln!("{json}");
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
