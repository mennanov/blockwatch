use crate::blocks::BlockSeverity;
use crate::repo_path::RepoPath;
use crate::validators::{SimpleDiagnostic, Violation};
use crate::violation_address::ViolationAddress;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The SARIF version of every log this module writes.
const SARIF_VERSION: &str = "2.1.0";
const SARIF_SCHEMA: &str = "https://json.schemastore.org/sarif-2.1.0.json";

/// The tool name reported in every log.
const TOOL_NAME: &str = "blockwatch";

/// The `partialFingerprints` key that holds a violation's address.
///
/// The key carries a version number because SARIF asks for one: it lets a consumer tell an address
/// written by an older release from one written after the way addresses are derived has changed.
const ADDRESS_FINGERPRINT_KEY: &str = "blockwatchAddress/v1";

/// Cargo leaves `CARGO_PKG_REPOSITORY` empty when the package declares no repository, which would
/// turn every documentation link into a path with no host in front of it. Failing the build is the
/// only way to notice, as an empty variable is still a variable.
const _: () = assert!(
    !env!("CARGO_PKG_REPOSITORY").is_empty(),
    "the package needs a repository, which the documentation links are built from"
);

/// What a column number counts, which a consumer otherwise has to guess: SARIF assumes UTF-16 code
/// units, while blockwatch counts characters.
const COLUMN_KIND: &str = "unicodeCodePoints";

/// The characters a path cannot carry literally once it becomes a URI.
///
/// A `#` would start a fragment and a `?` a query, so a file whose name contains either would be
/// reported as a different, shorter path. The separator `/` is deliberately absent: it keeps its
/// meaning in a URI.
const PATH_UNSAFE: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// A one-line description of every validator, by validator name.
///
/// Each registered validator needs an entry here. A test enforces that.
const VALIDATOR_DESCRIPTIONS: &[(&str, &str)] = &[
    (
        "affects",
        "Requires co-dependent blocks to be updated together.",
    ),
    ("check-ai", "Checks a block with AI."),
    ("check-lua", "Checks a block with a custom Lua script."),
    (
        "keep-sorted",
        "Requires the lines of a block to stay in order.",
    ),
    (
        "keep-unique",
        "Requires the lines of a block to stay free of duplicates.",
    ),
    ("line-count", "Constrains how many lines a block has."),
    (
        "line-pattern",
        "Requires every line of a block to match a pattern.",
    ),
    (
        "same-as",
        "Requires two places to keep holding the same value.",
    ),
];

/// A blockwatch run, in the shape a SARIF log is serialized in.
#[derive(Serialize, Debug)]
pub struct SarifLog<'a> {
    #[serde(rename = "$schema")]
    schema: &'static str,
    version: &'static str,
    runs: [Run<'a>; 1],
}

impl<'a> SarifLog<'a> {
    /// Builds the log that reports `violations`.
    ///
    /// The log always has exactly one run, and it describes only the validators that reported
    /// something. Its results come out in a fixed order, so two runs over an unchanged tree will
    /// produce identical outputs.
    pub fn new(violations: &'a HashMap<RepoPath, Vec<Violation>>) -> Self {
        let diagnostics = diagnostics_in_stable_order(violations);
        let rules = rules_that_fired(&diagnostics);
        let results = sarif_results(&diagnostics, &rules);
        Self {
            schema: SARIF_SCHEMA,
            version: SARIF_VERSION,
            runs: [Run {
                tool: Tool {
                    driver: ToolComponent::new(rules),
                },
                column_kind: COLUMN_KIND,
                results,
            }],
        }
    }
}

/// Returns every violation, paired with the file it was found in.
///
/// Files come out in path order, and the violations of one file in the order the validators
/// reported them.
fn diagnostics_in_stable_order(
    violations: &HashMap<RepoPath, Vec<Violation>>,
) -> Vec<(&RepoPath, SimpleDiagnostic<'_>)> {
    violations
        .iter()
        // The files arrive in the arbitrary order of a hash map, so sort them by path to make the
        // output reproducible.
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .flat_map(|(file, file_violations)| {
            file_violations
                .iter()
                .map(move |violation| (file, violation.as_simple_diagnostic()))
        })
        .collect()
}

/// Returns one description per validator that reported at least one of `diagnostics`, in validator
/// name order.
///
/// A validator that reported nothing is left out. A consumer reads the list as the rules this run
/// had something to say about, not as the rules that exist.
fn rules_that_fired(diagnostics: &[(&RepoPath, SimpleDiagnostic<'_>)]) -> Vec<ReportingDescriptor> {
    diagnostics
        .iter()
        .map(|(_, diagnostic)| diagnostic.code())
        // Several diagnostics can come from the same validator, and the set both removes those
        // repeats and puts the names in order.
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(ReportingDescriptor::new)
        .collect()
}

/// Returns one result per diagnostic, in the order the diagnostics are given.
///
/// Each result points at the rule that reported it by its index in `rules`.
///
/// # Panics
///
/// Panics if `rules` has no entry for a validator that reported one of the diagnostics.
fn sarif_results<'a>(
    diagnostics: &[(&RepoPath, SimpleDiagnostic<'a>)],
    rules: &[ReportingDescriptor],
) -> Vec<SarifResult<'a>> {
    diagnostics
        .iter()
        .map(|(file, diagnostic)| {
            let rule_index = rules
                .iter()
                .position(|rule| rule.id == diagnostic.code())
                .expect("every rule that reported a diagnostic is described");
            SarifResult::new(file, diagnostic, rule_index)
        })
        .collect()
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Run<'a> {
    tool: Tool,
    column_kind: &'static str,
    results: Vec<SarifResult<'a>>,
}

#[derive(Serialize, Debug)]
struct Tool {
    driver: ToolComponent,
}

/// The analysis tool itself, together with the rules it reports. A `tool` names one of these as
/// its `driver`.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ToolComponent {
    name: &'static str,
    version: &'static str,
    /// The same version again, promised to be a semantic version, which `version` alone is not.
    /// Only this one is safe for a consumer to compare against another version.
    semantic_version: &'static str,
    information_uri: &'static str,
    rules: Vec<ReportingDescriptor>,
}

impl ToolComponent {
    /// Describes this build of blockwatch as the tool that enforced `rules`.
    fn new(rules: Vec<ReportingDescriptor>) -> Self {
        Self {
            name: TOOL_NAME,
            version: env!("CARGO_PKG_VERSION"),
            semantic_version: env!("CARGO_PKG_VERSION"),
            information_uri: env!("CARGO_PKG_REPOSITORY"),
            rules,
        }
    }
}

/// One rule a result can be attributed to, which here is one validator.
///
/// SARIF calls the object a reporting descriptor and the property that holds a list of them
/// `rules`.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ReportingDescriptor {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    short_description: Option<Message>,
    help_uri: String,
}

impl ReportingDescriptor {
    /// Describes the validator called `validator`, whether or not it has a description on file.
    fn new(validator: &str) -> Self {
        Self {
            id: validator.to_string(),
            name: validator.to_string(),
            short_description: validator_description(validator).map(Message::new),
            help_uri: help_uri(validator),
        }
    }
}

/// One finding, at the place it was found, which here is one violation.
///
/// SARIF calls the object simply a result. The name carries the `Sarif` prefix only to keep it
/// apart from Rust's own `Result`.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct SarifResult<'a> {
    rule_id: &'a str,
    rule_index: usize,
    level: &'static str,
    message: Message,
    locations: [Location; 1],
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    partial_fingerprints: BTreeMap<&'static str, String>,
    /// Present only when the violation was suppressed. It is the verdict that tells a consumer to
    /// keep the finding out of the ones it counts as outstanding.
    #[serde(skip_serializing_if = "Option::is_none")]
    suppressions: Option<[Suppression; 1]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    properties: Option<Properties<'a>>,
}

impl<'a> SarifResult<'a> {
    /// Reports `diagnostic`, found in `file`, against the rule at `rule_index`.
    fn new(file: &RepoPath, diagnostic: &SimpleDiagnostic<'a>, rule_index: usize) -> Self {
        Self {
            rule_id: diagnostic.code(),
            rule_index,
            level: sarif_level(diagnostic.severity()),
            message: Message::new(diagnostic.message()),
            locations: [Location::new(file, diagnostic)],
            partial_fingerprints: diagnostic
                .address()
                .map(|address| BTreeMap::from([(ADDRESS_FINGERPRINT_KEY, address.to_string())]))
                .unwrap_or_default(),
            suppressions: diagnostic
                .is_suppressed()
                .then_some([Suppression { kind: "external" }]),
            properties: Properties::new(diagnostic),
        }
    }
}

/// Extra detail about a finding that is specific to this tool. A consumer may show it, and is free
/// to ignore it.
#[derive(Serialize, Debug)]
struct Properties<'a> {
    /// The address `--suppress` takes to silence the violation. It repeats the fingerprint in a
    /// form a person can read and copy, which a fingerprint is not meant for.
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<&'a ViolationAddress>,
    /// Whatever the validator recorded about the violation, such as an expected and an actual line
    /// count.
    #[serde(skip_serializing_if = "Option::is_none")]
    data: &'a Option<serde_json::Value>,
}

impl<'a> Properties<'a> {
    /// Collects the extra detail of `diagnostic`, or returns `None` when it has none.
    fn new(diagnostic: &SimpleDiagnostic<'a>) -> Option<Self> {
        let (address, data) = (diagnostic.address(), diagnostic.data());
        (address.is_some() || data.is_some()).then_some(Self { address, data })
    }
}

#[derive(Serialize, Debug)]
struct Suppression {
    kind: &'static str,
}

#[derive(Serialize, Debug)]
struct Message {
    text: String,
}

impl Message {
    /// Wraps `text` as a message a consumer displays as it is.
    fn new(text: &str) -> Self {
        Self {
            text: text.to_string(),
        }
    }
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Location {
    physical_location: PhysicalLocation,
}

impl Location {
    /// Points at the span `diagnostic` underlines in `file`.
    fn new(file: &RepoPath, diagnostic: &SimpleDiagnostic<'_>) -> Self {
        let range = diagnostic.range();
        Self {
            physical_location: PhysicalLocation {
                artifact_location: ArtifactLocation {
                    uri: utf8_percent_encode(file.as_str(), PATH_UNSAFE).to_string(),
                },
                region: Region {
                    start_line: range.start().line,
                    start_column: range.start().character,
                    end_line: range.end().line,
                    end_column: range.end().character,
                },
            },
        }
    }
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct PhysicalLocation {
    artifact_location: ArtifactLocation,
    region: Region,
}

#[derive(Serialize, Debug)]
struct ArtifactLocation {
    /// The path of the file, relative to the root of the repository, and encoded so that it reads
    /// as a URI rather than as a path that happens to look like one.
    uri: String,
}

/// The span of a finding within a file.
///
/// Lines and columns count from 1, and the end column points just past the last column of the
/// span.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Region {
    start_line: usize,
    start_column: usize,
    end_line: usize,
    end_column: usize,
}

/// Returns the SARIF level a finding of `severity` is reported at.
fn sarif_level(severity: BlockSeverity) -> &'static str {
    match severity {
        BlockSeverity::Error => "error",
        BlockSeverity::Warning => "warning",
        // The only SARIF level below `note` is `none`, and some consumers hide it altogether, so
        // the quieter severities stay notes instead of disappearing.
        BlockSeverity::Info | BlockSeverity::Hint => "note",
    }
}

/// Returns the one-line description of `validator`, or `None` if it has no entry.
fn validator_description(validator: &str) -> Option<&'static str> {
    VALIDATOR_DESCRIPTIONS
        .iter()
        .find(|(name, _)| *name == validator)
        .map(|(_, description)| *description)
}

/// Returns the link to the documentation page of `validator`, pinned to the running version.
fn help_uri(validator: &str) -> String {
    // Linking to the release tag rather than to the default branch keeps the page describing the
    // rule as this binary enforces it, however far the branch moves on.
    format!(
        "{}/blob/v{}/docs/validators/{validator}.md",
        env!("CARGO_PKG_REPOSITORY"),
        env!("CARGO_PKG_VERSION"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Position;
    use crate::blocks::Block;
    use crate::validators::ViolationRange;
    use assert_json_diff::assert_json_include;
    use serde_json::{Value, json};
    use std::collections::HashMap;
    use std::path::Path;

    fn repo_path(path: &str) -> RepoPath {
        RepoPath::from_reference(path).expect("a repository-relative path")
    }

    /// A block carrying the given `name` and `severity` attributes, and nothing else.
    fn block(name: Option<&str>, severity: &str) -> Block {
        let mut attributes = HashMap::from([("severity".to_string(), severity.to_string())]);
        if let Some(name) = name {
            attributes.insert("name".to_string(), name.to_string());
        }
        Block::new(
            attributes,
            Position::new(1, 1)..=Position::new(1, 1),
            0..0,
            Position::new(1, 1)..Position::new(1, 1),
        )
    }

    /// One violation of `code` on a block named `name`, underlining line 3.
    fn violation(
        file: &RepoPath,
        name: Option<&str>,
        severity: &str,
        code: &str,
        violation_text: Option<&str>,
    ) -> Violation {
        Violation::new(
            ViolationRange::new(Position::new(3, 1), Position::new(3, 4)),
            file,
            &block(name, severity),
            code.to_string(),
            format!("{code} is unhappy"),
            violation_text,
            None,
        )
        .expect("the block declares a known severity")
    }

    /// The violations of a run, keyed the way the validators leave them.
    fn violations(entries: Vec<(RepoPath, Vec<Violation>)>) -> HashMap<RepoPath, Vec<Violation>> {
        entries.into_iter().collect()
    }

    /// The log as a consumer receives it: serialized, then read back.
    fn log_value(violations: &HashMap<RepoPath, Vec<Violation>>) -> Value {
        serde_json::to_value(SarifLog::new(violations)).expect("the log serializes")
    }

    /// The page a rule is expected to link to, spelled out rather than taken from `help_uri`.
    fn documentation_link(validator: &str) -> String {
        format!(
            "{}/blob/v{}/docs/validators/{validator}.md",
            env!("CARGO_PKG_REPOSITORY"),
            env!("CARGO_PKG_VERSION")
        )
    }

    #[test]
    fn violation_is_reported_as_a_result_at_the_place_it_was_found() {
        let file = repo_path("src/lib.rs");
        let violations = violations(vec![(
            file.clone(),
            vec![violation(
                &file,
                Some("languages"),
                "error",
                "keep-sorted",
                Some("rust"),
            )],
        )]);

        let result = log_value(&violations)["runs"][0]["results"][0].clone();

        // Comparing the whole result also pins down what is absent: an unsuppressed violation
        // carries no verdict.
        let address = "src/lib.rs:languages:keep-sorted:6f66c727";
        assert_eq!(
            result,
            json!({
                "ruleId": "keep-sorted",
                "ruleIndex": 0,
                "level": "error",
                "message": {"text": "keep-sorted is unhappy"},
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": {"uri": "src/lib.rs"},
                        "region": {"startLine": 3, "startColumn": 1, "endLine": 3, "endColumn": 4},
                    },
                }],
                "partialFingerprints": {ADDRESS_FINGERPRINT_KEY: address},
                "properties": {"address": address},
            })
        );
    }

    #[test]
    fn no_violations_produce_a_log_with_no_results() {
        let log = log_value(&HashMap::new());

        // A clean run still produces a log, so a consumer can tell it apart from a run that never
        // happened. The two lists are compared exactly, because an inclusive match against an
        // empty list passes whatever the list holds.
        assert_eq!(log["runs"][0]["results"], json!([]), "{log}");
        assert_eq!(
            log["runs"][0]["tool"]["driver"]["rules"],
            json!([]),
            "{log}"
        );
    }

    #[test]
    fn log_contains_correct_metadata_fields() {
        let log = log_value(&HashMap::new());

        assert_json_include!(
            actual: log,
            expected: json!({
                "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
                "version": "2.1.0",
                "runs": [{
                    "tool": {
                        "driver": {
                            "name": "blockwatch",
                            "version": env!("CARGO_PKG_VERSION"),
                            "semanticVersion": env!("CARGO_PKG_VERSION"),
                            "informationUri": env!("CARGO_PKG_REPOSITORY"),
                        },
                    },
                    // Without this a consumer assumes the columns count UTF-16 code units and
                    // misplaces every finding on a line holding a character outside the BMP.
                    "columnKind": "unicodeCodePoints",
                }],
            })
        );
    }

    #[test]
    fn severity_below_a_warning_is_reported_as_a_note() {
        for severity in [
            BlockSeverity::Error,
            BlockSeverity::Warning,
            BlockSeverity::Info,
            BlockSeverity::Hint,
        ] {
            // Exhaustive match is used to ensure that the test fails when new severity is added.
            match severity {
                BlockSeverity::Error => {
                    assert_eq!(sarif_level(severity), "error");
                }
                BlockSeverity::Warning => {
                    assert_eq!(sarif_level(severity), "warning");
                }
                BlockSeverity::Info => {
                    assert_eq!(sarif_level(severity), "note");
                }
                BlockSeverity::Hint => {
                    assert_eq!(sarif_level(severity), "note");
                }
            }
        }
    }

    #[test]
    fn only_the_validators_that_reported_something_are_described_as_rules() {
        let file = repo_path("src/lib.rs");
        let violations = violations(vec![(
            file.clone(),
            vec![
                violation(&file, Some("languages"), "warning", "line-count", None),
                violation(&file, Some("languages"), "error", "keep-sorted", Some("a")),
            ],
        )]);

        let log = log_value(&violations);
        let rules = log["runs"][0]["tool"]["driver"]["rules"].clone();

        assert_eq!(
            rules,
            json!([
                {
                    "id": "keep-sorted",
                    "name": "keep-sorted",
                    "shortDescription": {"text": "Requires the lines of a block to stay in order."},
                    "helpUri": documentation_link("keep-sorted"),
                },
                {
                    "id": "line-count",
                    "name": "line-count",
                    "shortDescription": {"text": "Constrains how many lines a block has."},
                    "helpUri": documentation_link("line-count"),
                },
            ])
        );

        // Every result points at its own rule, whatever order the validators reported in.
        for result in log["runs"][0]["results"].as_array().expect("results") {
            let index = result["ruleIndex"].as_u64().expect("a rule index") as usize;
            assert_eq!(rules[index]["id"], result["ruleId"]);
        }
    }

    #[test]
    fn suppressed_violation_is_reported_with_an_external_suppression() {
        let file = repo_path("src/lib.rs");
        let mut suppressed = violation(&file, Some("languages"), "error", "line-count", None);
        suppressed.suppress();
        let violations = violations(vec![(file, vec![suppressed])]);

        let result = log_value(&violations)["runs"][0]["results"][0].clone();

        // The finding is reported, not hidden. It keeps the level its author declared and carries
        // the verdict that takes it out of the findings a consumer counts as outstanding.
        assert_json_include!(
            actual: result,
            expected: json!({"level": "error", "suppressions": [{"kind": "external"}]})
        );
    }

    #[test]
    fn violation_on_an_unnamed_block_carries_no_fingerprint() {
        let file = repo_path("src/lib.rs");
        let violations = violations(vec![(
            file.clone(),
            vec![violation(&file, None, "error", "keep-sorted", Some("rust"))],
        )]);

        let result = log_value(&violations)["runs"][0]["results"][0].clone();

        // An unnamed block has no address, so there is nothing to fingerprint it by, and no
        // property bag to put one in.
        assert_eq!(
            json!({
                "partialFingerprints": result.get("partialFingerprints"),
                "properties": result.get("properties"),
            }),
            json!({"partialFingerprints": null, "properties": null}),
            "{result}"
        );
    }

    #[test]
    fn special_characters_in_path_are_encoded() {
        let file = repo_path("src/is#1 note.py");
        let violations = violations(vec![(
            file.clone(),
            vec![violation(
                &file,
                Some("a"),
                "error",
                "keep-sorted",
                Some("x"),
            )],
        )]);

        let result = log_value(&violations)["runs"][0]["results"][0].clone();

        assert_json_include!(
            actual: result,
            expected: json!({
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": {"uri": "src/is%231%20note.py"},
                    },
                }],
            })
        );
    }

    #[test]
    fn files_found_in_any_order_are_reported_in_path_order() {
        let (first, second) = (repo_path("a.py"), repo_path("b.py"));
        let ordered = violations(vec![
            (
                first.clone(),
                vec![violation(
                    &first,
                    Some("a"),
                    "error",
                    "keep-sorted",
                    Some("x"),
                )],
            ),
            (
                second.clone(),
                vec![violation(
                    &second,
                    Some("b"),
                    "error",
                    "keep-sorted",
                    Some("y"),
                )],
            ),
        ]);

        let uris: Vec<String> = log_value(&ordered)["runs"][0]["results"]
            .as_array()
            .expect("results")
            .iter()
            .map(|result| {
                result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"].to_string()
            })
            .collect();

        assert_eq!(uris, vec!["\"a.py\"".to_string(), "\"b.py\"".to_string()]);
    }

    #[test]
    fn every_registered_validator_is_described() {
        let described: Vec<&str> = VALIDATOR_DESCRIPTIONS
            .iter()
            .map(|(name, _)| *name)
            .collect();
        let mut registered = crate::validators::validator_names();
        registered.sort_unstable();
        let mut described_sorted = described.clone();
        described_sorted.sort_unstable();

        assert_eq!(
            described_sorted, registered,
            "every validator needs a description, and no description may outlive its validator"
        );
    }

    #[test]
    fn every_described_validator_has_the_documentation_page_its_help_uri_points_at() {
        for (validator, _) in VALIDATOR_DESCRIPTIONS {
            let page = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("docs/validators")
                .join(format!("{validator}.md"));
            assert!(
                page.exists(),
                "{validator} has no documentation page at {}, so its helpUri would be a dead link",
                page.display()
            );
        }
    }
}
