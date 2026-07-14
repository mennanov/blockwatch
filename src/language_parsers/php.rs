use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers;
use crate::language_parsers::{Comment, CommentsParser, TreeSitterCommentsParser};
use itertools::Itertools;
use tree_sitter::StreamingIterator;

/// Returns a [`BlocksParser`] for PHP.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    Ok(PhpCommentsParser::new())
}

/// Parses PHP comments and the HTML comments found in the template sections of a PHP file.
///
/// tree-sitter-php exposes the HTML sections of a template as opaque `text` nodes, so the HTML
/// comments in them are extracted with a separate HTML grammar pass, mirroring how markdown.rs
/// handles its `html_block` nodes.
struct PhpCommentsParser {
    php_comments_parser: TreeSitterCommentsParser,
    php_tree_sitter_parser: tree_sitter::Parser,
    text_query: tree_sitter::Query,
    html_comments_parser: TreeSitterCommentsParser,
}

impl PhpCommentsParser {
    fn new() -> Self {
        let php_language: tree_sitter::Language = tree_sitter_php::LANGUAGE_PHP.into();
        let php_comments_parser =
            language_parsers::hash_and_c_style_comments_parser(&php_language, "comment");
        let mut php_tree_sitter_parser = tree_sitter::Parser::new();
        php_tree_sitter_parser
            .set_language(&php_language)
            .expect("Error setting Tree-sitter language");
        let text_query = tree_sitter::Query::new(&php_language, "(text) @text").unwrap();
        let html_comments_parser = language_parsers::xml_style_comments_parser(
            &tree_sitter_html::LANGUAGE.into(),
            "comment",
        );
        Self {
            php_comments_parser,
            php_tree_sitter_parser,
            text_query,
            html_comments_parser,
        }
    }

    fn parse_html_comments(&mut self, source_code: &str) -> Vec<Comment> {
        // An HTML comment requires a `<!--`; without one there is nothing to extract, so skip the
        // whole second parse and the view construction for the common (pure-PHP) case.
        if !source_code.contains("<!--") {
            return Vec::new();
        }
        let tree = self
            .php_tree_sitter_parser
            .parse(source_code, None)
            .unwrap();
        let root_node = tree.root_node();
        let mut query_cursor = tree_sitter::QueryCursor::new();
        let mut matches = query_cursor.matches(&self.text_query, root_node, source_code.as_bytes());

        // Build an "HTML view" of the file: every byte of a PHP region is blanked to a space
        // (line breaks kept, so rows and columns still line up), while the `text` nodes — the
        // HTML template sections — are copied verbatim. An HTML comment interrupted by a
        // `<?php ?>` island therefore reforms into one comment with the island as interior
        // whitespace, and, because the view is byte-for-byte aligned with the source, the parsed
        // comment positions are already the source positions (no offsetting needed).
        let mut html_view: Vec<u8> = source_code
            .bytes()
            .map(|b| if b == b'\n' || b == b'\r' { b } else { b' ' })
            .collect();
        while let Some(query_match) = matches.next() {
            let range = query_match
                .captures
                .first()
                .expect("Empty Tree-sitter text query match")
                .node
                .byte_range();
            html_view[range.clone()].copy_from_slice(&source_code.as_bytes()[range]);
        }
        let html_view =
            String::from_utf8(html_view).expect("HTML view is built from the valid-UTF-8 source");

        self.html_comments_parser.parse(&html_view).collect()
    }
}

impl CommentsParser for PhpCommentsParser {
    fn parse<'source>(
        &'source mut self,
        source_code: &'source str,
    ) -> impl Iterator<Item = Comment> + 'source {
        let html_comments = self.parse_html_comments(source_code);
        self.php_comments_parser
            .parse(source_code)
            .merge_by(html_comments, |a, b| {
                a.source_range.start <= b.source_range.start
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_html_comments_in_template_sections_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"<?php
$colors = ['blue']; // php comment
?><!-- same-line html comment -->
<!-- multi-line html comment
spanning two lines -->
<ul>
    <li>blue</li>
</ul>
<!-- html comment before php -->
<?php # trailing php comment ?>
"#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 21)..Position::new(2, 35),
                    source_range: 26..40,
                    comment_text: "   php comment".to_string()
                },
                Comment {
                    position_range: Position::new(3, 3)..Position::new(3, 34),
                    source_range: 43..74,
                    comment_text: "     same-line html comment    ".to_string()
                },
                Comment {
                    position_range: Position::new(4, 1)..Position::new(5, 23),
                    source_range: 75..126,
                    comment_text: "     multi-line html comment\nspanning two lines    "
                        .to_string()
                },
                Comment {
                    position_range: Position::new(9, 1)..Position::new(9, 33),
                    source_range: 156..188,
                    comment_text: "     html comment before php    ".to_string()
                },
                Comment {
                    position_range: Position::new(10, 7)..Position::new(10, 30),
                    source_range: 195..218,
                    comment_text: "  trailing php comment ".to_string()
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn extracts_html_comment_split_by_a_php_island() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let comments: Vec<Comment> = comments_parser
            .parse("<ul>\n<!-- <block name=\"x\"> <?php echo 1; ?> </block>-->\n</ul>\n")
            .collect();

        assert_eq!(
            comments,
            vec![Comment {
                position_range: Position::new(2, 1)..Position::new(2, 51),
                source_range: 5..55,
                comment_text: "     <block name=\"x\">                  </block>   ".to_string()
            }]
        );

        Ok(())
    }

    #[test]
    fn parses_php_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"<?php
            // This is a single-line comment in PHP
    
            /*
             * This is a multi-line comment.
             * It spans multiple lines in PHP.
             */
    
            function main() {
                echo "Hello, PHP!"; # Prints a message to the console.
    
                /* Another comment
                 * split into
                 * multiple lines.
                 */
                 
                return 0;
            }
            ?>
            <h1>This is an <?php # inlined comment ?> example</h1>
            <p>The header above will say 'This is an  example'.</p>
<?php
/// Triple slash comment.
?>
            "#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 13)..Position::new(2, 52),
                    source_range: 18..57,
                    comment_text: "   This is a single-line comment in PHP".to_string()
                },
                Comment {
                    position_range: Position::new(4, 13)..Position::new(7, 16),
                    source_range: 75..185,
                    comment_text:
                        "  \n               This is a multi-line comment.\n               It spans multiple lines in PHP.\n               "
                            .to_string()
                },
                Comment {
                    position_range: Position::new(10, 37)..Position::new(10, 71),
                    source_range: 257..291,
                    comment_text: "  Prints a message to the console.".to_string()
                },
                Comment {
                    position_range: Position::new(12, 17)..Position::new(15, 20),
                    source_range: 313..416,
                    comment_text: "   Another comment\n                   split into\n                   multiple lines.\n                   ".to_string()
                },
                Comment {
                    position_range: Position::new(20, 34)..Position::new(20, 52),
                    source_range: 523..541,
                    comment_text: "  inlined comment ".to_string()
                },
                Comment {
                    position_range: Position::new(23, 1)..Position::new(23, 26),
                    source_range: 631..656,
                    comment_text: "  / Triple slash comment.".to_string()
                },
            ]
        );

        Ok(())
    }
}
