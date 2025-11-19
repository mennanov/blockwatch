use similar::DiffOp;
use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::path::PathBuf;
use std::str::FromStr;
use unidiff::{Line, PatchSet, PatchedFile};

/// Represents a line change from a diff.
#[derive(Debug, Eq, PartialEq)]
pub struct LineChange {
    /// 1-based line number with a change.
    pub line: usize,
    /// Modified ranges in this line. Can only be `Some` for modified lines, not added or deleted.
    pub ranges: Option<Vec<Range<usize>>>, // TODO: consider making it 1-based to be consistent with `line`.
}

/// Extracts line changes from a unified diff patch string.
///
/// Parses a patch/diff string and extracts all line changes grouped by file path.
/// Deleted files are ignored and not included in the result.
pub fn extract(patch_diff: &str) -> anyhow::Result<HashMap<PathBuf, Vec<LineChange>>> {
    let patch_set = PatchSet::from_str(patch_diff)?;
    let mut result = HashMap::new();
    for patched_file in patch_set {
        if patched_file.is_removed_file() {
            // Deleted files are ignored.
            continue;
        }
        result.insert(
            patched_file.target_file.trim_start_matches("b/").into(),
            line_changes(&patched_file),
        );
    }
    Ok(result)
}

fn line_changes(patched_file: &PatchedFile) -> Vec<LineChange> {
    let mut line_changes = Vec::new();
    let mut deleted_lines: VecDeque<&Line> = VecDeque::new();
    let mut prev_line = None;
    for hunk in patched_file.hunks() {
        for line in hunk.lines() {
            if line.is_added() {
                if let Some(deleted_line) = deleted_lines.pop_front() {
                    // This is a modified line. Find modified ranges in it.
                    let ranges = line_diff(&deleted_line.value, &line.value);
                    line_changes.push(LineChange {
                        line: line.target_line_no.unwrap(),
                        ranges: Some(ranges),
                    });
                } else {
                    // This is a new (added) line.
                    line_changes.push(LineChange {
                        line: line.target_line_no.unwrap(),
                        ranges: None,
                    });
                }
            } else if line.is_removed() {
                deleted_lines.push_back(line);
            } else if line.is_context() {
                clear_or_fold_deleted_lines(&prev_line, &mut deleted_lines, &mut line_changes);
            }
            prev_line = Some(line);
        }
        clear_or_fold_deleted_lines(&prev_line, &mut deleted_lines, &mut line_changes);
    }
    line_changes
}

/// Returns sorted character ranges in `new` that represent changes from `old`.
fn line_diff(old: &str, new: &str) -> Vec<Range<usize>> {
    let mut result = Vec::new();
    let diff = similar::TextDiff::from_chars(old, new);
    let mut prev_op = None;
    for op in diff.ops() {
        match op {
            DiffOp::Delete { new_index, .. } => {
                if prev_op.is_none_or(|c: &DiffOp| !matches!(c, DiffOp::Delete { .. })) {
                    let idx = new.len().saturating_sub(1).min(*new_index);
                    push_or_merge_range(&mut result, idx..idx + 1);
                }
            }
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                push_or_merge_range(&mut result, *new_index..(new_index + new_len));
            }
            DiffOp::Replace {
                new_index, new_len, ..
            } => {
                push_or_merge_range(&mut result, *new_index..(new_index + new_len));
            }
            DiffOp::Equal { .. } => {}
        }
        prev_op = Some(op);
    }
    result
}

fn push_or_merge_range(ranges: &mut Vec<Range<usize>>, mut new: Range<usize>) {
    if let Some(overlapping) =
        // Contiguous ranges are also merged (e.g. [6, 8) and [8, 10) -> [6, 10)).
        ranges.pop_if(|range| new.start <= range.end && new.end >= range.start)
    {
        let start = new.start.min(overlapping.start);
        let end = new.end.max(overlapping.end);
        new = start..end;
    }
    ranges.push(new);
    // Sink the new/merged range to its sorted position.
    let mut i = ranges.len() - 1;
    while i > 0 && ranges[i].start < ranges[i - 1].start {
        ranges.swap(i, i - 1);
        i -= 1;
    }
}

/// Pushes the first deleted line to the `line_changes` and deletes all the rest.
fn fold_deleted_lines(deleted_lines: &mut VecDeque<&Line>, line_changes: &mut Vec<LineChange>) {
    if let Some(deleted_line) = deleted_lines.pop_front() {
        line_changes.push(LineChange {
            line: deleted_line.source_line_no.unwrap(),
            ranges: None,
        })
    }
    deleted_lines.clear()
}

/// Clears `deleted_lines` if `prev_line` is a new line, folds them otherwise.
fn clear_or_fold_deleted_lines(
    prev_line: &Option<&Line>,
    deleted_lines: &mut VecDeque<&Line>,
    line_changes: &mut Vec<LineChange>,
) {
    if prev_line.is_some_and(|prev: &Line| prev.is_added()) {
        // Consecutive deleted lines followed by a new line is a single modified line and
        // should already be handled by the new line handler.
        deleted_lines.clear();
    } else {
        fold_deleted_lines(deleted_lines, line_changes);
    }
}

#[cfg(test)]
mod modified_line_ranges_tests {
    use super::*;

    #[test]
    fn equal_lines_returns_empty_ranges() {
        let ranges = line_diff("box", "box");

        assert!(ranges.is_empty());
    }

    #[test]
    fn replaced_nonconsecutive_characters_returns_separate_ranges() {
        let ranges = line_diff("box", "for");

        assert_eq!(ranges, vec![0..1, 2..3]);
    }

    #[test]
    fn replaced_consecutive_characters_returns_merged_ranges() {
        let ranges = line_diff("boxes", "faxed");

        assert_eq!(ranges, vec![0..2, 4..5]);
    }

    #[test]
    fn inserted_nonconsecutive_characters_returns_separate_ranges() {
        let ranges = line_diff("box", "aboxa");

        assert_eq!(ranges, vec![0..1, 4..5]);
    }

    #[test]
    fn inserted_consecutive_characters_returns_merged_ranges() {
        let ranges = line_diff("box", "2 boxes");

        assert_eq!(ranges, vec![0..2, 5..7]);
    }

    #[test]
    fn deleted_consecutive_characters_in_the_beginning_are_treated_as_single() {
        let ranges = line_diff("abracadabra", "cadabra");

        assert_eq!(ranges, vec![0..1]);
    }

    #[test]
    fn deleted_consecutive_characters_in_the_end_are_treated_as_single() {
        let ranges = line_diff("abracadabra", "abra");

        assert_eq!(ranges, vec![3..4]);
    }

    #[test]
    fn deleted_consecutive_characters_are_treated_as_single() {
        let ranges = line_diff("abracadabra", "cdar");

        assert_eq!(ranges, vec![0..2, 3..4]);
    }

    #[test]
    fn mixed_ops_returns_correct_ranges() {
        let ranges = line_diff("there was three", "there is thora");

        assert_eq!(ranges, vec![6..7, 11..12, 13..14]);
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Creates a whole line change (either added or deleted line).
    fn line_change(line: usize) -> LineChange {
        LineChange { line, ranges: None }
    }

    #[test]
    fn single_file_diff_extracts_ranges_for_single_file() -> anyhow::Result<()> {
        let ranges = extract(
            r#"diff --git a/Cargo.toml b/Cargo.toml
index 8c34c48..23ddd69 100644
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -9,6 +9,7 @@ tree-sitter = "0.25.3"
 tree-sitter-rust = "0.23"
 tree-sitter-java = "0.23.5"
 quick-xml = "0.37.2"
+diffy = "0.4.2"
 
 [build-dependencies]
 cc="1.2.16"
\ No newline at end of file"#,
        )?;
        assert_eq!(ranges.keys().collect::<Vec<_>>(), vec!["Cargo.toml"]);
        Ok(())
    }

    #[test]
    fn multiple_files_diff_extracts_ranges_for_multiple_files() -> anyhow::Result<()> {
        let ranges = extract(
            r#"diff --git a/Cargo.toml b/Cargo.toml
index 8c34c48..23ddd69 100644
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -9,6 +9,7 @@ tree-sitter = "0.25.3"
 tree-sitter-rust = "0.23"
 tree-sitter-java = "0.23.5"
 quick-xml = "0.37.2"
+diffy = "0.4.2"
 
 [build-dependencies]
 cc="1.2.16"
\ No newline at end of file
diff --git a/src/main.rs b/src/main.rs
index 63c5842..34d1d3f 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,4 +1,5 @@
 mod parsers;
+mod differ;
 
 fn main() {
     println!("Hello, world!");
diff --git a/src/differ.rs b/src/differ.rs
index e69de29..215ed53 100644
--- a/src/differ.rs
+++ b/src/differ.rs
@@ -0,0 +1,1 @@
+use std::collections::HashMap;
"#,
        )?;
        assert_eq!(
            ranges
                .keys()
                .map(|k| k.to_str().unwrap())
                .collect::<HashSet<_>>(),
            HashSet::from(["Cargo.toml", "src/main.rs", "src/differ.rs"])
        );
        Ok(())
    }

    #[test]
    fn single_new_line_diff_returns_single_line_change() -> anyhow::Result<()> {
        let ranges = extract(
            r#"diff --git a/a.txt b/a.txt
index f384549..b4b0c67 100644
--- a/a.txt
+++ b/a.txt
@@ -1,4 +1,5 @@
 one
 two
 three
+three and a half
 four"#,
        )?;
        assert_eq!(ranges[&PathBuf::from("a.txt")], vec![line_change(4)]);
        Ok(())
    }

    #[test]
    fn single_first_new_line_diff_returns_single_line_change() -> anyhow::Result<()> {
        let ranges = extract(
            r#"diff --git a/a.txt b/a.txt
index f384549..fa220f8 100644
--- a/a.txt
+++ b/a.txt
@@ -1,3 +1,4 @@
+zero
 one
 two
 three"#,
        )?;
        assert_eq!(ranges[&PathBuf::from("a.txt")], vec![line_change(1)]);
        Ok(())
    }

    #[test]
    fn multiple_contiguous_new_lines_diff_returns_multiple_line_changes() -> anyhow::Result<()> {
        let ranges = extract(
            r#"diff --git a/a.txt b/a.txt
index f384549..3a7bc2a 100644
--- a/a.txt
+++ b/a.txt
@@ -1,4 +1,6 @@
 one
 two
 three
+three and a half
+almost four
 four"#,
        )?;
        assert_eq!(
            ranges[&PathBuf::from("a.txt")],
            vec![line_change(4), line_change(5),]
        );
        Ok(())
    }

    #[test]
    fn multiple_first_contiguous_new_lines_diff_returns_multiple_line_changes() -> anyhow::Result<()>
    {
        let ranges = extract(
            r#"diff --git a/a.txt b/a.txt
index f384549..3ccae75 100644
--- a/a.txt
+++ b/a.txt
@@ -1,3 +1,5 @@
+sub-zero
+zero
 one
 two
 three"#,
        )?;
        assert_eq!(
            ranges[&PathBuf::from("a.txt")],
            vec![line_change(1), line_change(2),]
        );
        Ok(())
    }

    #[test]
    fn multiple_non_contiguous_new_lines_diff_returns_multiple_line_changes() -> anyhow::Result<()>
    {
        let ranges = extract(
            r#"diff --git a/a.txt b/a.txt
index f384549..e797e7c 100644
--- a/a.txt
+++ b/a.txt
@@ -1,4 +1,6 @@
 one
 two
+two and a half
 three
+three and a half
 four"#,
        )?;
        assert_eq!(
            ranges[&PathBuf::from("a.txt")],
            vec![line_change(3), line_change(5)]
        );
        Ok(())
    }

    #[test]
    fn multiple_contiguous_new_line_groups_diff_returns_multiple_line_changes() -> anyhow::Result<()>
    {
        let ranges = extract(
            r#"diff --git a/a.txt b/a.txt
index f384549..ab47fb2 100644
--- a/a.txt
+++ b/a.txt
@@ -1,4 +1,8 @@
 one
 two
+two and a half
+almost three
 three
+three and a half
+almost four
 four"#,
        )?;
        assert_eq!(
            ranges[&PathBuf::from("a.txt")],
            vec![
                line_change(3),
                line_change(4),
                line_change(6),
                line_change(7),
            ]
        );
        Ok(())
    }

    #[test]
    fn modified_line_returns_single_line_change_with_ranges() -> anyhow::Result<()> {
        let ranges = extract(
            r#"diff --git a/a.txt b/a.txt
index f384549..e4c2829 100644
--- a/a.txt
+++ b/a.txt
@@ -1,4 +1,4 @@
 one
 two
-there was three
+there is thora
 four"#,
        )?;
        assert_eq!(
            ranges[&PathBuf::from("a.txt")],
            vec![LineChange {
                line: 3,
                // "i" in "is" was modified, "o" and "a" in "thora" were modified.
                ranges: Some(vec![6..7, 11..12, 13..14])
            }]
        );
        Ok(())
    }

    #[test]
    fn multiple_non_consecutive_modified_line_returns_separate_line_changes_with_ranges()
    -> anyhow::Result<()> {
        let ranges = extract(
            r#"diff --git a/a.txt b/a.txt
index f384549..46c7533 100644
--- a/a.txt
+++ b/a.txt
@@ -1,5 +1,5 @@
-one
+modified one
 two
-three white rabbits
+three rabbits
 four
-five brown foxes
+five own boxes
 "#,
        )?;
        assert_eq!(
            ranges[&PathBuf::from("a.txt")],
            vec![
                LineChange {
                    line: 1,
                    // "modified " was inserted.
                    ranges: Some(vec![0..9])
                },
                LineChange {
                    line: 3,
                    // "white " was deleted. Consecutive deletions are treated as single.
                    ranges: Some(vec![6..7])
                },
                LineChange {
                    line: 5,
                    // "br" from "brown" was deleted. "b" in "boxes" was modified.
                    ranges: Some(vec![5..6, 9..10])
                }
            ]
        );
        Ok(())
    }

    #[test]
    fn multiple_consecutive_modified_lines_returns_single_line_changes_with_ranges()
    -> anyhow::Result<()> {
        let ranges = extract(
            r#"diff --git a/a.txt b/a.txt
index f384549..676cbb7 100644
--- a/a.txt
+++ b/a.txt
@@ -1,4 +1,4 @@
 one
-two
-three
+modified two
+modified three
 four"#,
        )?;
        assert_eq!(
            ranges[&PathBuf::from("a.txt")],
            vec![
                LineChange {
                    line: 2,
                    ranges: Some(vec![0..9])
                },
                LineChange {
                    line: 3,
                    ranges: Some(vec![0..9])
                }
            ]
        );
        Ok(())
    }

    #[test]
    fn all_lines_replaced_returns_single_line_changes_with_ranges() -> anyhow::Result<()> {
        let ranges = extract(
            r#"diff --git a/a.txt b/a.txt
index f384549..676cbb7 100644
--- a/a.txt
+++ b/a.txt
@@ -1,4 +1,2 @@
-one
-two
-three
-four
+modified one
+modified two"#,
        )?;
        assert_eq!(
            ranges[&PathBuf::from("a.txt")],
            vec![
                LineChange {
                    line: 1,
                    ranges: Some(vec![0..9])
                },
                LineChange {
                    line: 2,
                    ranges: Some(vec![0..9])
                }
            ]
        );
        Ok(())
    }

    #[test]
    fn single_deleted_line_returns_single_line_change() -> anyhow::Result<()> {
        let ranges = extract(
            r#"diff --git a/a.txt b/a.txt
index f384549..87a123c 100644
--- a/a.txt
+++ b/a.txt
@@ -1,4 +1,3 @@
 one
 two
-three
 four"#,
        )?;
        assert_eq!(ranges[&PathBuf::from("a.txt")], vec![line_change(3)]);
        Ok(())
    }

    #[test]
    fn single_first_deleted_line_returns_single_line_change() -> anyhow::Result<()> {
        let ranges = extract(
            r#"diff --git a/a.txt b/a.txt
index f384549..58ac960 100644
--- a/a.txt
+++ b/a.txt
@@ -1,4 +1,3 @@
-one
 two
 three
 four"#,
        )?;
        assert_eq!(ranges[&PathBuf::from("a.txt")], vec![line_change(1)]);
        Ok(())
    }

    #[test]
    fn single_last_deleted_line_returns_single_line_change() -> anyhow::Result<()> {
        let ranges = extract(
            r#"diff --git a/a.txt b/a.txt
index f384549..4cb29ea 100644
--- a/a.txt
+++ b/a.txt
@@ -1,4 +1,3 @@
 one
 two
 three
-four"#,
        )?;
        assert_eq!(ranges[&PathBuf::from("a.txt")], vec![line_change(4)]);
        Ok(())
    }

    #[test]
    fn multiple_non_consecutive_deleted_lines_returns_separate_line_changes() -> anyhow::Result<()>
    {
        let ranges = extract(
            r#"diff --git a/a.txt b/a.txt
index f384549..8c05df4 100644
--- a/a.txt
+++ b/a.txt
@@ -1,4 +1,2 @@
-one
 two
-three
 four"#,
        )?;
        assert_eq!(
            ranges[&PathBuf::from("a.txt")],
            vec![line_change(1), line_change(3)]
        );
        Ok(())
    }

    #[test]
    fn multiple_consecutive_deleted_lines_returns_single_line_change() -> anyhow::Result<()> {
        let ranges = extract(
            r#"diff --git a/a.txt b/a.txt
index f384549..a9c7698 100644
--- a/a.txt
+++ b/a.txt
@@ -1,4 +1,2 @@
 one
-two
-three
 four"#,
        )?;
        // Consecutive deleted lines are treated as a single one-line range because they no longer
        // exist in the target file.
        assert_eq!(ranges[&PathBuf::from("a.txt")], vec![line_change(2)]);
        Ok(())
    }

    #[test]
    fn all_lines_deleted_treated_as_deleted_file() -> anyhow::Result<()> {
        let ranges = extract(
            r#"diff --git a/a.txt b/a.txt
index f384549..e69de29 100644
--- a/a.txt
+++ b/a.txt
@@ -1,4 +0,0 @@
-one
-two
-three
-four"#,
        )?;
        assert!(ranges.is_empty());
        Ok(())
    }

    #[test]
    fn mixed_changes_returns_correct_line_changes() -> anyhow::Result<()> {
        let ranges = extract(
            r#"diff --git a/a.txt b/a.txt
index f384549..58a279e 100644
--- a/a.txt
+++ b/a.txt
@@ -1,4 +1,5 @@
-one
+modified one
 two
-three
-four
+modified three
+modified four
+added five"#,
        )?;
        assert_eq!(
            ranges[&PathBuf::from("a.txt")],
            vec![
                LineChange {
                    line: 1,
                    ranges: Some(vec![0..9])
                },
                LineChange {
                    line: 3,
                    ranges: Some(vec![0..9])
                },
                LineChange {
                    line: 4,
                    ranges: Some(vec![0..9])
                },
                line_change(5)
            ]
        );
        Ok(())
    }

    #[test]
    fn new_file_diff_returns_line_changes_for_every_line() -> anyhow::Result<()> {
        let ranges = extract(
            r#"diff --git a/example.rs b/example.rs
new file mode 100644
index 0000000..710d1d9
--- /dev/null
+++ b/example.rs
@@ -0,0 +1,3 @@
+fn main() {
+    println!("New file");
+}
"#,
        )?;
        assert_eq!(
            ranges[&PathBuf::from("example.rs")],
            vec![line_change(1), line_change(2), line_change(3)]
        );
        Ok(())
    }

    #[test]
    fn deleted_file_diff_is_ignored() -> anyhow::Result<()> {
        let ranges = extract(
            r#"diff --git a/a.txt b/a.txt
deleted file mode 100644
index f384549..0000000
--- a/a.txt
+++ /dev/null
@@ -1,4 +0,0 @@
-one
-two
-three
-four"#,
        )?;
        assert!(ranges.is_empty());
        Ok(())
    }
}
