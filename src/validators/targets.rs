//! Reads the files that references, such as those of `affects` and `same-as`, point into.

use super::ValidationContext;
use crate::fs::FileSystem;
use crate::repo_path::RepoPath;
use crate::symbols::SymbolPath;
use crate::symbols::{ResolveError, Symbol, Symbols};
use anyhow::{Context, anyhow};
use std::collections::HashMap;
use std::collections::hash_map::Entry;

/// What a reference's target resolves to, or the reason it does not resolve, such as a missing
/// block or a missing symbol. The reason is reported as the reference's violation. A failure that
/// ends the run is the `Err` of an enclosing `anyhow::Result` instead.
pub(super) type TargetResult<T> = Result<T, String>;

/// The text and the symbols of the files that references point into, each read and derived at most
/// once.
pub(super) struct TargetFiles<'a, Fs: FileSystem> {
    context: &'a ValidationContext,
    /// Reads the target files that are not in `context`.
    file_system: &'a Fs,
    /// The text of the target files that are not in `context`.
    contents: HashMap<RepoPath, String>,
    /// The symbols of every file a symbol reference points into.
    symbols: HashMap<RepoPath, Symbols>,
}

impl<'a, Fs: FileSystem> TargetFiles<'a, Fs> {
    pub(super) fn new(context: &'a ValidationContext, file_system: &'a Fs) -> Self {
        Self {
            context,
            file_system,
            contents: HashMap::new(),
            symbols: HashMap::new(),
        }
    }

    /// The text of `file`: from the validation context when the file is in it, and otherwise read
    /// through the file system.
    ///
    /// # Errors
    /// Returns an error if the file is not in the context and cannot be read.
    pub(super) fn content(&mut self, file: &RepoPath) -> anyhow::Result<&str> {
        file_content(self.context, self.file_system, &mut self.contents, file)
    }

    /// Resolves `path` among the symbols of `file`. Returns the text of `file`, together with the
    /// symbol or with the reason why no single symbol has `path`.
    ///
    /// # Errors
    /// Returns an error that shows the reference if `file` cannot be read, has no grammar, is in a
    /// language without symbols, or does not parse, or if `path` starts with a name whose
    /// definition or value can't be read reliably.
    pub(super) fn resolve_symbol(
        &mut self,
        file: &RepoPath,
        path: &SymbolPath,
    ) -> anyhow::Result<(&str, TargetResult<&Symbol>)> {
        let context = || format!("failed to resolve symbol reference {file}#{path}");
        let (content, symbols) = self.content_and_symbols(file).with_context(context)?;
        let resolution = match symbols.resolve(path) {
            Ok(symbol) => Ok(symbol),
            // The file is as broken near the name as a data file with a syntax error is, so it
            // stops the run the same way.
            Err(ResolveError::Unreadable(unreadable)) => {
                return Err(anyhow!("{unreadable}").context(context()));
            }
            Err(
                error @ (ResolveError::NotFound { .. }
                | ResolveError::AmbiguousSymbol { .. }
                | ResolveError::AmbiguousFilter { .. }),
            ) => Err(error.reason(content)),
        };
        Ok((content, resolution))
    }

    /// The text of `file` and the symbols derived from it.
    fn content_and_symbols(&mut self, file: &RepoPath) -> anyhow::Result<(&str, &Symbols)> {
        let content = file_content(self.context, self.file_system, &mut self.contents, file)?;
        let symbols = match self.symbols.entry(file.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let parser = self
                    .context
                    .parsers
                    .parser_for(file.as_path())
                    .context("file format is unsupported")?;
                let symbols = parser
                    .lock()
                    .expect("a thread panicked while it held the parser")
                    .parse_symbols(content)?;
                entry.insert(symbols)
            }
        };
        Ok((content, symbols))
    }
}

/// The text of `file`: from `context` when the file is in it, and otherwise read through
/// `file_system` once and kept in `contents`.
///
/// # Errors
/// Returns an error if the file is not in `context` and cannot be read.
fn file_content<'c, Fs: FileSystem>(
    context: &'c ValidationContext,
    file_system: &Fs,
    contents: &'c mut HashMap<RepoPath, String>,
    file: &RepoPath,
) -> anyhow::Result<&'c str> {
    if let Some(file_blocks) = context.blocks.get(file) {
        return Ok(&file_blocks.file_content);
    }
    match contents.entry(file.clone()) {
        Entry::Occupied(entry) => Ok(entry.into_mut()),
        Entry::Vacant(entry) => Ok(entry.insert(file_system.read_to_string(file.as_path())?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::test_utils::FakeFileSystem;
    use crate::validators::test_utils::validation_context;

    /// The error of resolving `path` in `file`, a file on the disk that holds `content`.
    fn resolve_symbol_error(file: &str, content: &str, path: &str) -> anyhow::Result<String> {
        let context = validation_context("referencing.py", "");
        let file_system =
            FakeFileSystem::new(HashMap::from([(file.to_string(), content.to_string())]));
        let mut files = TargetFiles::new(&context, &file_system);
        let err = files
            .resolve_symbol(&RepoPath::from_reference(file)?, &SymbolPath::parse(path)?)
            .unwrap_err();
        Ok(format!("{err:#}"))
    }

    #[test]
    fn file_without_a_grammar_resolve_symbol_returns_an_error_with_the_reference()
    -> anyhow::Result<()> {
        assert_eq!(
            resolve_symbol_error("VERSION", "1.2.3\n", "/x")?,
            "failed to resolve symbol reference VERSION#/x: file format is unsupported"
        );
        Ok(())
    }

    #[test]
    fn unreadable_name_resolve_symbol_returns_an_error_with_the_reference() -> anyhow::Result<()> {
        assert_eq!(
            resolve_symbol_error("consts.go", "package p\n\nconst X = (1\n", "/X")?,
            "failed to resolve symbol reference consts.go#/X: file has a syntax error at line 3, column 1 near X, and the path could resolve to the wrong value"
        );
        Ok(())
    }

    #[test]
    fn language_without_symbols_resolve_symbol_returns_an_error_with_the_reference()
    -> anyhow::Result<()> {
        let message = resolve_symbol_error("config.rb", "x = 1\n", "/x")?;
        assert!(
            message.starts_with("failed to resolve symbol reference config.rb#/x: "),
            "unexpected message: {message}"
        );
        Ok(())
    }
}
