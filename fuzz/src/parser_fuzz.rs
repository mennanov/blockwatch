use afl::fuzz;
use blockwatch::blocks::Block;
use blockwatch::language_parsers;
use regex::Regex;
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
        let noise1_before = trim_multiline_comment(noise1_before);
        let noise1_after = trim_multiline_comment(noise1_after);
        let noise2_before = trim_single_line_comment(noise2_before);
        let noise2_after = trim_single_line_comment(noise2_after);
        let source = format!(
            "/* {noise1_before} <block> {noise1_after} */\nlet variable = \"value\";\n// {noise2_before} </block> {noise2_after}"
        );

        match parse_rust_blocks(&source) {
            Ok(blocks) => {
                assert_eq!(blocks.len(), 1, "input: {source}");
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

fn trim_multiline_comment(input: &str) -> String {
    let re = Regex::new(r"(/+\*+)|(\*+/+)").unwrap();
    re.replace_all(input, "").replace("\0", "").to_string()
}

fn trim_single_line_comment(input: &str) -> String {
    input.replace(['\0', '\n'], "")
}
