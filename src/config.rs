use crate::Position;
use crate::blocks::validate_attributes;
use crate::fs::FileSystem;
use crate::settings::RawSettings;
use crate::validators::{TargetReference, parse_single_reference};
use crate::virtual_blocks::{ConfigEntry, VirtualBlock};
use anyhow::{Context, anyhow, bail};
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_spanned::Spanned;
use std::collections::HashMap;
use std::path::Path;

/// The default name of the config file.
pub const DEFAULT_FILE: &str = "blockwatch.toml";

/// What the config file holds.
#[derive(Debug, Default)]
pub struct Config {
    /// The settings. They are not validated yet.
    pub settings: RawSettings,
    /// The virtual blocks, in the order the file declares them.
    pub blocks: Vec<VirtualBlock>,
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
/// The settings are not validated here. Returns an error if the file can't be read, isn't valid
/// TOML, has an unknown key or a value of the wrong type, or declares a block that is not valid.
/// The error shows the file, and the line of the problem.
pub fn read(path: Option<&Path>, file_system: &impl FileSystem) -> anyhow::Result<Config> {
    let (path, text) = match path {
        // Like `--suppress-from`, this is a path the person running the tool provided which can be
        // outside the repository.
        Some(path) => (
            path,
            std::fs::read_to_string(path)
                .with_context(|| format!("failed to read config file \"{}\"", path.display()))?,
        ),
        None => {
            let path = Path::new(DEFAULT_FILE);
            if !file_system.exists(path) {
                return Ok(Config::default());
            }
            (path, file_system.read_to_string(path)?)
        }
    };
    let ConfigFile {
        ignore,
        extensions,
        enable,
        disable,
        block,
    } = toml_edit::de::from_str(&text)
        .with_context(|| format!("invalid config file \"{}\"", path.display()))?;
    let blocks: Vec<VirtualBlock> = block
        .into_iter()
        .map(|block_entry| {
            let entry = ConfigEntry {
                file: path.to_path_buf(),
                line: Position::from_byte_offset(&text, block_entry.span().start).line,
            };
            let context = entry.error_context();
            virtual_block(block_entry.into_inner(), entry).context(context)
        })
        .collect::<anyhow::Result<_>>()?;
    // A symbol has at most one block, as all its rules fit in one. Allowing more blocks later
    // breaks no config, while forbidding them later would.
    let mut first_lines = HashMap::new();
    for block in &blocks {
        if let Some(first_line) = first_lines.insert((&block.file, &block.path), block.entry.line) {
            return Err(anyhow!(
                "target {} already has the block at line {first_line}",
                block.target()
            )
            .context(block.entry.error_context()));
        }
    }
    Ok(Config {
        settings: RawSettings {
            ignore,
            extensions,
            enable,
            disable,
        },
        blocks,
    })
}

/// The virtual block that `block_entry`, written as `entry` of the config file, declares.
///
/// # Errors
/// Returns an error if the target is not a symbol in a file, or if an attribute is not one a tag
/// can have, or has a value that is not a string, `true` or an integer.
fn virtual_block(block_entry: BlockEntry, entry: ConfigEntry) -> anyhow::Result<VirtualBlock> {
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::test_utils::FakeFileSystem;
    use crate::repo_path::RepoPath;
    use crate::symbol_path::SymbolPath;
    use std::path::PathBuf;

    /// Reads `text` as the default config file.
    fn read_text(text: &str) -> anyhow::Result<Config> {
        read(
            None,
            &FakeFileSystem::new(HashMap::from([(
                DEFAULT_FILE.to_string(),
                text.to_string(),
            )])),
        )
    }

    /// The error of reading `text` as the default config file, with its causes.
    fn read_error(text: &str) -> String {
        let error = read_text(text).expect_err("the config file must be rejected");
        format!("{error:#}")
    }

    #[test]
    fn block_entries_read_as_virtual_blocks() -> anyhow::Result<()> {
        let config = read_text(
            "ignore = []\n\n[[block]]\ntarget = 'package.json#/version'\nname = 'version'\n\
             keep-unique = true\ncheck-lua-timeout = 30\n\n[[block]]\ntarget = 'a.yaml#/b'\n",
        )?;

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
                },
                VirtualBlock {
                    file: RepoPath::from_reference("a.yaml")?,
                    path: SymbolPath::parse("/b")?,
                    attributes: HashMap::new(),
                    entry: ConfigEntry {
                        file: PathBuf::from(DEFAULT_FILE),
                        line: 9,
                    },
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn second_block_for_a_symbol_is_rejected() {
        // `./package.json` is another spelling of the same file.
        assert_eq!(
            read_error(
                "[[block]]\ntarget = 'package.json#/version'\nline-count = '1'\n\n\
                 [[block]]\ntarget = './package.json#/version'\nname = 'version'\n"
            ),
            "invalid block at line 5 of \"blockwatch.toml\": target package.json#/version \
             already has the block at line 1"
        );
    }

    #[test]
    fn blocks_for_other_symbols_are_accepted() -> anyhow::Result<()> {
        // Each target shares its file or its path with another one, but not both.
        let config = read_text(
            "[[block]]\ntarget = 'a.json#/x'\n\n[[block]]\ntarget = 'a.json#/y'\n\n\
             [[block]]\ntarget = 'b.json#/x'\n",
        )?;

        assert_eq!(config.blocks.len(), 3);
        Ok(())
    }

    #[test]
    fn target_that_is_not_a_symbol_in_a_file_is_rejected() {
        for target in ["#/x", "a.md:name", "a.json"] {
            assert_eq!(
                read_error(&format!("[[block]]\ntarget = '{target}'\n")),
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
            assert_eq!(
                read_error(&format!(
                    "[[block]]\ntarget = 'a.json#/x'\nline-count = {value}\n"
                )),
                "invalid block at line 1 of \"blockwatch.toml\": `line-count` must be a string, \
                 `true` or an integer",
                "{value}"
            );
        }
    }

    #[test]
    fn unknown_attribute_is_rejected() {
        assert_eq!(
            read_error("[[block]]\ntarget = 'a.json#/x'\nkeep-sortd = true\n"),
            "invalid block at line 1 of \"blockwatch.toml\": unrecognized attribute `keep-sortd`"
        );
    }

    #[test]
    fn unknown_severity_is_rejected() {
        assert_eq!(
            read_error("[[block]]\ntarget = 'a.json#/x'\nseverity = 'loud'\n"),
            "invalid block at line 1 of \"blockwatch.toml\": unrecognized severity value `loud`"
        );
    }

    #[test]
    fn missing_default_file_reads_as_an_empty_config() -> anyhow::Result<()> {
        let config = read(None, &FakeFileSystem::new(HashMap::new()))?;
        assert!(config.settings.ignore.is_empty());
        assert!(config.settings.extensions.is_empty());
        assert!(config.settings.enable.is_empty());
        assert!(config.settings.disable.is_empty());
        assert!(config.blocks.is_empty());
        Ok(())
    }

    #[test]
    fn missing_config_flag_file_is_rejected() {
        let error = read(
            Some(Path::new("no-such-config.toml")),
            &FakeFileSystem::new(HashMap::new()),
        )
        .expect_err("a config file given by name must exist");
        assert!(
            error.to_string().contains("no-such-config.toml"),
            "the error must show the file: {error}"
        );
    }
}
