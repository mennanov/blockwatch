//! `PathsGlob`: a glob that lists the repository paths it matches.

use crate::fs::FileSystem;
use crate::repo_path::RepoPath;
use globset::{GlobBuilder, GlobMatcher};
use std::collections::BTreeSet;

/// The characters that have a special meaning in a glob. A glob without them matches only the
/// path it spells.
pub(crate) const GLOB_CHARACTERS: [char; 5] = ['*', '?', '[', '{', '\\'];

/// A glob over the repository's paths, compiled to list the files or the directories it matches.
pub(crate) struct PathsGlob {
    /// Matches a path written without a trailing `/`.
    matcher: GlobMatcher,
    /// The directory that holds every match, or `None` for the repository root. Only it is walked.
    base: Option<RepoPath>,
}

impl PathsGlob {
    /// Compiles `glob`, such as `services/*`. It matches paths written without a trailing `/`.
    ///
    /// # Errors
    /// Returns an error if `glob` is not a valid glob.
    pub(crate) fn new(glob: &str) -> anyhow::Result<Self> {
        // `*` stops at `/`, so `services/*` matches the services and not the directories inside
        // them.
        let matcher = GlobBuilder::new(glob)
            .literal_separator(true)
            .build()?
            .compile_matcher();
        Ok(Self {
            matcher,
            base: base_directory(glob),
        })
    }

    /// The files in `file_system` that the glob matches, sorted.
    ///
    /// # Errors
    /// Returns an error if the repository can't be walked.
    pub(crate) fn matching_files(
        &self,
        file_system: &impl FileSystem,
    ) -> anyhow::Result<Vec<RepoPath>> {
        let mut files = Vec::new();
        for file in file_system.walk(self.base.as_ref()) {
            let file = file?;
            if self.matcher.is_match(file.as_str()) {
                files.push(file);
            }
        }
        files.sort();
        Ok(files)
    }

    /// The directories in `file_system` that the glob matches and that hold a file, sorted and
    /// each listed once. Each one ends with `/`, such as `services/auth/`.
    ///
    /// # Errors
    /// Returns an error if the repository can't be walked.
    pub(crate) fn matching_directories(
        &self,
        file_system: &impl FileSystem,
    ) -> anyhow::Result<Vec<String>> {
        let mut directories = BTreeSet::new();
        for file in file_system.walk(self.base.as_ref()) {
            let file = file?;
            for (end, _) in file.as_str().match_indices('/') {
                #[expect(clippy::string_slice, reason = "`end` is the index of a `/`")]
                let directory = &file.as_str()[..end];
                if self.matcher.is_match(directory) {
                    directories.insert(format!("{directory}/"));
                }
            }
        }
        Ok(directories.into_iter().collect())
    }
}

/// The directory that holds every path `glob` matches: its segments before the first one with a
/// glob character, such as `services` for `services/*`. Returns `None` when that is the repository
/// root, as for `{services,workers}/*` or `**/AGENTS.md`.
fn base_directory(glob: &str) -> Option<RepoPath> {
    let segments: Vec<&str> = glob.split('/').collect();
    // The last segment is matched against a name, so it is never part of the directory.
    let fixed_segments: Vec<&str> = segments[..segments.len() - 1]
        .iter()
        .take_while(|segment| !segment.contains(GLOB_CHARACTERS))
        .copied()
        .collect();
    // A directory that isn't a repository path, such as an empty one or one with `..`, can't narrow
    // the walk. Walking the whole repository gives the same paths, only more slowly.
    RepoPath::from_reference(&fixed_segments.join("/")).ok()
}
