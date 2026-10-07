use crate::repo_path::RepoPath;
use anyhow::{Context, anyhow};
use globset::GlobSet;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

/// Directory names a version control system keeps its own state in.
///
/// A superset of the markers that identify a repository root.
// <block name="vcs-metadata-directories">
const VCS_METADATA_DIRECTORY_NAMES: [&str; 4] = [".git", ".hg", ".jj", ".svn"];
// </block>

/// Every read the program performs, behind a trait.
///
/// `Send + Sync` so an `Arc<Fs>` can be shared into validator threads (std::thread and Tokio).
pub(crate) trait FileSystem: Send + Sync {
    /// Reads the entire contents of a file as bytes.
    fn read(&self, path: &Path) -> anyhow::Result<Vec<u8>>;

    /// Reads the entire contents of a file into a string.
    ///
    /// # Errors
    /// Returns an error if the file can't be read or is not valid UTF-8.
    fn read_to_string(&self, path: &Path) -> anyhow::Result<String> {
        String::from_utf8(self.read(path)?)
            .with_context(|| format!("file \"{}\" is not valid UTF-8", path.display()))
    }

    /// Whether a readable file exists at `path` inside the repository.
    fn exists(&self, path: &Path) -> bool;

    /// The path of the file at `path`, relative to the repository root. Returns `None` if the file
    /// does not exist or is outside the repository.
    fn repo_path(&self, path: &Path) -> Option<RepoPath>;

    /// What `path` points at inside the repository. A relative `path` starts from the repository
    /// root. Symlinks are followed, so a path through a symlink gives the path of its target.
    ///
    /// Returns `None` if nothing exists at `path`, if it resolves to a place outside the
    /// repository, or if its path is not valid UTF-8.
    fn entry(&self, path: &Path) -> Option<Entry>;

    /// Walks the directory tree rooted at the file system's root path, returning an iterator over the paths of all files.
    fn walk(&self) -> impl Iterator<Item = anyhow::Result<RepoPath>>;
}

/// What a path points at inside the repository.
#[derive(Debug, PartialEq)]
pub(crate) enum Entry {
    /// The repository root itself.
    Root,
    /// A directory below the root.
    Directory(RepoPath),
    /// Anything that is not a directory, such as a file.
    File(RepoPath),
}

/// Checks whether a path should be allowed or ignored when parsing blocks from files.
pub(crate) trait PathChecker {
    /// Whether the given `path` should be explicitly allowed.
    fn should_allow(&self, path: &Path) -> bool;

    /// Whether the given `path` should be explicitly ignored.
    fn should_ignore(&self, path: &Path) -> bool;
}

/// The real filesystem, confined to one VCS repository.
///
/// Block attributes name other files (`affects="docs/cli.md:intro"`, `check-lua="scripts/x.lua"`),
/// and those names come from the files being linted. Routing every read through this type means a
/// crafted attribute cannot make the linter read outside the repository.
pub(crate) struct FileSystemImpl {
    /// The repository root, canonicalized so that containment checks compare like with like.
    root_path: PathBuf,
}

impl FileSystemImpl {
    /// Creates a reader confined to `root_path`, which must name an existing directory.
    ///
    /// The root is canonicalized here so every later resolution can compare against it directly.
    pub(crate) fn new(root_path: &Path) -> anyhow::Result<Self> {
        let root_path = std::fs::canonicalize(root_path).with_context(|| {
            format!(
                "failed to canonicalize repository root: {}",
                root_path.display()
            )
        })?;
        Ok(Self { root_path })
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
        let canonical = std::fs::canonicalize(&candidate)
            .with_context(|| format!("failed to canonicalize path \"{}\"", path.display()))?;
        if !canonical.starts_with(&self.root_path) {
            return Err(anyhow!(
                "path \"{}\" resolves to \"{}\" which is outside the repository root \"{}\"",
                path.display(),
                canonical.display(),
                self.root_path.display(),
            ));
        }
        Ok(canonical)
    }
}

impl FileSystem for FileSystemImpl {
    fn read(&self, path: &Path) -> anyhow::Result<Vec<u8>> {
        let resolved = self.resolve_within_root(path)?;
        std::fs::read(&resolved)
            .with_context(|| format!("Failed to read file \"{}\"", path.display()))
    }

    fn exists(&self, path: &Path) -> bool {
        // `resolve_within_root` fails for a missing path as well as for one escaping the root;
        // both mean "not a file this run may read".
        self.resolve_within_root(path)
            .is_ok_and(|resolved| resolved.is_file())
    }

    fn repo_path(&self, path: &Path) -> Option<RepoPath> {
        let resolved = self.resolve_within_root(path).ok()?;
        RepoPath::from_relative(resolved.strip_prefix(&self.root_path).ok()?).ok()
    }

    fn entry(&self, path: &Path) -> Option<Entry> {
        let resolved = self.resolve_within_root(path).ok()?;
        let relative = resolved.strip_prefix(&self.root_path).ok()?;
        if relative.as_os_str().is_empty() {
            return Some(Entry::Root);
        }
        let repo_path = RepoPath::from_relative(relative).ok()?;
        if resolved.is_dir() {
            Some(Entry::Directory(repo_path))
        } else {
            Some(Entry::File(repo_path))
        }
    }

    fn walk(&self) -> impl Iterator<Item = anyhow::Result<RepoPath>> {
        // Clone root_path for the closure.
        let root_path = self.root_path.clone();
        WalkBuilder::new(&self.root_path)
            // Hidden files should not be ignored as e.g. `.github` directory should be scanned.
            .hidden(false)
            .filter_entry(|entry| {
                !entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| VCS_METADATA_DIRECTORY_NAMES.contains(&name))
            })
            .build()
            .filter_map(move |entry| match entry {
                Ok(entry) => {
                    let path = entry.path();
                    if path.is_dir() {
                        return None;
                    }
                    // Relative to the root. A name that is not valid UTF-8 cannot be written in a
                    // glob or a block reference, so it is skipped rather than failing the run.
                    let relative_path = path.strip_prefix(&root_path).unwrap_or(path);
                    RepoPath::from_relative(relative_path).ok().map(Ok)
                }
                Err(err) => Some(Err(anyhow::Error::from(err))),
            })
    }
}

/// Checks whether a path should be allowed or ignored.
pub(crate) struct PathCheckerImpl {
    glob_set: GlobSet,
    ignored_glob_set: GlobSet,
}

impl PathCheckerImpl {
    /// Builds a checker from the compiled globs of the positional file filters and of `--ignore`.
    ///
    /// An empty `glob_set` matches nothing, so callers treat "no filters given" as "every file" on
    /// their own rather than relying on this type.
    pub(crate) fn new(glob_set: GlobSet, ignored_glob_set: GlobSet) -> Self {
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
    use crate::fs::{Entry, FileSystem, FileSystemImpl};
    use crate::repo_path::RepoPath;
    use std::path::{Path, PathBuf};

    /// Writes `content` to `name` inside a fresh temp dir that doubles as the repository root.
    /// Returns the held temp dir (kept alive for the test) and the file's absolute path.
    fn root_with_file(name: &str, content: &str) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(name);
        std::fs::write(&path, content).unwrap();
        (root, path)
    }

    /// Writes `content` to `relative_path` below `root`, creating the intermediate directories.
    fn write_file(root: &Path, relative_path: &str, content: &str) {
        let path = root.join(relative_path);
        std::fs::create_dir_all(path.parent().expect("a file path has a parent")).unwrap();
        std::fs::write(&path, content).unwrap();
    }

    /// The paths [`FileSystem::walk`] yields, relative to the root and sorted for comparison.
    fn walked_paths(file_system: &FileSystemImpl) -> anyhow::Result<Vec<String>> {
        let mut paths = file_system
            .walk()
            .map(|path| path.map(|path| path.as_str().to_owned()))
            .collect::<anyhow::Result<Vec<_>>>()?;
        paths.sort();
        Ok(paths)
    }

    #[test]
    fn walk_yields_files_inside_hidden_directories() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        write_file(root.path(), ".github/workflows/ci.yml", "name: CI");
        let file_system = FileSystemImpl::new(root.path())?;

        assert_eq!(walked_paths(&file_system)?, [".github/workflows/ci.yml"]);
        Ok(())
    }

    #[test]
    fn walk_yields_hidden_files() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        write_file(root.path(), ".eslintrc.yml", "root: true");
        let file_system = FileSystemImpl::new(root.path())?;

        assert_eq!(walked_paths(&file_system)?, [".eslintrc.yml"]);
        Ok(())
    }

    #[test]
    fn walk_skips_version_control_metadata_directories() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        for marker in [".git", ".hg", ".jj", ".svn"] {
            write_file(root.path(), &format!("{marker}/config.yml"), "internal");
        }
        write_file(root.path(), "src/main.yml", "name: app");
        let file_system = FileSystemImpl::new(root.path())?;

        assert_eq!(walked_paths(&file_system)?, ["src/main.yml"]);
        Ok(())
    }

    #[test]
    fn walk_skips_version_control_metadata_directories_below_the_root() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        write_file(root.path(), "vendor/dep/.git/config.yml", "internal");
        write_file(root.path(), "vendor/dep/main.yml", "name: dep");
        let file_system = FileSystemImpl::new(root.path())?;

        assert_eq!(walked_paths(&file_system)?, ["vendor/dep/main.yml"]);
        Ok(())
    }

    #[test]
    fn read_to_string_reads_relative_path_inside_root() -> anyhow::Result<()> {
        let (root, _path) = root_with_file("a.txt", "hello");
        let file_system = FileSystemImpl::new(root.path())?;

        assert_eq!(file_system.read_to_string(Path::new("a.txt"))?, "hello");
        Ok(())
    }

    #[test]
    fn read_to_string_reads_absolute_path_inside_root() -> anyhow::Result<()> {
        let (root, abs_path) = root_with_file("a.txt", "hello");
        let file_system = FileSystemImpl::new(root.path())?;

        assert_eq!(file_system.read_to_string(&abs_path)?, "hello");
        Ok(())
    }

    #[test]
    fn read_to_string_rejects_absolute_path_outside_root() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        // A file that exists and is readable, but lives outside the repository root.
        let (_outside_root, outside) = root_with_file("secret.txt", "secret");
        let file_system = FileSystemImpl::new(root.path())?;

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
        let file_system = FileSystemImpl::new(&root)?;

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
        let file_system = FileSystemImpl::new(root.path())?;

        let err = file_system
            .read_to_string(Path::new("does_not_exist.txt"))
            .unwrap_err();

        assert!(
            format!("{err:#}").contains("failed to canonicalize path"),
            "unexpected error: {err:#}"
        );
        Ok(())
    }

    #[test]
    fn read_to_string_rejects_non_utf8_file() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        // `é` in Latin-1 is the single byte 0xE9, which is not valid UTF-8.
        std::fs::write(root.path().join("a.txt"), b"caf\xe9")?;
        let file_system = FileSystemImpl::new(root.path())?;

        let err = file_system.read_to_string(Path::new("a.txt")).unwrap_err();

        assert!(
            format!("{err:#}").contains("file \"a.txt\" is not valid UTF-8"),
            "unexpected error: {err:#}"
        );
        Ok(())
    }

    #[test]
    fn absolute_path_inside_root_repo_path_returns_it_from_the_root() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        write_file(root.path(), "src/a.txt", "hello");
        let file_system = FileSystemImpl::new(root.path())?;

        let repo_path = file_system.repo_path(&root.path().join("src/a.txt"));

        assert_eq!(repo_path.as_ref().map(RepoPath::as_str), Some("src/a.txt"));
        Ok(())
    }

    #[test]
    fn path_outside_root_repo_path_returns_none() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let (_outside_root, outside) = root_with_file("a.txt", "hello");
        let file_system = FileSystemImpl::new(root.path())?;

        assert_eq!(file_system.repo_path(&outside), None);
        Ok(())
    }

    #[test]
    fn dot_entry_returns_the_root() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let file_system = FileSystemImpl::new(root.path())?;

        assert_eq!(file_system.entry(Path::new(".")), Some(Entry::Root));
        Ok(())
    }

    #[test]
    fn directory_with_trailing_slash_entry_returns_the_directory() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        write_file(root.path(), "src/a.txt", "hello");
        let file_system = FileSystemImpl::new(root.path())?;

        assert_eq!(
            file_system.entry(Path::new("src/")),
            Some(Entry::Directory(RepoPath::from_reference("src")?))
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn absolute_path_through_a_symlink_entry_returns_the_file_from_the_root() -> anyhow::Result<()>
    {
        let parent = tempfile::tempdir()?;
        let root = parent.path().join("repo");
        write_file(&root, "src/a.txt", "hello");
        std::os::unix::fs::symlink(&root, parent.path().join("link"))?;
        let file_system = FileSystemImpl::new(&root)?;

        assert_eq!(
            file_system.entry(&parent.path().join("link/src/a.txt")),
            Some(Entry::File(RepoPath::from_reference("src/a.txt")?))
        );
        Ok(())
    }

    #[test]
    fn missing_path_entry_returns_none() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let file_system = FileSystemImpl::new(root.path())?;

        assert_eq!(file_system.entry(Path::new("src/a.txt")), None);
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
        let file_system = FileSystemImpl::new(&root)?;

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

/// In-memory stand-ins for [`FileSystem`] and [`PathChecker`], so unit tests can describe a source
/// tree as a map of strings instead of creating temporary directories.
#[cfg(test)]
pub(crate) mod test_utils {
    use crate::fs::{Entry, FileSystem, PathChecker};
    use crate::repo_path::RepoPath;
    use globset::GlobSet;
    use std::collections::{HashMap, HashSet};
    use std::path::{Component, Path};

    /// A source tree held in memory, keyed by path exactly as it is spelled by the caller.
    ///
    /// Unlike [`super::FileSystemImpl`] it enforces no root confinement, so tests that care about
    /// containment must exercise the real implementation.
    pub(crate) struct FakeFileSystem {
        files: HashMap<String, String>,
    }

    impl FakeFileSystem {
        /// Creates a fake tree from a path -> contents map.
        pub(crate) fn new(files: HashMap<String, String>) -> Self {
            Self { files }
        }
    }

    impl FileSystem for FakeFileSystem {
        fn read(&self, path: &Path) -> anyhow::Result<Vec<u8>> {
            // Mirror a real filesystem: a missing file is an error, not a panic. This lets
            // validators' read-failure paths be exercised with the fake.
            self.files
                .get(&path.display().to_string())
                .map(|content| content.clone().into_bytes())
                .ok_or_else(|| anyhow::anyhow!("File {} not found", path.display()))
        }

        fn exists(&self, path: &Path) -> bool {
            self.files.contains_key(&path.display().to_string())
        }

        fn repo_path(&self, path: &Path) -> Option<RepoPath> {
            RepoPath::from_relative(path)
                .ok()
                .filter(|repo_path| self.exists(repo_path.as_path()))
        }

        fn entry(&self, path: &Path) -> Option<Entry> {
            if path
                .components()
                .all(|component| component == Component::CurDir)
            {
                return Some(Entry::Root);
            }
            // The fake has no place on disk, so an absolute path points at nothing.
            let repo_path = RepoPath::from_relative(path).ok()?;
            let files: Vec<RepoPath> = self
                .files
                .keys()
                .filter_map(|file| RepoPath::from_reference(file).ok())
                .collect();
            // There are no empty directories: a directory exists when a file is under it.
            let directory_prefix = format!("{}/", repo_path.as_str());
            if files.contains(&repo_path) {
                Some(Entry::File(repo_path))
            } else if files
                .iter()
                .any(|file| file.as_str().starts_with(&directory_prefix))
            {
                Some(Entry::Directory(repo_path))
            } else {
                None
            }
        }

        fn walk(&self) -> impl Iterator<Item = anyhow::Result<RepoPath>> {
            self.files.keys().map(|path| RepoPath::from_reference(path))
        }
    }

    /// A path filter for tests: an explicit deny list, so a file can be excluded without writing
    /// glob patterns, plus an optional allow-list of globs for the tests that are about globbing.
    pub(crate) struct FakePathChecker {
        /// The globs a path must match to be allowed. `None` allows every path, which is what most
        /// tests want; `Some` mirrors the real checker, whose empty glob set matches nothing.
        allowed_globs: Option<GlobSet>,
        ignored_paths: HashSet<String>,
    }

    impl FakePathChecker {
        /// Allows every path except those listed.
        pub(crate) fn with_ignored_paths(ignored_paths: HashSet<String>) -> Self {
            Self {
                allowed_globs: None,
                ignored_paths,
            }
        }

        /// Allows every path — the default for tests that are not about filtering.
        pub(crate) fn allow_all() -> Self {
            Self::with_ignored_paths(HashSet::new())
        }

        /// Allows only the paths matching `glob`.
        pub(crate) fn allow_only(glob: &str) -> Self {
            let glob_set = GlobSet::builder()
                .add(globset::Glob::new(glob).expect("malformed test glob"))
                .build()
                .expect("failed to build test glob set");
            Self {
                allowed_globs: Some(glob_set),
                ignored_paths: HashSet::new(),
            }
        }
    }

    impl PathChecker for FakePathChecker {
        fn should_allow(&self, path: &Path) -> bool {
            self.allowed_globs
                .as_ref()
                .is_none_or(|globs| globs.is_match(path))
        }

        fn should_ignore(&self, path: &Path) -> bool {
            self.ignored_paths.contains(&path.display().to_string())
        }
    }
}
