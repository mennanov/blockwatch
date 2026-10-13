use crate::fs::FileSystem;
use crate::repo_path::RepoPath;
use globset::{GlobBuilder, GlobMatcher};
use std::collections::BTreeSet;

/// A glob over the repository's paths, compiled to list the paths it matches.
pub(crate) struct PathsGlob {
    /// Matches a path without its trailing `/`.
    matcher: GlobMatcher,
    /// The directory that holds every match, or `None` for the repository root. Only it is walked.
    base: Option<RepoPath>,
    /// Whether the glob ends with `/`, which lists directories instead of files.
    list_directories: bool,
}

impl PathsGlob {
    /// Compiles `glob`.
    ///
    /// # Errors
    /// Returns an error if `glob` is not a valid glob.
    pub(crate) fn new(glob: &str) -> anyhow::Result<Self> {
        let (file_glob, list_directories) = match glob.strip_suffix('/') {
            Some(directory_glob) => (directory_glob, true),
            None => (glob, false),
        };
        // `*` stops at `/`, so `services/*/` lists the services and not the directories inside
        // them.
        let matcher = GlobBuilder::new(file_glob)
            .literal_separator(true)
            .build()?
            .compile_matcher();
        Ok(Self {
            matcher,
            base: base_directory(file_glob),
            list_directories,
        })
    }

    /// The paths in `file_system` that the glob matches, sorted and each listed once.
    ///
    /// A glob that lists directories is matched against the directories that hold a file, and each
    /// one is listed with a trailing `/`. Any other glob is matched against the files.
    ///
    /// # Errors
    /// Returns an error if the repository can't be walked.
    pub(crate) fn matching_paths(
        &self,
        file_system: &impl FileSystem,
    ) -> anyhow::Result<Vec<String>> {
        let mut paths = BTreeSet::new();
        for file in file_system.walk(self.base.as_ref()) {
            let file = file?;
            let file = file.as_str();
            if self.list_directories {
                for (end, _) in file.match_indices('/') {
                    let directory = &file[..end];
                    if self.matcher.is_match(directory) {
                        paths.insert(format!("{directory}/"));
                    }
                }
            } else if self.matcher.is_match(file) {
                paths.insert(file.to_string());
            }
        }
        Ok(paths.into_iter().collect())
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
        .take_while(|segment| !segment.contains(['*', '?', '[', '{', '\\']))
        .copied()
        .collect();
    // A directory that isn't a repository path, such as an empty one or one with `..`, can't narrow
    // the walk. Walking the whole repository gives the same paths, only more slowly.
    RepoPath::from_reference(&fixed_segments.join("/")).ok()
}
