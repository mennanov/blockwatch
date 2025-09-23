use std::collections::HashMap;
use std::str::FromStr;
use unidiff::{Line, PatchSet, PatchedFile};

/// Extracts hunks from a diff in a patch format (e.g. output of `git diff --patch`).
pub trait HunksExtractor {
    /// Returns a mapping from the changed filenames to the sorted modified line ranges.
    ///
    /// Consecutive deleted lines are represented as a single line range.
    /// Consecutive added lines are treated as a single range.
    /// Intersecting ranges can be merged.
    fn extract(&self, patch_diff: &str) -> anyhow::Result<HashMap<String, Vec<(usize, usize)>>>;
}

pub struct UnidiffExtractor;

impl Default for UnidiffExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl UnidiffExtractor {
    pub fn new() -> Self {
        Self {}
    }

    fn modified_ranges(patched_file: &PatchedFile) -> Vec<(usize, usize)> {
        let mut ranges = Vec::new();
        let mut start = None;
        let mut end = None;
        let mut prev_line = None;
        for hunk in patched_file.hunks() {
            for line in hunk.lines() {
                if line.is_added() {
                    if prev_line.is_some_and(|prev: &Line| !prev.is_added()) {
                        Self::try_add_range(&mut ranges, &mut start, &mut end);
                    }
                    if let Some(line_number) = line.target_line_no {
                        if start.is_none() {
                            start = Some(line_number);
                        }
                        end = Some(line_number);
                    }
                } else if line.is_removed() {
                    if let Some(prev_line) = prev_line {
                        if !prev_line.is_removed() {
                            Self::try_add_range(&mut ranges, &mut start, &mut end);
                            if let Some(line_number) = line.source_line_no {
                                start = Some(line_number);
                                end = Some(line_number);
                            }
                        }
                    } else if let Some(line_number) = line.source_line_no {
                        start = Some(line_number);
                        end = Some(line_number);
                    }
                } else if line.is_context() {
                    Self::try_add_range(&mut ranges, &mut start, &mut end);
                }
                prev_line = Some(line);
            }
            Self::try_add_range(&mut ranges, &mut start, &mut end);
        }
        ranges
    }

    fn try_add_range(
        ranges: &mut Vec<(usize, usize)>,
        start: &mut Option<usize>,
        end: &mut Option<usize>,
    ) {
        if let Some(start) = start.take() {
            let end = end.take().unwrap_or(start);
            if let Some((last_start, last_end)) = ranges.last_mut() {
                // Merge intersecting ranges.
                if *last_start <= end && start <= *last_end {
                    *last_end = end;
                    return;
                }
            }
            ranges.push((start, end));
        }
    }
}

impl HunksExtractor for UnidiffExtractor {
    fn extract(&self, patch_diff: &str) -> anyhow::Result<HashMap<String, Vec<(usize, usize)>>> {
        let patch_set = PatchSet::from_str(patch_diff)?;
        let mut result = HashMap::new();
        for patched_file in patch_set {
            if patched_file.is_removed_file() {
                // Deleted files are ignored.
                continue;
            }
            result.insert(
                patched_file
                    .target_file
                    .trim_start_matches("b/")
                    .to_string(),
                Self::modified_ranges(&patched_file),
            );
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn create_extractor() -> impl HunksExtractor {
        UnidiffExtractor::new()
    }

    #[test]
    fn single_file_diff_extracts_ranges_for_single_file() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let ranges = extractor.extract(
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
        let extractor = create_extractor();
        let ranges = extractor.extract(
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
            ranges.keys().map(|k| k.as_str()).collect::<HashSet<_>>(),
            HashSet::from(["Cargo.toml", "src/main.rs", "src/differ.rs"])
        );
        Ok(())
    }

    #[test]
    fn single_new_line_diff_returns_single_range() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let ranges = extractor.extract(
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
        assert_eq!(ranges["a.txt"], vec![(4, 4)]);
        Ok(())
    }

    #[test]
    fn single_first_new_line_diff_returns_single_range() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let ranges = extractor.extract(
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
        assert_eq!(ranges["a.txt"], vec![(1, 1)]);
        Ok(())
    }

    #[test]
    fn multiple_contiguous_new_lines_diff_returns_single_range() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let ranges = extractor.extract(
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
        assert_eq!(ranges["a.txt"], vec![(4, 5)]);
        Ok(())
    }

    #[test]
    fn multiple_first_contiguous_new_lines_diff_returns_single_range() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let ranges = extractor.extract(
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
        assert_eq!(ranges["a.txt"], vec![(1, 2)]);
        Ok(())
    }

    #[test]
    fn multiple_non_contiguous_new_lines_diff_returns_multiple_ranges() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let ranges = extractor.extract(
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
        assert_eq!(ranges["a.txt"], vec![(3, 3), (5, 5)]);
        Ok(())
    }

    #[test]
    fn multiple_contiguous_new_lines_diff_returns_multiple_ranges() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let ranges = extractor.extract(
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
        assert_eq!(ranges["a.txt"], vec![(3, 4), (6, 7)]);
        Ok(())
    }

    #[test]
    fn modified_line_returns_single_range() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let ranges = extractor.extract(
            r#"diff --git a/a.txt b/a.txt
index f384549..e4c2829 100644
--- a/a.txt
+++ b/a.txt
@@ -1,4 +1,4 @@
 one
 two
-three
+modified three
 four"#,
        )?;
        assert_eq!(ranges["a.txt"], vec![(3, 3)]);
        Ok(())
    }

    #[test]
    fn multiple_non_consecutive_modified_line_returns_separate_ranges() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let ranges = extractor.extract(
            r#"diff --git a/a.txt b/a.txt
index f384549..46c7533 100644
--- a/a.txt
+++ b/a.txt
@@ -1,4 +1,4 @@
-one
+modified one
 two
-three
+modified three
 four"#,
        )?;
        assert_eq!(ranges["a.txt"], vec![(1, 1), (3, 3)]);
        Ok(())
    }

    #[test]
    fn multiple_consecutive_modified_lines_returns_single_ranges() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let ranges = extractor.extract(
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
        assert_eq!(ranges["a.txt"], vec![(2, 3)]);
        Ok(())
    }

    #[test]
    fn single_deleted_line_returns_single_range() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let ranges = extractor.extract(
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
        assert_eq!(ranges["a.txt"], vec![(3, 3)]);
        Ok(())
    }

    #[test]
    fn single_first_deleted_line_returns_single_range() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let ranges = extractor.extract(
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
        assert_eq!(ranges["a.txt"], vec![(1, 1)]);
        Ok(())
    }

    #[test]
    fn single_last_deleted_line_returns_single_range() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let ranges = extractor.extract(
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
        assert_eq!(ranges["a.txt"], vec![(4, 4)]);
        Ok(())
    }

    #[test]
    fn multiple_non_consecutive_deleted_lines_returns_separate_ranges() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let ranges = extractor.extract(
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
        assert_eq!(ranges["a.txt"], vec![(1, 1), (3, 3)]);
        Ok(())
    }

    #[test]
    fn multiple_consecutive_deleted_lines_returns_single_range() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let ranges = extractor.extract(
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
        assert_eq!(ranges["a.txt"], vec![(2, 2)]);
        Ok(())
    }

    #[test]
    fn all_lines_deleted_treated_as_deleted_file() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let ranges = extractor.extract(
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
    fn mixed_changes_returns_correct_ranges() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let ranges = extractor.extract(
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
        assert_eq!(ranges["a.txt"], vec![(1, 1), (3, 5)]);
        Ok(())
    }

    #[test]
    fn new_file_diff_returns_single_range_for_entire_file() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let ranges = extractor.extract(
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
        assert_eq!(ranges["example.rs"], vec![(1, 3)]);
        Ok(())
    }

    #[test]
    fn deleted_file_diff_is_ignored() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let ranges = extractor.extract(
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
