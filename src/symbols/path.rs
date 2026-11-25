//! `SymbolPath`: a parsed and validated symbol path. It follows RFC 6901 and adds filters, such as
//! `~[name=blue]` and `~[kind=Rollout][metadata/name=api]`.

use anyhow::{Context, bail};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use std::fmt;

/// A symbol path: the rooted path after the `#` of a symbol reference, which addresses a symbol
/// within a file.
///
/// Follows RFC 6901 JSON Pointer syntax:
/// - Always begins with a leading `/`.
/// - A `,` is percent-encoded in references (`%2C`). Any other character can be too.
/// - Percent-decoding is validated strictly (rejecting malformed `%xx` or invalid UTF-8).
/// - Segments are separated by `/`.
/// - RFC 6901 escape sequences in each segment are unescaped: `~1` becomes `/` and `~0` becomes `~`.
///
/// It adds one thing to RFC 6901: a segment such as `~[name=blue]` is a [`Segment::Filter`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SymbolPath {
    segments: Vec<Segment>,
}

/// One `[key_path=value]` condition of a [`Segment::Filter`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FilterCondition {
    pub(super) key_path: Vec<String>,
    pub(super) value: String,
}

/// One segment of a [`SymbolPath`], with its escapes decoded.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Segment {
    /// A key, or a position in a list, such as `0`.
    Key(String),
    /// A filter, written `~[key=value]` or `~[a=1][b/c=2]`. It stands for the one child matching
    /// every condition. Never empty.
    Filter(Vec<FilterCondition>),
}

impl SymbolPath {
    /// Parses and validates a symbol path.
    ///
    /// The input may optionally include a leading `#`. The path must be rooted with a leading `/`.
    /// `%2F` acts as a `/` separator as well.
    pub(crate) fn parse(fragment: &str) -> anyhow::Result<Self> {
        let trimmed = fragment.trim();
        let path_str = trimmed.strip_prefix('#').unwrap_or(trimmed);
        if path_str.is_empty() {
            bail!("symbol path cannot be empty; it must start with '/' (e.g. \"/\")");
        }

        validate_percent_encoding(path_str)?;

        // `%2F` acts as a `/` separator in all positions, as required by RFC 6901 §6. Decoding
        // happens per piece after splitting, so `%5D` inside a filter value stays distinct from
        // the `]` that closes a condition.
        let Some(leading_slash) = slash_len(path_str.as_bytes()) else {
            let decoded = percent_decode(path_str, fragment)?;
            if trimmed.starts_with('#') && decoded.starts_with(|c: char| c.is_whitespace()) {
                bail!("unexpected whitespace after '#'; symbol path must start with '/'");
            }
            bail!("symbol path must start with '/' (e.g. \"/{decoded}\")");
        };

        #[expect(
            clippy::string_slice,
            reason = "`leading_slash` is the byte length of leading `/` or `%2F`"
        )]
        let segments = parse_segments(&path_str[leading_slash..], fragment)?;

        Ok(Self { segments })
    }

    /// Returns the parsed and unescaped path segments.
    pub(crate) fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Returns this path with its segment at `index` replaced by `segment`.
    ///
    /// # Panics
    /// Panics if the path has no segment at `index`.
    pub(crate) fn with_segment(mut self, index: usize, segment: Segment) -> Self {
        self.segments[index] = segment;
        self
    }

    /// Creates a rooted [`SymbolPath`] from unescaped segments.
    ///
    /// # Panics
    /// Panics if `segments` is empty. A rooted path has at least one segment: `/` is the path of
    /// the empty name.
    pub(crate) fn from_segments(segments: Vec<String>) -> Self {
        assert!(
            !segments.is_empty(),
            "a symbol path needs at least one segment"
        );
        Self {
            segments: segments.into_iter().map(Segment::Key).collect(),
        }
    }
}

/// The ASCII characters that must be percent-encoded when formatting a symbol path segment.
///
/// In addition to ASCII control characters, encodes characters that have special meaning in
/// blockwatch attributes and URI fragments:
/// - `%`: prefix for percent-encoded bytes; raw `%` must be `%25` to avoid ambiguity.
/// - `,`: separates multiple references in block attributes.
/// - `#`: separates the file path from the symbol path.
/// - `"`: delimits double-quoted attribute values in block comments.
/// - `'`: delimits single-quoted attribute values in block comments.
/// - `' '`: space, because block references are trimmed.
const FRAGMENT_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'%')
    .add(b',')
    .add(b'#')
    .add(b'"')
    .add(b'\'');

impl fmt::Display for SymbolPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for segment in &self.segments {
            write!(f, "/{segment}")?;
        }
        Ok(())
    }
}

/// Writes the segment as a path writes it, escapes included, without the `/` before it.
impl fmt::Display for Segment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key(key) => write!(f, "{}", encode(key)),
            Self::Filter(conditions) => {
                f.write_str("~")?;
                for condition in conditions {
                    write!(f, "{condition}")?;
                }
                Ok(())
            }
        }
    }
}

/// Writes one filter condition as `[key_path=value]`, escapes included.
impl fmt::Display for FilterCondition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[")?;
        for (index, part) in self.key_path.iter().enumerate() {
            if index > 0 {
                f.write_str("/")?;
            }
            f.write_str(&encode_filter_key(part))?;
        }
        write!(f, "={}]", encode_filter_value(&self.value))
    }
}

/// Escapes `text`, a key or a filter's key or value, so that [`SymbolPath::parse`] reads it back
/// as the same text.
fn encode(text: &str) -> String {
    let escaped = escape_rfc6901(text);
    let inner_start = escaped.len() - escaped.trim_start_matches(' ').len();
    let inner_end = escaped.trim_end_matches(' ').len();
    let mut encoded = String::with_capacity(escaped.len());
    let mut buffer = [0; 4];
    for (index, char) in escaped.char_indices() {
        // A space between other characters stays as written, so a key such as `Dark red` reads
        // as it does in the file. One at the end would be lost, because references are trimmed,
        // and one at the start is hard to see after the `/`. Other non-ASCII characters stay as
        // written too, so a message shows `/café`, not `/caf%C3%A9`. Other whitespace and
        // control characters are encoded because they are hard to see.
        let as_written = match char {
            ' ' => (inner_start..inner_end).contains(&index),
            _ => !(char.is_ascii() || char.is_whitespace() || char.is_control()),
        };
        if as_written {
            encoded.push(char);
        } else {
            encoded.extend(utf8_percent_encode(
                char.encode_utf8(&mut buffer),
                FRAGMENT_ENCODE_SET,
            ));
        }
    }
    encoded
}

/// Escapes one segment of a filter condition's key path so [`SymbolPath::parse`] reads it back as
/// the same text.
fn encode_filter_key(text: &str) -> String {
    // `=` ends the key path and `][` starts the next condition, so both are percent-encoded inside
    // a key segment.
    encode(text).replace('=', "%3D").replace("][", "%5D[")
}

/// Escapes a filter condition's value so [`SymbolPath::parse`] reads it back as the same text.
fn encode_filter_value(text: &str) -> String {
    // `][` starts the next condition in a compound filter, so a literal `]` before `[` is
    // percent-encoded while a lone `]` elsewhere stays as written.
    encode(text).replace("][", "%5D[")
}

/// If `bytes` starts with `/` (raw or percent-encoded as `%2F`/`%2f`), returns its byte length.
fn slash_len(bytes: &[u8]) -> Option<usize> {
    if bytes.first() == Some(&b'/') {
        Some(1)
    } else if bytes.len() >= 3
        && bytes[0] == b'%'
        && bytes[1] == b'2'
        && bytes[2].eq_ignore_ascii_case(&b'f')
    {
        Some(3)
    } else {
        None
    }
}

/// Splits `text` at the first `/` (or `%2F`/`%2f`), returning the part before it and the remainder
/// after it (or `None` if there is no slash).
fn split_next_slash(text: &str) -> (&str, Option<&str>) {
    let bytes = text.as_bytes();
    for i in 0..bytes.len() {
        if let Some(len) = slash_len(&bytes[i..]) {
            #[expect(
                clippy::string_slice,
                reason = "`i` and `i + len` bound an ASCII `/` or `%2F`"
            )]
            return (&text[..i], Some(&text[i + len..]));
        }
    }
    (text, None)
}

/// Splits `text` on every `/` (or `%2F`/`%2f`), preserving empty parts.
fn split_slashes(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = Some(text);
    std::iter::from_fn(move || {
        let current = rest?;
        let (part, next) = split_next_slash(current);
        rest = next;
        Some(part)
    })
}

/// Parses all raw segments of `path` (a percent-encoded symbol path after its leading `/` or
/// `%2F`) into [`Segment`]s.
fn parse_segments(path: &str, fragment: &str) -> anyhow::Result<Vec<Segment>> {
    let mut segments = Vec::new();
    let mut rest = path;
    loop {
        // RFC 6901 gives `~` a meaning only before `0` and `1`. So an escaped key never starts
        // with `~[`, and a filter can't be mistaken for a key.
        let (segment, next) = if rest.starts_with("~[") {
            parse_filter_segment(rest, fragment)?
        } else {
            let (raw, next) = split_next_slash(rest);
            (Segment::Key(decode_and_unescape(raw, fragment)?), next)
        };
        segments.push(segment);
        match next {
            Some(tail) => rest = tail,
            None => return Ok(segments),
        }
    }
}

/// How a filter condition's value ended during scanning.
enum ValueEnd<'a> {
    /// Closed by `]` immediately followed by `[`, starting the next condition.
    NextCondition(&'a str),
    /// Closed by `]` at the end of the path or right before `/` (`%2F`).
    EndOfSegment(Option<&'a str>),
    /// Reached `/` (`%2F`) or end of input without a closing `]`.
    Invalid,
}

/// Scans a percent-encoded filter value after `=` until its closing `]` (before `[`, `/`, `%2F`,
/// or end of string).
fn scan_filter_value(text: &str) -> (&str, ValueEnd<'_>) {
    let bytes = text.as_bytes();
    for i in 0..bytes.len() {
        if slash_len(&bytes[i..]).is_some() {
            return ("", ValueEnd::Invalid);
        }
        if bytes[i] == b']' {
            #[expect(clippy::string_slice, reason = "`bytes[i]` is ASCII `]`")]
            let (value, after) = (&text[..i], &text[i + 1..]);
            if after.is_empty() {
                return (value, ValueEnd::EndOfSegment(None));
            }
            if let Some(len) = slash_len(after.as_bytes()) {
                #[expect(
                    clippy::string_slice,
                    reason = "`len` is the byte length of leading `/` or `%2F`"
                )]
                return (value, ValueEnd::EndOfSegment(Some(&after[len..])));
            }
            if after.starts_with('[') {
                return (value, ValueEnd::NextCondition(after));
            }
        }
    }
    ("", ValueEnd::Invalid)
}

/// Splits a percent-encoded filter condition after `[` at the `=` that ends its key path,
/// returning `None` if `][` or the end of `text` appears before `=`.
fn scan_filter_key(text: &str) -> Option<(&str, &str)> {
    let bytes = text.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'=' {
            #[expect(clippy::string_slice, reason = "`bytes[i]` is ASCII `=`")]
            return Some((&text[..i], &text[i + 1..]));
        }
        if bytes[i] == b']' && bytes.get(i + 1) == Some(&b'[') {
            return None;
        }
    }
    None
}

/// Parses a filter segment starting with `~[` at the front of `rest`, returning the parsed
/// [`Segment::Filter`] and the remainder of `rest` after the following `/` (or `None` at end).
fn parse_filter_segment<'a>(
    rest: &'a str,
    fragment: &str,
) -> anyhow::Result<(Segment, Option<&'a str>)> {
    let mut conditions = Vec::new();
    #[expect(clippy::string_slice, reason = "`rest` starts with ASCII `~[`")]
    let mut cursor = &rest[1..];
    loop {
        let after_open = cursor
            .strip_prefix('[')
            .expect("each filter condition starts with `[`");
        let Some((raw_key, after_eq)) = scan_filter_key(after_open) else {
            let (bad_key, _) = split_next_slash(after_open);
            let bad_len = rest.len() - after_open.len() + bad_key.len();
            #[expect(
                clippy::string_slice,
                reason = "`bad_len` ends on `/` or `%2F` or at `rest.len()`"
            )]
            let bad = &rest[..bad_len];
            bail!("invalid filter \"{bad}\" in symbol path; write it as ~[key=value]");
        };
        let (raw_value, end) = scan_filter_value(after_eq);
        match end {
            ValueEnd::NextCondition(next_cond) => {
                conditions.push(parse_condition(raw_key, raw_value, fragment)?);
                cursor = next_cond;
            }
            ValueEnd::EndOfSegment(next_segment) => {
                conditions.push(parse_condition(raw_key, raw_value, fragment)?);
                return Ok((Segment::Filter(conditions), next_segment));
            }
            ValueEnd::Invalid => {
                let (bad_value, _) = split_next_slash(after_eq);
                let bad_len = rest.len() - after_eq.len() + bad_value.len();
                #[expect(
                    clippy::string_slice,
                    reason = "`bad_len` ends on `/` or `%2F` or at `rest.len()`"
                )]
                let bad = &rest[..bad_len];
                bail!("invalid filter \"{bad}\" in symbol path; write it as ~[key=value]");
            }
        }
    }
}

/// Decodes and unescapes one `[key_path=value]` filter condition.
fn parse_condition(
    raw_key: &str,
    raw_value: &str,
    fragment: &str,
) -> anyhow::Result<FilterCondition> {
    let key_path = split_slashes(raw_key)
        .map(|part| decode_and_unescape(part, fragment))
        .collect::<anyhow::Result<Vec<String>>>()?;
    let value = decode_and_unescape(raw_value, fragment)?;
    Ok(FilterCondition { key_path, value })
}

/// Percent-decodes `piece` as UTF-8, reporting `fragment` in any error.
fn percent_decode<'a>(piece: &'a str, fragment: &str) -> anyhow::Result<std::borrow::Cow<'a, str>> {
    percent_encoding::percent_decode_str(piece)
        .decode_utf8()
        .with_context(|| format!("symbol path \"{fragment}\" contains invalid UTF-8 bytes"))
}

/// Percent-decodes `piece` as UTF-8 and unescapes its RFC 6901 `~0`/`~1` sequences.
fn decode_and_unescape(piece: &str, fragment: &str) -> anyhow::Result<String> {
    let decoded = percent_decode(piece, fragment)?;
    unescape_rfc6901(&decoded)
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
            _ => bail!("invalid percent escape in symbol path \"{s}\""),
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

    fn key(key: &str) -> Segment {
        Segment::Key(key.to_string())
    }

    fn keys(keys: &[&str]) -> Vec<Segment> {
        keys.iter().map(|k| key(k)).collect()
    }

    fn filter(key_path: &[&str], value: &str) -> Segment {
        compound_filter(&[(key_path, value)])
    }

    fn compound_filter(conditions: &[(&[&str], &str)]) -> Segment {
        Segment::Filter(
            conditions
                .iter()
                .map(|(key_path, value)| FilterCondition {
                    key_path: key_path.iter().map(|part| (*part).to_string()).collect(),
                    value: (*value).to_string(),
                })
                .collect(),
        )
    }

    #[test]
    fn rooted_path_parses_successfully() -> anyhow::Result<()> {
        let path = SymbolPath::parse("/dependencies/inngest")?;
        assert_eq!(path.to_string(), "/dependencies/inngest");
        assert_eq!(path.segments(), keys(&["dependencies", "inngest"]));
        Ok(())
    }

    #[test]
    fn leading_hash_path_parses_identical_to_unprefixed() -> anyhow::Result<()> {
        let path = SymbolPath::parse("#/project/version")?;
        assert_eq!(path.to_string(), "/project/version");
        assert_eq!(path.segments(), keys(&["project", "version"]));
        Ok(())
    }

    #[test]
    fn leading_and_trailing_whitespace_trimmed_successfully() -> anyhow::Result<()> {
        let path = SymbolPath::parse("  #/project/version  ")?;
        assert_eq!(path.to_string(), "/project/version");
        assert_eq!(path.segments(), keys(&["project", "version"]));

        let path2 = SymbolPath::parse("  /project/version  ")?;
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
            let err = SymbolPath::parse(raw).unwrap_err();
            assert!(
                err.to_string().contains("unexpected whitespace after '#'"),
                "unexpected error message for {raw}: {err}"
            );
        }
    }

    #[test]
    fn unrooted_path_fails_with_hint() {
        let err = SymbolPath::parse("dependencies/inngest").unwrap_err();
        assert!(
            err.to_string().contains("symbol path must start with '/'"),
            "unexpected error message: {err}"
        );

        let err_percent = SymbolPath::parse("%20dependencies/inngest").unwrap_err();
        assert!(
            err_percent
                .to_string()
                .contains("symbol path must start with '/'"),
            "unexpected error message: {err_percent}"
        );
    }

    #[test]
    fn empty_or_hash_only_path_fails() {
        assert!(SymbolPath::parse("").is_err());
        assert!(SymbolPath::parse("#").is_err());
        assert!(SymbolPath::parse("   ").is_err());
        assert!(SymbolPath::parse("  #  ").is_err());
    }

    #[test]
    fn percent_encoded_characters_decode_before_splitting() -> anyhow::Result<()> {
        // %2C is comma, %3A is colon, %20 is space, %25 is percent
        let path = SymbolPath::parse("/keys/%2C/%3A/%20/%25")?;
        assert_eq!(path.segments(), keys(&["keys", ",", ":", " ", "%"]));
        Ok(())
    }

    #[test]
    fn percent_encoded_slash_acts_as_separator() -> anyhow::Result<()> {
        // Because decoding comes first, %2F is a separator everywhere, including the leading position
        let path = SymbolPath::parse("/a%2Fb")?;
        assert_eq!(path.segments(), keys(&["a", "b"]));

        let leading = SymbolPath::parse("%2Fa%2Fb")?;
        assert_eq!(leading.segments(), keys(&["a", "b"]));
        assert_eq!(path, leading);

        let hash_leading = SymbolPath::parse("#%2Fa%2Fb")?;
        assert_eq!(hash_leading, leading);

        let root_only = SymbolPath::parse("%2F")?;
        assert_eq!(root_only, SymbolPath::parse("/")?);
        Ok(())
    }

    #[test]
    fn malformed_percent_escape_fails() {
        assert!(SymbolPath::parse("/invalid/%2").is_err());
        assert!(SymbolPath::parse("/invalid/%").is_err());
        assert!(SymbolPath::parse("/invalid/%zz").is_err());
        assert!(SymbolPath::parse("/invalid/%2g").is_err());
    }

    #[test]
    fn invalid_utf8_percent_sequence_fails() {
        // %FF%FF is not valid UTF-8
        assert!(SymbolPath::parse("/invalid/%FF%FF").is_err());
    }

    #[test]
    fn rfc6901_escape_sequences_decode_correctly() -> anyhow::Result<()> {
        // ~1 becomes /, ~0 becomes ~
        let path = SymbolPath::parse("/deps/@types~1node/v~01")?;
        assert_eq!(path.segments(), keys(&["deps", "@types/node", "v~1"]));
        assert_eq!(path.to_string(), "/deps/@types~1node/v~01");
        Ok(())
    }

    #[test]
    fn invalid_rfc6901_escape_sequence_fails() {
        assert!(SymbolPath::parse("/invalid/~2").is_err());
        assert!(SymbolPath::parse("/invalid/~").is_err());
        assert!(SymbolPath::parse("/invalid/foo~bar").is_err());
        assert!(SymbolPath::parse("/invalid/foo~~").is_err());
        assert!(SymbolPath::parse("/~[key=~value]").is_err());
        assert!(SymbolPath::parse("/~[~key=value]").is_err());
    }

    #[test]
    fn filter_parses_into_key_and_value_and_displays_as_written() -> anyhow::Result<()> {
        let test_cases = [
            (
                "/items/~[name=blue]/content",
                vec![key("items"), filter(&["name"], "blue"), key("content")],
            ),
            // The value starts after the first `=` and ends at the last `]`.
            ("/~[k=a=b]c]", vec![filter(&["k"], "a=b]c")]),
            ("/~[=]", vec![filter(&[""], "")]),
            ("/~[a~1b=c~0d%2C]", vec![filter(&["a/b"], "c~d,")]),
        ];
        for (raw, segments) in test_cases {
            let path = SymbolPath::parse(raw)?;
            assert_eq!(path.segments(), segments, "unexpected segments for {raw}");
            assert_eq!(path.to_string(), raw);
        }
        Ok(())
    }

    #[test]
    fn nested_filter_key_parses_into_segments_and_displays_as_written() -> anyhow::Result<()> {
        // `/` inside a filter key separates nested key segments while `~1` stays a literal `/`
        // in both keys and values.
        let test_cases = [
            (
                "/items/~[metadata/name=api]/spec/replicas",
                vec![
                    key("items"),
                    filter(&["metadata", "name"], "api"),
                    key("spec"),
                    key("replicas"),
                ],
            ),
            (
                "/~[spec/containers/0/name=web]",
                vec![filter(&["spec", "containers", "0", "name"], "web")],
            ),
            (
                "/items/~[metadata/labels/app.kubernetes.io~1name=api]/kind",
                vec![
                    key("items"),
                    filter(&["metadata", "labels", "app.kubernetes.io/name"], "api"),
                    key("kind"),
                ],
            ),
            (
                "/items/~[apiVersion=argoproj.io~1v1alpha1]/metadata/name",
                vec![
                    key("items"),
                    filter(&["apiVersion"], "argoproj.io/v1alpha1"),
                    key("metadata"),
                    key("name"),
                ],
            ),
            (
                "/items/~[[python]/indent=4]/x",
                vec![key("items"), filter(&["[python]", "indent"], "4"), key("x")],
            ),
            (
                "/items/~[a/b=1]/~[c/d=2]/val",
                vec![
                    key("items"),
                    filter(&["a", "b"], "1"),
                    filter(&["c", "d"], "2"),
                    key("val"),
                ],
            ),
            (
                "/~[k=a=b]c]/next",
                vec![filter(&["k"], "a=b]c"), key("next")],
            ),
            ("/~[a/b=1]/", vec![filter(&["a", "b"], "1"), key("")]),
            ("/~[a//b=1]", vec![filter(&["a", "", "b"], "1")]),
        ];
        for (raw, segments) in test_cases {
            let path = SymbolPath::parse(raw)?;
            assert_eq!(path.segments(), segments, "unexpected segments for {raw}");
            assert_eq!(path.to_string(), raw);
        }
        // `%2F` decodes before splitting, so it separates nested filter key segments too.
        assert_eq!(
            SymbolPath::parse("/items/~[metadata%2Fname=api]/kind")?,
            SymbolPath::parse("/items/~[metadata/name=api]/kind")?,
        );
        Ok(())
    }

    #[test]
    fn compound_filter_parses_conditions_and_displays_as_written() -> anyhow::Result<()> {
        // Chained `[key=value]` brackets combine conditions on one item and display as written.
        let test_cases = [
            (
                "/items/~[kind=Rollout][metadata/name=api]/spec/replicas",
                vec![
                    key("items"),
                    compound_filter(&[(&["kind"], "Rollout"), (&["metadata", "name"], "api")]),
                    key("spec"),
                    key("replicas"),
                ],
            ),
            (
                "/~[a=1][b=2][c/d=3]",
                vec![compound_filter(&[
                    (&["a"], "1"),
                    (&["b"], "2"),
                    (&["c", "d"], "3"),
                ])],
            ),
            (
                "/items/~[kind=Rollout][apiVersion=argoproj.io~1v1alpha1]/spec",
                vec![
                    key("items"),
                    compound_filter(&[
                        (&["kind"], "Rollout"),
                        (&["apiVersion"], "argoproj.io/v1alpha1"),
                    ]),
                    key("spec"),
                ],
            ),
            (
                "/items/~[kind=Rollout][metadata/labels/app.kubernetes.io~1name=api]/spec",
                vec![
                    key("items"),
                    compound_filter(&[
                        (&["kind"], "Rollout"),
                        (&["metadata", "labels", "app.kubernetes.io/name"], "api"),
                    ]),
                    key("spec"),
                ],
            ),
            (
                "/items/~[a=1][b=2]/~[c=3][d=4]/val",
                vec![
                    key("items"),
                    compound_filter(&[(&["a"], "1"), (&["b"], "2")]),
                    compound_filter(&[(&["c"], "3"), (&["d"], "4")]),
                    key("val"),
                ],
            ),
            (
                "/~[k=a=b]c][m=2]/next",
                vec![
                    compound_filter(&[(&["k"], "a=b]c"), (&["m"], "2")]),
                    key("next"),
                ],
            ),
            (
                "/~[k=a]][m=2]",
                vec![compound_filter(&[(&["k"], "a]"), (&["m"], "2")])],
            ),
            (
                "/~[k=a][[python]=2]",
                vec![compound_filter(&[(&["k"], "a"), (&["[python]"], "2")])],
            ),
            (
                "/~[=][=]",
                vec![compound_filter(&[(&[""], ""), (&[""], "")])],
            ),
        ];
        for (raw, segments) in test_cases {
            let path = SymbolPath::parse(raw)?;
            assert_eq!(path.segments(), segments, "unexpected segments for {raw}");
            assert_eq!(path.to_string(), raw);
        }
        Ok(())
    }

    #[test]
    fn filter_value_with_bracket_pair_round_trips_using_percent_5d() -> anyhow::Result<()> {
        // `%5D` encodes a literal `]` before `[` so a value holding `][` is not split into
        // another condition, without double-decoding `%255D`.
        let test_cases = [
            ("/~[k=a%5D[b=1]", vec![filter(&["k"], "a][b=1")]),
            (
                "/~[k=a%5D[x]][m=2]",
                vec![compound_filter(&[(&["k"], "a][x]"), (&["m"], "2")])],
            ),
            ("/~[k=a%255D[b=1]", vec![filter(&["k"], "a%5D[b=1")]),
            ("/~[k=a%25%5D[b]", vec![filter(&["k"], "a%][b")]),
            ("/~[a%3Db=1]", vec![filter(&["a=b"], "1")]),
            ("/~[a%5D[b=1]", vec![filter(&["a][b"], "1")]),
        ];
        for (raw, segments) in test_cases {
            let path = SymbolPath::parse(raw)?;
            assert_eq!(path.segments(), segments, "unexpected segments for {raw}");
            assert_eq!(path.to_string(), raw);
        }
        assert_eq!(
            SymbolPath::parse("/~[k=a%5d[b]")?,
            SymbolPath::parse("/~[k=a%5D[b]")?,
        );
        assert_eq!(
            SymbolPath::parse("/~[a=1]%2Fspec")?,
            SymbolPath::parse("/~[a=1]/spec")?,
        );
        Ok(())
    }

    #[test]
    fn key_that_resembles_a_filter_parses_as_key() -> anyhow::Result<()> {
        // VS Code settings have keys such as `[python]`.
        let path = SymbolPath::parse("/[python]/[a=b]/~0[a=b]")?;
        assert_eq!(path.segments(), keys(&["[python]", "[a=b]", "~[a=b]"]));
        assert_eq!(path.to_string(), "/[python]/[a=b]/~0[a=b]");
        Ok(())
    }

    #[test]
    fn incomplete_filter_parse_fails() {
        // Unclosed brackets, missing `=`, raw `/` in a value, and trailing text after `]` fail.
        for raw in [
            "/~[name=blue",
            "/~[name]",
            "/~[]",
            "/~[metadata/name=api",
            "/~[metadata/name=api/spec",
            "/~[metadata/name]/spec",
            "/~[metadata/name",
            "/items/~[apiVersion=argoproj.io/v1alpha1]/kind",
            "/~[a=1/b=2]",
            "/~[a/b=1]x/next",
            "/~[a=1][]",
            "/~[kind=Rollout][name]",
            "/~[a=1][b=2",
            "/~[a=1][b=2/spec",
            "/~[a=1][b=2]x",
            "/~[kind=Rollout][apiVersion=argoproj.io/v1alpha1]",
            "/~[apiVersion=argoproj.io/v1alpha1][kind=Rollout]",
            "/~[a=1%2F2]",
            "/~[a=1%5D",
            "/~[kind][metadata/name=api]",
            "/~[][a=1]",
            "/~[a=1][][b=2]",
            "/~[kind=Rollout][name][metadata/namespace=prod]",
        ] {
            let err = SymbolPath::parse(raw).unwrap_err();
            assert!(
                err.to_string().contains("invalid filter"),
                "unexpected error message for {raw}: {err}"
            );
        }
        // Error text for a broken later condition keeps the earlier nested key intact.
        let err = SymbolPath::parse("/~[metadata/name=api][bad]/spec").unwrap_err();
        assert!(
            err.to_string().contains("~[metadata/name=api][bad]"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn nested_filter_inside_filter_key_fails_as_invalid_escape() {
        // A nested `~[...]` inside a filter key is rejected as an invalid RFC 6901 escape.
        for raw in ["/~[a/~[b=1]/c=2]", "/~[a/~2=1]", "/~[k=1][a/~[b=1]/c=2]"] {
            let err = SymbolPath::parse(raw).unwrap_err();
            assert!(
                err.to_string().contains("invalid RFC 6901 escape"),
                "unexpected error message for {raw}: {err}"
            );
        }
    }

    #[test]
    fn empty_segments_are_preserved() -> anyhow::Result<()> {
        let path = SymbolPath::parse("/a//b")?;
        assert_eq!(path.segments(), keys(&["a", "", "b"]));
        assert_eq!(path.to_string(), "/a//b");
        Ok(())
    }

    #[test]
    fn root_only_path_yields_single_empty_segment() -> anyhow::Result<()> {
        let path = SymbolPath::parse("/")?;
        assert_eq!(path.segments(), keys(&[""]));
        assert_eq!(path.to_string(), "/");
        Ok(())
    }

    #[test]
    fn different_raw_encodings_compare_equal() -> anyhow::Result<()> {
        let p1 = SymbolPath::parse("/deps/%41")?;
        let p2 = SymbolPath::parse("/deps/A")?;
        assert_eq!(p1, p2);
        assert_eq!(p1.to_string(), "/deps/A");

        let p_hex_upper = SymbolPath::parse("/keys/%2C")?;
        let p_hex_lower = SymbolPath::parse("/keys/%2c")?;
        assert_eq!(p_hex_upper, p_hex_lower);

        let p_hash = SymbolPath::parse("#/a/b")?;
        let p_no_hash = SymbolPath::parse("/a/b")?;
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
            let parsed = SymbolPath::parse(raw)?;
            let formatted = parsed.to_string();
            let reparsed = SymbolPath::parse(&formatted)?;
            assert_eq!(parsed, reparsed, "failed round-trip for {raw}");
        }
        Ok(())
    }

    #[test]
    fn path_with_single_and_double_quotes_display_percent_encoded() -> anyhow::Result<()> {
        let path = SymbolPath::parse("/items/\"double\"/'single'")?;
        assert_eq!(path.segments(), keys(&["items", "\"double\"", "'single'"]));
        assert_eq!(path.to_string(), "/items/%22double%22/%27single%27");

        let reparsed = SymbolPath::parse(&path.to_string())?;
        assert_eq!(reparsed, path);
        Ok(())
    }

    #[test]
    fn non_ascii_key_displays_as_written_except_whitespace_and_controls() -> anyhow::Result<()> {
        let path = SymbolPath::from_segments(vec![
            "café".to_string(),
            "\u{9b}".to_string(),
            "end\u{a0}".to_string(),
        ]);
        assert_eq!(path.to_string(), "/café/%C2%9B/end%C2%A0");

        let reparsed = SymbolPath::parse(&path.to_string())?;
        assert_eq!(reparsed, path);
        Ok(())
    }

    #[test]
    fn key_with_spaces_display_encodes_only_those_at_its_ends() -> anyhow::Result<()> {
        let path = SymbolPath::from_segments(vec![
            "Dark red".to_string(),
            " start".to_string(),
            "end  ".to_string(),
            " ".to_string(),
        ]);
        assert_eq!(path.to_string(), "/Dark red/%20start/end%20%20/%20");

        let reparsed = SymbolPath::parse(&path.to_string())?;
        assert_eq!(reparsed, path);
        Ok(())
    }

    #[test]
    fn valid_segments_provided_from_segments_constructs_rooted_path() {
        let path =
            SymbolPath::from_segments(vec!["dependencies".to_string(), "inngest".to_string()]);
        assert_eq!(path.to_string(), "/dependencies/inngest");
        assert_eq!(path.segments(), keys(&["dependencies", "inngest"]));
    }

    #[test]
    #[should_panic(expected = "a symbol path needs at least one segment")]
    fn empty_segments_provided_from_segments_panics() {
        SymbolPath::from_segments(vec![]);
    }
}
