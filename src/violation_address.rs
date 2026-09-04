use crate::repo_path::RepoPath;
use crate::validators;
use anyhow::{Context, bail};
use serde::{Serialize, Serializer};
use std::fmt;
use std::hash::Hasher;

/// How many hex digits of the text hash go into an address.
const TEXT_HASH_HEX_LEN: usize = 8;

/// Violation address: `FILE[:BLOCK_NAME[:VALIDATOR[:HASH]]]`.
///
/// Only the leading `FILE` segment is required. Every segment left off the end widens what the
/// address covers. See [`Self::matches`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ViolationAddress {
    file: RepoPath,
    block_name: Option<String>,
    validator: Option<String>,
    text_hash: Option<String>,
}

impl ViolationAddress {
    /// The address of one violation, or `None` when the block has no `name`.
    pub(crate) fn new(
        file: &RepoPath,
        block_name: Option<&str>,
        validator: &str,
        violation_text: Option<&str>,
    ) -> Option<Self> {
        Some(Self {
            file: file.clone(),
            block_name: Some(block_name?.to_string()),
            validator: Some(validator.to_string()),
            text_hash: violation_text.map(text_hash),
        })
    }

    /// Parses the [`ViolationAddress`] from string.
    pub fn parse(address: &str) -> anyhow::Result<Self> {
        let segments: Vec<&str> = address.split(':').collect();
        if segments.len() > 4 {
            bail!("expected FILE[:BLOCK_NAME[:VALIDATOR[:HASH]]], got \"{address}\"");
        }
        if segments.iter().any(|segment| segment.is_empty()) {
            bail!("no segment of a violation address may be empty, got \"{address}\"");
        }
        let file = RepoPath::from_reference(segments[0])
            .with_context(|| format!("invalid file in the violation address \"{address}\""))?;
        let validator = match segments.get(2) {
            Some(name) => Some(
                known_validator(name)
                    .with_context(|| format!("invalid violation address \"{address}\""))?,
            ),
            None => None,
        };
        let text_hash = match segments.get(3) {
            Some(hash) => Some(checked_hash_segment(hash, address)?),
            None => None,
        };
        Ok(Self {
            file,
            block_name: segments.get(1).map(|name| name.to_string()),
            validator,
            text_hash,
        })
    }

    /// Whether this address, used as a `--suppress` argument, covers `violation_address`.
    ///
    /// A segment this address leaves off covers whatever the violation has in its place, so
    /// `FILE` covers every violation in the file, `FILE:BLOCK_NAME` every violation of that block,
    /// and so on down to the single violation a four-segment address mentions.
    pub fn matches(&self, violation_address: &Self) -> bool {
        /// An absent segment matches any value; a present one has to be equal.
        fn covers(suppressed: &Option<String>, reported: &Option<String>) -> bool {
            suppressed.is_none() || suppressed == reported
        }

        self.file == violation_address.file
            && covers(&self.block_name, &violation_address.block_name)
            && covers(&self.validator, &violation_address.validator)
            && covers(&self.text_hash, &violation_address.text_hash)
    }

    /// Whether this address covers every violation in `file`, including the ones on unnamed blocks
    /// that have no address of their own.
    pub fn covers_whole_file(&self, file: &RepoPath) -> bool {
        self.file == *file && self.block_name.is_none()
    }
}

impl fmt::Display for ViolationAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.file)?;
        for segment in [&self.block_name, &self.validator, &self.text_hash]
            .into_iter()
            .flatten()
        {
            write!(formatter, ":{segment}")?;
        }
        Ok(())
    }
}

impl Serialize for ViolationAddress {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// The opaque segment that tells one violation of a block from its siblings.
///
/// FNV-1a is fixed by specification, stable across platforms, Rust versions, and compilers.
fn text_hash(text: &str) -> String {
    let mut hasher = fnv::FnvHasher::default();
    hasher.write(text.as_bytes());
    format!(
        "{:0width$x}",
        hasher.finish() as u32,
        width = TEXT_HASH_HEX_LEN
    )
}

/// The registered validator `name`, or an error listing the ones that exist.
fn known_validator(name: &str) -> anyhow::Result<String> {
    let validators = validators::validator_names();
    if !validators.contains(&name) {
        bail!(
            "unknown validator: {name}. Available validators: {}",
            validators.join(", ")
        );
    }
    Ok(name.to_string())
}

/// The hash segment of `address`, rejected unless it is shaped like one this program emits.
///
/// Being strict turns a truncated or mistyped paste into an error the run reports at once, rather
/// than a suppression that silently matches nothing.
fn checked_hash_segment(hash: &str, address: &str) -> anyhow::Result<String> {
    let well_formed = hash.len() == TEXT_HASH_HEX_LEN
        && hash
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
    if !well_formed {
        bail!(
            "the HASH segment of \"{address}\" must be {TEXT_HASH_HEX_LEN} lowercase hex digits, \
             got \"{hash}\""
        );
    }
    Ok(hash.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo_path::RepoPath;

    fn repo_path(path: &str) -> RepoPath {
        RepoPath::from_reference(path).expect("a valid repository path")
    }

    fn violation_address(block_name: &str, validator: &str, text: &str) -> ViolationAddress {
        ViolationAddress::new(&repo_path("a.md"), Some(block_name), validator, Some(text))
            .expect("a named block has an address")
    }

    #[test]
    fn one_segment_address_matches_every_violation_in_the_file() -> anyhow::Result<()> {
        let suppressed_address = ViolationAddress::parse("a.md")?;
        for (block_name, validator) in [("n", "keep-unique"), ("other", "keep-sorted")] {
            assert!(
                suppressed_address.matches(&violation_address(block_name, validator, "apple")),
                "{block_name}:{validator} must match"
            );
        }
        Ok(())
    }

    #[test]
    fn one_segment_address_does_not_match_another_file() -> anyhow::Result<()> {
        let suppressed_address = ViolationAddress::parse("b.md")?;
        assert!(!suppressed_address.matches(&violation_address("n", "keep-unique", "apple")));
        Ok(())
    }

    #[test]
    fn two_segment_address_matches_every_validator_of_that_block() -> anyhow::Result<()> {
        let suppressed_address = ViolationAddress::parse("a.md:n")?;
        for validator in ["keep-unique", "keep-sorted"] {
            assert!(
                suppressed_address.matches(&violation_address("n", validator, "apple")),
                "{validator} must match"
            );
        }
        assert!(!suppressed_address.matches(&violation_address("other", "keep-unique", "apple")));
        Ok(())
    }

    #[test]
    fn only_a_one_segment_address_covers_a_whole_file() -> anyhow::Result<()> {
        assert!(ViolationAddress::parse("a.md")?.covers_whole_file(&repo_path("a.md")));
        assert!(!ViolationAddress::parse("a.md")?.covers_whole_file(&repo_path("b.md")));
        for address in [
            "a.md:n",
            "a.md:n:keep-unique",
            "a.md:n:keep-unique:8601ec8c",
        ] {
            assert!(
                !ViolationAddress::parse(address)?.covers_whole_file(&repo_path("a.md")),
                "{address} names a block, so it must not cover the whole file"
            );
        }
        Ok(())
    }

    #[test]
    fn three_segment_address_matches_every_violation_of_that_validator() -> anyhow::Result<()> {
        let suppressed_address = ViolationAddress::parse("a.md:n:keep-unique")?;
        for text in ["apple", "banana"] {
            assert!(
                suppressed_address.matches(&violation_address("n", "keep-unique", text)),
                "{text} must match"
            );
        }
        Ok(())
    }

    #[test]
    fn four_segment_address_matches_only_its_own_violation() -> anyhow::Result<()> {
        let apple = violation_address("n", "keep-unique", "apple");
        let banana = violation_address("n", "keep-unique", "banana");
        let suppressed_address = ViolationAddress::parse(&apple.to_string())?;
        assert!(suppressed_address.matches(&apple));
        assert!(!suppressed_address.matches(&banana));
        Ok(())
    }

    #[test]
    fn violation_of_another_validator_on_the_same_block_does_not_match() -> anyhow::Result<()> {
        let suppressed_address = ViolationAddress::parse("a.md:n:keep-unique")?;
        assert!(!suppressed_address.matches(&violation_address("n", "keep-sorted", "apple")));
        Ok(())
    }

    #[test]
    fn violation_text_is_hashed_with_fnv_hash_algo() {
        let address = ViolationAddress::new(
            &repo_path("src/lib.rs"),
            Some("languages"),
            "keep-sorted",
            Some("a"),
        )
        .expect("a named block has an address");
        assert_eq!(
            address.to_string(),
            "src/lib.rs:languages:keep-sorted:8601ec8c"
        );
    }

    #[test]
    fn identical_violation_text_yields_the_same_address() {
        assert_eq!(
            violation_address("n", "keep-unique", "apple"),
            violation_address("n", "keep-unique", "apple")
        );
    }

    #[test]
    fn violation_on_an_unnamed_block_has_no_address() {
        assert_eq!(
            ViolationAddress::new(&repo_path("src/lib.rs"), None, "keep-sorted", Some("apple")),
            None
        );
    }

    #[test]
    fn three_segment_address_display_matches_parse_input() -> anyhow::Result<()> {
        let address = ViolationAddress::parse("docs/cli.md:cli-docs:keep-sorted")?;
        assert_eq!(address.to_string(), "docs/cli.md:cli-docs:keep-sorted");
        Ok(())
    }

    #[test]
    fn four_segment_address_display_matches_parse_input() -> anyhow::Result<()> {
        let address = ViolationAddress::parse("docs/cli.md:cli-docs:keep-sorted:3f9a1c4e")?;
        assert_eq!(
            address.to_string(),
            "docs/cli.md:cli-docs:keep-sorted:3f9a1c4e"
        );
        Ok(())
    }

    #[test]
    fn address_with_a_leading_dot_slash_parses_to_the_same_address() -> anyhow::Result<()> {
        assert_eq!(
            ViolationAddress::parse("./docs/cli.md:cli-docs:keep-sorted")?,
            ViolationAddress::parse("docs/cli.md:cli-docs:keep-sorted")?,
        );
        Ok(())
    }

    #[test]
    fn shorter_address_display_matches_parse_input() -> anyhow::Result<()> {
        for address in ["docs/cli.md", "docs/cli.md:cli-docs"] {
            assert_eq!(ViolationAddress::parse(address)?.to_string(), address);
        }
        Ok(())
    }

    #[test]
    fn address_with_too_many_segments_is_rejected() {
        assert!(
            ViolationAddress::parse("docs/cli.md:cli-docs:keep-sorted:3f9a1c4e:extra").is_err()
        );
    }

    #[test]
    fn address_with_an_empty_segment_is_rejected() {
        for address in [
            "",
            "::keep-sorted",
            "docs/cli.md::keep-sorted",
            "docs/cli.md:cli-docs:",
        ] {
            assert!(
                ViolationAddress::parse(address).is_err(),
                "{address} must be rejected"
            );
        }
    }

    #[test]
    fn address_with_an_unknown_validator_is_rejected() {
        let error = ViolationAddress::parse("docs/cli.md:cli-docs:keep-tidy")
            .expect_err("an unregistered validator must be rejected");
        assert!(
            error.to_string().contains("keep-tidy"),
            "the error must name the offending validator: {error:#}"
        );
    }

    #[test]
    fn address_with_malformed_hash_is_rejected() {
        for address in [
            "docs/cli.md:cli-docs:keep-sorted:3F9A1C4E",
            "docs/cli.md:cli-docs:keep-sorted:zzzzzzzz",
            "docs/cli.md:cli-docs:keep-sorted:3f9a1c4",
            "docs/cli.md:cli-docs:keep-sorted:3f9a1c4e0",
        ] {
            assert!(
                ViolationAddress::parse(address).is_err(),
                "{address} must be rejected"
            );
        }
    }

    #[test]
    fn address_whose_path_escapes_the_repository_is_rejected() {
        assert!(ViolationAddress::parse("../outside.md:name:keep-sorted").is_err());
    }
}
