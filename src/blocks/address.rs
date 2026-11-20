//! `BlockAddress`: `FILE:BLOCK_NAME`, the address of a named block.

use crate::repo_path::RepoPath;
use anyhow::{Context, bail};
use std::fmt;

/// What tells a named block apart from every other block in the repository: `FILE:BLOCK_NAME`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BlockAddress {
    file: RepoPath,
    name: String,
}

impl BlockAddress {
    /// The address of the block called `name` in `file`.
    pub(crate) fn new(file: RepoPath, name: String) -> Self {
        Self { file, name }
    }

    /// Parses `FILE:BLOCK_NAME`.
    ///
    /// Returns an error for any other address, such as `FILE` alone, an empty segment, or a file
    /// outside the repository.
    pub(crate) fn parse(address: &str) -> anyhow::Result<Self> {
        // `FILE` alone could select every block in a file, but a path argument or `--ignore`
        // already does that.
        match address.split(':').collect::<Vec<_>>().as_slice() {
            [file, name] if !file.is_empty() && !name.is_empty() => Ok(Self {
                file: RepoPath::from_reference(file)
                    .with_context(|| format!("invalid file in the block address \"{address}\""))?,
                name: name.to_string(),
            }),
            _ => bail!("expected FILE:BLOCK_NAME, got \"{address}\""),
        }
    }

    /// The file the block is in.
    pub(crate) fn file(&self) -> &RepoPath {
        &self.file
    }
}

impl fmt::Display for BlockAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.file, self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::BlockAddress;
    use crate::repo_path::RepoPath;

    #[test]
    fn parsed_address_equals_the_address_of_that_block() -> anyhow::Result<()> {
        let address = BlockAddress::parse("./a.md:n")?;
        assert_eq!(
            address,
            BlockAddress::new(RepoPath::from_reference("a.md")?, "n".to_string())
        );
        assert_ne!(
            address,
            BlockAddress::new(RepoPath::from_reference("a.md")?, "other".to_string())
        );
        assert_ne!(
            address,
            BlockAddress::new(RepoPath::from_reference("b.md")?, "n".to_string())
        );
        Ok(())
    }

    #[test]
    fn address_without_two_non_empty_segments_is_rejected() {
        for address in ["a.md", "a.md:n:keep-unique", "a.md:", ":n"] {
            let error = BlockAddress::parse(address).expect_err("the address must be rejected");
            assert!(
                format!("{error:#}").contains("expected FILE:BLOCK_NAME"),
                "{address}: {error:#}"
            );
        }
    }
}
