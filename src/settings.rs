use crate::blocks::BlockAddress;
use crate::validators;
use anyhow::{Context, bail};
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;

/// Project-wide raw settings.
///
/// Can be constructed from a config file or the command line.
/// The values are not guaranteed to be valid. The [`RawSettings::validate`] method validates them.
#[derive(Debug, Default)]
pub(crate) struct RawSettings {
    /// Glob patterns of files to leave out.
    pub(crate) ignore: Vec<String>,
    /// Extra file extension mappings.
    pub(crate) extensions: HashMap<String, String>,
    /// The only validators to run.
    pub(crate) enable: Vec<String>,
    /// Validators to leave out.
    pub(crate) disable: Vec<String>,
    /// The addresses of the only blocks to check.
    pub(crate) only_blocks: Vec<String>,
    /// The addresses of blocks to leave out.
    pub(crate) skip_blocks: Vec<String>,
}

impl RawSettings {
    /// Validates all the setting values.
    ///
    /// `supported_extensions` is a set of supported file extensions.
    /// If `Ok` is returned, then all the values are valid.
    fn validate(&self, supported_extensions: &HashSet<&OsString>) -> anyhow::Result<()> {
        for glob in &self.ignore {
            Glob::new(glob).with_context(|| format!("Invalid ignore glob pattern: {glob}"))?;
        }
        for (key, value) in &self.extensions {
            if !supported_extensions.contains(&OsString::from(value)) {
                bail!("Unsupported extension mapping: {key}={value}");
            }
        }
        for name in self.enable.iter().chain(&self.disable) {
            parse_validator(name)?;
        }
        if !self.enable.is_empty() && !self.disable.is_empty() {
            bail!("`enable` and `disable` must not both be set");
        }
        for address in self.only_blocks.iter().chain(&self.skip_blocks) {
            BlockAddress::parse(address)?;
        }
        if !self.only_blocks.is_empty() && !self.skip_blocks.is_empty() {
            bail!("`only-block` and `skip-block` must not both be set");
        }
        Ok(())
    }
}

/// The validated settings for a run: the config file merged with the flags.
///
/// The fields are private, so the only way to get a `Settings` is [`Settings::resolve`], which
/// checks every value.
#[derive(Debug)]
pub(crate) struct Settings {
    ignored_globs: GlobSet,
    extensions: HashMap<OsString, OsString>,
    disabled_validators: HashSet<&'static str>,
    enabled_validators: HashSet<&'static str>,
    block_selection: BlockSelection,
}

impl Settings {
    /// Validates the settings from the flags and from the config file, then merges them.
    ///
    /// Both are validated in full, even values that the other one overrides. When merging:
    ///
    /// - The ignore globs from both are used.
    /// - The extension mappings from both are used. If both map the same extension, `flags` wins.
    /// - If `flags` enables or disables any validator, the `enable` and `disable` lists of
    ///   `config_file` are ignored.
    /// - If `flags` lists any block to check or to skip, the `only_blocks` and `skip_blocks` lists
    ///   of `config_file` are ignored.
    ///
    /// `supported_extensions` are the extensions that have a language parser. Returns an error if
    /// either source has an invalid value. The error says which source it came from.
    pub(crate) fn resolve(
        flags: RawSettings,
        config_file: RawSettings,
        supported_extensions: &HashSet<&OsString>,
    ) -> anyhow::Result<Settings> {
        flags
            .validate(supported_extensions)
            .context("invalid command-line flags")?;
        config_file
            .validate(supported_extensions)
            .context("invalid config file")?;

        // `-e keep-sorted` usually means "run only keep-sorted this time". Adding it to the config
        // file's lists would not do that. It could also mix `enable` with `disable`, which is not
        // allowed.
        let selection = if flags.enable.is_empty() && flags.disable.is_empty() {
            &config_file
        } else {
            &flags
        };
        // The names were validated above, so these lookups can't fail. They are only here to get
        // the `&'static str` for each name.
        let disabled_validators = selection
            .disable
            .iter()
            .map(|name| parse_validator(name))
            .collect::<anyhow::Result<_>>()?;
        let enabled_validators = selection
            .enable
            .iter()
            .map(|name| parse_validator(name))
            .collect::<anyhow::Result<_>>()?;
        // `--only-block` usually means "check only this block this time", even a block that the
        // config file skips.
        let block_lists = if flags.only_blocks.is_empty() && flags.skip_blocks.is_empty() {
            &config_file
        } else {
            &flags
        };
        let block_selection = block_selection(block_lists)?;

        let mut ignored_globs = GlobSetBuilder::new();
        for glob in config_file.ignore.iter().chain(&flags.ignore) {
            ignored_globs.add(Glob::new(glob)?);
        }
        // If an extension is mapped twice, the later mapping wins. The flags go last so that they
        // win.
        let extensions = config_file
            .extensions
            .into_iter()
            .chain(flags.extensions)
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect();

        Ok(Settings {
            ignored_globs: ignored_globs
                .build()
                .context("Failed to build ignore glob set")?,
            extensions,
            disabled_validators,
            enabled_validators,
            block_selection,
        })
    }

    /// Files to skip, even if a glob selects them.
    pub(crate) fn ignored_globs(&self) -> &GlobSet {
        &self.ignored_globs
    }

    /// Maps an extension to a supported one. For example, `cxx` to `cpp` makes `.cxx` files parse
    /// as C++.
    pub(crate) fn extensions(&self) -> &HashMap<OsString, OsString> {
        &self.extensions
    }

    /// Validators to skip. Empty if [`Settings::enabled_validators`] is not.
    pub(crate) fn disabled_validators(&self) -> &HashSet<&'static str> {
        &self.disabled_validators
    }

    /// If not empty, only these validators run. If empty, every validator runs except the
    /// disabled ones.
    pub(crate) fn enabled_validators(&self) -> &HashSet<&'static str> {
        &self.enabled_validators
    }

    /// Which blocks the run checks.
    pub(crate) fn block_selection(&self) -> &BlockSelection {
        &self.block_selection
    }
}

/// The blocks that `settings` selects. `settings` must be validated, so that at most one of its
/// lists is set.
fn block_selection(settings: &RawSettings) -> anyhow::Result<BlockSelection> {
    let parse = |addresses: &[String]| {
        addresses
            .iter()
            .map(|address| BlockAddress::parse(address))
            .collect::<anyhow::Result<_>>()
    };
    let selection = match (
        settings.only_blocks.is_empty(),
        settings.skip_blocks.is_empty(),
    ) {
        (true, true) => BlockSelection::All,
        (false, _) => BlockSelection::Only(parse(&settings.only_blocks)?),
        (true, false) => BlockSelection::Skip(parse(&settings.skip_blocks)?),
    };
    Ok(selection)
}

/// Which blocks a run checks. Each address is `FILE:BLOCK_NAME`.
#[derive(Debug, PartialEq)]
pub(crate) enum BlockSelection {
    /// Every block.
    All,
    /// Only the blocks that an address selects.
    Only(Vec<BlockAddress>),
    /// Every block except the ones that an address selects.
    Skip(Vec<BlockAddress>),
}

/// Looks up the validator called `value` and returns its name. Returns an error if there is no
/// validator with that name.
pub(crate) fn parse_validator(value: &str) -> anyhow::Result<&'static str> {
    let validators = validators::validator_names();
    validators
        .iter()
        .find(|name| **name == value)
        .copied()
        .with_context(|| {
            format!(
                "Unknown validator: {value}. Available validators: {}",
                validators.join(", ")
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(flags: RawSettings, config_file: RawSettings) -> anyhow::Result<Settings> {
        let (cpp, py) = (OsString::from("cpp"), OsString::from("py"));
        Settings::resolve(flags, config_file, &HashSet::from([&cpp, &py]))
    }

    fn check_error(settings: RawSettings) -> String {
        let cpp = OsString::from("cpp");
        let error = settings
            .validate(&HashSet::from([&cpp]))
            .expect_err("the settings must be rejected");
        format!("{error:#}")
    }

    fn to_string_vec(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn block_addresses(addresses: &[&str]) -> Vec<BlockAddress> {
        addresses
            .iter()
            .map(|address| BlockAddress::parse(address).expect("a valid address"))
            .collect()
    }

    #[test]
    fn ignore_globs_in_flags_and_config_file_are_combined() -> anyhow::Result<()> {
        let flags = RawSettings {
            ignore: to_string_vec(&["vendor/**"]),
            ..RawSettings::default()
        };
        let config_file = RawSettings {
            ignore: to_string_vec(&["generated/**"]),
            ..RawSettings::default()
        };
        let settings = resolve(flags, config_file)?;
        assert!(settings.ignored_globs.is_match("generated/a.rs"));
        assert!(settings.ignored_globs.is_match("vendor/b.rs"));
        assert!(!settings.ignored_globs.is_match("src/c.rs"));
        Ok(())
    }

    #[test]
    fn extensions_in_flags_and_config_file_are_combined() -> anyhow::Result<()> {
        let flags = RawSettings {
            extensions: HashMap::from([("cxx".to_string(), "cpp".to_string())]),
            ..RawSettings::default()
        };
        let config_file = RawSettings {
            extensions: HashMap::from([("pyw".to_string(), "py".to_string())]),
            ..RawSettings::default()
        };
        assert_eq!(
            resolve(flags, config_file)?.extensions,
            HashMap::from([
                (OsString::from("cxx"), OsString::from("cpp")),
                (OsString::from("pyw"), OsString::from("py")),
            ])
        );
        Ok(())
    }

    #[test]
    fn same_extension_in_flags_and_config_file_uses_the_flag() -> anyhow::Result<()> {
        let flags = RawSettings {
            extensions: HashMap::from([("hpp".to_string(), "cpp".to_string())]),
            ..RawSettings::default()
        };
        let config_file = RawSettings {
            extensions: HashMap::from([("hpp".to_string(), "py".to_string())]),
            ..RawSettings::default()
        };
        assert_eq!(
            resolve(flags, config_file)?.extensions,
            HashMap::from([(OsString::from("hpp"), OsString::from("cpp"))])
        );
        Ok(())
    }

    #[test]
    fn enable_in_config_file_runs_only_those_validators() -> anyhow::Result<()> {
        let config_file = RawSettings {
            enable: to_string_vec(&["keep-sorted"]),
            ..RawSettings::default()
        };
        let settings = resolve(RawSettings::default(), config_file)?;
        assert_eq!(settings.enabled_validators, HashSet::from(["keep-sorted"]));
        assert!(settings.disabled_validators.is_empty());
        Ok(())
    }

    #[test]
    fn disable_in_config_file_skips_those_validators() -> anyhow::Result<()> {
        let config_file = RawSettings {
            disable: to_string_vec(&["check-ai"]),
            ..RawSettings::default()
        };
        let settings = resolve(RawSettings::default(), config_file)?;
        assert_eq!(settings.disabled_validators, HashSet::from(["check-ai"]));
        assert!(settings.enabled_validators.is_empty());
        Ok(())
    }

    #[test]
    fn disable_flag_replaces_enable_in_config_file() -> anyhow::Result<()> {
        let flags = RawSettings {
            disable: to_string_vec(&["check-ai"]),
            ..RawSettings::default()
        };
        let config_file = RawSettings {
            enable: to_string_vec(&["keep-sorted"]),
            ..RawSettings::default()
        };
        let settings = resolve(flags, config_file)?;
        assert_eq!(settings.disabled_validators, HashSet::from(["check-ai"]));
        assert!(settings.enabled_validators.is_empty());
        Ok(())
    }

    #[test]
    fn enable_flag_replaces_disable_in_config_file() -> anyhow::Result<()> {
        let flags = RawSettings {
            enable: to_string_vec(&["keep-sorted"]),
            ..RawSettings::default()
        };
        let config_file = RawSettings {
            disable: to_string_vec(&["check-ai"]),
            ..RawSettings::default()
        };
        let settings = resolve(flags, config_file)?;
        assert_eq!(settings.enabled_validators, HashSet::from(["keep-sorted"]));
        assert!(settings.disabled_validators.is_empty());
        Ok(())
    }

    #[test]
    fn only_blocks_in_config_file_check_only_those_blocks() -> anyhow::Result<()> {
        let config_file = RawSettings {
            only_blocks: to_string_vec(&["a.py:fruits"]),
            ..RawSettings::default()
        };
        let settings = resolve(RawSettings::default(), config_file)?;
        assert_eq!(
            settings.block_selection,
            BlockSelection::Only(block_addresses(&["a.py:fruits"]))
        );
        Ok(())
    }

    #[test]
    fn skip_blocks_in_config_file_skip_those_blocks() -> anyhow::Result<()> {
        let config_file = RawSettings {
            skip_blocks: to_string_vec(&["a.py:fruits"]),
            ..RawSettings::default()
        };
        let settings = resolve(RawSettings::default(), config_file)?;
        assert_eq!(
            settings.block_selection,
            BlockSelection::Skip(block_addresses(&["a.py:fruits"]))
        );
        Ok(())
    }

    #[test]
    fn only_block_flag_replaces_skip_blocks_in_config_file() -> anyhow::Result<()> {
        // A weekly job runs only the block that the config file skips on every other run.
        let flags = RawSettings {
            only_blocks: to_string_vec(&["a.py:fruits"]),
            ..RawSettings::default()
        };
        let config_file = RawSettings {
            skip_blocks: to_string_vec(&["a.py:fruits"]),
            ..RawSettings::default()
        };
        let settings = resolve(flags, config_file)?;
        assert_eq!(
            settings.block_selection,
            BlockSelection::Only(block_addresses(&["a.py:fruits"]))
        );
        Ok(())
    }

    #[test]
    fn skip_block_flag_replaces_only_blocks_in_config_file() -> anyhow::Result<()> {
        let flags = RawSettings {
            skip_blocks: to_string_vec(&["a.py:vegetables"]),
            ..RawSettings::default()
        };
        let config_file = RawSettings {
            only_blocks: to_string_vec(&["a.py:fruits"]),
            ..RawSettings::default()
        };
        let settings = resolve(flags, config_file)?;
        assert_eq!(
            settings.block_selection,
            BlockSelection::Skip(block_addresses(&["a.py:vegetables"]))
        );
        Ok(())
    }

    #[test]
    fn bad_flag_is_rejected_as_a_flag_error() {
        let flags = RawSettings {
            extensions: HashMap::from([("cxx".to_string(), "cobol".to_string())]),
            ..RawSettings::default()
        };
        let error = format!(
            "{:#}",
            resolve(flags, RawSettings::default()).expect_err("the flags must be rejected")
        );
        assert!(error.contains("invalid command-line flags"), "{error}");
        assert!(error.contains("cxx=cobol"), "{error}");
    }

    #[test]
    fn bad_config_file_value_is_rejected_even_if_a_flag_overrides_it() {
        let flags = RawSettings {
            enable: to_string_vec(&["keep-sorted"]),
            ..RawSettings::default()
        };
        let config_file = RawSettings {
            disable: to_string_vec(&["keep-tidy"]),
            ..RawSettings::default()
        };
        let error = format!(
            "{:#}",
            resolve(flags, config_file).expect_err("the config file must be rejected")
        );
        assert!(error.contains("invalid config file"), "{error}");
        assert!(error.contains("keep-tidy"), "{error}");
    }

    #[test]
    fn ignore_glob_that_does_not_compile_is_rejected() {
        let error = check_error(RawSettings {
            ignore: to_string_vec(&["docs/**", "a/[b"]),
            ..RawSettings::default()
        });
        assert!(error.contains("a/[b"), "{error}");
    }

    #[test]
    fn extension_mapped_to_unsupported_language_is_rejected() {
        let error = check_error(RawSettings {
            extensions: HashMap::from([
                ("cxx".to_string(), "cpp".to_string()),
                ("hpp".to_string(), "cobol".to_string()),
            ]),
            ..RawSettings::default()
        });
        assert!(error.contains("hpp=cobol"), "{error}");
    }

    #[test]
    fn unknown_validator_is_rejected() {
        let error = check_error(RawSettings {
            disable: to_string_vec(&["keep-sorted", "keep-tidy"]),
            ..RawSettings::default()
        });
        assert!(error.contains("Unknown validator: keep-tidy"), "{error}");
    }

    #[test]
    fn enable_and_disable_together_are_rejected() {
        let error = check_error(RawSettings {
            enable: to_string_vec(&["keep-sorted"]),
            disable: to_string_vec(&["check-ai"]),
            ..RawSettings::default()
        });
        assert!(error.contains("`enable` and `disable`"), "{error}");
    }

    #[test]
    fn block_address_without_a_block_name_is_rejected() {
        let error = check_error(RawSettings {
            skip_blocks: to_string_vec(&["a.py:fruits", "a.py"]),
            ..RawSettings::default()
        });
        assert!(error.contains("expected FILE:BLOCK_NAME"), "{error}");
    }

    #[test]
    fn only_blocks_and_skip_blocks_together_are_rejected() {
        let error = check_error(RawSettings {
            only_blocks: to_string_vec(&["a.py:fruits"]),
            skip_blocks: to_string_vec(&["a.py:vegetables"]),
            ..RawSettings::default()
        });
        assert!(error.contains("`only-block` and `skip-block`"), "{error}");
    }
}
