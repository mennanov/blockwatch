//! The reference to a block, a symbol or a file that `affects`, `check-lua` and `same-as` write in
//! an attribute, and the target it points at.

use crate::repo_path::RepoPath;
use crate::symbol_path::SymbolPath;
use anyhow::bail;
use std::fmt;

/// One target named by a reference-based validator's attribute (`affects`, `check-lua`, `same-as`).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TargetReference {
    /// A named block, in `file` or — when `file` is `None` — in the referencing file itself.
    Block {
        file: Option<RepoPath>,
        name: String,
    },
    /// A whole file. Nothing is parsed out of it, which is what lets a reference point at a format
    /// blockwatch has no grammar for (`.env`, lockfiles, plain-text fixtures).
    File(RepoPath),
    /// A symbol, found by `path` in `file`, or in the referencing file itself when `file` is `None`.
    Symbol {
        file: Option<RepoPath>,
        path: SymbolPath,
    },
}

impl TargetReference {
    /// Parses a comma-separated list of the references shared by the reference-based validators
    /// (`affects`, `check-lua`, `same-as`).
    ///
    /// A reference can take one of three mutually exclusive forms:
    /// - `file#/path` or `#/path`: addresses a symbol by its symbol path.
    /// - `file:name` or `:name`: addresses a named block.
    /// - `file`: addresses a whole file.
    ///
    /// Combining `#` and `:` in the same reference is rejected. Colons within a symbol path
    /// must be percent-encoded as `%3A`.
    ///
    /// A reference whose block name or symbol path is empty (`file.rs:` or `file.json#`) is
    /// rejected rather than treated as a whole-file reference, avoiding silently turning a syntax
    /// typo into a much coarser rule.
    pub(crate) fn parse_list(value: &str) -> anyhow::Result<Vec<Self>> {
        value.split(',').map(Self::parse).collect()
    }

    /// Parses a single target reference string. See [`Self::parse_list`] for the forms it takes.
    pub(crate) fn parse(raw: &str) -> anyhow::Result<Self> {
        let reference = raw.trim();
        if reference.is_empty() {
            bail!("invalid target reference: \"{reference}\"");
        }
        // A formatter that wraps long comments can break a tag inside a reference. The file or
        // block would then not be found, and that error would not say why.
        if reference.contains('\n') {
            bail!(
                "invalid target reference: \"{reference}\" has a line break. Put the reference on \
                 one line"
            );
        }
        if reference.contains('#') && reference.contains(':') {
            bail!(
                "invalid target reference: \"{reference}\" combines '#' and ':'; \
             these forms are mutually exclusive"
            );
        }
        if let Some((file_part, _)) = reference.split_once('#') {
            #[expect(clippy::string_slice, reason = "`file_part` ends right before a `#`")]
            return parse_symbol_reference(file_part, &reference[file_part.len()..], reference);
        }
        if let Some((file_part, name_part)) = reference.split_once(':') {
            return parse_block_reference(file_part, name_part, reference);
        }
        // Normalized so `./target.json` and `target.json` resolve to the same file.
        Ok(TargetReference::File(RepoPath::from_reference(reference)?))
    }

    /// The target this reference points at, written in `referencing_file`. A reference without a
    /// file, like `:name` or `#/path`, points into the file it is written in.
    pub(crate) fn resolve(self, referencing_file: &RepoPath) -> ResolvedTarget {
        let in_referencing_file =
            |file: Option<RepoPath>| file.unwrap_or_else(|| referencing_file.clone());
        match self {
            Self::Block { file, name } => ResolvedTarget {
                file: in_referencing_file(file),
                kind: TargetKind::Block(name),
            },
            Self::File(file) => ResolvedTarget {
                file,
                kind: TargetKind::WholeFile,
            },
            Self::Symbol { file, path } => ResolvedTarget {
                file: in_referencing_file(file),
                kind: TargetKind::Symbol(path),
            },
        }
    }
}

/// Parses a symbol reference (`file#/path` or `#/path`).
fn parse_symbol_reference(
    file_part: &str,
    fragment: &str,
    reference: &str,
) -> anyhow::Result<TargetReference> {
    let file_part = file_part.trim();
    if fragment == "#" {
        if file_part.is_empty() {
            bail!("invalid target reference: \"{reference}\"");
        }
        bail!(
            "invalid target reference: \"{reference}\" has no symbol path after '#'; \
             drop the '#' to reference the whole file"
        );
    }
    let file = parse_optional_repo_path(file_part)?;
    let path = SymbolPath::parse(fragment)?;
    Ok(TargetReference::Symbol { file, path })
}

/// Parses a `:`-separated named block reference (`file:name` or `:name`).
fn parse_block_reference(
    file_part: &str,
    name_part: &str,
    reference: &str,
) -> anyhow::Result<TargetReference> {
    let file_part = file_part.trim();
    let name_part = name_part.trim();
    if name_part.is_empty() {
        bail!(
            "invalid target reference: \"{reference}\" has no block name after ':'; \
             drop the ':' to reference the whole file"
        );
    }
    let file = parse_optional_repo_path(file_part)?;
    Ok(TargetReference::Block {
        file,
        name: name_part.to_string(),
    })
}

/// Parses a file path into an optional [`RepoPath`], returning `None` if the input is empty.
fn parse_optional_repo_path(file_part: &str) -> anyhow::Result<Option<RepoPath>> {
    if file_part.is_empty() {
        Ok(None)
    } else {
        // Normalized so `./target.py` and `target.py` resolve to the same file.
        Ok(Some(RepoPath::from_reference(file_part)?))
    }
}

/// A [`TargetReference`] once the file it points into is known.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ResolvedTarget {
    pub(crate) file: RepoPath,
    pub(crate) kind: TargetKind,
}

/// What a [`ResolvedTarget`] points at in its file.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TargetKind {
    /// A named block.
    Block(String),
    /// A whole file. Nothing is parsed out of it.
    WholeFile,
    /// A symbol, found by its path.
    Symbol(SymbolPath),
}

impl ResolvedTarget {
    /// The target's name, as a violation's JSON `data` and the Lua `ctx.affects` show it: a block
    /// name, or a symbol path with its leading `#`. `None` for a whole file.
    pub(crate) fn name(&self) -> Option<String> {
        match &self.kind {
            TargetKind::Block(name) => Some(name.clone()),
            TargetKind::WholeFile => None,
            TargetKind::Symbol(path) => Some(format!("#{path}")),
        }
    }
}

/// How a message shows the target: `file:name` for a block, `file#/path` for a symbol, and the
/// path alone for a whole file.
impl fmt::Display for ResolvedTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let file = &self.file;
        match &self.kind {
            TargetKind::Block(name) => write!(f, "{file}:{name}"),
            // A symbol path keeps its `#`, which already separates it from the file.
            TargetKind::Symbol(path) => write!(f, "{file}#{path}"),
            TargetKind::WholeFile => write!(f, "{file}"),
        }
    }
}

#[cfg(test)]
mod parse_target_references_tests {
    use super::TargetReference;
    use crate::repo_path::RepoPath;
    use crate::symbol_path::SymbolPath;

    /// A `file:name` reference, for the expected values below.
    fn block(file: &str, name: &str) -> anyhow::Result<TargetReference> {
        Ok(TargetReference::Block {
            file: Some(RepoPath::from_reference(file)?),
            name: name.to_string(),
        })
    }

    /// A `file#path` or `#/path` reference, for the expected values below.
    fn path(file: Option<&str>, fragment: &str) -> anyhow::Result<TargetReference> {
        Ok(TargetReference::Symbol {
            file: file.map(RepoPath::from_reference).transpose()?,
            path: SymbolPath::parse(fragment)?,
        })
    }

    #[test]
    fn single_reference() -> anyhow::Result<()> {
        let result = TargetReference::parse_list("file.rs:block_name")?;
        assert_eq!(result, vec![block("file.rs", "block_name")?]);
        Ok(())
    }

    #[test]
    fn multiple_references() -> anyhow::Result<()> {
        let result = TargetReference::parse_list("file1.rs:block1, file2.rs:block2")?;
        assert_eq!(
            result,
            vec![block("file1.rs", "block1")?, block("file2.rs", "block2")?]
        );
        Ok(())
    }

    #[test]
    fn multiple_references_with_a_line_break_after_a_comma() -> anyhow::Result<()> {
        let result = TargetReference::parse_list("file1.rs:block1,\n   file2.rs:block2")?;
        assert_eq!(
            result,
            vec![block("file1.rs", "block1")?, block("file2.rs", "block2")?]
        );
        Ok(())
    }

    #[test]
    fn a_reference_with_a_line_break_returns_error() {
        let err = TargetReference::parse_list("docs/config.\n   md:block").unwrap_err();
        assert!(
            err.to_string().contains("line break"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn empty_filename_returns_none_for_filename() -> anyhow::Result<()> {
        let result = TargetReference::parse_list(":block_name")?;
        assert_eq!(
            result,
            vec![TargetReference::Block {
                file: None,
                name: "block_name".to_string()
            }]
        );
        Ok(())
    }

    #[test]
    fn multiple_empty_filename_references_returns_non_for_filename() -> anyhow::Result<()> {
        let result = TargetReference::parse_list(":block1, :block2")?;
        assert_eq!(
            result,
            vec![
                TargetReference::Block {
                    file: None,
                    name: "block1".to_string()
                },
                TargetReference::Block {
                    file: None,
                    name: "block2".to_string()
                }
            ]
        );
        Ok(())
    }

    #[test]
    fn a_reference_without_a_colon_is_a_whole_file_reference() -> anyhow::Result<()> {
        let result = TargetReference::parse_list("config/schema.json")?;
        assert_eq!(
            result,
            vec![TargetReference::File(RepoPath::from_reference(
                "config/schema.json"
            )?)]
        );
        Ok(())
    }

    #[test]
    fn whole_file_and_block_references_can_be_mixed() -> anyhow::Result<()> {
        let result = TargetReference::parse_list("src/lib.rs:languages-code, locales/en.json")?;
        assert_eq!(
            result,
            vec![
                block("src/lib.rs", "languages-code")?,
                TargetReference::File(RepoPath::from_reference("locales/en.json")?)
            ]
        );
        Ok(())
    }

    #[test]
    fn a_whole_file_reference_is_normalized() -> anyhow::Result<()> {
        assert_eq!(
            TargetReference::parse_list("./config.json")?,
            TargetReference::parse_list("config.json")?
        );
        Ok(())
    }

    #[test]
    fn a_reference_with_an_empty_block_name_returns_error() {
        let err = TargetReference::parse_list("config.json:").unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid target reference: \"config.json:\" has no block name after ':'; \
             drop the ':' to reference the whole file"
        );
    }

    #[test]
    fn an_empty_reference_returns_error() {
        assert!(TargetReference::parse_list("").is_err());
        assert!(TargetReference::parse_list("file.rs:block, ").is_err());
    }

    #[test]
    fn path_reference_with_file_parses_successfully() -> anyhow::Result<()> {
        let result = TargetReference::parse_list("package.json#/dependencies/inngest")?;
        assert_eq!(
            result,
            vec![path(Some("package.json"), "/dependencies/inngest")?]
        );
        Ok(())
    }

    #[test]
    fn path_reference_without_file_parses_successfully() -> anyhow::Result<()> {
        let result = TargetReference::parse_list("#/dependencies/inngest")?;
        assert_eq!(result, vec![path(None, "/dependencies/inngest")?]);
        Ok(())
    }

    #[test]
    fn mixed_reference_types_parse_successfully() -> anyhow::Result<()> {
        let result =
            TargetReference::parse_list("package.json#/version, docs/cli.md:version, README.md")?;
        assert_eq!(
            result,
            vec![
                path(Some("package.json"), "/version")?,
                block("docs/cli.md", "version")?,
                TargetReference::File(RepoPath::from_reference("README.md")?),
            ]
        );
        Ok(())
    }

    #[test]
    fn percent_encoded_colon_in_path_reference_parses_successfully() -> anyhow::Result<()> {
        let result = TargetReference::parse_list("package.json#/%3A")?;
        assert_eq!(result, vec![path(Some("package.json"), "/:")?]);
        Ok(())
    }

    #[test]
    fn reference_combining_hash_and_colon_returns_error() {
        let err1 = TargetReference::parse_list("package.json#/foo:bar").unwrap_err();
        assert!(
            err1.to_string().contains("combines '#' and ':'"),
            "unexpected error message: {err1}"
        );

        let err2 = TargetReference::parse_list("file.rs:block#/foo").unwrap_err();
        assert!(
            err2.to_string().contains("combines '#' and ':'"),
            "unexpected error message: {err2}"
        );
    }

    #[test]
    fn unrooted_path_reference_returns_error() {
        let err = TargetReference::parse_list("package.json#dependencies/inngest").unwrap_err();
        assert!(
            err.to_string().contains("symbol path must start with '/'"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn reference_with_empty_path_returns_error() {
        let err = TargetReference::parse_list("package.json#").unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid target reference: \"package.json#\" has no symbol path after '#'; \
             drop the '#' to reference the whole file"
        );
        assert!(TargetReference::parse_list("#").is_err());
    }
}
