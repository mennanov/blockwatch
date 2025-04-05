use crate::differ::HunksExtractor;
use std::io::Read;
use std::path::PathBuf;
use std::{env, fs, io};

mod checker;
mod differ;
mod parsers;

fn main() -> anyhow::Result<()> {
    let mut stdin = io::stdin();
    let mut diff = String::new();
    stdin.read_to_string(&mut diff)?;
    let extractor = differ::DiffyExtractor::new();
    let modified_ranges_by_file = extractor.extract(diff.as_str())?;
    let root_path = repository_root_path(fs::canonicalize(env::current_dir()?)?)?;

    checker::check_blocks(
        modified_ranges_by_file
            .iter()
            .map(|(file_path, ranges)| (file_path.as_str(), ranges.as_slice())),
        checker::FsReader::new(root_path),
        parsers::language_parsers()?,
    )
}

fn repository_root_path(mut current_path: PathBuf) -> anyhow::Result<PathBuf> {
    loop {
        if current_path.join(".git").is_dir() {
            return Ok(current_path);
        }
        if let Some(parent) = current_path.parent() {
            current_path = parent.to_path_buf();
        } else {
            return Err(anyhow::anyhow!("Could not find repository root directory"));
        }
    }
}
