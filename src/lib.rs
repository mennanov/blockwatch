pub mod blocks;
pub mod differ;
pub mod flags;
pub mod parsers;
pub mod validators;

#[cfg(test)]
mod test_utils {
    use std::ops::Range;

    pub(crate) fn substr_range(input: &str, substr: &str) -> Range<usize> {
        let pos = input.find(substr).unwrap();
        pos..(pos + substr.len())
    }
}
