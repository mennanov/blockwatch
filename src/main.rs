#![feature(iter_map_windows)]

use crate::differ::HunksExtractor;
use crate::validators::Context;
use clap::Parser;
use std::path::PathBuf;
use std::{env, fs, process};
use tokio::io::AsyncReadExt;

mod blocks;
mod differ;
mod flags;
mod parsers;
mod validators;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = flags::Args::parse();
    let languages = parsers::language_parsers()?;
    args.validate(languages.keys().cloned().collect())?;

    let mut diff = String::new();
    tokio::io::stdin().read_to_string(&mut diff).await?;
    let extractor = differ::DiffyExtractor::new();
    let modified_ranges_by_file = extractor.extract(diff.as_str())?;

    let file_reader = blocks::FsReader::new(repository_root_path(fs::canonicalize(
        env::current_dir()?,
    )?)?);
    let modified_blocks = blocks::parse_blocks(
        &modified_ranges_by_file,
        &file_reader,
        languages,
        args.extensions(),
    )
    .await?;
    let violations = validators::run(Context::new(modified_blocks)).await?;
    if !violations.is_empty() {
        let json = serde_json::to_string_pretty(&violations)?;
        eprintln!("{json}");
        process::exit(1);
    }
    Ok(())
}

fn repository_root_path(mut current_path: PathBuf) -> anyhow::Result<PathBuf> {
    loop {
        if current_path.join(".git").is_dir() {
            return Ok(current_path);
        }
        if let Some(parent) = current_path.parent() {
            current_path = parent.to_path_buf();
        } else {
            return Err(anyhow::anyhow!(
                "Could not find the repository root directory"
            ));
        }
    }
}
