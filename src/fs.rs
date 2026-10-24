//! File access and path filtering behind traits, so tests can substitute fakes for the real disk.

use crate::repo_path::RepoPath;
use anyhow::{Context, anyhow};
use globset::GlobSet;
use ignore::WalkBuilder;
#[cfg(test)]
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Directory names a version control system keeps its own state in.
///
/// A superset of the markers that identify a repository root.
// <block name="vcs-metadata-directories" same-as-pattern='"(?P<value>\.[a-z]+)"'>
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

    /// Whether `path` is a symlink. The link itself is checked, not the file it points at, so a
    /// dangling link is a symlink too. Returns `false` if nothing exists at `path`.
    fn is_symlink(&self, path: &RepoPath) -> bool;

    /// Walks the directory tree rooted at the file system's root path, returning an iterator over
    /// the paths of all regular files. A symlink is not a regular file, so it is never returned.
    ///
    /// With `directory`, only the files below it are returned. A `directory` that doesn't exist,
    /// or that is reached through a symlink, gives no files.
    fn walk(&self, directory: Option<&RepoPath>) -> impl Iterator<Item = anyhow::Result<RepoPath>>;
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
            .with_context(|| format!("failed to read file \"{}\"", path.display()))
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

    fn is_symlink(&self, path: &RepoPath) -> bool {
        std::fs::symlink_metadata(self.root_path.join(path.as_path()))
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
    }

    fn walk(&self, directory: Option<&RepoPath>) -> impl Iterator<Item = anyhow::Result<RepoPath>> {
        // Clone root_path for the closures.
        let root_path = self.root_path.clone();
        let filter_root_path = self.root_path.clone();
        let directory = directory.map(|directory| directory.as_path().to_path_buf());
        WalkBuilder::new(&self.root_path)
            // Hidden files should not be ignored as e.g. `.github` directory should be scanned.
            .hidden(false)
            .filter_entry(move |entry| {
                let is_vcs_metadata = entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| VCS_METADATA_DIRECTORY_NAMES.contains(&name));
                // The walk starts at the root even for a `directory`. So a symlink on the way to
                // it is skipped like any other, and can't lead out of the repository. Entering only
                // the directories on the way to it keeps the rest of the tree unread.
                let relative_path = entry
                    .path()
                    .strip_prefix(&filter_root_path)
                    .unwrap_or(entry.path());
                let is_wanted = directory.as_ref().is_none_or(|directory| {
                    let is_directory = entry
                        .file_type()
                        .is_some_and(|file_type| file_type.is_dir());
                    (is_directory && directory.starts_with(relative_path))
                        || (relative_path != directory && relative_path.starts_with(directory))
                });
                !is_vcs_metadata && is_wanted
            })
            .build()
            .filter_map(move |entry| match entry {
                Ok(entry) => {
                    // The walk doesn't follow links, so a symlink has its own file type here and
                    // is skipped. Its target is checked under the target's own path, so a linked
                    // file is not checked twice. A dangling link is skipped too.
                    if !entry
                        .file_type()
                        .is_some_and(|file_type| file_type.is_file())
                    {
                        return None;
                    }
                    let path = entry.path();
                    // Relative to the root. A name that is not valid UTF-8 cannot be written in a
                    // glob or a block reference, so it is skipped rather than failing the run.
                    let relative_path = path.strip_prefix(&root_path).unwrap_or(path);
                    RepoPath::from_relative(relative_path).ok().map(Ok)
                }
                Err(err) => Some(Err(anyhow::Error::from(err))),
            })
    }
}

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

/// Constructors that let a test describe a filter without building glob sets.
#[cfg(test)]
impl PathFilter {
    /// Selects every path.
    pub(crate) fn allow_all() -> Self {
        Self::allow_only("**")
    }

    /// Selects only the paths matching `glob`.
    pub(crate) fn allow_only(glob: &str) -> Self {
        Self::new(glob_set([glob]), GlobSet::empty())
    }

    /// Selects every path except those listed.
    pub(crate) fn with_ignored_paths(ignored_paths: HashSet<String>) -> Self {
        Self::new(glob_set(["**"]), glob_set(ignored_paths))
    }
}

/// Compiles `globs`, for the test constructors of [`PathFilter`].
#[cfg(test)]
fn glob_set(globs: impl IntoIterator<Item = impl AsRef<str>>) -> GlobSet {
    let mut builder = GlobSet::builder();
    for glob in globs {
        builder.add(
            crate::paths_glob::glob_builder(glob.as_ref())
                .build()
                .expect("malformed test glob"),
        );
    }
    builder.build().expect("failed to build test glob set")
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

    /// The paths [`FileSystem::walk`] yields under `directory`, relative to the root and sorted for
    /// comparison.
    fn walked_paths(
        file_system: &FileSystemImpl,
        directory: Option<&str>,
    ) -> anyhow::Result<Vec<String>> {
        let directory = directory.map(RepoPath::from_reference).transpose()?;
        let mut paths = file_system
            .walk(directory.as_ref())
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

        assert_eq!(
            walked_paths(&file_system, None)?,
            [".github/workflows/ci.yml"]
        );
        Ok(())
    }

    #[test]
    fn walk_yields_hidden_files() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        write_file(root.path(), ".eslintrc.yml", "root: true");
        let file_system = FileSystemImpl::new(root.path())?;

        assert_eq!(walked_paths(&file_system, None)?, [".eslintrc.yml"]);
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

        assert_eq!(walked_paths(&file_system, None)?, ["src/main.yml"]);
        Ok(())
    }

    #[test]
    fn walk_skips_version_control_metadata_directories_below_the_root() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        write_file(root.path(), "vendor/dep/.git/config.yml", "internal");
        write_file(root.path(), "vendor/dep/main.yml", "name: dep");
        let file_system = FileSystemImpl::new(root.path())?;

        assert_eq!(walked_paths(&file_system, None)?, ["vendor/dep/main.yml"]);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn walk_skips_symlinks_to_files() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        write_file(root.path(), "AGENTS.md", "rules");
        std::os::unix::fs::symlink("AGENTS.md", root.path().join("CLAUDE.md"))?;
        let file_system = FileSystemImpl::new(root.path())?;

        assert_eq!(walked_paths(&file_system, None)?, ["AGENTS.md"]);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn walk_skips_dangling_symlinks() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        std::os::unix::fs::symlink("missing.md", root.path().join("link.md"))?;
        let file_system = FileSystemImpl::new(root.path())?;

        assert_eq!(walked_paths(&file_system, None)?, [] as [&str; 0]);
        Ok(())
    }

    #[test]
    fn walk_under_a_directory_yields_only_the_files_below_it() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        for path in ["a/b/y.txt", "a/x.txt", "a/bc/z.txt", "c.txt"] {
            write_file(root.path(), path, "");
        }
        let file_system = FileSystemImpl::new(root.path())?;

        assert_eq!(walked_paths(&file_system, Some("a/b"))?, ["a/b/y.txt"]);
        // A file has nothing below it.
        assert_eq!(walked_paths(&file_system, Some("c.txt"))?, [] as [&str; 0]);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn walk_under_a_symlinked_directory_yields_nothing() -> anyhow::Result<()> {
        let outside = tempfile::tempdir()?;
        write_file(outside.path(), "secret.txt", "secret");
        let root = tempfile::tempdir()?;
        std::os::unix::fs::symlink(outside.path(), root.path().join("link"))?;
        let file_system = FileSystemImpl::new(root.path())?;

        assert_eq!(walked_paths(&file_system, Some("link"))?, [] as [&str; 0]);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlink_is_symlink_returns_true_and_its_target_returns_false() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        write_file(root.path(), "AGENTS.md", "rules");
        std::os::unix::fs::symlink("AGENTS.md", root.path().join("CLAUDE.md"))?;
        let file_system = FileSystemImpl::new(root.path())?;

        assert!(file_system.is_symlink(&RepoPath::from_reference("CLAUDE.md")?));
        assert!(!file_system.is_symlink(&RepoPath::from_reference("AGENTS.md")?));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_is_symlink_returns_true() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        std::os::unix::fs::symlink("missing.md", root.path().join("link.md"))?;
        let file_system = FileSystemImpl::new(root.path())?;

        assert!(file_system.is_symlink(&RepoPath::from_reference("link.md")?));
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

/// An in-memory stand-in for [`FileSystem`], so unit tests can describe a source tree as a map of
/// strings instead of creating temporary directories.
#[cfg(test)]
pub(crate) mod test_utils {
    use crate::fs::{Entry, FileSystem};
    use crate::repo_path::RepoPath;
    use std::collections::HashMap;
    use std::path::{Component, Path};

    /// A source tree held in memory, keyed by path exactly as it is spelled by the caller.
    ///
    /// Unlike [`super::FileSystemImpl`] it enforces no root confinement, so tests that care about
    /// containment must exercise the real implementation.
    pub(crate) struct FakeFileSystem {
        files: HashMap<String, String>,
        /// Each symlink's path, and the path it points at. A link needs no entry in `files`. As on
        /// disk, every method follows a link, except `is_symlink` and `walk`.
        symlinks: HashMap<String, String>,
    }

    impl FakeFileSystem {
        /// Creates a fake tree from a path -> contents map, with no symlinks.
        pub(crate) fn new(files: HashMap<String, String>) -> Self {
            Self {
                files,
                symlinks: HashMap::new(),
            }
        }

        /// Returns the tree with `symlinks` added, each as (link, the path it points at).
        pub(crate) fn with_symlinks(mut self, symlinks: &[(&str, &str)]) -> Self {
            self.symlinks.extend(
                symlinks
                    .iter()
                    .map(|(link, target)| ((*link).to_string(), (*target).to_string())),
            );
            self
        }

        /// The key of `path` in `files`: the path a symlink points at, or else `path` itself.
        fn file_key(&self, path: &Path) -> String {
            let key = path.display().to_string();
            self.symlinks.get(&key).cloned().unwrap_or(key)
        }
    }

    impl FileSystem for FakeFileSystem {
        fn read(&self, path: &Path) -> anyhow::Result<Vec<u8>> {
            // Mirror a real filesystem: a missing file is an error, not a panic. This lets
            // validators' read-failure paths be exercised with the fake.
            self.files
                .get(&self.file_key(path))
                .map(|content| content.clone().into_bytes())
                .ok_or_else(|| anyhow::anyhow!("file {} not found", path.display()))
        }

        fn exists(&self, path: &Path) -> bool {
            self.files.contains_key(&self.file_key(path))
        }

        fn repo_path(&self, path: &Path) -> Option<RepoPath> {
            RepoPath::from_relative(Path::new(&self.file_key(path)))
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
            let repo_path = RepoPath::from_relative(Path::new(&self.file_key(path))).ok()?;
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

        fn is_symlink(&self, path: &RepoPath) -> bool {
            self.symlinks.contains_key(path.as_str())
        }

        fn walk(
            &self,
            directory: Option<&RepoPath>,
        ) -> impl Iterator<Item = anyhow::Result<RepoPath>> {
            let prefix = directory.map(|directory| format!("{}/", directory.as_str()));
            self.files
                .keys()
                .filter(move |path| {
                    prefix
                        .as_ref()
                        .is_none_or(|prefix| path.starts_with(prefix.as_str()))
                })
                .map(|path| RepoPath::from_reference(path))
        }
    }
}
