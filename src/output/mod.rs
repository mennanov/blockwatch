//! What a run writes: the violations as JSON or SARIF, the `list` and `explain` output, and the
//! `--verbosity` report.

mod explain;
mod listing;
mod run_report;
mod sarif;

pub(crate) use explain::Explanation;
pub(crate) use listing::{BlockListing, listings};
pub(crate) use run_report::{RunMode, RunReport};

use crate::repo_path::RepoPath;
use crate::validators::Violation;
use std::collections::BTreeMap;
use std::io::Write;

/// Writes `document` to `output` as pretty-printed JSON, followed by a newline, and flushes it.
///
/// Like [`write_line`], returns `Ok` when the reader has closed the pipe.
pub(crate) fn write_json(
    output: impl Write,
    document: &impl serde::Serialize,
) -> anyhow::Result<()> {
    write_line(output, &serde_json::to_string_pretty(document)?)
}

/// Writes `text` to `output`, followed by a newline, and flushes it.
///
/// Returns `Ok` without writing the rest when the reader has closed the pipe, as `head` does after
/// reading enough. Returns an error for any other failed write.
pub(crate) fn write_line(mut output: impl Write, text: &str) -> anyhow::Result<()> {
    match writeln!(output, "{text}").and_then(|()| output.flush()) {
        // Rust ignores SIGPIPE, so a write to a closed pipe returns this error instead of ending
        // the process. A reader that closed the pipe wants no more output, so it is not a failure.
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        result => Ok(result?),
    }
}

/// Writes the violations to stderr as one JSON object of diagnostics grouped by file, with the
/// files sorted by path.
pub(crate) fn write_json_violations(
    violations: &BTreeMap<RepoPath, Vec<Violation>>,
) -> anyhow::Result<()> {
    let diagnostics: BTreeMap<&RepoPath, Vec<_>> = violations
        .iter()
        .map(|(file_path, file_violations)| {
            let file_diagnostics = file_violations
                .iter()
                .map(Violation::as_simple_diagnostic)
                .collect();
            (file_path, file_diagnostics)
        })
        .collect();
    write_json(std::io::stderr(), &diagnostics)
}

/// Writes the violations to stderr as a SARIF log.
pub(crate) fn write_sarif_violations(
    violations: &BTreeMap<RepoPath, Vec<Violation>>,
) -> anyhow::Result<()> {
    write_json(std::io::stderr(), &sarif::SarifLog::new(violations))
}
