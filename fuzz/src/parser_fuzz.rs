use afl::fuzz;
use blockwatch::blocks::Block;
use blockwatch::language_parsers;
use std::ffi::OsString;

fn main() {
    fuzz!(|data: &[u8]| {
        // Check if the input is valid UTF-8, return early if not
        let Ok(input_str) = std::str::from_utf8(data) else {
            return;
        };
        let Some((noise1, noise2)) = split_in_half(input_str) else {
            return;
        };
        let Some((noise1_before, noise1_after)) = split_in_half(noise1) else {
            return;
        };
        let Some((noise2_before, noise2_after)) = split_in_half(noise2) else {
            return;
        };
        let noise1_before = noise1_before.replace("*/", "").replace("\0", "");
        let noise1_after = noise1_after.replace("*/", "").replace("\0", "");
        let noise2_before = noise2_before
            .replace("//", "")
            .replace("\n", "")
            .replace("\0", "");
        let source = format!(
            "/* {noise1_before} <block> {noise1_after} */\nlet variable = \"value\";\n// {noise2_before} </block> {noise2_after}"
        );

        match parse_rust_blocks(&source) {
            Ok(blocks) => {
                assert_eq!(blocks.len(), 1, "expected exactly one <block> ... </block>");
            }
            Err(err) => {
                panic!("parser returned error: {err}\ninput:\n{source}");
            }
        }
    });
}

fn parse_rust_blocks(source: &str) -> anyhow::Result<Vec<Block>> {
    let parsers = language_parsers::language_parsers()?;
    parsers[&OsString::from("rs")].parse(source)
}

fn split_in_half(input: &str) -> Option<(&str, &str)> {
    if input.is_empty() {
        return None;
    }
    let mid = input.len() / 2;
    // Find the nearest valid UTF-8 character boundary
    let mut pos = mid;
    while pos > 0 && !input.is_char_boundary(pos) {
        pos -= 1;
    }
    input.split_at_checked(pos)
}
