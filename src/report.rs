//! Builds the report that `--verbosity` prints: the files a run scanned, the blocks it found, and
//! the validators that checked them.

use crate::blocks::BlockListing;
use crate::repo_path::RepoPath;
use crate::scan::ScanStats;
use crate::validators::{ValidationContext, ValidationLog};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Which of the three run modes produced a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunMode {
    /// No diff: every block in the scope is parsed and validated, and no block counts as changed.
    All,
    /// A diff marks which blocks changed, but every file in the scope is still parsed.
    AllWithDiff,
    /// A diff both marks the changed blocks and narrows the run down to them.
    OnlyChanged,
}

impl RunMode {
    /// The name the mode goes by in the report, on both the summary line and in the JSON.
    fn as_str(&self) -> &'static str {
        match self {
            Self::All => "all",
            Self::AllWithDiff => "all+diff",
            Self::OnlyChanged => "only-changed",
        }
    }
}

impl Serialize for RunMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// The totals for a whole run.
#[derive(Serialize, Debug)]
struct ReportSummary {
    mode: RunMode,
    files_scanned: usize,
    files_with_blocks: usize,
    files_skipped: usize,
    blocks: usize,
    blocks_unchecked: usize,
    /// Blocks carrying a rule that cannot fire unless a diff is supplied; see
    /// [`crate::validators::Validator::needs_diff`].
    #[serde(skip_serializing_if = "Option::is_none")]
    blocks_needing_diff: Option<usize>,
    checks: usize,
    violations: usize,
    /// Number of checks each validator ran.
    /// `BTreeMap` is used for its deterministic sorting order.
    validators: BTreeMap<&'static str, usize>,
}

/// A full account of what a run checked.
///
/// Each block is described the same way the `list` subcommand describes it, plus the checks that
/// ran on it.
#[derive(Serialize, Debug)]
pub(crate) struct RunReport<'a> {
    summary: ReportSummary,
    /// `BTreeMap` is used for its deterministic sorting order.
    files: BTreeMap<&'a RepoPath, Vec<BlockListing<'a>>>,
}

impl<'a> RunReport<'a> {
    /// Builds a report from the run mode, the scan counts, the blocks in scope, and the checks that
    /// ran.
    pub(crate) fn new(
        mode: RunMode,
        blocks_needing_diff: Option<usize>,
        stats: ScanStats,
        context: &'a ValidationContext,
        log: &ValidationLog,
    ) -> Self {
        let mut checks = 0;
        let mut validators: BTreeMap<&'static str, usize> = BTreeMap::new();
        for blocks_in_file in log.checked_blocks.values() {
            for validators_of_block in blocks_in_file.values() {
                checks += validators_of_block.len();
                for validator in validators_of_block {
                    *validators.entry(validator.as_str()).or_default() += 1;
                }
            }
        }

        let mut files = BTreeMap::new();
        let mut blocks = 0;
        let mut blocks_unchecked = 0;
        for (file_path, file_blocks) in context.blocks() {
            let checked_in_file = log.checked_blocks.get(file_path);
            let mut listings = Vec::with_capacity(file_blocks.blocks_with_context.len());
            for block_with_context in &file_blocks.blocks_with_context {
                blocks += 1;
                let mut listing = BlockListing::from(block_with_context);
                let key = block_with_context.block.key();
                listing.checks = Some(match checked_in_file.and_then(|blocks| blocks.get(&key)) {
                    Some(block_validators) => block_validators.clone(),
                    None => {
                        blocks_unchecked += 1;
                        BTreeSet::new()
                    }
                });
                listings.push(listing);
            }
            files.insert(file_path, listings);
        }

        Self {
            summary: ReportSummary {
                mode,
                files_scanned: stats.files_scanned,
                files_with_blocks: context.blocks().len(),
                files_skipped: stats.files_skipped,
                blocks,
                blocks_unchecked,
                blocks_needing_diff,
                checks,
                violations: log.violations.values().map(Vec::len).sum(),
                validators,
            },
            files,
        }
    }

    /// Returns the run totals as a single line of text.
    pub(crate) fn summary_line(&self) -> String {
        let needs_diff = match self.summary.blocks_needing_diff {
            Some(count) => format!(", {}", counted(count, "needs --diff", "need --diff")),
            None => String::new(),
        };
        format!(
            "blockwatch: mode={}, {}/{} files, {} ({} unchecked{}), {}, {}",
            self.summary.mode.as_str(),
            self.summary.files_with_blocks,
            self.summary.files_scanned,
            counted(self.summary.blocks, "block", "blocks"),
            self.summary.blocks_unchecked,
            needs_diff,
            counted(self.summary.checks, "check", "checks"),
            counted(self.summary.violations, "violation", "violations"),
        )
    }
}

/// Returns `count` followed by `one` if `count` is 1, or by `many` otherwise. For example, `1 block`,
/// `0 blocks` and `2 blocks`.
fn counted(count: usize, one: &str, many: &str) -> String {
    let word = if count == 1 { one } else { many };
    format!("{count} {word}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Position;
    use crate::diff_parser::DiffChanges;
    use crate::fs::PathFilter;
    use crate::fs::test_utils::FakeFileSystem;
    use crate::language_parsers::{LanguageParsers, language_parsers};
    use crate::repo_path::RepoPath;
    use crate::scan::{Scan, ScanMode};
    use crate::test_utils::{validation_context, virtual_block};
    use crate::validators::{
        ValidationLog, ValidationReport, Validator, Violation, ViolationRange,
    };
    use std::collections::HashMap;

    const CONTENTS: &str = r#"# <block name="both" keep-sorted="asc" line-count="<=2">
'apple',
# </block>
# <block name="sorted-only" keep-sorted="asc">
'apple',
# </block>
# <block name="unchecked" keep-sorted="asc">
'apple',
# </block>"#;

    fn violation(context: &ValidationContext) -> Violation {
        let file_path = RepoPath::from_reference("example.py").expect("a valid repository path");
        let block = &context.blocks()[&file_path].blocks_with_context[0].block;
        Violation::new(
            ViolationRange::new(Position::new(2, 1), Position::new(2, 8)),
            &file_path,
            block,
            Validator::KeepSorted,
            "out of order".to_string(),
            Some("'apple',"),
            None,
        )
    }

    /// Builds a run log in which `validator` checked the block at `block_index` of `example.py`.
    fn log_with_check(
        context: &ValidationContext,
        block_index: usize,
        validator: Validator,
        violations: Vec<Violation>,
    ) -> anyhow::Result<ValidationLog> {
        let file_path = RepoPath::from_reference("example.py")?;
        let mut report = ValidationReport::default();
        report.add_all(
            &file_path,
            &context.blocks()[&file_path].blocks_with_context[block_index].block,
            violations,
        );
        let mut log = ValidationLog::default();
        log.add_validation_report(validator, report);
        Ok(log)
    }

    #[test]
    fn summary_line_reports_every_count() -> anyhow::Result<()> {
        let context = validation_context("example.py", CONTENTS);
        let log = log_with_check(
            &context,
            0,
            Validator::KeepSorted,
            vec![violation(&context)],
        )?;

        let report = RunReport::new(
            RunMode::All,
            Some(2),
            ScanStats {
                files_scanned: 4,
                files_skipped: 2,
            },
            &context,
            &log,
        );

        assert_eq!(
            report.summary_line(),
            "blockwatch: mode=all, 1/4 files, 3 blocks (2 unchecked, 2 need --diff), 1 check, 1 violation"
        );
        Ok(())
    }

    #[test]
    fn summary_line_under_a_diff_leaves_out_the_needs_diff_clause() -> anyhow::Result<()> {
        let context = validation_context("example.py", CONTENTS);
        let log = log_with_check(
            &context,
            0,
            Validator::KeepSorted,
            vec![violation(&context)],
        )?;

        let report = RunReport::new(
            RunMode::AllWithDiff,
            None,
            ScanStats {
                files_scanned: 4,
                files_skipped: 2,
            },
            &context,
            &log,
        );

        assert_eq!(
            report.summary_line(),
            "blockwatch: mode=all+diff, 1/4 files, 3 blocks (2 unchecked), 1 check, 1 violation"
        );
        Ok(())
    }

    #[test]
    fn json_under_a_diff_omits_the_needs_diff_key() -> anyhow::Result<()> {
        let context = validation_context("example.py", CONTENTS);
        let log = log_with_check(
            &context,
            0,
            Validator::KeepSorted,
            vec![violation(&context)],
        )?;

        let report = RunReport::new(
            RunMode::OnlyChanged,
            None,
            ScanStats {
                files_scanned: 4,
                files_skipped: 2,
            },
            &context,
            &log,
        );

        let summary = &serde_json::to_value(&report)?["summary"];
        assert_eq!(summary.get("blocks_needing_diff"), None);
        // The rest of the summary keeps its shape, so only the one field varies with the mode.
        assert_eq!(summary["blocks_unchecked"], 2);
        Ok(())
    }

    #[test]
    fn json_describes_every_block_and_the_checks_that_ran() -> anyhow::Result<()> {
        let context = validation_context("example.py", CONTENTS);
        let file_path = RepoPath::from_reference("example.py")?;
        let blocks = &context.blocks()[&file_path].blocks_with_context;
        let mut log = ValidationLog::default();

        // `keep-sorted` checks the first two blocks and finds one violation; `line-count` checks
        // only the first. The third block's attribute is misspelled, so no validator matches it.
        let mut sorted = ValidationReport::default();
        sorted.add_all(&file_path, &blocks[0].block, vec![violation(&context)]);
        sorted.add_all(&file_path, &blocks[1].block, Vec::new());
        log.add_validation_report(Validator::KeepSorted, sorted);

        let mut counted = ValidationReport::default();
        counted.add_all(&file_path, &blocks[0].block, Vec::new());
        log.add_validation_report(Validator::LineCount, counted);

        let report = RunReport::new(
            RunMode::OnlyChanged,
            None,
            ScanStats {
                files_scanned: 4,
                files_skipped: 2,
            },
            &context,
            &log,
        );

        assert_eq!(
            serde_json::to_value(&report)?,
            serde_json::json!({
                "summary": {
                    "mode": "only-changed",
                    "files_scanned": 4,
                    "files_with_blocks": 1,
                    "files_skipped": 2,
                    "blocks": 3,
                    "blocks_unchecked": 1,
                    "checks": 3,
                    "violations": 1,
                    "validators": { "keep-sorted": 2, "line-count": 1 }
                },
                "files": {
                    "example.py": [
                        {
                            "name": "both",
                            "line": 1,
                            "column": 3,
                            "is_content_modified": true,
                            "attributes": {
                                "name": "both",
                                "keep-sorted": "asc",
                                "line-count": "<=2"
                            },
                            "checks": ["keep-sorted", "line-count"]
                        },
                        {
                            "name": "sorted-only",
                            "line": 4,
                            "column": 3,
                            "is_content_modified": true,
                            "attributes": {
                                "name": "sorted-only",
                                "keep-sorted": "asc"
                            },
                            "checks": ["keep-sorted"]
                        },
                        {
                            "name": "unchecked",
                            "line": 7,
                            "column": 3,
                            "is_content_modified": true,
                            "attributes": {
                                "name": "unchecked",
                                "keep-sorted": "asc"
                            },
                            "checks": []
                        }
                    ]
                }
            })
        );
        Ok(())
    }

    #[test]
    fn virtual_blocks_that_start_at_the_same_place_keep_their_own_checks() -> anyhow::Result<()> {
        // In `a.b = 1`, the symbols `/a` and `/a/b` both start at `b`.
        let file_path = RepoPath::from_reference("a.toml")?;
        let mut inner = virtual_block("a.toml#/a/b", &[])?;
        inner.entry.line = 9;
        let parsed = Scan::new(
            &FakeFileSystem::new(HashMap::from([(
                "a.toml".to_string(),
                "a.b = 1\n".to_string(),
            )])),
            &PathFilter::allow_all(),
            &language_parsers()?,
            &[virtual_block("a.toml#/a", &[])?, inner],
        )
        .parse_blocks(&DiffChanges::default(), ScanMode::All)?;
        let context = ValidationContext::new(
            parsed.blocks,
            LanguageParsers::default(),
            DiffChanges::default(),
            Vec::new(),
        );
        let mut log = ValidationLog::default();
        let blocks = &context.blocks()[&file_path].blocks_with_context;
        for (block_with_context, validator) in
            blocks.iter().zip([Validator::LineCount, Validator::SameAs])
        {
            let mut report = ValidationReport::default();
            report.add_all(&file_path, &block_with_context.block, Vec::new());
            log.add_validation_report(validator, report);
        }

        let report = RunReport::new(RunMode::All, None, ScanStats::default(), &context, &log);

        let checks: Vec<serde_json::Value> = serde_json::to_value(&report)?["files"]["a.toml"]
            .as_array()
            .expect("the file is listed")
            .iter()
            .map(|listing| listing["checks"].clone())
            .collect();
        assert_eq!(
            checks,
            vec![
                serde_json::json!(["line-count"]),
                serde_json::json!(["same-as"])
            ]
        );
        Ok(())
    }
}
