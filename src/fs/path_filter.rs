//! `PathFilter`: which paths a run reads.

use globset::GlobSet;
use std::path::Path;

/// Decides which paths a run reads: those that match an allow glob and no ignore glob.
pub(crate) struct PathFilter {
    allowed_glob_set: GlobSet,
    ignored_glob_set: GlobSet,
}

impl PathFilter {
    /// Builds a filter from the compiled globs of the positional file filters and of `--ignore`.
    ///
    /// An empty `allowed_glob_set` matches nothing, so callers treat "no filters given" as "every
    /// file" on their own rather than relying on this type.
    pub(crate) fn new(allowed_glob_set: GlobSet, ignored_glob_set: GlobSet) -> Self {
        Self {
            allowed_glob_set,
            ignored_glob_set,
        }
    }

    /// Whether `path` matches an allow glob and no ignore glob.
    pub(crate) fn is_selected(&self, path: &Path) -> bool {
        self.allowed_glob_set.is_match(path) && !self.is_ignored(path)
    }

    /// Whether `path` matches an ignore glob. The allow globs are not looked at.
    pub(crate) fn is_ignored(&self, path: &Path) -> bool {
        self.ignored_glob_set.is_match(path)
    }
}
