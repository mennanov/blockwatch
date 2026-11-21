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

/// Test helpers for building path filters.
#[cfg(test)]
pub(crate) mod test_utils {
    use super::PathFilter;
    use crate::fs::glob::glob_builder;
    use globset::GlobSet;

    /// Selects every path.
    pub(crate) fn path_filter_all() -> PathFilter {
        path_filter_only("**")
    }

    /// Selects only the paths matching `glob`.
    pub(crate) fn path_filter_only(glob: &str) -> PathFilter {
        PathFilter::new(glob_set([glob]), GlobSet::empty())
    }

    /// Selects every path except those matching `ignored_globs`.
    pub(crate) fn path_filter_ignoring(
        ignored_globs: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> PathFilter {
        PathFilter::new(glob_set(["**"]), glob_set(ignored_globs))
    }

    /// Compiles `globs` for the test filter builders.
    fn glob_set(globs: impl IntoIterator<Item = impl AsRef<str>>) -> GlobSet {
        let mut builder = GlobSet::builder();
        for glob in globs {
            builder.add(
                glob_builder(glob.as_ref())
                    .build()
                    .expect("malformed test glob"),
            );
        }
        builder.build().expect("failed to build test glob set")
    }
}
