use afl::fuzz;
use blockwatch::blocks::Block;
use blockwatch::parsers;

fn main() {
    fuzz!(|data: &[u8]| {
        let s = String::from_utf8_lossy(data);
        let mid = s.len() / 2;
        let (noise1, noise2) = (0..=mid)
            .rev()
            .find_map(|pos| s.split_at_checked(pos))
            .unwrap_or(("", ""));
        let source =
            format!("/* {noise1} <block> */\nlet variable = \"value\";\n// </block> {noise2}\n");

        match parse_rust_blocks(&source) {
            Ok(blocks) => {
                assert_eq!(blocks.len(), 1, "expected exactly one <block> ... </block>");
            }
            Err(err) => {
                panic!("parser returned error: {err}");
            }
        }
    });
}

fn parse_rust_blocks(source: &str) -> anyhow::Result<Vec<Block>> {
    let parsers = parsers::language_parsers()?;
    parsers["rs"].parse(source)
}
