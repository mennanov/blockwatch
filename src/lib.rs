//! blockwatch is a linter that works with any language. Rules are written as block tags in code
//! comments, and blockwatch checks them. It can check the whole repository, or only the lines
//! that a unified diff changes.
//!
//! This crate is a program, not a library. The library exists so that the `blockwatch` binary and
//! the fuzz target in `fuzz/` can share the code. Only [`run`] and [`count_rust_blocks`] are
//! `pub`.

// blockwatch is a program, not a library. The library exists only for the binary and the fuzz
// targets, so nothing but `run` and `count_rust_blocks` may be `pub`. This lint catches any other.
#![warn(unreachable_pub)]
// A `pub` item is all the binary and the fuzz target see of this crate, so each one needs a doc.
#![warn(missing_docs)]
// Slicing a `str` panics when an index falls inside a multi-byte character, such as `é`. So each
// slice outside tests needs an `expect` that says why its indexes are on character boundaries.
#![cfg_attr(not(test), warn(clippy::string_slice))]
// A module with child modules keeps its own code in `mod.rs`, inside its folder, so each such
// module looks the same.
#![warn(clippy::self_named_module_files)]

mod attributes;
mod block_parser;
mod blocks;
mod cli;
mod config;
mod diff_parser;
mod explain;
mod flags;
mod fs;
mod language_parsers;
mod language_resolver;
mod path_arguments;
mod paths_glob;
mod position;
mod repo_path;
mod report;
mod sarif;
mod scan;
mod settings;
mod symbol_path;
mod symbols;
mod tag_parser;
mod validators;
mod virtual_blocks;

pub use cli::run;

/// Counts the blocks in `source`, which is Rust code. Returns an error if a tag is malformed or
/// not closed.
///
/// It is `pub` because a fuzz target is a separate crate, and a separate crate can only call `pub`
/// functions.
///
/// # Panics
/// Panics if the tree-sitter crate can't load the Rust grammar, which is a build bug.
///
/// # Examples
///
/// ```
/// let count = blockwatch::count_rust_blocks("fn main() {}\n")?;
/// assert_eq!(count, 0);
/// # Ok::<(), anyhow::Error>(())
/// ```
pub fn count_rust_blocks(source: &str) -> anyhow::Result<usize> {
    let mut parser = language_parsers::rust::parser();
    language_parsers::LanguageParser::parse_blocks(&mut parser, source)
        .try_fold(0, |count, block| block.map(|_| count + 1))
}

#[cfg(test)]
mod count_rust_blocks_tests {
    use super::count_rust_blocks;

    #[test]
    fn source_with_two_blocks_returns_two() -> anyhow::Result<()> {
        let source = "// <block>\nlet a = 1;\n// </block>\n// <block>\nlet b = 2;\n// </block>\n";
        assert_eq!(count_rust_blocks(source)?, 2);
        Ok(())
    }

    #[test]
    fn source_with_an_unclosed_block_returns_an_error() {
        assert!(count_rust_blocks("// <block>\nlet a = 1;\n").is_err());
    }
}
