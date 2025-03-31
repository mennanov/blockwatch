use anyhow::Context;
use diffy::{Line, Patch};
use std::cmp::min;
use std::collections::HashMap;

/// Extracts hunks from a diff in a patch format (e.g. output of `git diff --patch`).
trait HunksExtractor {
    /// Returns a mapping from the changed filenames to a list of closed-closed ranges of the
    /// changed lines.
    fn extract(&self, patch_diff: &str) -> anyhow::Result<HashMap<String, Vec<(usize, usize)>>>;
}

struct DiffyExtractor;

const DIFF_FILE_PREFIX: &str = "diff --git";

impl DiffyExtractor {
    fn new() -> Self {
        Self {}
    }

    /// Breaks the given `patch_diff` into a vector of disjoint diffs per file.
    fn file_diffs(patch_diff: &str) -> Vec<&str> {
        let mut file_diffs: Vec<&str> = Vec::new();
        let mut prev_pos = 0;
        for (mut pos, _) in patch_diff.match_indices('\n') {
            pos += 1;
            let prefix = &patch_diff[pos..min(pos + DIFF_FILE_PREFIX.len(), patch_diff.len())];
            if !prefix.starts_with(DIFF_FILE_PREFIX) {
                continue;
            }
            file_diffs.push(&patch_diff[prev_pos..pos]);
            prev_pos = pos;
        }
        file_diffs.push(&patch_diff[prev_pos..]);
        file_diffs
    }

    fn hunks(patch: &Patch<str>) -> anyhow::Result<Vec<(usize, usize)>> {
        let mut hunks = Vec::new();
        for hunk in patch.hunks() {
            let hunk_start = hunk.new_range().start();
            let mut start = None;
            let mut end = None;
            let mut prev_line = None;
            for (idx, line) in hunk.lines().iter().enumerate() {
                match line {
                    Line::Insert(_) => {
                        if start.is_none() {
                            start = Some(hunk_start + idx);
                        }
                        if !matches!(prev_line, Some(&Line::Delete(_))) {
                            // A `Delete` followed immediately by the `Insert` is a modified
                            // line and should be counted as single.
                            end = Some(hunk_start + idx);
                        }
                    }
                    Line::Delete(_) => {
                        if start.is_none() {
                            start = Some(hunk_start + idx);
                        }
                        end = Some(hunk_start + idx);
                    }
                    Line::Context(_) => {
                        if let Some(start) = start.take() {
                            let end = end.take().unwrap_or(start);
                            hunks.push((start, end));
                        }
                    }
                }
                prev_line = Some(line);
            }
            if let Some(start) = start.take() {
                let end = end.take().unwrap_or(start);
                hunks.push((start, end));
            }
        }
        Ok(hunks)
    }
}

impl HunksExtractor for DiffyExtractor {
    fn extract(&self, patch_diff: &str) -> anyhow::Result<HashMap<String, Vec<(usize, usize)>>> {
        let file_diffs = Self::file_diffs(patch_diff);
        let mut result = HashMap::new();
        for file_diff in file_diffs {
            let patch = diffy::Patch::from_str(file_diff)?;
            if patch.modified().is_none()
                || patch.modified().is_some_and(|f| f.starts_with("/dev/null"))
            {
                // The file is deleted.
                continue;
            }
            let filename = patch
                .modified()
                .context("Modified filename not found in diff")?;
            let hunks = Self::hunks(&patch)?;
            result.insert(filename.trim_start_matches("b/").to_string(), hunks);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn create_extractor() -> impl HunksExtractor {
        DiffyExtractor::new()
    }

    #[test]
    fn single_file_diff_extracts_ranges_for_single_file() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let hunks = extractor.extract(
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
        assert_eq!(hunks.keys().collect::<Vec<_>>(), vec!["Cargo.toml"]);
        Ok(())
    }

    #[test]
    fn multiple_files_diff_extracts_ranges_for_multiple_files() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let hunks = extractor.extract(
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
            hunks.keys().map(|k| k.as_str()).collect::<HashSet<_>>(),
            HashSet::from(["Cargo.toml", "src/main.rs", "src/differ.rs"])
        );
        Ok(())
    }

    #[test]
    fn single_new_line_diff_returns_single_range() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let hunks = extractor.extract(
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
        assert_eq!(hunks["Cargo.toml"], vec![(12, 12)]);
        Ok(())
    }

    #[test]
    fn multiple_contiguous_new_lines_diff_returns_single_range() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let hunks = extractor.extract(
            r#"diff --git a/Cargo.toml b/Cargo.toml
index 8c34c48..23ddd69 100644
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -9,6 +9,8 @@ tree-sitter = "0.25.3"
 tree-sitter-rust = "0.23"
 tree-sitter-java = "0.23.5"
 quick-xml = "0.37.2"
+diffy = "0.4.2"
+foo = "0.0.1"
 
 [build-dependencies]
 cc="1.2.16"
\ No newline at end of file"#,
        )?;
        assert_eq!(hunks["Cargo.toml"], vec![(12, 13)]);
        Ok(())
    }

    #[test]
    fn multiple_non_contiguous_new_lines_diff_returns_multiple_ranges() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let hunks = extractor.extract(
            r#"diff --git a/Cargo.toml b/Cargo.toml
index 8c34c48..23ddd69 100644
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -8,7 +8,9 @@ anyhow = "1.0.97"
 tree-sitter = "0.25.3"
 tree-sitter-rust = "0.23"
 tree-sitter-java = "0.23.5"
+foo = "0.0.0"
 quick-xml = "0.37.2"
+diffy = "0.4.2"
 
 [build-dependencies]
 cc="1.2.16"
\ No newline at end of file"#,
        )?;
        assert_eq!(hunks["Cargo.toml"], vec![(11, 11), (13, 13)]);
        Ok(())
    }

    #[test]
    fn modified_line_returns_single_range() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let hunks = extractor.extract(
            r#"diff --git a/src/main.rs b/src/main.rs
index 63c5842..34d1d3f 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,1 +1,1 @@
-    println!("Hello!");
+    println!("Hello, world!");
"#,
        )?;
        assert_eq!(hunks["src/main.rs"], vec![(1, 1)]);
        Ok(())
    }

    #[test]
    fn deleted_line_returns_single_range() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let hunks = extractor.extract(
            r#"diff --git a/src/main.rs b/src/main.rs
index 63c5842..34d1d3f 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,2 +1,1 @@
-    println!("Debug line");
 println!("Hello, world!");
"#,
        )?;
        assert_eq!(hunks["src/main.rs"], vec![(1, 1)]);
        Ok(())
    }

    #[test]
    fn mixed_changes_returns_correct_ranges() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let hunks = extractor.extract(
            r#"diff --git a/src/main.rs b/src/main.rs
index 63c5842..34d1d3f 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
-    // Old comment
+    // New comment
 println!("Hello!");
+    println!("World!");
-    println!("Bye!");
+    println!("Goodbye!");
"#,
        )?;
        assert_eq!(hunks["src/main.rs"], vec![(1, 1), (4, 5)]);
        Ok(())
    }

    #[test]
    fn new_file_diff_returns_single_range_for_entire_file() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let hunks = extractor.extract(
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
        assert_eq!(hunks["example.rs"], vec![(1, 3)]);
        Ok(())
    }

    #[test]
    fn deleted_file_diff_is_ignored() -> anyhow::Result<()> {
        let extractor = create_extractor();
        let hunks = extractor.extract(
            r#"diff --git a/src/deleted_file.rs b/src/deleted_file.rs
deleted file mode 100644
index 1234567..0000000
--- a/src/deleted_file.rs
+++ /dev/null
@@ -1,3 +0,0 @@
-fn main() {
-    println!("Old file");
-}
"#,
        )?;
        assert!(hunks.is_empty());
        Ok(())
    }
}
