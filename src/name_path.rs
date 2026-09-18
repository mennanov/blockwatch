use anyhow::{Context, bail};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use std::fmt;

/// A rooted name path selector addressing a syntax element within a file.
///
/// Follows RFC 6901 JSON Pointer syntax:
/// - Always begins with a leading `/`.
/// - Characters `,` and `:` are percent-encoded in references (`%2C`, `%3A`).
/// - Percent-decoding is validated strictly (rejecting malformed `%xx` or invalid UTF-8).
/// - Segments are separated by `/`.
/// - RFC 6901 escape sequences in each segment are unescaped: `~1` becomes `/` and `~0` becomes `~`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NamePath {
    segments: Vec<String>,
}

impl NamePath {
    /// Parses and validates a name path fragment.
    ///
    /// The input may optionally include a leading `#`. The path must be rooted with a leading `/`.
    /// `%2F` acts as a `/` separator as well.
    pub fn parse(fragment: &str) -> anyhow::Result<Self> {
        let trimmed = fragment.trim();
        let path_str = trimmed.strip_prefix('#').unwrap_or(trimmed);
        if path_str.is_empty() {
            bail!("path cannot be empty; must start with '/' (e.g. \"/\")");
        }

        validate_percent_encoding(path_str)?;

        // Percent-decoding precedes leading-slash validation and segment splitting, so `%2F`
        // consistently acts as a separator in all positions, as required by RFC 6901 §6.
        let decoded = percent_encoding::percent_decode_str(path_str)
            .decode_utf8()
            .with_context(|| format!("path \"{fragment}\" contains invalid UTF-8 bytes"))?;

        if trimmed.starts_with('#') && decoded.starts_with(|c: char| c.is_whitespace()) {
            bail!("unexpected whitespace after '#'; path must start with '/'");
        }
        if !decoded.starts_with('/') {
            bail!("path must start with '/' (e.g. \"/{decoded}\")");
        }

        // A rooted path always has a leading `/`, so the segment slice starts after index 1.
        let segments: Vec<String> = decoded[1..]
            .split('/')
            .map(unescape_rfc6901)
            .collect::<anyhow::Result<Vec<String>>>()?;

        Ok(Self { segments })
    }

    /// Returns the parsed and unescaped path segments.
    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    /// Creates a rooted [`NamePath`] from unescaped segments.
    ///
    /// # Panics
    /// Panics if `segments` is empty. A rooted path has at least one segment: `/` is the path of
    /// the empty name.
    pub(crate) fn from_segments(segments: Vec<String>) -> Self {
        assert!(
            !segments.is_empty(),
            "a name path needs at least one segment"
        );
        Self { segments }
    }
}

/// Characters that must be percent-encoded when formatting a name path segment.
///
/// In addition to ASCII control characters and non-ASCII bytes, encodes characters that have
/// special meaning in BlockWatch attributes and URI fragments:
/// - `%`: prefix for percent-encoded bytes; raw `%` must be `%25` to avoid ambiguity.
/// - `,`: separates multiple references in block attributes.
/// - `:`: separates file and block names in references.
/// - `#`: separates file path and selector fragment.
/// - `"`: delimits double-quoted attribute values in block comments.
/// - `'`: delimits single-quoted attribute values in block comments.
/// - `' '`: space, because block references are trimmed.
const FRAGMENT_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'%')
    .add(b',')
    .add(b':')
    .add(b'#')
    .add(b'"')
    .add(b'\'');

impl fmt::Display for NamePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for segment in &self.segments {
            let escaped = escape_rfc6901(segment);
            write!(f, "/{}", utf8_percent_encode(&escaped, FRAGMENT_ENCODE_SET))?;
        }
        Ok(())
    }
}

/// Escapes a segment according to RFC 6901 JSON Pointer syntax.
fn escape_rfc6901(segment: &str) -> String {
    // Order matters: `~` must be replaced with `~0` before `/` is replaced with `~1`, avoiding
    // turning `~1` into `~01`.
    segment.replace('~', "~0").replace('/', "~1")
}

/// Validates that every `%` character in `s` is followed by two ASCII hexadecimal digits.
fn validate_percent_encoding(s: &str) -> anyhow::Result<()> {
    for remainder in s.split('%').skip(1) {
        match remainder.as_bytes() {
            [h, l, ..] if h.is_ascii_hexdigit() && l.is_ascii_hexdigit() => {}
            _ => bail!("invalid percent escape in path \"{s}\""),
        }
    }
    Ok(())
}

/// Unescapes and validates an RFC 6901 JSON Pointer segment.
///
/// RFC 6901 §3 allows `~` only in the escapes `~0` (representing `~`) and `~1` (representing `/`).
/// Any lone `~` or `~` followed by another character is rejected.
fn unescape_rfc6901(segment: &str) -> anyhow::Result<String> {
    let mut unescaped = String::with_capacity(segment.len());
    let mut chars = segment.chars();
    while let Some(c) = chars.next() {
        if c == '~' {
            match chars.next() {
                Some('0') => unescaped.push('~'),
                Some('1') => unescaped.push('/'),
                Some(other) => {
                    bail!("invalid RFC 6901 escape \"~{other}\" in segment \"{segment}\"");
                }
                None => {
                    bail!("invalid RFC 6901 escape: unescaped '~' at end of segment \"{segment}\"");
                }
            }
        } else {
            unescaped.push(c);
        }
    }
    Ok(unescaped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooted_path_parses_successfully() -> anyhow::Result<()> {
        let path = NamePath::parse("/dependencies/inngest")?;
        assert_eq!(path.to_string(), "/dependencies/inngest");
        assert_eq!(path.segments(), &["dependencies", "inngest"]);
        Ok(())
    }

    #[test]
    fn leading_hash_path_parses_identical_to_unprefixed() -> anyhow::Result<()> {
        let path = NamePath::parse("#/project/version")?;
        assert_eq!(path.to_string(), "/project/version");
        assert_eq!(path.segments(), &["project", "version"]);
        Ok(())
    }

    #[test]
    fn leading_and_trailing_whitespace_trimmed_successfully() -> anyhow::Result<()> {
        let path = NamePath::parse("  #/project/version  ")?;
        assert_eq!(path.to_string(), "/project/version");
        assert_eq!(path.segments(), &["project", "version"]);

        let path2 = NamePath::parse("  /project/version  ")?;
        assert_eq!(path2.to_string(), "/project/version");
        Ok(())
    }

    #[test]
    fn whitespace_between_hash_and_slash_fails() {
        let test_cases = [
            "# /project/version",
            "#%20/project/version",
            "#\t/project/version",
            "#%09/project/version",
            "#\u{a0}/project/version",
            "#%C2%A0/project/version",
        ];
        for raw in test_cases {
            let err = NamePath::parse(raw).unwrap_err();
            assert!(
                err.to_string().contains("unexpected whitespace after '#'"),
                "unexpected error message for {raw}: {err}"
            );
        }
    }

    #[test]
    fn unrooted_path_fails_with_hint() {
        let err = NamePath::parse("dependencies/inngest").unwrap_err();
        assert!(
            err.to_string().contains("path must start with '/'"),
            "unexpected error message: {err}"
        );

        let err_percent = NamePath::parse("%20dependencies/inngest").unwrap_err();
        assert!(
            err_percent.to_string().contains("path must start with '/'"),
            "unexpected error message: {err_percent}"
        );
    }

    #[test]
    fn empty_or_hash_only_path_fails() {
        assert!(NamePath::parse("").is_err());
        assert!(NamePath::parse("#").is_err());
        assert!(NamePath::parse("   ").is_err());
        assert!(NamePath::parse("  #  ").is_err());
    }

    #[test]
    fn percent_encoded_characters_decode_before_splitting() -> anyhow::Result<()> {
        // %2C is comma, %3A is colon, %20 is space, %25 is percent
        let path = NamePath::parse("/keys/%2C/%3A/%20/%25")?;
        assert_eq!(path.segments(), &["keys", ",", ":", " ", "%"]);
        Ok(())
    }

    #[test]
    fn percent_encoded_slash_acts_as_separator() -> anyhow::Result<()> {
        // Because decoding comes first, %2F is a separator everywhere, including the leading position
        let path = NamePath::parse("/a%2Fb")?;
        assert_eq!(path.segments(), &["a", "b"]);

        let leading = NamePath::parse("%2Fa%2Fb")?;
        assert_eq!(leading.segments(), &["a", "b"]);
        assert_eq!(path, leading);

        let hash_leading = NamePath::parse("#%2Fa%2Fb")?;
        assert_eq!(hash_leading, leading);

        let root_only = NamePath::parse("%2F")?;
        assert_eq!(root_only, NamePath::parse("/")?);
        Ok(())
    }

    #[test]
    fn malformed_percent_escape_fails() {
        assert!(NamePath::parse("/invalid/%2").is_err());
        assert!(NamePath::parse("/invalid/%").is_err());
        assert!(NamePath::parse("/invalid/%zz").is_err());
        assert!(NamePath::parse("/invalid/%2g").is_err());
    }

    #[test]
    fn invalid_utf8_percent_sequence_fails() {
        // %FF%FF is not valid UTF-8
        assert!(NamePath::parse("/invalid/%FF%FF").is_err());
    }

    #[test]
    fn rfc6901_escape_sequences_decode_correctly() -> anyhow::Result<()> {
        // ~1 becomes /, ~0 becomes ~
        let path = NamePath::parse("/deps/@types~1node/v~01")?;
        assert_eq!(path.segments(), &["deps", "@types/node", "v~1"]);
        assert_eq!(path.to_string(), "/deps/@types~1node/v~01");
        Ok(())
    }

    #[test]
    fn invalid_rfc6901_escape_sequence_fails() {
        assert!(NamePath::parse("/invalid/~2").is_err());
        assert!(NamePath::parse("/invalid/~").is_err());
        assert!(NamePath::parse("/invalid/foo~bar").is_err());
        assert!(NamePath::parse("/invalid/foo~~").is_err());
    }

    #[test]
    fn empty_segments_are_preserved() -> anyhow::Result<()> {
        let path = NamePath::parse("/a//b")?;
        assert_eq!(path.segments(), &["a", "", "b"]);
        assert_eq!(path.to_string(), "/a//b");
        Ok(())
    }

    #[test]
    fn root_only_path_yields_single_empty_segment() -> anyhow::Result<()> {
        let path = NamePath::parse("/")?;
        assert_eq!(path.segments(), &[""]);
        assert_eq!(path.to_string(), "/");
        Ok(())
    }

    #[test]
    fn different_raw_encodings_compare_equal() -> anyhow::Result<()> {
        let p1 = NamePath::parse("/deps/%41")?;
        let p2 = NamePath::parse("/deps/A")?;
        assert_eq!(p1, p2);
        assert_eq!(p1.to_string(), "/deps/A");

        let p_hex_upper = NamePath::parse("/keys/%2C")?;
        let p_hex_lower = NamePath::parse("/keys/%2c")?;
        assert_eq!(p_hex_upper, p_hex_lower);

        let p_hash = NamePath::parse("#/a/b")?;
        let p_no_hash = NamePath::parse("/a/b")?;
        assert_eq!(p_hash, p_no_hash);
        Ok(())
    }

    #[test]
    fn parse_and_display_round_trips_identically() -> anyhow::Result<()> {
        let test_cases = [
            "/",
            "/dependencies/inngest",
            "/keys/%2C/%3A/%20/%25",
            "/keys/%252C",
            "/deps/@types~1node/v~01",
            "/a//b",
            "/caf%C3%A9",
            "/~0~1%3A%25",
            "/special/%23/%22/%27",
        ];
        for raw in test_cases {
            let parsed = NamePath::parse(raw)?;
            let formatted = parsed.to_string();
            let reparsed = NamePath::parse(&formatted)?;
            assert_eq!(parsed, reparsed, "failed round-trip for {raw}");
        }
        Ok(())
    }

    #[test]
    fn path_with_single_and_double_quotes_display_percent_encoded() -> anyhow::Result<()> {
        let path = NamePath::parse("/items/\"double\"/'single'")?;
        assert_eq!(path.segments(), &["items", "\"double\"", "'single'"]);
        assert_eq!(path.to_string(), "/items/%22double%22/%27single%27");

        let reparsed = NamePath::parse(&path.to_string())?;
        assert_eq!(reparsed, path);
        Ok(())
    }

    #[test]
    fn valid_segments_provided_from_segments_constructs_rooted_path() {
        let path = NamePath::from_segments(vec!["dependencies".to_string(), "inngest".to_string()]);
        assert_eq!(path.to_string(), "/dependencies/inngest");
        assert_eq!(path.segments(), &["dependencies", "inngest"]);
    }

    #[test]
    #[should_panic(expected = "a name path needs at least one segment")]
    fn empty_segments_provided_from_segments_panics() {
        NamePath::from_segments(vec![]);
    }
}
