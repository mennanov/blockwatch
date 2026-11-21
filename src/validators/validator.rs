//! The validators a block can turn on, and the attributes that turn each one on.

use crate::attributes::{AttributeName, Attributes};
use anyhow::Context;
use serde::Serialize;
use strum::IntoEnumIterator;
use strum_macros::{Display, EnumIter, EnumString, IntoStaticStr};

/// The registered validators that check blocks.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Display,
    EnumString,
    IntoStaticStr,
    EnumIter,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub(crate) enum Validator {
    /* <block name="validator-registry"
    same-as="README.md:available-validators, docs/validators/README.md:validators-index"
    same-as-format="identifier"
    same-as-pattern='^\s*(?P<value>[A-Za-z]+),'> */
    Affects,
    CheckAi,
    CheckLua,
    KeepSorted,
    KeepUnique,
    LineCount,
    LinePattern,
    SameAs,
    // </block>
}

impl Validator {
    /// A one-line description of the validator.
    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Affects => "Requires co-dependent blocks to be updated together.",
            Self::CheckAi => "Checks a block with AI.",
            Self::CheckLua => "Checks a block with a custom Lua script.",
            Self::KeepSorted => "Requires the lines of a block to stay in order.",
            Self::KeepUnique => "Requires the lines of a block to stay free of duplicates.",
            Self::LineCount => "Constrains how many lines a block has.",
            Self::LinePattern => {
                "Checks every line of a block against a required or a forbidden pattern."
            }
            Self::SameAs => "Requires two places to keep holding the same value.",
        }
    }

    /// Whether this validator only ever fires when a diff is supplied.
    pub(crate) fn needs_diff(self) -> bool {
        match self {
            Self::Affects => true,
            Self::CheckAi
            | Self::CheckLua
            | Self::KeepSorted
            | Self::KeepUnique
            | Self::LineCount
            | Self::LinePattern
            | Self::SameAs => false,
        }
    }

    /// The attributes that turn this validator on, such as `line-pattern` and
    /// `line-pattern-forbidden` for `line-pattern`. The validator's own name comes first.
    pub(crate) fn attributes(self) -> &'static [AttributeName] {
        match self {
            Self::Affects => &[AttributeName::Affects],
            Self::CheckAi => &[AttributeName::CheckAi],
            Self::CheckLua => &[AttributeName::CheckLua],
            Self::KeepSorted => &[AttributeName::KeepSorted],
            Self::KeepUnique => &[AttributeName::KeepUnique],
            Self::LineCount => &[AttributeName::LineCount],
            Self::LinePattern => &[
                AttributeName::LinePattern,
                AttributeName::LinePatternForbidden,
            ],
            Self::SameAs => &[AttributeName::SameAs, AttributeName::SameAsPaths],
        }
    }

    /// Whether `attributes` has one of the [`attributes`](Self::attributes) that turn this
    /// validator on.
    pub(crate) fn is_turned_on_by(self, attributes: &Attributes) -> bool {
        self.attributes().iter().any(|&name| attributes.has(name))
    }

    /// The name of this validator as a static string.
    pub(crate) fn as_str(self) -> &'static str {
        self.into()
    }

    /// Looks up the validator called `value`. Returns an error if there is no validator with that
    /// name.
    pub(crate) fn parse(value: &str) -> anyhow::Result<Self> {
        value.parse::<Self>().with_context(|| {
            let validators: Vec<&str> = Self::iter().map(Self::as_str).collect();
            format!(
                "unknown validator: {value}. Available validators: {}",
                validators.join(", ")
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Validator;
    use crate::attributes::{AttributeName, Attributes};
    use strum::IntoEnumIterator;

    #[test]
    fn validator_is_turned_on_by_each_of_its_attributes_and_no_other() {
        // `explain` and the diff-gated count must find a validator from any attribute that makes
        // the run check it, not only from the validator's own name. Any other attribute, such as
        // `keep-sorted-pattern`, turns no validator on.
        let turned_on_by = [
            (AttributeName::Affects, Validator::Affects),
            (AttributeName::CheckAi, Validator::CheckAi),
            (AttributeName::CheckLua, Validator::CheckLua),
            (AttributeName::KeepSorted, Validator::KeepSorted),
            (AttributeName::KeepUnique, Validator::KeepUnique),
            (AttributeName::LineCount, Validator::LineCount),
            (AttributeName::LinePattern, Validator::LinePattern),
            (AttributeName::LinePatternForbidden, Validator::LinePattern),
            (AttributeName::SameAs, Validator::SameAs),
            (AttributeName::SameAsPaths, Validator::SameAs),
        ];
        for (attribute, entry) in crate::attributes::test_utils::sample_entries() {
            let expected: Vec<Validator> = turned_on_by
                .iter()
                .filter(|(turning_on, _)| *turning_on == attribute)
                .map(|(_, validator)| *validator)
                .collect();
            let listed: Vec<Validator> = Validator::iter()
                .filter(|validator| validator.attributes().contains(&attribute))
                .collect();
            assert_eq!(listed, expected, "{attribute}");
            if let Ok(attributes) =
                Attributes::from_entries([(attribute.as_str().to_string(), entry)])
            {
                let turned_on: Vec<Validator> = Validator::iter()
                    .filter(|validator| validator.is_turned_on_by(&attributes))
                    .collect();
                assert_eq!(turned_on, expected, "{attribute}");
            }
        }
    }

    #[test]
    fn validator_attribute_alone_returns_ok_and_modifier_alone_returns_error() {
        // A new attribute must either work alone or stop the run alone. Otherwise a block with
        // only that attribute would pass without being checked.
        let needs_no_other: Vec<AttributeName> = Validator::iter()
            .flat_map(Validator::attributes)
            .copied()
            .chain([AttributeName::Name, AttributeName::Severity])
            .collect();
        for (name, entry) in crate::attributes::test_utils::sample_entries() {
            let result = Attributes::from_entries([(name.as_str().to_string(), entry)]);

            assert_eq!(result.is_ok(), needs_no_other.contains(&name), "{name}");
        }
    }

    #[test]
    fn modifier_with_one_other_attribute_returns_ok_only_when_that_one_turns_on_its_validator() {
        let entries = crate::attributes::test_utils::sample_entries();
        let modifiers = entries.iter().filter(|(name, _)| {
            !matches!(name, AttributeName::Name | AttributeName::Severity)
                && Validator::iter().all(|validator| !validator.attributes().contains(name))
        });
        for (modifier, modifier_entry) in modifiers {
            // A modifier's name starts with its validator's name, such as `keep-sorted-format`.
            let validator = Validator::iter()
                .find(|validator| {
                    modifier
                        .as_str()
                        .starts_with(&format!("{}-", validator.as_str()))
                })
                .unwrap_or_else(|| panic!("no validator for `{modifier}`"));
            for (other, other_entry) in entries.iter().filter(|(other, _)| other != modifier) {
                // A block with a `name` can be a `same-as` target, which reads its own pattern.
                let expected = validator.attributes().contains(other)
                    || (*modifier == AttributeName::SameAsPattern && *other == AttributeName::Name);

                let result = Attributes::from_entries([
                    (modifier.as_str().to_string(), modifier_entry.clone()),
                    (other.as_str().to_string(), other_entry.clone()),
                ]);

                assert_eq!(result.is_ok(), expected, "`{modifier}` with `{other}`");
            }
        }
    }

    #[test]
    fn validator_lists_its_own_name_first() {
        // An error such as "`keep-sorted-format` without `keep-sorted`" shows the validator's own
        // name first.
        for validator in Validator::iter() {
            assert_eq!(
                validator.attributes()[0].as_str(),
                validator.as_str(),
                "{validator}"
            );
        }
    }
}
