use crate::fs::FileSystem;
use crate::settings::RawSettings;
use anyhow::Context;
use std::path::Path;

/// The default name of the config file.
pub const DEFAULT_FILE: &str = "blockwatch.toml";

/// Reads the settings from the config file.
///
/// If `path` is given, that file is read, and it must exist. A relative `path` starts from the
/// current directory, not from the root of `file_system`. If `path` is `None`, [`DEFAULT_FILE`] is
/// read from the root of `file_system`. If that file does not exist, the settings are empty.
///
/// The values are not validated here. Returns an error if the file can't be read, isn't valid
/// TOML, has an unknown key, or has a value of the wrong type. The error names the file and shows
/// the line and column of the problem.
pub fn read(path: Option<&Path>, file_system: &impl FileSystem) -> anyhow::Result<RawSettings> {
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
                return Ok(RawSettings::default());
            }
            (path, file_system.read_to_string(path)?)
        }
    };
    toml_edit::de::from_str(&text)
        .with_context(|| format!("invalid config file \"{}\"", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::test_utils::FakeFileSystem;
    use std::collections::HashMap;

    #[test]
    fn missing_default_file_reads_as_empty_settings() -> anyhow::Result<()> {
        let settings = read(None, &FakeFileSystem::new(HashMap::new()))?;
        assert!(settings.ignore.is_empty());
        assert!(settings.extensions.is_empty());
        assert!(settings.enable.is_empty());
        assert!(settings.disable.is_empty());
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
