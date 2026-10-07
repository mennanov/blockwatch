use crate::Position;
use crate::blocks::validate_attributes;
use crate::diff_parser::{self, LineChange};
use crate::fs::FileSystem;
use crate::language_parsers::toml;
use crate::repo_path::RepoPath;
use crate::settings::RawSettings;
use crate::symbol_path::SymbolPath;
use crate::symbols::{self, Symbol};
use crate::validators::{TargetReference, parse_single_reference};
use crate::virtual_blocks::{ConfigEntry, VirtualBlock};
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
    /// The `[[block]]` entries, each with the byte range of its header.
    #[serde(default)]
    block: Vec<Spanned<BlockEntry>>,
}

/// A `[[block]]` entry, as written.
#[derive(Deserialize)]
struct BlockEntry {
    /// The symbol the block wraps, such as `package.json#/version`.
    target: String,
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
    line_changes_by_file: &HashMap<RepoPath, Vec<LineChange>>,
) -> anyhow::Result<Config> {
    let (path, text, repo_path) = match read_file(path, file_system)? {
        Some(file) => file,
        None => return Ok(Config::default()),
    };
    let line_changes = repo_path
        .and_then(|repo_path| line_changes_by_file.get(&repo_path))
        .map_or(&[][..], Vec::as_slice);
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
        blocks: virtual_blocks(block, path, &text, line_changes)?,
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

/// The virtual blocks that `entries` declare, in the order of `entries`. `path` and `text` are
/// the config file's, and `line_changes` are the diff's changes to it.
///
/// # Errors
/// Returns an error if an entry is not valid, or if two entries wrap the same symbol. The error
/// shows the line of the entry.
fn virtual_blocks(
    entries: Vec<Spanned<BlockEntry>>,
    path: &Path,
    text: &str,
    line_changes: &[LineChange],
) -> anyhow::Result<Vec<VirtualBlock>> {
    let symbols = toml::parse_symbols(text)?;
    let blocks: Vec<VirtualBlock> = entries
        .into_iter()
        .enumerate()
        .map(|(index, block_entry)| {
            let entry = ConfigEntry {
                file: path.to_path_buf(),
                line: Position::from_byte_offset(text, block_entry.span().start).line,
            };
            let context = entry.error_context();
            let is_entry_modified = is_entry_modified(text, &symbols, index, line_changes)?;
            virtual_block(block_entry.into_inner(), entry, is_entry_modified).context(context)
        })
        .collect::<anyhow::Result<_>>()?;
    reject_second_blocks(&blocks)?;
    Ok(blocks)
}

/// Rejects a block whose symbol already has a block earlier in `blocks`.
///
/// # Errors
/// Returns an error that shows the line of the second block's entry and of the first one.
fn reject_second_blocks(blocks: &[VirtualBlock]) -> anyhow::Result<()> {
    // A symbol has at most one block, as all its rules fit in one. Allowing more blocks later
    // breaks no config, while forbidding them later would.
    let mut first_lines = HashMap::new();
    for block in blocks {
        if let Some(first_line) = first_lines.insert((&block.file, &block.path), block.entry.line) {
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
/// `symbols` are the symbols of `text`.
fn is_entry_modified(
    text: &str,
    symbols: &[Symbol],
    index: usize,
    line_changes: &[LineChange],
) -> anyhow::Result<bool> {
    // The symbol covers every line of the entry. The span that serde gives covers only its header.
    let path = SymbolPath::parse(&format!("/block/{index}"))?;
    let symbol = symbols::resolve(symbols, &path).map_err(|error| anyhow!(error.reason(text)))?;
    Ok(symbol.def_byte_ranges.iter().any(|range| {
        let positions = Position::from_byte_offset(text, range.start)
            ..Position::from_byte_offset(text, range.end);
        diff_parser::range_intersects_any(&positions, line_changes)
    }))
}

/// The virtual block that `block_entry`, written as `entry` of the config file, declares.
/// `is_entry_modified` tells whether the diff touches the entry.
///
/// # Errors
/// Returns an error if the target is not a symbol in a file, or if an attribute is not one a tag
/// can have, or has a value that is not a string, `true` or an integer.
fn virtual_block(
    block_entry: BlockEntry,
    entry: ConfigEntry,
    is_entry_modified: bool,
) -> anyhow::Result<VirtualBlock> {
    let (file, path) = match parse_single_reference(&block_entry.target)? {
        TargetReference::Symbol {
            file: Some(file),
            path,
        } => (file, path),
        TargetReference::Symbol { file: None, .. }
        | TargetReference::Block { .. }
        | TargetReference::File(_) => bail!(
            "target `{}` must be a symbol in a file, such as `package.json#/version`",
            block_entry.target
        ),
    };
    let attributes = block_entry
        .attributes
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
        .collect::<anyhow::Result<_>>()?;
    validate_attributes(&attributes)?;
    Ok(VirtualBlock {
        file,
        path,
        attributes,
        entry,
        is_entry_modified,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_parser::LineChangeKind;
    use crate::fs::test_utils::FakeFileSystem;
    use std::path::PathBuf;

    /// Reads `text` as the default config file, with no diff.
    fn read_text(text: &str) -> anyhow::Result<Config> {
        read_changed_text(text, &[])
    }

    /// Reads `text` as the default config file, which the diff makes `line_changes` to.
    fn read_changed_text(text: &str, line_changes: &[LineChange]) -> anyhow::Result<Config> {
        read(
            None,
            &FakeFileSystem::new(HashMap::from([(
                DEFAULT_FILE.to_string(),
                text.to_string(),
            )])),
            &HashMap::from([(
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
                    path: SymbolPath::parse("/version")?,
                    attributes: HashMap::from([
                        ("name".to_string(), "version".to_string()),
                        ("keep-unique".to_string(), String::new()),
                        ("check-lua-timeout".to_string(), "30".to_string()),
                    ]),
                    entry: ConfigEntry {
                        file: PathBuf::from(DEFAULT_FILE),
                        line: 3,
                    },
                    is_entry_modified: false,
                },
                VirtualBlock {
                    file: RepoPath::from_reference("a.yaml")?,
                    path: SymbolPath::parse("/b")?,
                    attributes: HashMap::new(),
                    entry: ConfigEntry {
                        file: PathBuf::from(DEFAULT_FILE),
                        line: 9,
                    },
                    is_entry_modified: false,
                },
            ]
        );
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
    fn second_block_for_a_symbol_is_rejected() {
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
    fn blocks_for_other_symbols_are_accepted() -> anyhow::Result<()> {
        // Each target shares its file or its path with another one, but not both.
        let text = r"[[block]]
target = 'a.json#/x'

[[block]]
target = 'a.json#/y'

[[block]]
target = 'b.json#/x'
";

        let config = read_text(text)?;

        assert_eq!(config.blocks.len(), 3);
        Ok(())
    }

    #[test]
    fn target_that_is_not_a_symbol_in_a_file_is_rejected() {
        for target in ["#/x", "a.md:name", "a.json"] {
            let text = format!(
                r"[[block]]
target = '{target}'
"
            );

            assert_eq!(
                read_error(&text),
                format!(
                    "invalid block at line 1 of \"blockwatch.toml\": target `{target}` must be a \
                     symbol in a file, such as `package.json#/version`"
                )
            );
        }
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
    fn block_lists_read_into_the_settings() -> anyhow::Result<()> {
        let config =
            read_text("only-blocks = ['a.py:fruits']\nskip-blocks = ['b.py:vegetables']\n")?;
        assert_eq!(config.settings.only_blocks, ["a.py:fruits"]);
        assert_eq!(config.settings.skip_blocks, ["b.py:vegetables"]);
        Ok(())
    }

    #[test]
    fn missing_default_file_reads_as_an_empty_config() -> anyhow::Result<()> {
        let config = read(None, &FakeFileSystem::new(HashMap::new()), &HashMap::new())?;
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
            &HashMap::new(),
        )
        .expect_err("a config file given by name must exist");
        assert!(
            error.to_string().contains("no-such-config.toml"),
            "the error must show the file: {error}"
        );
    }
}
