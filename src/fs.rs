use anyhow::{Context, anyhow};
use globset::GlobSet;
use ignore::Walk;
use std::path::{Path, PathBuf};

// `Send + Sync` so an `Arc<Fs>` can be shared into validator threads (std::thread and Tokio tasks).
pub trait FileSystem: Send + Sync {
    /// Reads the entire contents of a file into a string.
    fn read_to_string(&self, path: &Path) -> anyhow::Result<String>;

    /// Walks the directory tree rooted at the file system's root path, returning an iterator over the paths of all files.
    fn walk(&self) -> impl Iterator<Item = anyhow::Result<PathBuf>>;
}

/// Checks whether a path should be allowed or ignored when parsing blocks from files.
pub trait PathChecker {
    /// Whether the given `path` should be explicitly allowed.
    fn should_allow(&self, path: &Path) -> bool;

    /// Whether the given `path` should be explicitly ignored.
    fn should_ignore(&self, path: &Path) -> bool;
}

pub struct FileSystemImpl {
    root_path: PathBuf,
}

impl FileSystemImpl {
    /// Creates a new filesystem-backed reader rooted at `root_path`.
    pub fn new(root_path: PathBuf) -> Self {
        Self { root_path }
    }

    /// Resolves `path` against the repository root and guarantees the result stays inside it.
    ///
    /// Relative paths are joined to `root_path`; absolute paths are used as-is. Both the candidate
    /// and the root are canonicalized (resolving symlinks and `..`), and the canonical candidate
    /// must remain within the canonical root. This confines every read to the repository, rejecting
    /// `..` traversal, absolute escapes, and symlinks that resolve outside the root — so callers
    /// (e.g. the `check-lua` script path or a cross-file validator's target) get containment for
    /// free without re-implementing the check.
    fn resolve_within_root(&self, path: &Path) -> anyhow::Result<PathBuf> {
        let candidate = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root_path.join(path)
        };
        let canonical_root = std::fs::canonicalize(&self.root_path).with_context(|| {
            format!(
                "failed to canonicalize repository root: {}",
                self.root_path.display()
            )
        })?;
        let canonical = std::fs::canonicalize(&candidate)
            .with_context(|| format!("failed to canonicalize path \"{}\"", path.display()))?;
        if !canonical.starts_with(&canonical_root) {
            return Err(anyhow!(
                "path \"{}\" resolves to \"{}\" which is outside the repository root \"{}\"",
                path.display(),
                canonical.display(),
                canonical_root.display(),
            ));
        }
        Ok(canonical)
    }
}

impl FileSystem for FileSystemImpl {
    fn read_to_string(&self, path: &Path) -> anyhow::Result<String> {
        let resolved = self.resolve_within_root(path)?;
        std::fs::read_to_string(&resolved)
            .with_context(|| format!("Failed to read file \"{}\"", path.display()))
    }

    fn walk(&self) -> impl Iterator<Item = anyhow::Result<PathBuf>> {
        // Clone root_path for the closure.
        let root_path = self.root_path.clone();
        Walk::new(&self.root_path).filter_map(move |entry| match entry {
            Ok(entry) => {
                let path = entry.path();
                if path.is_dir() {
                    return None;
                }
                // Return path relative to the root.
                let relative_path = path.strip_prefix(&root_path).unwrap_or(path);
                Some(Ok(relative_path.to_path_buf()))
            }
            Err(err) => Some(Err(anyhow::Error::from(err))),
        })
    }
}

/// Checks whether a path should be allowed or ignored.
pub struct PathCheckerImpl {
    glob_set: GlobSet,
    ignored_glob_set: GlobSet,
}

impl PathCheckerImpl {
    pub fn new(glob_set: GlobSet, ignored_glob_set: GlobSet) -> Self {
        Self {
            glob_set,
            ignored_glob_set,
        }
    }
}

impl PathChecker for PathCheckerImpl {
    fn should_allow(&self, path: &Path) -> bool {
        self.glob_set.is_match(path)
    }

    fn should_ignore(&self, path: &Path) -> bool {
        self.ignored_glob_set.is_match(path)
    }
}

#[cfg(test)]
mod file_system_impl_tests {
    use crate::fs::{FileSystem, FileSystemImpl};
    use std::path::{Path, PathBuf};

    /// Writes `content` to `name` inside a fresh temp dir that doubles as the repository root.
    /// Returns the held temp dir (kept alive for the test) and the file's absolute path.
    fn root_with_file(name: &str, content: &str) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(name);
        std::fs::write(&path, content).unwrap();
        (root, path)
    }

    #[test]
    fn read_to_string_reads_relative_path_inside_root() -> anyhow::Result<()> {
        let (root, _path) = root_with_file("a.txt", "hello");
        let file_system = FileSystemImpl::new(root.path().to_path_buf());

        assert_eq!(file_system.read_to_string(Path::new("a.txt"))?, "hello");
        Ok(())
    }

    #[test]
    fn read_to_string_reads_absolute_path_inside_root() -> anyhow::Result<()> {
        let (root, abs_path) = root_with_file("a.txt", "hello");
        let file_system = FileSystemImpl::new(root.path().to_path_buf());

        assert_eq!(file_system.read_to_string(&abs_path)?, "hello");
        Ok(())
    }

    #[test]
    fn read_to_string_rejects_absolute_path_outside_root() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        // A file that exists and is readable, but lives outside the repository root.
        let (_outside_root, outside) = root_with_file("secret.txt", "secret");
        let file_system = FileSystemImpl::new(root.path().to_path_buf());

        let err = file_system.read_to_string(&outside).unwrap_err();

        assert!(
            format!("{err:#}").contains("outside the repository root"),
            "unexpected error: {err:#}"
        );
        Ok(())
    }

    #[test]
    fn read_to_string_rejects_relative_path_escaping_root() -> anyhow::Result<()> {
        let parent = tempfile::tempdir()?;
        std::fs::write(parent.path().join("evil.txt"), "evil")?;
        let root = parent.path().join("repo");
        std::fs::create_dir(&root)?;
        let file_system = FileSystemImpl::new(root);

        let err = file_system
            .read_to_string(Path::new("../evil.txt"))
            .unwrap_err();

        assert!(
            format!("{err:#}").contains("outside the repository root"),
            "unexpected error: {err:#}"
        );
        Ok(())
    }

    #[test]
    fn read_to_string_rejects_missing_path() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let file_system = FileSystemImpl::new(root.path().to_path_buf());

        let err = file_system
            .read_to_string(Path::new("does_not_exist.txt"))
            .unwrap_err();

        assert!(
            format!("{err:#}").contains("failed to canonicalize path"),
            "unexpected error: {err:#}"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn read_to_string_rejects_symlink_escaping_root() -> anyhow::Result<()> {
        let parent = tempfile::tempdir()?;
        std::fs::write(parent.path().join("secret.txt"), "secret")?;
        let root = parent.path().join("repo");
        std::fs::create_dir(&root)?;
        std::os::unix::fs::symlink(parent.path().join("secret.txt"), root.join("link.txt"))?;
        let file_system = FileSystemImpl::new(root);

        let err = file_system
            .read_to_string(Path::new("link.txt"))
            .unwrap_err();

        assert!(
            format!("{err:#}").contains("outside the repository root"),
            "unexpected error: {err:#}"
        );
        Ok(())
    }
}

#[cfg(test)]
pub mod test_utils {
    use crate::fs::{FileSystem, PathChecker};
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};

    pub(crate) struct FakeFileSystem {
        files: HashMap<String, String>,
    }

    impl FakeFileSystem {
        pub(crate) fn new(files: HashMap<String, String>) -> Self {
            Self { files }
        }
    }

    impl FileSystem for FakeFileSystem {
        fn read_to_string(&self, path: &Path) -> anyhow::Result<String> {
            // Mirror a real filesystem: a missing file is an error, not a panic. This lets
            // validators' read-failure paths be exercised with the fake.
            self.files
                .get(&path.display().to_string())
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("File {} not found", path.display()))
        }

        fn walk(&self) -> impl Iterator<Item = anyhow::Result<PathBuf>> {
            self.files.keys().map(|p| Ok(PathBuf::from(p)))
        }
    }

    pub(crate) struct FakePathChecker {
        ignored_paths: HashSet<String>,
    }

    impl FakePathChecker {
        pub(crate) fn with_ignored_paths(ignored_paths: HashSet<String>) -> Self {
            Self { ignored_paths }
        }

        pub(crate) fn allow_all() -> Self {
            Self::with_ignored_paths(HashSet::new())
        }
    }

    impl PathChecker for FakePathChecker {
        fn should_allow(&self, _unused_path: &Path) -> bool {
            true
        }

        fn should_ignore(&self, path: &Path) -> bool {
            self.ignored_paths.contains(&path.display().to_string())
        }
    }
}
