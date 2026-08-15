//! Builds the report that `--verbosity` prints: the files a run scanned, the blocks it found, and
//! the validators that checked them.

use crate::Position;
use crate::blocks::ScanStats;
use crate::repo_path::RepoPath;
use crate::validators::{ValidationContext, ValidationLog};
use serde::Serialize;
use std::collections::BTreeMap;

/// The totals for a whole run.
#[derive(Serialize, Debug)]
struct ReportSummary {
    files_scanned: usize,
    files_with_blocks: usize,
    files_skipped: usize,
    blocks: usize,
    blocks_unchecked: usize,
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
pub struct RunReport {
    summary: ReportSummary,
    /// `BTreeMap` is used for its deterministic sorting order.
    files: BTreeMap<RepoPath, Vec<serde_json::Value>>,
}

impl RunReport {
    /// Builds a report from the scan counts, the blocks in scope, and the checks that ran.
    pub fn new(
        stats: ScanStats,
        context: &ValidationContext,
        log: &ValidationLog,
    ) -> anyhow::Result<Self> {
        let mut checks = 0;
        let mut validators: BTreeMap<&'static str, usize> = BTreeMap::new();
        for blocks_in_file in log.checked_blocks.values() {
            for validators_of_block in blocks_in_file.values() {
                checks += validators_of_block.len();
                for validator in validators_of_block {
                    *validators.entry(validator).or_default() += 1;
                }
            }
        }

        let mut files = BTreeMap::new();
        let mut blocks = 0;
        let mut blocks_unchecked = 0;
        for (file_path, file_blocks) in &context.blocks {
            let checked_in_file = log.checked_blocks.get(file_path);
            let mut listings = file_blocks.to_serializable_report();
            for listing in &mut listings {
                blocks += 1;
                // The listings are sorted by line, so a block is found by its start position
                // rather than by its index.
                let position = Position::new(
                    listing["line"].as_u64().unwrap_or_default() as usize,
                    listing["column"].as_u64().unwrap_or_default() as usize,
                );
                match checked_in_file.and_then(|blocks| blocks.get(&position)) {
                    Some(block_validators) => {
                        listing["checks"] = serde_json::to_value(block_validators)?;
                    }
                    None => {
                        blocks_unchecked += 1;
                        listing["checks"] = serde_json::json!([]);
                    }
                }
            }
            files.insert(file_path.clone(), listings);
        }

        Ok(Self {
            summary: ReportSummary {
                files_scanned: stats.files_scanned,
                files_with_blocks: context.blocks.len(),
                files_skipped: stats.files_skipped,
                blocks,
                blocks_unchecked,
                checks,
                violations: log.violations.values().map(Vec::len).sum(),
                validators,
            },
            files,
        })
    }

    /// Returns the run totals as a single line of text.
    pub fn summary_line(&self) -> String {
        format!(
            "blockwatch: {}/{} files, {} blocks ({} unchecked), {} checks, {} violations",
            self.summary.files_with_blocks,
            self.summary.files_scanned,
            self.summary.blocks,
            self.summary.blocks_unchecked,
            self.summary.checks,
            self.summary.violations,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Position;
    use crate::blocks::BlockSeverity;
    use crate::repo_path::RepoPath;
    use crate::test_utils::validation_context;
    use crate::validators::{ValidationLog, ValidationReport, Violation, ViolationRange};

    const CONTENTS: &str = r#"# <block name="both" keep-sorted="asc" line-count="<=2">
'apple',
# </block>
# <block name="sorted-only" keep-sorted="asc">
'apple',
# </block>
# <block name="unchecked" keep-sorted="asc">
'apple',
# </block>"#;

    fn violation() -> Violation {
        Violation::new(
            ViolationRange::new(Position::new(2, 1), Position::new(2, 8)),
            "keep-sorted".to_string(),
            "out of order".to_string(),
            BlockSeverity::Error,
            None,
        )
    }

    /// Builds a run log in which `validator` checked the block at `block_index` of `example.py`.
    fn log_with_check(
        context: &ValidationContext,
        block_index: usize,
        validator: &'static str,
        violations: Vec<Violation>,
    ) -> anyhow::Result<ValidationLog> {
        let file_path = RepoPath::from_reference("example.py")?;
        let mut report = ValidationReport::default();
        report.add_all(
            &file_path,
            &context.blocks[&file_path].blocks_with_context[block_index].block,
            violations,
        );
        let mut log = ValidationLog::default();
        log.add_validation_report(validator, report);
        Ok(log)
    }

    #[test]
    fn summary_line_reports_every_count() -> anyhow::Result<()> {
        let context = validation_context("example.py", CONTENTS);
        let log = log_with_check(&context, 0, "keep-sorted", vec![violation()])?;

        let report = RunReport::new(
            ScanStats {
                files_scanned: 4,
                files_skipped: 2,
            },
            &context,
            &log,
        )?;

        assert_eq!(
            report.summary_line(),
            "blockwatch: 1/4 files, 3 blocks (2 unchecked), 1 checks, 1 violations"
        );
        Ok(())
    }

    #[test]
    fn json_describes_every_block_and_the_checks_that_ran() -> anyhow::Result<()> {
        let context = validation_context("example.py", CONTENTS);
        let file_path = RepoPath::from_reference("example.py")?;
        let blocks = &context.blocks[&file_path].blocks_with_context;
        let mut log = ValidationLog::default();

        // `keep-sorted` checks the first two blocks and finds one violation; `line-count` checks
        // only the first. The third block's attribute is misspelled, so no validator matches it.
        let mut sorted = ValidationReport::default();
        sorted.add_all(&file_path, &blocks[0].block, vec![violation()]);
        sorted.add_all(&file_path, &blocks[1].block, Vec::new());
        log.add_validation_report("keep-sorted", sorted);

        let mut counted = ValidationReport::default();
        counted.add_all(&file_path, &blocks[0].block, Vec::new());
        log.add_validation_report("line-count", counted);

        let report = RunReport::new(
            ScanStats {
                files_scanned: 4,
                files_skipped: 2,
            },
            &context,
            &log,
        )?;

        assert_eq!(
            serde_json::to_value(&report)?,
            serde_json::json!({
                "summary": {
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
}
