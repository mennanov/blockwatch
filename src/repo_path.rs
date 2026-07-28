use crate::fs::FileSystem;
use anyhow::{Context, anyhow, bail};
use serde::{Serialize, Serializer};
use std::fmt;
use std::ops::Deref;
use std::path::{Component, Path};
use winnow::Result as PResult;
use winnow::combinator::{alt, delimited, preceded, repeat};
use winnow::prelude::*;
use winnow::token::{one_of, take_while};

/// The stand-in Git writes for the missing side of an added or deleted file.
const DEV_NULL: &str = "/dev/null";

/// The one-component prefixes Git puts ahead of a diff path: `a/` and `b/` by default, and the
/// four `diff.mnemonicPrefix` spellings. Any other prefix, or none, is rejected — see
/// [`RepoPath::from_diff_target`].
const DIFF_PATH_PREFIXES: [&str; 6] = ["a/", "b/", "i/", "w/", "c/", "o/"];

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
pub struct RepoPath(String);

impl RepoPath {
    /// Builds a path from a value that is already relative to the repository root, such as an
    /// entry produced by the directory walk.
    ///
    /// Returns an error for anything that could name a file outside the repository — an absolute
    /// path, or one containing `..` — so that confinement does not depend on the caller
    /// remembering to check.
    pub fn from_relative(path: &Path) -> anyhow::Result<Self> {
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
    pub fn from_reference(reference: &str) -> anyhow::Result<Self> {
        Self::from_relative(Path::new(reference))
    }

    /// Resolves a diff header's target to the repository path it names.
    ///
    /// Exactly one of the prefixes Git writes ahead of a diff path is removed; a target carrying
    /// none is rejected rather than guessed at. See "Supported Diff Input" in `docs/cli.md`.
    ///
    /// The path is returned whether or not it names an existing file; `parse_blocks` decides
    /// whether an absent one matters.
    pub fn from_diff_target(
        source: &str,
        target: &str,
        file_system: &impl FileSystem,
    ) -> anyhow::Result<Option<Self>> {
        if target == DEV_NULL {
            // The missing side of a deleted file; there is nothing to validate.
            return Ok(None);
        }
        let decoded_target = unquote_git_path(target)?;
        let decoded_source = if source == DEV_NULL {
            None
        } else {
            unquote_git_path(source).ok()
        };
        // A header repeating one prefix on both sides carries none.
        let repeats_prefix = decoded_source.as_ref().is_some_and(|source| {
            source.split_once('/').map(|(prefix, _)| prefix)
                == decoded_target.split_once('/').map(|(prefix, _)| prefix)
        });
        let stripped = if repeats_prefix {
            None
        } else {
            DIFF_PATH_PREFIXES
                .iter()
                .find_map(|prefix| decoded_target.strip_prefix(prefix))
        };
        let Some(without_prefix) = stripped else {
            bail!(
                "diff target \"{decoded_target}\" has no recognized Git path prefix.\n\
                 BlockWatch reads diffs written with the prefixes Git produces by default. This \
                 one looks like the output of --no-prefix, diff.noprefix, or a custom \
                 diff.srcPrefix/diff.dstPrefix. Re-run with:\n    git diff --default-prefix"
            );
        };
        // `from_relative` rejects anything that would escape the repository, including the rooted
        // path left behind by a target such as "b//etc/passwd".
        let path = Self::from_relative(Path::new(without_prefix))?;

        // "/dev/null" says nothing about the target's prefix, so the working tree decides.
        let path = if decoded_source.is_none() {
            let unprefixed = Self::from_relative(Path::new(&decoded_target))
                .ok()
                .filter(|candidate| file_system.exists(candidate.as_path()));
            match unprefixed {
                Some(unprefixed) if file_system.exists(path.as_path()) => bail!(
                    "cannot tell whether the diff writes Git's path prefixes: \"{decoded_target}\" reads \
                     as both \"{path}\" and \"{unprefixed}\", and both exist.\nThis file is newly \
                     added, and an added file's header records no prefix. Re-run with:\n    \
                     git diff --default-prefix"
                ),
                Some(unprefixed) => unprefixed,
                None => path,
            }
        } else {
            path
        };

        Ok(Some(path))
    }

    /// Borrows the path for filesystem operations.
    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }

    /// Borrows the canonical `/`-separated spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for RepoPath {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
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

/// Decodes a diff-header path, undoing the C-style quoting Git applies to paths that contain
/// non-ASCII bytes or characters needing escapes.
///
/// Git's default `core.quotePath` setting writes `café.py` as `"caf\303\251.py"`: the whole path
/// is wrapped in double quotes and individual *bytes* are escaped in octal, so the escapes are
/// decoded to bytes and only then interpreted as UTF-8. An unquoted path is returned unchanged.
fn unquote_git_path(raw: &str) -> anyhow::Result<String> {
    if !raw.starts_with('"') {
        return Ok(raw.to_string());
    }
    let bytes = quoted_path
        .parse(raw)
        .map_err(|error| anyhow!("diff path {raw} is not validly quoted: {error}"))?;
    String::from_utf8(bytes).with_context(|| format!("diff path {raw} is not valid UTF-8"))
}

/// A double-quoted path, as the bytes it stands for.
fn quoted_path(input: &mut &str) -> PResult<Vec<u8>> {
    delimited(
        '"',
        repeat(0.., path_chunk).fold(Vec::new, |mut decoded: Vec<u8>, chunk: Chunk| {
            match chunk {
                Chunk::Byte(byte) => decoded.push(byte),
                Chunk::Literal(text) => decoded.extend_from_slice(text.as_bytes()),
            }
            decoded
        }),
        '"',
    )
    .parse_next(input)
}

/// What one step of [`quoted_path`] contributes: an escape denotes a single byte, while an
/// unescaped run is copied through as-is.
enum Chunk<'a> {
    Byte(u8),
    Literal(&'a str),
}

/// One escape sequence, or the longest run of characters needing none.
fn path_chunk<'a>(input: &mut &'a str) -> PResult<Chunk<'a>> {
    alt((
        preceded('\\', escape).map(Chunk::Byte),
        take_while(1.., |c: char| c != '"' && c != '\\').map(Chunk::Literal),
    ))
    .parse_next(input)
}

/// The byte an escape sequence denotes, given the text after its backslash.
fn escape(input: &mut &str) -> PResult<u8> {
    alt((
        // One to three octal digits denote one byte, which is how Git spells every byte of a
        // non-ASCII name.
        take_while(1..=3, |c: char| c.is_digit(8))
            .verify_map(|digits: &str| u8::try_from(u32::from_str_radix(digits, 8).ok()?).ok()),
        one_of(['a', 'b', 'f', 'n', 'r', 't', 'v', '\\', '"']).map(|escape: char| match escape {
            'a' => 0x07,
            'b' => 0x08,
            'f' => 0x0C,
            'n' => b'\n',
            'r' => b'\r',
            't' => b'\t',
            'v' => 0x0B,
            other => other as u8,
        }),
    ))
    .parse_next(input)
}

#[cfg(test)]
mod repo_path_tests {
    use super::*;
    use crate::fs::test_utils::FakeFileSystem;
    use std::collections::HashMap;

    /// A fake tree containing exactly the named files, for exercising diff-target resolution.
    fn tree(files: &[&str]) -> FakeFileSystem {
        FakeFileSystem::new(
            files
                .iter()
                .map(|path| ((*path).to_string(), String::new()))
                .collect::<HashMap<_, _>>(),
        )
    }

    #[test]
    fn diff_target_strips_the_default_prefix() -> anyhow::Result<()> {
        let files = tree(&["src/main.rs"]);
        assert_eq!(
            RepoPath::from_diff_target("a/src/main.rs", "b/src/main.rs", &files)?
                .unwrap()
                .as_str(),
            "src/main.rs"
        );
        Ok(())
    }

    #[test]
    fn diff_target_keeps_a_real_directory_named_like_the_prefix() -> anyhow::Result<()> {
        // Git writes `b/b/rules.py` for a file in a top-level directory named `b`. Only the first
        // component is Git's prefix.
        let files = tree(&["b/rules.py"]);
        assert_eq!(
            RepoPath::from_diff_target("a/b/rules.py", "b/b/rules.py", &files)?
                .unwrap()
                .as_str(),
            "b/rules.py"
        );
        Ok(())
    }

    #[test]
    fn diff_target_strips_a_mnemonic_prefix() -> anyhow::Result<()> {
        let files = tree(&["rules.py"]);
        assert_eq!(
            RepoPath::from_diff_target("i/rules.py", "w/rules.py", &files)?
                .unwrap()
                .as_str(),
            "rules.py"
        );
        Ok(())
    }

    #[test]
    fn diff_target_rejects_a_repeated_prefix() {
        // `git diff --no-prefix` on a file under a top-level `b/` writes a target that looks
        // prefixed. Git never repeats one prefix on both sides, so the repetition gives it away.
        let files = tree(&["b/rules.py", "rules.py"]);
        let error = RepoPath::from_diff_target("b/rules.py", "b/rules.py", &files).unwrap_err();
        assert!(
            format!("{error:#}").contains("no recognized Git path prefix"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn added_file_resolves_to_the_reading_that_exists() -> anyhow::Result<()> {
        // "/dev/null" carries no prefix, so an added file's header cannot show whether the target
        // has one. Under --no-prefix the unstripped reading is the real file.
        let files = tree(&["b/x.py"]);
        assert_eq!(
            RepoPath::from_diff_target("/dev/null", "b/x.py", &files)?
                .unwrap()
                .as_str(),
            "b/x.py"
        );
        // With prefixes the stripped reading is.
        let files = tree(&["x.py"]);
        assert_eq!(
            RepoPath::from_diff_target("/dev/null", "b/x.py", &files)?
                .unwrap()
                .as_str(),
            "x.py"
        );
        Ok(())
    }

    #[test]
    fn added_file_with_two_real_readings_is_ambiguous() {
        // Both "b/x.py" and "x.py" exist, so the target is equally readable either way. Picking
        // one would validate it while the file the diff names went unchecked.
        let files = tree(&["b/x.py", "x.py"]);
        let error = RepoPath::from_diff_target("/dev/null", "b/x.py", &files).unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("cannot tell whether the diff writes Git's path prefixes"),
            "unexpected error: {message}"
        );
        assert!(
            message.contains("newly added"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn diff_target_rejects_a_custom_prefix() {
        // A custom diff.dstPrefix cannot be told apart from a repository directory of the same
        // name, so it is reported rather than guessed at.
        let files = tree(&["rules.py"]);
        let error = RepoPath::from_diff_target("old/rules.py", "new/rules.py", &files).unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("no recognized Git path prefix"),
            "unexpected error: {message}"
        );
        assert!(
            message.contains("--default-prefix"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn diff_target_rejects_output_without_a_prefix() {
        // `git diff --no-prefix` writes the repository path directly, which is indistinguishable
        // from a prefixed path whose repository directory shares the prefix's name.
        let files = tree(&["rules.py"]);
        let error = RepoPath::from_diff_target("rules.py", "rules.py", &files).unwrap_err();
        assert!(
            format!("{error:#}").contains("no recognized Git path prefix"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn diff_target_of_a_deleted_file_is_skipped() -> anyhow::Result<()> {
        let files = tree(&["rules.py"]);
        assert_eq!(
            RepoPath::from_diff_target("a/rules.py", "/dev/null", &files)?,
            None
        );
        Ok(())
    }

    #[test]
    fn diff_target_of_a_new_file_drops_the_prefix() -> anyhow::Result<()> {
        // An added file's target carries a prefix like any other.
        let files = tree(&["src/added.py"]);
        assert_eq!(
            RepoPath::from_diff_target("/dev/null", "b/src/added.py", &files)?
                .unwrap()
                .as_str(),
            "src/added.py"
        );
        Ok(())
    }

    #[test]
    fn diff_target_decodes_a_quoted_path() -> anyhow::Result<()> {
        let files = tree(&["café.py"]);
        assert_eq!(
            RepoPath::from_diff_target(r#""a/caf\303\251.py""#, r#""b/caf\303\251.py""#, &files)?
                .unwrap()
                .as_str(),
            "café.py"
        );
        Ok(())
    }

    #[test]
    fn diff_target_rejects_a_path_escaping_the_repository() {
        let files = tree(&["rules.py"]);
        assert!(
            RepoPath::from_diff_target("a/../../etc/passwd", "b/../../etc/passwd", &files).is_err()
        );
    }

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
    fn dereferences_to_a_path() -> anyhow::Result<()> {
        let path = RepoPath::from_relative(Path::new("a/b.py"))?;
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("py"));
        Ok(())
    }

    #[test]
    fn unquote_passes_through_an_unquoted_path() -> anyhow::Result<()> {
        assert_eq!(unquote_git_path("b/src/main.rs")?, "b/src/main.rs");
        Ok(())
    }

    #[test]
    fn unquote_decodes_octal_byte_escapes() -> anyhow::Result<()> {
        // Git writes `café.py` this way under its default core.quotePath setting.
        assert_eq!(unquote_git_path(r#""b/caf\303\251.py""#)?, "b/café.py");
        Ok(())
    }

    #[test]
    fn unquote_decodes_character_escapes() -> anyhow::Result<()> {
        assert_eq!(unquote_git_path(r#""b/a\tb.py""#)?, "b/a\tb.py");
        assert_eq!(unquote_git_path(r#""b/a\"b.py""#)?, "b/a\"b.py");
        assert_eq!(unquote_git_path(r#""b/a\\b.py""#)?, "b/a\\b.py");
        Ok(())
    }

    #[test]
    fn unquote_decodes_a_short_octal_escape() -> anyhow::Result<()> {
        // Octal escapes are one to three digits; `\7` must not swallow the following character.
        assert_eq!(unquote_git_path(r#""b/a\7b.py""#)?, "b/a\u{7}b.py");
        Ok(())
    }

    #[test]
    fn unquote_rejects_invalid_utf8() {
        // A lone continuation byte cannot start a UTF-8 sequence.
        assert!(unquote_git_path(r#""b/\251.py""#).is_err());
    }

    #[test]
    fn unquote_rejects_an_unknown_escape() {
        assert!(unquote_git_path(r#""b/a\qb.py""#).is_err());
    }

    #[test]
    fn unquote_rejects_a_trailing_backslash() {
        assert!(unquote_git_path(r#""b/a\""#).is_err());
    }
}
