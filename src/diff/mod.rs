//! The changes a diff makes to the files of the repository, line by line, and whether they touch a
//! range of a file.

mod unified;

pub(crate) use unified::{line_changes_from_diff, validate_diff_input};

use crate::position::Position;
use crate::repo_path::RepoPath;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::ops::Range;

/// Represents a line change from a diff.
#[derive(Debug, Eq, PartialEq, Clone)]
pub(crate) struct LineChange {
    /// 1-based line number with a change, in the diff's target file.
    pub(crate) line: usize,
    /// What the diff did to that line.
    pub(crate) kind: LineChangeKind,
}

/// The changes a diff makes to one file, in ascending order of line.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct LineChanges(Vec<LineChange>);

impl FromIterator<LineChange> for LineChanges {
    /// Sorts the changes by line. Changes on the same line keep their order.
    fn from_iter<I: IntoIterator<Item = LineChange>>(changes: I) -> Self {
        let mut changes: Vec<LineChange> = changes.into_iter().collect();
        changes.sort_by_key(|line_change| line_change.line);
        Self(changes)
    }
}

impl LineChanges {
    /// Whether there are no changes.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether any change touches a character in `range`.
    ///
    /// Text deleted right before `range.start` does not count, since it was never part of the range.
    pub(crate) fn touches(&self, range: &Range<Position>) -> bool {
        self.on_lines(range.start.line, range.end.line)
            .any(|line_change| range_intersects(range, line_change))
    }

    /// The changes that fall on lines `first_line..=last_line`, in order.
    pub(crate) fn on_lines(
        &self,
        first_line: usize,
        last_line: usize,
    ) -> impl Iterator<Item = &LineChange> {
        let first = self
            .0
            .partition_point(|line_change| line_change.line < first_line);
        self.0[first..]
            .iter()
            .take_while(move |line_change| line_change.line <= last_line)
    }
}

/// The changes a diff makes to the files of the repository.
#[derive(Debug, Default)]
pub(crate) struct DiffChanges(HashMap<RepoPath, LineChanges>);

impl FromIterator<(RepoPath, LineChanges)> for DiffChanges {
    fn from_iter<I: IntoIterator<Item = (RepoPath, LineChanges)>>(files: I) -> Self {
        Self(files.into_iter().collect())
    }
}

impl DiffChanges {
    /// The changes to `file`. A file the diff does not list has none.
    pub(crate) fn for_file(&self, file: &RepoPath) -> &LineChanges {
        static NO_CHANGES: LineChanges = LineChanges(Vec::new());
        self.0.get(file).unwrap_or(&NO_CHANGES)
    }

    /// Whether the diff lists `file`. A listed file can have no line changes, such as a renamed
    /// file whose lines are the same.
    pub(crate) fn mentions(&self, file: &RepoPath) -> bool {
        self.0.contains_key(file)
    }

    /// The files the diff lists, in no particular order.
    pub(crate) fn files(&self) -> impl Iterator<Item = &RepoPath> {
        self.0.keys()
    }
}

/// Checks whether the 0-based character ranges of a modified line intersect a 1-based half-open
/// column span `[start_col, end_col)`.
///
/// Column numbers count from 1: `start_col` is 1-based inclusive, and `end_col` is 1-based
/// exclusive. Pass `usize::MAX` for `end_col` when the span covers the rest of the line.
///
/// The `ranges` must be sorted in ascending order and non-overlapping, as this function
/// binary-searches them.
pub(crate) fn intersects_columns(
    ranges: &[Range<usize>],
    start_col: usize,
    end_col: usize,
) -> bool {
    let start_idx = start_col.saturating_sub(1);
    let end_idx = if end_col == usize::MAX {
        usize::MAX
    } else {
        end_col.saturating_sub(1)
    };
    ranges
        .binary_search_by(|range| {
            if range.end > start_idx && range.start < end_idx {
                Ordering::Equal
            } else if range.end <= start_idx {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        })
        .is_ok()
}

/// Whether `line_change`, which has to fall on one of the lines `range` spans, touches a character
/// in `range`.
fn range_intersects(range: &Range<Position>, line_change: &LineChange) -> bool {
    match &line_change.kind {
        LineChangeKind::Deleted => {
            let point = deletion_point(line_change.line);
            // The start is excluded even though the range includes it: text removed from just
            // before the range's start was never part of the range, and removing it leaves the
            // gap exactly at that start.
            // The end is included for the mirror reason: a gap sitting exactly at the range's
            // exclusive end is where the range's last character used to be. A whole-line
            // deletion never lands there, since its gap is always at column 1, so this bound
            // only bites if deletion points ever get finer-grained than a line.
            range.start < point && point <= range.end
        }
        // An insertion moves the line breaks around the line, not just the characters on it,
        // so there is nothing to compare column by column. Every position on the line counts
        // as changed.
        LineChangeKind::Added => true,
        LineChangeKind::Modified(ranges) => {
            let start_col = if line_change.line == range.start.line {
                range.start.character
            } else {
                1
            };
            let end_col = if line_change.line < range.end.line {
                usize::MAX
            } else {
                range.end.character
            };

            intersects_columns(ranges, start_col, end_col)
        }
    }
}

/// The place in the target file where a deletion anchored at `line` removed its text.
///
/// A deletion leaves no characters behind, only a gap, so it names a point rather than a line. The
/// removed text ended with a line break, which puts the gap at the start of the line that now
/// occupies it.
pub(crate) fn deletion_point(line: usize) -> Position {
    Position::new(line, 1)
}

/// What a diff did to one line of its target file.
#[derive(Debug, Eq, PartialEq, Clone)]
pub(crate) enum LineChangeKind {
    /// The line replaced one the diff removed. Holds the 0-based character ranges whose text
    /// differs from the removed line; everything outside them is unchanged, the line break that
    /// ends the line included.
    ///
    /// Ranges must be sorted in ascending order and non-overlapping.
    Modified(Vec<Range<usize>>),
    /// The newly added line (including the line break it adds).
    Added,
    /// The deleted line.
    Deleted,
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::test_utils::{added, deleted};
    use super::*;

    #[test]
    fn unsorted_changes_collected_are_sorted_by_line() {
        let changes: LineChanges = [added(3), deleted(1), added(2)].into_iter().collect();

        assert_eq!(changes.0, vec![deleted(1), added(2), added(3)]);
    }

    #[test]
    fn modified_ranges_with_overlapping_column_span_returns_true() {
        // 0-based range 4..10 covers characters at 1-based columns 5..11
        let ranges = vec![4..10];
        // Span 1-based 8..15 overlaps with 5..11
        assert!(intersects_columns(&ranges, 8, 15));
    }

    #[test]
    fn modified_ranges_with_non_overlapping_column_span_returns_false() {
        // 0-based range 4..10 covers characters at 1-based columns 5..11
        let ranges = vec![4..10];
        // Span 1-based 1..5 ends before column 5
        assert!(!intersects_columns(&ranges, 1, 5));
        // Span 1-based 11..20 starts at column 11
        assert!(!intersects_columns(&ranges, 11, 20));
    }

    #[test]
    fn modified_ranges_with_unbounded_end_column_returns_true() {
        let ranges = vec![10..20];
        assert!(intersects_columns(&ranges, 5, usize::MAX));
    }

    #[test]
    fn modified_ranges_touching_first_column_intersects_one_based_span() {
        // 0-based range 0..1 covers the first character (1-based column 1).
        let ranges = vec![0..1];
        // Span covers 1-based column 1 ([1, 2)).
        assert!(intersects_columns(&ranges, 1, 2));
    }

    #[test]
    fn modified_ranges_at_column_boundary_intersects_span_starting_at_same_column() {
        // 0-based range 4..5 covers the character at 1-based column 5.
        let ranges = vec![4..5];
        // Span [5, 6) starts at 1-based column 5.
        assert!(intersects_columns(&ranges, 5, 6));
    }
}

#[cfg(test)]
pub(crate) mod test_utils {
    use super::{DiffChanges, LineChange, LineChangeKind};
    use crate::repo_path::RepoPath;
    use std::ops::Range;

    /// The diff's changes to each of `files`.
    pub(crate) fn diff_changes(
        files: impl IntoIterator<Item = (RepoPath, Vec<LineChange>)>,
    ) -> DiffChanges {
        files
            .into_iter()
            .map(|(file, line_changes)| (file, line_changes.into_iter().collect()))
            .collect()
    }

    /// A newly added line.
    pub(crate) fn added(line: usize) -> LineChange {
        LineChange {
            line,
            kind: LineChangeKind::Added,
        }
    }

    /// A modified line with `length` chars modified.
    #[allow(clippy::single_range_in_vec_init)]
    pub(crate) fn modified(line: usize, length: usize) -> LineChange {
        LineChange {
            line,
            kind: LineChangeKind::Modified(vec![0..length]),
        }
    }

    /// A modified line with character range `range` (0-based) modified.
    pub(crate) fn modified_range(line: usize, range: Range<usize>) -> LineChange {
        LineChange {
            line,
            kind: LineChangeKind::Modified(vec![range]),
        }
    }

    /// A deletion anchored at `line`, the line that now occupies the gap the removed lines left.
    pub(crate) fn deleted(line: usize) -> LineChange {
        LineChange {
            line,
            kind: LineChangeKind::Deleted,
        }
    }
}
