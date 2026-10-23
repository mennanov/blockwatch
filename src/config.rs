//! Reads the settings from the config file, `blockwatch.toml`.

use crate::LineIndex;
use crate::blocks::{BlockSeverity, validate_attributes};
use crate::diff_parser::{DiffChanges, LineChanges};
use crate::fs::FileSystem;
use crate::language_parsers::toml;
use crate::paths_glob::{GLOB_CHARACTERS, PathsGlob};
use crate::repo_path::RepoPath;
use crate::settings::RawSettings;
use crate::symbol_path::SymbolPath;
use crate::symbols::{self, Symbol};
use crate::target_reference::TargetReference;
use crate::virtual_blocks::{ConfigEntry, Locator, TargetPattern, VirtualBlock};
use anyhow::{Context, anyhow, bail};
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_spanned::Spanned;
use std::collections::HashMap;
use std::path::Path;

/// The default name of the config file.
const DEFAULT_FILE: &str = "blockwatch.toml";

/// What the config file holds.
#[derive(Debug, Default)]
pub(crate) struct Config {
    /// The settings. They are not validated yet.
    pub(crate) settings: RawSettings,
    /// The virtual blocks, in the order the file declares them.
    pub(crate) blocks: Vec<VirtualBlock>,
}

/// The config file, as written.
///
/// It lists the keys of [`RawSettings`] again, instead of flattening it in, because a flattened
/// field reports every error at line 1, column 1. [`read`] takes the file apart field by field,
/// so a key added to one of the two structs and not to the other does not compile.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    // The error example in the docs also shows `block` and a misspelled key, so the docs only have
    // to hold each of these keys.
    /* <block name="config-keys" same-as="docs/cli.md:config-keys, docs/cli.md:config-key-error"
    same-as-mode="subset" same-as-format="identifier" same-as-pattern="^(?P<value>[a-z_]+):"> */
    #[serde(default)]
    ignore: Vec<String>,
    #[serde(default)]
    extensions: HashMap<String, String>,
    #[serde(default)]
    enable: Vec<String>,
    #[serde(default)]
    disable: Vec<String>,
    #[serde(default, rename = "only-blocks")]
    only_blocks: Vec<String>,
    #[serde(default, rename = "skip-blocks")]
    skip_blocks: Vec<String>,
    // </block>
    /// The `[[block]]` entries, each with the byte range of its header.
    #[serde(default)]
    block: Vec<Spanned<BlockEntry>>,
}

/// A `[[block]]` entry, as written.
#[derive(Deserialize)]
struct BlockEntry {
    /// The file the block is in, such as `settings.py`, or a symbol in a file, such as
    /// `package.json#/version`. The file can be a glob, such as `plugins/*/package.json`, to
    /// declare the block in each file that matches.
    target: String,
    /// The regex that finds the block in the `target` file, such as `^MODEL = .*`. Without it,
    /// the block is the whole file.
    #[serde(rename = "target-pattern")]
    target_pattern: Option<String>,
    /// Every other key: the block's attributes.
    // `deny_unknown_fields` does not work with `flatten`, so an unknown attribute is caught by the
    // same check that tags go through.
    #[serde(flatten)]
    attributes: HashMap<String, AttributeValue>,
}

/// The value of an attribute, as written.
#[derive(Deserialize)]
#[serde(untagged)]
enum AttributeValue {
    Text(String),
    Flag(bool),
    Integer(i64),
    /// Any other value. It deserializes too, so that the error can show which attribute has it.
    Other(IgnoredAny),
}

/// Reads the config file.
///
/// If `path` is given, that file is read, and it must exist. A relative `path` starts from the
/// current directory, not from the root of `file_system`. If `path` is `None`, [`DEFAULT_FILE`] is
/// read from the root of `file_system`. If that file does not exist, the config is empty.
///
/// `line_changes_by_file` are the diff's changes, by file. A block whose entry the changes to the
/// config file touch is marked as modified. A config file outside the repository has no changes.
///
/// The settings are not validated here. Returns an error if the file can't be read, isn't valid
/// TOML, has an unknown key or a value of the wrong type, or declares a block that is not valid.
/// The error shows the file, and the line of the problem.
pub(crate) fn read(
    path: Option<&Path>,
    file_system: &impl FileSystem,
    line_changes_by_file: &DiffChanges,
) -> anyhow::Result<Config> {
    let (path, text, repo_path) = match read_file(path, file_system)? {
        Some(file) => file,
        None => return Ok(Config::default()),
    };
    let no_changes = LineChanges::default();
    let line_changes = repo_path.map_or(&no_changes, |repo_path| {
        line_changes_by_file.for_file(&repo_path)
    });
    let ConfigFile {
        ignore,
        extensions,
        enable,
        disable,
        only_blocks,
        skip_blocks,
        block,
    } = toml_edit::de::from_str(&text)
        .with_context(|| format!("invalid config file \"{}\"", path.display()))?;
    Ok(Config {
        settings: RawSettings {
            ignore,
            extensions,
            enable,
            disable,
            only_blocks,
            skip_blocks,
        },
        blocks: virtual_blocks(block, path, &text, line_changes, file_system)?,
    })
}

/// Reads the config file at `path`, or [`DEFAULT_FILE`] if `path` is `None`.
///
/// Returns the path that was read, the file's text, and the file's path in the repository, which
/// is `None` for a file outside it. Returns `None` if [`DEFAULT_FILE`] does not exist.
///
/// # Errors
/// Returns an error if the file can't be read, or if `path` is given and does not exist.
fn read_file<'p>(
    path: Option<&'p Path>,
    file_system: &impl FileSystem,
) -> anyhow::Result<Option<(&'p Path, String, Option<RepoPath>)>> {
    match path {
        // Like `--suppress-from`, this is a path the person running the tool provided which can be
        // outside the repository.
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read config file \"{}\"", path.display()))?;
            // `path` starts from the current directory, and the file system's paths start from
            // the repository root.
            let repo_path = std::path::absolute(path)
                .ok()
                .and_then(|path| file_system.repo_path(&path));
            Ok(Some((path, text, repo_path)))
        }
        None => {
            let path = Path::new(DEFAULT_FILE);
            if !file_system.exists(path) {
                return Ok(None);
            }
            Ok(Some((
                path,
                file_system.read_to_string(path)?,
                file_system.repo_path(path),
            )))
        }
    }
}

/// The virtual blocks that `entries` declare, in the order of `entries`. An entry whose target is
/// a glob declares one block in each file that matches, sorted by file. `path` and `text` are the
/// config file's, and `line_changes` are the diff's changes to it. A target file that is a symlink
/// in `file_system` becomes the file it points at.
///
/// # Errors
/// Returns an error if an entry is not valid, or if two entries wrap the same symbol. The error
/// shows the line of the entry.
fn virtual_blocks(
    entries: Vec<Spanned<BlockEntry>>,
    path: &Path,
    text: &str,
    line_changes: &LineChanges,
    file_system: &impl FileSystem,
) -> anyhow::Result<Vec<VirtualBlock>> {
    let symbols = toml::parse_symbols(text)?;
    let lines = LineIndex::new(text);
    let blocks: Vec<VirtualBlock> = entries
        .into_iter()
        .enumerate()
        .map(|(index, block_entry)| {
            let entry = ConfigEntry {
                file: path.to_path_buf(),
                line: lines.position(block_entry.span().start).line,
            };
            let context = entry.error_context();
            let is_entry_modified = is_entry_modified(text, &lines, &symbols, index, line_changes)?;
            entry_blocks(
                block_entry.into_inner(),
                entry,
                is_entry_modified,
                file_system,
            )
            .context(context)
        })
        .collect::<anyhow::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    reject_duplicate_blocks(&blocks)?;
    Ok(blocks)
}

/// Rejects a block with the same file and locator as a block earlier in `blocks`.
///
/// # Errors
/// Returns an error that shows the line of the second block's entry and of the first one.
// A `Regex` keeps a cache that it changes as it runs, so the lint calls the key mutable. But a
// `TargetPattern` is hashed and compared by its text, which never changes.
#[allow(clippy::mutable_key_type)]
fn reject_duplicate_blocks(blocks: &[VirtualBlock]) -> anyhow::Result<()> {
    // A symbol, a pattern or a whole file has at most one block, as all its rules fit in one.
    // Allowing more blocks later breaks no config, while forbidding them later would.
    let mut first_lines = HashMap::new();
    for block in blocks {
        if let Some(first_line) =
            first_lines.insert((&block.file, &block.locator), block.entry.line)
        {
            return Err(anyhow!(
                "target {} already has the block at line {first_line}",
                block.target()
            )
            .context(block.entry.error_context()));
        }
    }
    Ok(())
}

/// Whether `line_changes` touch the `[[block]]` entry at `index` of `text`, the config file.
/// `symbols` are the symbols of `text`, and `lines` indexes it.
fn is_entry_modified(
    text: &str,
    lines: &LineIndex,
    symbols: &[Symbol],
    index: usize,
    line_changes: &LineChanges,
) -> anyhow::Result<bool> {
    // The symbol covers every line of the entry. The span that serde gives covers only its header.
    let path = SymbolPath::parse(&format!("/block/{index}"))?;
    let symbol = symbols::resolve(symbols, &path).map_err(|error| anyhow!(error.reason(text)))?;
    Ok(symbol
        .def_byte_ranges
        .iter()
        .any(|range| line_changes.touches(&lines.range(range))))
}

/// The virtual blocks that `block_entry`, written as `entry` of the config file, declares: one in
/// each file of [`target_files`], in that order. `is_entry_modified` tells whether the diff
/// touches the entry.
///
/// # Errors
/// Returns an error if:
/// - the target is not a file or a symbol in a file;
/// - the target's file is a glob that is not valid or that matches no file;
/// - `target-pattern` comes with a symbol, or is not a valid regex;
/// - an attribute is not one a tag can have, or has a value that is not a string, `true` or an
///   integer;
/// - an attribute comes without the attribute its validator needs, such as `check-lua-timeout`
///   without `check-lua`.
fn entry_blocks(
    block_entry: BlockEntry,
    entry: ConfigEntry,
    is_entry_modified: bool,
    file_system: &impl FileSystem,
) -> anyhow::Result<Vec<VirtualBlock>> {
    let (file, locator) = parse_target(&block_entry.target, block_entry.target_pattern)?;
    let attributes = attributes_as_text(block_entry.attributes)?;
    validate_attributes(&attributes)?;
    let severity = BlockSeverity::from_attributes(&attributes)?;
    let files = target_files(file, file_system)?;
    if files.is_empty() {
        // Without this error, a typo in the glob would turn the rules off without a word.
        return Err(anyhow!("no file matches the glob")
            .context(format!("target {} does not resolve", block_entry.target)));
    }
    Ok(files
        .into_iter()
        .map(|file| VirtualBlock {
            file,
            locator: locator.clone(),
            attributes: attributes.clone(),
            entry: entry.clone(),
            is_entry_modified,
            severity,
        })
        .collect())
}

/// The files that `file`, the file of a target, refers to:
///
/// - A path that exists is one file, even when it has a glob character, such as `app/[id].json`.
///   A symlink becomes the file it points at.
/// - A path that doesn't exist and has a glob character is a glob. It refers to every file that
///   matches it, sorted. It can match none.
/// - Any other path is one file, as written.
///
/// # Errors
/// Returns an error if the glob is not valid, or if the repository can't be walked.
fn target_files(file: RepoPath, file_system: &impl FileSystem) -> anyhow::Result<Vec<RepoPath>> {
    // The walk skips a symlink, so a block on one would never be checked.
    if let Some(file) = file_system.repo_path(file.as_path()) {
        return Ok(vec![file]);
    }
    if !file.as_str().contains(GLOB_CHARACTERS) {
        // The error about the missing file shows the path from the config.
        return Ok(vec![file]);
    }
    PathsGlob::new(file.as_str())?.matching_files(file_system)
}

/// Returns the file that `target` refers to, and how to find the block in it: the symbol of
/// `target`, the text that `target_pattern` matches, or else the whole file.
///
/// # Errors
/// Returns an error if `target` is not a file or a symbol in a file, or if `target_pattern` comes
/// with a symbol or is not a valid regex.
fn parse_target(
    target: &str,
    target_pattern: Option<String>,
) -> anyhow::Result<(RepoPath, Locator)> {
    Ok(match (TargetReference::parse(target)?, target_pattern) {
        (
            TargetReference::Symbol {
                file: Some(file),
                path,
            },
            None,
        ) => (file, Locator::Symbol(path)),
        (TargetReference::File(file), None) => (file, Locator::File),
        // The block is found only when its file is read. A bad regex fails here, so that it fails
        // every run.
        (TargetReference::File(file), Some(pattern)) => {
            (file, Locator::Pattern(TargetPattern::new(&pattern)?))
        }
        (TargetReference::Symbol { file: Some(_), .. }, Some(_)) => {
            bail!("`target-pattern` needs a whole file as `target`, such as `settings.py`")
        }
        (TargetReference::Symbol { file: None, .. } | TargetReference::Block { .. }, _) => bail!(
            "target `{target}` must be a file or a symbol in a file, such as `settings.py` or \
             `package.json#/version`"
        ),
    })
}

/// Returns `attributes` with each value as the text a tag would give it.
///
/// # Errors
/// Returns an error for a value that is not a string, `true` or an integer.
fn attributes_as_text(
    attributes: HashMap<String, AttributeValue>,
) -> anyhow::Result<HashMap<String, String>> {
    attributes
        .into_iter()
        .map(|(name, value)| {
            let text = match value {
                AttributeValue::Text(text) => text,
                // A tag can have an attribute without a value, such as `keep-unique`. TOML can't.
                AttributeValue::Flag(true) => String::new(),
                AttributeValue::Integer(number) => number.to_string(),
                AttributeValue::Flag(false) | AttributeValue::Other(_) => {
                    bail!("`{name}` must be a string, `true` or an integer")
                }
            };
            Ok((name, text))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_parser::{LineChange, LineChangeKind};
    use crate::fs::test_utils::FakeFileSystem;
    use crate::test_utils::diff_changes;
    use std::path::PathBuf;

    /// Reads `text` as the default config file, with no diff.
    fn read_text(text: &str) -> anyhow::Result<Config> {
        read_in_repository(text, &[], &[])
    }

    /// Reads `text` as the default config file, which the diff makes `line_changes` to.
    fn read_changed_text(text: &str, line_changes: &[LineChange]) -> anyhow::Result<Config> {
        read_in_repository(text, &[], line_changes)
    }

    /// Reads `text` as the default config file of a repository that also holds `files`, which are
    /// empty. The diff makes `line_changes` to the config file.
    fn read_in_repository(
        text: &str,
        files: &[&str],
        line_changes: &[LineChange],
    ) -> anyhow::Result<Config> {
        let mut contents = HashMap::from([(DEFAULT_FILE.to_string(), text.to_string())]);
        contents.extend(files.iter().map(|file| (file.to_string(), String::new())));
        read(
            None,
            &FakeFileSystem::new(contents),
            &diff_changes([(
                RepoPath::from_reference(DEFAULT_FILE)?,
                line_changes.to_vec(),
            )]),
        )
    }

    /// The error of reading `text` as the default config file, with its causes.
    fn read_error(text: &str) -> String {
        let error = read_text(text).expect_err("the config file must be rejected");
        format!("{error:#}")
    }

    #[test]
    fn block_entries_read_as_virtual_blocks() -> anyhow::Result<()> {
        let text = r"ignore = []

[[block]]
target = 'package.json#/version'
name = 'version'
keep-unique = true
check-lua = 'check.lua'
check-lua-timeout = 30

[[block]]
target = 'a.yaml#/b'
";

        let config = read_text(text)?;

        assert_eq!(
            config.blocks,
            vec![
                VirtualBlock {
                    file: RepoPath::from_reference("package.json")?,
                    locator: Locator::Symbol(SymbolPath::parse("/version")?),
                    attributes: HashMap::from([
                        ("name".to_string(), "version".to_string()),
                        ("keep-unique".to_string(), String::new()),
                        ("check-lua".to_string(), "check.lua".to_string()),
                        ("check-lua-timeout".to_string(), "30".to_string()),
                    ]),
                    entry: ConfigEntry {
                        file: PathBuf::from(DEFAULT_FILE),
                        line: 3,
                    },
                    is_entry_modified: false,
                    severity: BlockSeverity::Error,
                },
                VirtualBlock {
                    file: RepoPath::from_reference("a.yaml")?,
                    locator: Locator::Symbol(SymbolPath::parse("/b")?),
                    attributes: HashMap::new(),
                    entry: ConfigEntry {
                        file: PathBuf::from(DEFAULT_FILE),
                        line: 10,
                    },
                    is_entry_modified: false,
                    severity: BlockSeverity::Error,
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn file_target_read_wraps_the_whole_file_or_the_target_pattern_match() -> anyhow::Result<()> {
        let text = r"[[block]]
target = 'settings.py'

[[block]]
target = 'settings.py'
target-pattern = '^MODEL = .*'
";

        let config = read_text(text)?;

        // `target-pattern` is not an attribute.
        let blocks: Vec<(&str, &Locator, usize)> = config
            .blocks
            .iter()
            .map(|block| (block.file.as_str(), &block.locator, block.attributes.len()))
            .collect();
        assert_eq!(
            blocks,
            [
                ("settings.py", &Locator::File, 0),
                (
                    "settings.py",
                    &Locator::Pattern(TargetPattern::new("^MODEL = .*")?),
                    0
                ),
            ]
        );
        Ok(())
    }

    #[test]
    fn glob_target_read_declares_a_block_in_each_matching_file() -> anyhow::Result<()> {
        let text = r"[[block]]
target = 'plugins/*/package.json#/version'
name = 'version'
";
        let files = [
            "package.json",
            "plugins/red/package.json",
            "plugins/blue/package.json",
        ];

        let config = read_in_repository(text, &files, &[])?;

        let block = |file| -> anyhow::Result<VirtualBlock> {
            Ok(VirtualBlock {
                file: RepoPath::from_reference(file)?,
                locator: Locator::Symbol(SymbolPath::parse("/version")?),
                attributes: HashMap::from([("name".to_string(), "version".to_string())]),
                entry: ConfigEntry {
                    file: PathBuf::from(DEFAULT_FILE),
                    line: 1,
                },
                is_entry_modified: false,
                severity: BlockSeverity::Error,
            })
        };
        // Sorted by file.
        assert_eq!(
            config.blocks,
            [
                block("plugins/blue/package.json")?,
                block("plugins/red/package.json")?
            ]
        );
        Ok(())
    }

    #[test]
    fn existing_file_with_glob_characters_read_is_one_target() -> anyhow::Result<()> {
        // As a glob, `[id]` would match `app/i/meta.json`.
        let text = r"[[block]]
target = 'app/[id]/meta.json#/title'
";

        let config = read_in_repository(text, &["app/[id]/meta.json", "app/i/meta.json"], &[])?;

        let files: Vec<&str> = config
            .blocks
            .iter()
            .map(|block| block.file.as_str())
            .collect();
        assert_eq!(files, ["app/[id]/meta.json"]);
        Ok(())
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn diff_touching_an_entry_marks_only_its_block_modified() -> anyhow::Result<()> {
        let text = r"ignore = ['vendor/**']

[[block]]
target = 'a.json#/a'
name = 'a'

[[block]]
target = 'a.json#/b'
";
        let modified = |line| LineChange {
            line,
            kind: LineChangeKind::Modified(vec![0..1]),
        };
        let cases = [
            // The settings.
            (modified(1), [false, false]),
            // A line added just above the first `[[block]]`.
            (
                LineChange {
                    line: 2,
                    kind: LineChangeKind::Added,
                },
                [false, false],
            ),
            // The first `[[block]]`.
            (modified(3), [true, false]),
            // `name = 'a'`, the first entry's last line.
            (modified(5), [true, false]),
            // A line deleted just after `name = 'a'` was the first entry's last line.
            (
                LineChange {
                    line: 6,
                    kind: LineChangeKind::Deleted,
                },
                [true, false],
            ),
            // The second `[[block]]`.
            (modified(7), [false, true]),
        ];

        for (line_change, [is_block1_modified, is_block2_modified]) in cases {
            let modified: Vec<bool> = read_changed_text(text, std::slice::from_ref(&line_change))?
                .blocks
                .iter()
                .map(|block| block.is_entry_modified)
                .collect();
            assert_eq!(
                modified,
                [is_block1_modified, is_block2_modified],
                "{line_change:?}"
            );
        }
        Ok(())
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn diff_touching_a_glob_entry_marks_every_block_it_declares_modified() -> anyhow::Result<()> {
        let text = r"[[block]]
target = 'plugins/*/package.json#/version'
";
        let target_modified = LineChange {
            line: 2,
            kind: LineChangeKind::Modified(vec![0..6]),
        };

        let config = read_in_repository(
            text,
            &["plugins/red/package.json", "plugins/blue/package.json"],
            &[target_modified],
        )?;

        let modified: Vec<bool> = config
            .blocks
            .iter()
            .map(|block| block.is_entry_modified)
            .collect();
        assert_eq!(modified, [true, true]);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn target_in_a_symlink_reads_as_the_file_it_points_at() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        std::fs::write(root.path().join("real.json"), r#"{"a": 1}"#)?;
        std::os::unix::fs::symlink("real.json", root.path().join("link.json"))?;
        std::fs::write(
            root.path().join(DEFAULT_FILE),
            "[[block]]\ntarget = 'link.json#/a'\n",
        )?;

        let config = read(
            None,
            &crate::fs::FileSystemImpl::new(root.path())?,
            &DiffChanges::default(),
        )?;

        let files: Vec<&str> = config
            .blocks
            .iter()
            .map(|block| block.file.as_str())
            .collect();
        assert_eq!(files, ["real.json"]);
        Ok(())
    }

    #[test]
    fn duplicate_block_for_a_symbol_is_rejected() {
        // `./package.json` is another spelling of the same file.
        let text = r"[[block]]
target = 'package.json#/version'
line-count = '1'

[[block]]
target = './package.json#/version'
name = 'version'
";

        assert_eq!(
            read_error(text),
            "invalid block at line 5 of \"blockwatch.toml\": target package.json#/version \
             already has the block at line 1"
        );
    }

    #[test]
    fn duplicate_block_with_the_same_file_and_pattern_is_rejected() {
        let text = r"[[block]]
target = 'settings.py'
target-pattern = '^MODEL = .*'

[[block]]
target = 'settings.py'
target-pattern = '^MODEL = .*'
";

        assert_eq!(
            read_error(text),
            "invalid block at line 5 of \"blockwatch.toml\": target settings.py already has the \
             block at line 1"
        );
    }

    #[test]
    fn glob_target_overlapping_another_target_is_rejected() {
        let text = r"[[block]]
target = 'plugins/red/package.json#/version'

[[block]]
target = 'plugins/*/package.json#/version'
";

        let error = read_in_repository(text, &["plugins/red/package.json"], &[]).unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "invalid block at line 4 of \"blockwatch.toml\": target \
             plugins/red/package.json#/version already has the block at line 1"
        );
    }

    #[test]
    fn blocks_with_different_targets_or_patterns_are_accepted() -> anyhow::Result<()> {
        // Each target shares its file or its path with another one, but not both. The last three
        // share their file, but each finds a different part of it.
        let text = r"[[block]]
target = 'a.json#/x'

[[block]]
target = 'a.json#/y'

[[block]]
target = 'b.json#/x'

[[block]]
target = 'settings.py'

[[block]]
target = 'settings.py'
target-pattern = '^A = .*'

[[block]]
target = 'settings.py'
target-pattern = '^B = .*'
";

        let config = read_text(text)?;

        assert_eq!(config.blocks.len(), 6);
        Ok(())
    }

    #[test]
    fn target_that_is_not_a_file_or_a_symbol_in_a_file_is_rejected() {
        for target in ["#/x", "a.md:name"] {
            let text = format!(
                r"[[block]]
target = '{target}'
"
            );

            assert_eq!(
                read_error(&text),
                format!(
                    "invalid block at line 1 of \"blockwatch.toml\": target `{target}` must be a \
                     file or a symbol in a file, such as `settings.py` or `package.json#/version`"
                )
            );
        }
    }

    #[test]
    fn target_pattern_with_a_symbol_target_is_rejected() {
        let text = r"[[block]]
target = 'package.json#/version'
target-pattern = '^1'
";

        assert_eq!(
            read_error(text),
            "invalid block at line 1 of \"blockwatch.toml\": `target-pattern` needs a whole file \
             as `target`, such as `settings.py`"
        );
    }

    #[test]
    fn target_pattern_that_is_not_a_regex_is_rejected() {
        let text = r"[[block]]
target = 'settings.py'
target-pattern = '('
";

        let error = read_error(text);

        assert!(
            error.starts_with(
                "invalid block at line 1 of \"blockwatch.toml\": invalid `target-pattern`: regex \
                 parse error"
            ),
            "{error}"
        );
    }

    #[test]
    fn glob_target_matching_no_file_is_rejected() {
        let text = r"[[block]]
target = 'plugins/*/package.json#/version'
";

        assert_eq!(
            read_error(text),
            "invalid block at line 1 of \"blockwatch.toml\": target \
             plugins/*/package.json#/version does not resolve: no file matches the glob"
        );
    }

    #[test]
    fn glob_target_that_does_not_compile_is_rejected() {
        let text = r"[[block]]
target = 'plugins/{red,blue/package.json#/version'
";

        let error = read_error(text);

        assert!(
            error.starts_with(
                "invalid block at line 1 of \"blockwatch.toml\": error parsing glob \
                 'plugins/{red,blue/package.json'"
            ),
            "{error}"
        );
    }

    #[test]
    fn attribute_value_that_is_not_a_string_true_or_an_integer_is_rejected() {
        for value in ["false", "1.5"] {
            let text = format!(
                r"[[block]]
target = 'a.json#/x'
line-count = {value}
"
            );

            assert_eq!(
                read_error(&text),
                "invalid block at line 1 of \"blockwatch.toml\": `line-count` must be a string, \
                 `true` or an integer",
                "{value}"
            );
        }
    }

    #[test]
    fn unknown_attribute_is_rejected() {
        let text = r"[[block]]
target = 'a.json#/x'
keep-sortd = true
";

        assert_eq!(
            read_error(text),
            "invalid block at line 1 of \"blockwatch.toml\": unrecognized attribute `keep-sortd`"
        );
    }

    #[test]
    fn unknown_severity_is_rejected() {
        let text = r"[[block]]
target = 'a.json#/x'
severity = 'loud'
";

        assert_eq!(
            read_error(text),
            "invalid block at line 1 of \"blockwatch.toml\": unrecognized severity value `loud`"
        );
    }

    #[test]
    fn severity_attribute_read_returns_blocks_with_that_severity() -> anyhow::Result<()> {
        let text = r"[[block]]
target = 'a.json#/x'
severity = 'warning'
";

        let config = read_text(text)?;

        assert_eq!(config.blocks[0].severity, BlockSeverity::Warning);
        Ok(())
    }

    #[test]
    fn block_lists_read_into_the_settings() -> anyhow::Result<()> {
        let config =
            read_text("only-blocks = ['a.py:fruits']\nskip-blocks = ['b.py:vegetables']\n")?;
        assert_eq!(config.settings.only_blocks, ["a.py:fruits"]);
        assert_eq!(config.settings.skip_blocks, ["b.py:vegetables"]);
        Ok(())
    }

    #[test]
    fn missing_default_file_reads_as_an_empty_config() -> anyhow::Result<()> {
        let config = read(
            None,
            &FakeFileSystem::new(HashMap::new()),
            &DiffChanges::default(),
        )?;
        assert!(config.settings.ignore.is_empty());
        assert!(config.settings.extensions.is_empty());
        assert!(config.settings.enable.is_empty());
        assert!(config.settings.disable.is_empty());
        assert!(config.settings.only_blocks.is_empty());
        assert!(config.settings.skip_blocks.is_empty());
        assert!(config.blocks.is_empty());
        Ok(())
    }

    #[test]
    fn missing_config_flag_file_is_rejected() {
        let error = read(
            Some(Path::new("no-such-config.toml")),
            &FakeFileSystem::new(HashMap::new()),
            &DiffChanges::default(),
        )
        .expect_err("a config file given by name must exist");
        assert!(
            error.to_string().contains("no-such-config.toml"),
            "the error must show the file: {error}"
        );
    }
}
