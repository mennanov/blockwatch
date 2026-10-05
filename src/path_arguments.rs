use crate::fs::{Entry, FileSystem};
use crate::repo_path::RepoPath;
use anyhow::{Context, bail};
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::collections::BTreeSet;
use std::path::Path;

/// The paths and globs given on the command line, each turned into a glob that matches paths
/// relative to the repository root.
pub(crate) struct PathArguments {
    /// The arguments as written, in order.
    written: Vec<String>,
    /// One glob for each argument, in the same order.
    globs: GlobSet,
}

impl PathArguments {
    /// Turns each argument into a glob. A relative argument starts from the repository root, not
    /// from the current directory.
    ///
    /// An argument that points at an existing file selects only that file, even when its name is
    /// also a glob, such as `[id].py`. One that points at a directory selects every file under it.
    /// Any other argument is a glob, with a leading `./` removed.
    ///
    /// Errors when such a glob does not compile.
    pub(crate) fn resolve(
        arguments: Vec<String>,
        file_system: &impl FileSystem,
    ) -> anyhow::Result<Self> {
        let mut builder = GlobSetBuilder::new();
        for argument in &arguments {
            let glob = match file_system.entry(Path::new(argument)) {
                Some(Entry::Root) => "**".to_string(),
                Some(Entry::Directory(directory)) => {
                    format!("{}/**", globset::escape(directory.as_str()))
                }
                Some(Entry::File(file)) => globset::escape(file.as_str()),
                // A path from the walk never starts with `./`, so a glob that does would match
                // nothing.
                None => argument.trim_start_matches("./").to_string(),
            };
            builder.add(
                Glob::new(&glob).with_context(|| format!("Invalid glob pattern: {argument}"))?,
            );
        }
        Ok(Self {
            written: arguments,
            globs: builder.build().context("Failed to build glob set")?,
        })
    }

    /// The globs a file must match to be checked: one for each argument, or `**` when there are
    /// no arguments, because then every file is checked.
    pub(crate) fn allowed_globs(&self) -> anyhow::Result<GlobSet> {
        if self.written.is_empty() {
            return Ok(GlobSet::new([Glob::new("**")?])?);
        }
        Ok(self.globs.clone())
    }

    /// Checks that each argument selects at least one of `files`. Reads `files` only until every
    /// argument has selected one, so with no arguments it reads none.
    ///
    /// Errors with every argument that selects none of `files`, in the order they were written.
    /// Returns the first error in `files` that it reads.
    pub(crate) fn ensure_each_matches(
        &self,
        mut files: impl Iterator<Item = anyhow::Result<RepoPath>>,
    ) -> anyhow::Result<()> {
        let mut unmatched: BTreeSet<usize> = (0..self.written.len()).collect();
        let mut matches = Vec::new();
        while !unmatched.is_empty() {
            match files.next() {
                Some(file) => {
                    self.globs.matches_into(file?.as_path(), &mut matches);
                    for index in &matches {
                        unmatched.remove(index);
                    }
                }
                None => {
                    let quoted: Vec<String> = unmatched
                        .iter()
                        .map(|&index| format!("\"{}\"", self.written[index]))
                        .collect();
                    bail!(
                        "no file to check matches {}. Paths and globs start from the repository \
                         root, not from the current directory. A file is not checked if \
                         .gitignore, --ignore or the `ignore` key in the config file leaves it \
                         out, or if its extension is not supported.",
                        quoted.join(" or ")
                    );
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::test_utils::FakeFileSystem;
    use anyhow::anyhow;
    use std::collections::HashMap;

    /// Resolves `arguments` against a repository that holds `files`. Returns the files that the
    /// arguments select, in the order of `files`.
    fn selected_files(files: &[&str], arguments: &[&str]) -> anyhow::Result<Vec<String>> {
        let file_system = FakeFileSystem::new(
            files
                .iter()
                .map(|file| (file.to_string(), String::new()))
                .collect(),
        );
        let allowed_globs = resolve(&file_system, arguments)?.allowed_globs()?;
        Ok(files
            .iter()
            .filter(|file| allowed_globs.is_match(file))
            .map(ToString::to_string)
            .collect())
    }

    /// Resolves `arguments` against an empty repository, so each of them is a glob.
    fn resolve_globs(arguments: &[&str]) -> anyhow::Result<PathArguments> {
        resolve(&FakeFileSystem::new(HashMap::new()), arguments)
    }

    fn resolve(file_system: &impl FileSystem, arguments: &[&str]) -> anyhow::Result<PathArguments> {
        PathArguments::resolve(
            arguments.iter().map(ToString::to_string).collect(),
            file_system,
        )
    }

    /// Files that panic when read, for a check that must not read them.
    fn unreadable_files() -> impl Iterator<Item = anyhow::Result<RepoPath>> {
        std::iter::from_fn(|| panic!("the check read a file it did not need"))
    }

    #[test]
    fn every_argument_matched_ensure_each_matches_stops_reading_files() -> anyhow::Result<()> {
        let files =
            std::iter::once(Ok(RepoPath::from_reference("src/a.py")?)).chain(unreadable_files());

        resolve_globs(&["src/**"])?.ensure_each_matches(files)
    }

    #[test]
    fn no_arguments_ensure_each_matches_reads_no_file() -> anyhow::Result<()> {
        resolve_globs(&[])?.ensure_each_matches(unreadable_files())
    }

    #[test]
    fn directory_argument_selects_only_the_files_under_it() -> anyhow::Result<()> {
        assert_eq!(
            selected_files(&["src/a.py", "src.py", "srcx/b.py"], &["src"])?,
            ["src/a.py"]
        );
        Ok(())
    }

    #[test]
    fn directory_argument_with_glob_syntax_selects_only_the_files_under_it() -> anyhow::Result<()> {
        assert_eq!(
            selected_files(&["app/[id]/page.py", "app/i/page.py"], &["app/[id]"])?,
            ["app/[id]/page.py"]
        );
        Ok(())
    }

    #[test]
    fn glob_argument_with_leading_dot_slash_selects_files_from_the_root() -> anyhow::Result<()> {
        assert_eq!(
            selected_files(&["src/a.py", "b.py"], &["./src/*.py"])?,
            ["src/a.py"]
        );
        Ok(())
    }

    #[test]
    fn error_in_files_ensure_each_matches_returns_the_error() -> anyhow::Result<()> {
        let error = resolve_globs(&["src/**"])?
            .ensure_each_matches([Err(anyhow!("walk failed"))].into_iter())
            .expect_err("an error in the files must be returned");
        assert_eq!(error.to_string(), "walk failed");
        Ok(())
    }
}
