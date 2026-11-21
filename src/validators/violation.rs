//! One broken rule found in a block, and the form it is serialized in.

use super::{Validator, ViolationAddress};
use crate::attributes::Severity;
use crate::blocks::Block;
use crate::position::Position;
use crate::repo_path::RepoPath;
use serde::Serialize;
use std::ops::Range;

/// One rule breach found in one block: what is wrong, where, and how severe it is.
///
/// Fields are private because the public shape of a violation is [`SimpleDiagnostic`], the form
/// that gets serialized for editors and CI.
#[derive(Debug)]
pub(crate) struct Violation {
    /// Where to underline in the file — normally the block's start tag, or the offending line.
    pub(super) range: ViolationRange,
    /// The validator that reported it, e.g. [`Validator::KeepSorted`].
    pub(super) code: Validator,
    /// Human-readable explanation, printed to the developer.
    pub(super) message: String,
    /// Decides whether this breach fails the run; see [`Severity`].
    severity: Severity,
    /// A unique address of this violation.
    /// `None` if the block has no name as its address may not be unique.
    address: Option<ViolationAddress>,
    /// Whether `--suppress` matched this violation.
    suppressed: bool,
    /// Validator-specific details for tools, e.g. the expected and actual line count.
    pub(super) data: Option<serde_json::Value>,
}

impl Violation {
    /// Constructs a new violation record with a validator, error message, and optional
    /// machine-readable details.
    ///
    /// Severity and address are derived here from the block and the `code` rather than being passed
    /// in, so a validator cannot report a violation whose address disagrees with the rule that
    /// found it.
    ///
    /// `violation_text` is the text the violation is about — the offending line, or the target a
    /// reference names — and tells one violation of a block from its siblings. Validators that
    /// report at most one violation per block pass `None`.
    pub(crate) fn new(
        range: ViolationRange,
        file: &RepoPath,
        block: &Block,
        code: Validator,
        message: String,
        violation_text: Option<&str>,
        data: Option<serde_json::Value>,
    ) -> Self {
        Self {
            address: ViolationAddress::new(file, block.name(), code, violation_text),
            severity: block.attributes.severity(),
            range,
            code,
            message,
            suppressed: false,
            data,
        }
    }

    /// A unique address of this violation or `None` if its block is unnamed.
    pub(crate) fn address(&self) -> Option<&ViolationAddress> {
        self.address.as_ref()
    }

    /// Marks this violation as suppressed.
    pub(crate) fn suppress(&mut self) {
        self.suppressed = true;
    }

    pub(crate) fn as_simple_diagnostic(&self) -> SimpleDiagnostic<'_> {
        SimpleDiagnostic {
            range: &self.range,
            code: self.code,
            message: self.message.as_str(),
            severity: self.severity,
            address: self.address.as_ref(),
            suppressed: self.suppressed,
            data: self.data.as_ref(),
        }
    }
}

/// The span an editor should highlight for a violation. 1-based and half-open: the start is
/// inclusive, the end is exclusive.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub(crate) struct ViolationRange {
    pub(super) start: Position,
    end: Position,
}

impl ViolationRange {
    /// Creates a range from its two endpoints, which must be in the same file.
    pub(crate) fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }

    /// The first position of the range.
    pub(crate) fn start(&self) -> &Position {
        &self.start
    }

    /// The position just past the last one of the range: the end is exclusive.
    pub(crate) fn end(&self) -> &Position {
        &self.end
    }
}

impl From<Range<Position>> for ViolationRange {
    fn from(range: Range<Position>) -> Self {
        Self::new(range.start, range.end)
    }
}

/// Represents a simplified, serializable diagnostic message.
///
/// It mimics the [Diagnostic](https://github.com/microsoft/vscode-languageserver-node/blob/3412a17149850f445bf35b4ad71148cfe5f8411e/types/src/main.ts#L688)
/// object but omits some redundant fields and keeps all line numbers 1-based instead of zero-based.
#[derive(Serialize, Debug)]
pub(crate) struct SimpleDiagnostic<'a> {
    range: &'a ViolationRange,
    code: Validator,
    message: &'a str,
    severity: Severity,
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<&'a ViolationAddress>,
    #[serde(skip_serializing_if = "is_false")]
    suppressed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<&'a serde_json::Value>,
}

/// Keeps a `false` `suppressed` field out of the serialized diagnostic.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's `skip_serializing_if` passes the field by reference"
)]
fn is_false(value: &bool) -> bool {
    !*value
}

impl<'a> SimpleDiagnostic<'a> {
    /// Whether `--suppress` matched this violation, in which case it must not fail the run.
    pub(crate) fn is_suppressed(&self) -> bool {
        self.suppressed
    }

    /// The severity, which the caller uses to decide the process exit code.
    pub(crate) fn severity(&self) -> Severity {
        self.severity
    }

    /// Where to underline in the file.
    pub(crate) fn range(&self) -> &'a ViolationRange {
        self.range
    }

    /// The validator that reported it, e.g. [`Validator::KeepSorted`].
    pub(crate) fn code(&self) -> Validator {
        self.code
    }

    /// The human-readable explanation.
    pub(crate) fn message(&self) -> &'a str {
        self.message
    }

    /// The address `--suppress` may point at, or `None` when the block has no `name`.
    pub(crate) fn address(&self) -> Option<&'a ViolationAddress> {
        self.address
    }

    /// Validator-specific details for tools.
    pub(crate) fn data(&self) -> Option<&'a serde_json::Value> {
        self.data
    }
}
