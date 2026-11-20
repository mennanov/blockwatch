//! `RepoPath`: the single spelling of a repository-relative file path used as a map key.

use anyhow::{anyhow, bail};
use serde::{Serialize, Serializer};
use std::fmt;
use std::path::{Component, Path};

/// A file inside the repository, in the one spelling the whole program uses.
///
/// The same file arrives spelled differently from a diff header (`b/src/main.rs`), a directory
/// walk (platform separators) and a `file:name` attribute (`./src/main.rs`). These paths are map
/// keys, so a difference in spelling silently means a different file.
///
/// The constructors establish the invariants once:
///
/// - relative to the repository root, with no `..` component and no root prefix,
/// - `/`-separated on every platform, so a file has one key on Windows and Unix alike,
/// - free of `.` segments, non-empty, and valid UTF-8.
///
/// Windows accepts `/` in its filesystem APIs, so [`RepoPath::as_path`] stays usable for reads.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct RepoPath(String);

impl RepoPath {
    /// Builds a path from a value that is already relative to the repository root, such as an
    /// entry produced by the directory walk.
    ///
    /// Returns an error for anything that could name a file outside the repository — an absolute
    /// path, or one containing `..` — so that confinement does not depend on the caller
    /// remembering to check.
    pub(crate) fn from_relative(path: &Path) -> anyhow::Result<Self> {
        let mut segments = Vec::new();
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(segment) => segments.push(
                    segment
                        .to_str()
                        .ok_or_else(|| anyhow!("path \"{}\" is not valid UTF-8", path.display()))?,
                ),
                _ => bail!("path \"{}\" escapes the repository root", path.display()),
            }
        }
        if segments.is_empty() {
            bail!("empty repository path");
        }
        Ok(Self(segments.join("/")))
    }

    /// Builds a path from the file part of a `file:name` reference attribute.
    ///
    /// Authors spell these by hand, so `./target.py` and `target.py` both occur and must resolve
    /// to the same file.
    pub(crate) fn from_reference(reference: &str) -> anyhow::Result<Self> {
        Self::from_relative(Path::new(reference))
    }

    /// Borrows the path for filesystem operations.
    pub(crate) fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }

    /// Borrows the canonical `/`-separated spelling.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<Path> for RepoPath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl fmt::Display for RepoPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for RepoPath {
    /// Serializes as a plain string so the type works as a JSON object key, and so diagnostics
    /// report the same path spelling on every platform.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

#[cfg(test)]
mod repo_path_tests {
    use super::*;

    #[test]
    fn from_relative_keeps_a_plain_path() -> anyhow::Result<()> {
        assert_eq!(
            RepoPath::from_relative(Path::new("src/main.rs"))?.as_str(),
            "src/main.rs"
        );
        Ok(())
    }

    #[test]
    fn from_relative_drops_current_directory_segments() -> anyhow::Result<()> {
        assert_eq!(
            RepoPath::from_relative(Path::new("./src/./main.rs"))?.as_str(),
            "src/main.rs"
        );
        Ok(())
    }

    #[test]
    fn from_relative_rejects_parent_directory_segments() {
        assert!(RepoPath::from_relative(Path::new("../secret.txt")).is_err());
    }

    #[test]
    fn from_relative_rejects_absolute_paths() {
        assert!(RepoPath::from_relative(Path::new("/etc/passwd")).is_err());
    }

    #[test]
    fn from_relative_rejects_an_empty_path() {
        assert!(RepoPath::from_relative(Path::new("")).is_err());
    }

    #[test]
    fn from_reference_normalizes_a_leading_current_directory() -> anyhow::Result<()> {
        // `affects="./target.py:target"` must resolve to the same key as `affects="target.py:target"`.
        assert_eq!(
            RepoPath::from_reference("./target.py")?,
            RepoPath::from_reference("target.py")?
        );
        Ok(())
    }

    #[test]
    fn equal_paths_hash_equally() -> anyhow::Result<()> {
        use std::collections::HashMap;
        let mut map = HashMap::new();
        map.insert(RepoPath::from_reference("./a/b.py")?, 1);
        assert_eq!(map.get(&RepoPath::from_reference("a/b.py")?), Some(&1));
        Ok(())
    }

    #[test]
    fn display_uses_forward_slashes() -> anyhow::Result<()> {
        assert_eq!(
            RepoPath::from_relative(Path::new("a/b.py"))?.to_string(),
            "a/b.py"
        );
        Ok(())
    }

    #[test]
    fn serializes_as_a_plain_string() -> anyhow::Result<()> {
        let value = serde_json::to_value(RepoPath::from_relative(Path::new("a/b.py"))?)?;
        assert_eq!(value, serde_json::json!("a/b.py"));
        Ok(())
    }

    #[test]
    fn serializes_as_a_map_key() -> anyhow::Result<()> {
        use std::collections::HashMap;
        let map = HashMap::from([(RepoPath::from_relative(Path::new("a/b.py"))?, 1)]);
        assert_eq!(serde_json::to_value(map)?, serde_json::json!({"a/b.py": 1}));
        Ok(())
    }

    #[test]
    fn borrows_as_a_path() -> anyhow::Result<()> {
        let path = RepoPath::from_relative(Path::new("a/b.py"))?;
        assert_eq!(
            path.as_ref().extension().and_then(|e| e.to_str()),
            Some("py")
        );
        Ok(())
    }
}
