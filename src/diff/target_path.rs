//! The repository path that the header of a file in a unified diff points at.

use crate::fs::FileSystem;
use crate::repo_path::RepoPath;
use anyhow::{Context, anyhow, bail};
use std::path::Path;
use winnow::Result as PResult;
use winnow::combinator::{alt, delimited, preceded, repeat};
use winnow::prelude::*;
use winnow::token::{one_of, take_while};

/// The stand-in Git writes for the missing side of an added or deleted file.
const DEV_NULL: &str = "/dev/null";

/// The one-component prefixes Git puts ahead of a diff path: `a/` and `b/` by default, and the
/// four `diff.mnemonicPrefix` spellings. Any other prefix, or none, is rejected — see
/// [`target_path`].
// <block same-as="docs/cli.md:diff-input" same-as-pattern='"(?P<value>[a-z]/)"'>
const DIFF_PATH_PREFIXES: [&str; 6] = ["a/", "b/", "i/", "w/", "c/", "o/"];
// </block>

/// Resolves a diff header's target to the repository path it points at.
///
/// Exactly one of the prefixes Git writes ahead of a diff path is removed; a target carrying
/// none is rejected rather than guessed at. See "Supported Diff Input" in `docs/cli.md`.
///
/// The path is returned whether or not it points at an existing file; `parse_blocks` decides
/// whether an absent one matters.
pub(super) fn target_path(
    source: &str,
    target: &str,
    file_system: &impl FileSystem,
) -> anyhow::Result<Option<RepoPath>> {
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
            "diff target \"{decoded_target}\" has no recognized path prefix ({})",
            DIFF_PATH_PREFIXES.join(", ")
        );
    };
    // `from_relative` rejects anything that would escape the repository, including the rooted
    // path left behind by a target such as "b//etc/passwd".
    let path = RepoPath::from_relative(Path::new(without_prefix))?;

    // "/dev/null" says nothing about the target's prefix, so the working tree decides.
    let path = if decoded_source.is_none() {
        let unprefixed = RepoPath::from_relative(Path::new(&decoded_target))
            .ok()
            .filter(|candidate| file_system.exists(candidate.as_path()));
        match unprefixed {
            Some(unprefixed) if file_system.exists(path.as_path()) => bail!(
                "cannot tell whether diff target \"{decoded_target}\" carries a path prefix: it \
                 reads as both \"{path}\" and \"{unprefixed}\", and both exist. A newly added \
                 file's header records no prefix"
            ),
            Some(unprefixed) => unprefixed,
            None => path,
        }
    } else {
        path
    };

    Ok(Some(path))
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
mod tests {
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
            target_path("a/src/main.rs", "b/src/main.rs", &files)?
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
            target_path("a/b/rules.py", "b/b/rules.py", &files)?
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
            target_path("i/rules.py", "w/rules.py", &files)?
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
        let error = target_path("b/rules.py", "b/rules.py", &files).unwrap_err();
        assert!(
            format!("{error:#}").contains("no recognized path prefix"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn added_file_resolves_to_the_reading_that_exists() -> anyhow::Result<()> {
        // "/dev/null" carries no prefix, so an added file's header cannot show whether the target
        // has one. Under --no-prefix the unstripped reading is the real file.
        let files = tree(&["b/x.py"]);
        assert_eq!(
            target_path("/dev/null", "b/x.py", &files)?
                .unwrap()
                .as_str(),
            "b/x.py"
        );
        // With prefixes the stripped reading is.
        let files = tree(&["x.py"]);
        assert_eq!(
            target_path("/dev/null", "b/x.py", &files)?
                .unwrap()
                .as_str(),
            "x.py"
        );
        Ok(())
    }

    #[test]
    fn added_file_with_two_real_readings_is_ambiguous() {
        // Both "b/x.py" and "x.py" exist, so the target is equally readable either way. Picking
        // one would validate it while the file the diff points at went unchecked.
        let files = tree(&["b/x.py", "x.py"]);
        let error = target_path("/dev/null", "b/x.py", &files).unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("cannot tell whether diff target"),
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
        let error = target_path("old/rules.py", "new/rules.py", &files).unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("no recognized path prefix"),
            "unexpected error: {message}"
        );
        assert!(
            message.contains("new/rules.py"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn diff_target_rejects_output_without_a_prefix() {
        // `git diff --no-prefix` writes the repository path directly, which is indistinguishable
        // from a prefixed path whose repository directory shares the prefix's name.
        let files = tree(&["rules.py"]);
        let error = target_path("rules.py", "rules.py", &files).unwrap_err();
        assert!(
            format!("{error:#}").contains("no recognized path prefix"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn diff_target_of_a_deleted_file_is_skipped() -> anyhow::Result<()> {
        let files = tree(&["rules.py"]);
        assert_eq!(target_path("a/rules.py", "/dev/null", &files)?, None);
        Ok(())
    }

    #[test]
    fn diff_target_of_a_new_file_drops_the_prefix() -> anyhow::Result<()> {
        // An added file's target carries a prefix like any other.
        let files = tree(&["src/added.py"]);
        assert_eq!(
            target_path("/dev/null", "b/src/added.py", &files)?
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
            target_path(r#""a/caf\303\251.py""#, r#""b/caf\303\251.py""#, &files)?
                .unwrap()
                .as_str(),
            "café.py"
        );
        Ok(())
    }

    #[test]
    fn diff_target_rejects_a_path_escaping_the_repository() {
        let files = tree(&["rules.py"]);
        assert!(target_path("a/../../etc/passwd", "b/../../etc/passwd", &files).is_err());
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
