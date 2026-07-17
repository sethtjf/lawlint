//! Document tree types (skeleton — complete) + parsing (agent A).
//! Types verbatim from docs/engine-design.md §3.
//!
//! Every node's range indexes the ORIGINAL source. Never normalize text in
//! place.

use crate::types::TextRange;

#[derive(Debug, Clone)]
pub struct Document {
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone)]
pub struct Block {
    pub kind: BlockKind,
    pub range: TextRange,
    pub sentences: Vec<Sentence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading,
    ListItem,
    BlockQuote,
    CodeBlock,
}

#[derive(Debug, Clone)]
pub struct Sentence {
    pub range: TextRange,
    pub tokens: Vec<Token>,
    pub is_citation: bool,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub range: TextRange,
    pub kind: TokenKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Word,
    Number,
    Punct,
}

/// Parse `source` into a document tree. [agent A]
///
/// - `markdown=false`: blocks are paragraphs split on blank lines, all
///   `Paragraph`.
/// - `markdown=true`: use `pulldown-cmark` with `OffsetIter` to classify
///   blocks (heading, list item, block quote, fenced/indented code). Code
///   blocks get **no sentences** (never linted except `Scope::All`).
pub fn parse(source: &str, markdown: bool) -> Document {
    let raw = if markdown {
        crate::markdown::markdown_blocks(source)
    } else {
        crate::markdown::plain_blocks(source)
    };
    let blocks = raw
        .into_iter()
        .map(|(kind, range)| {
            let sentences = if kind == BlockKind::CodeBlock {
                Vec::new() // code is never segmented; linted only via Scope::All
            } else {
                crate::segment::split_sentences(source, range)
            };
            Block {
                kind,
                range,
                sentences,
            }
        })
        .collect();
    Document { blocks }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_parse_paragraphs_and_sentences() {
        let src = "The claim fails. It is dismissed.\n\nSee Roe v. Wade, 410 U.S. 113 (1973). The court held that this applies.";
        let doc = parse(src, false);
        assert_eq!(doc.blocks.len(), 2);
        assert!(doc.blocks.iter().all(|b| b.kind == BlockKind::Paragraph));
        assert_eq!(doc.blocks[0].sentences.len(), 2);
        assert_eq!(doc.blocks[1].sentences.len(), 2);
        assert!(doc.blocks[1].sentences[0].is_citation);
        assert_eq!(
            doc.blocks[1].sentences[0].range.slice(src),
            "See Roe v. Wade, 410 U.S. 113 (1973)."
        );
    }

    #[test]
    fn markdown_parse_classifies_blocks_and_skips_code_sentences() {
        let src = "# Heading One\n\nA paragraph. Another sentence.\n\n- item one\n- item two\n\n> Quoted text here.\n\n```\nnot. a. sentence.\n```\n";
        let doc = parse(src, true);
        let kinds: Vec<BlockKind> = doc.blocks.iter().map(|b| b.kind).collect();
        assert_eq!(
            kinds,
            vec![
                BlockKind::Heading,
                BlockKind::Paragraph,
                BlockKind::ListItem,
                BlockKind::ListItem,
                BlockKind::BlockQuote,
                BlockKind::CodeBlock,
            ]
        );
        assert_eq!(doc.blocks[1].sentences.len(), 2);
        // Code blocks get NO sentences.
        let code = doc
            .blocks
            .iter()
            .find(|b| b.kind == BlockKind::CodeBlock)
            .unwrap();
        assert!(code.sentences.is_empty());
        // Non-code blocks all segmented.
        assert!(doc.blocks[0].sentences.len() == 1);
        assert!(doc.blocks[4].sentences.len() == 1);
    }

    #[test]
    fn markdown_false_treats_markers_as_text() {
        let src = "# Not a heading in plain mode";
        let doc = parse(src, false);
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].kind, BlockKind::Paragraph);
        assert_eq!(doc.blocks[0].range.slice(src), src);
    }

    #[test]
    fn all_ranges_reproduce_original_source_including_multibyte() {
        let src = "# Café—Ruling\n\nThe naïve party’s brief was filed. “Justice delayed is justice denied.”\n\n- première point\n\n```\nlet s = \"—\";\n```\n";
        for markdown in [false, true] {
            let doc = parse(src, markdown);
            assert!(!doc.blocks.is_empty());
            for b in &doc.blocks {
                // Slicing must not panic: every range on a char boundary of
                // the ORIGINAL source.
                let block_text = b.range.slice(src);
                assert!(!block_text.is_empty());
                for s in &b.sentences {
                    let sent_text = s.range.slice(src);
                    assert!(!sent_text.is_empty());
                    assert!(b.range.contains(&s.range), "sentence outside block");
                    for t in &s.tokens {
                        let tok_text = t.range.slice(src);
                        assert!(!tok_text.is_empty());
                        assert!(s.range.contains(&t.range), "token outside sentence");
                    }
                }
            }
        }
        // Spot-check exact reproduction through nested ranges.
        let doc = parse(src, true);
        let para = doc
            .blocks
            .iter()
            .find(|b| b.kind == BlockKind::Paragraph)
            .unwrap();
        assert_eq!(para.sentences.len(), 2);
        assert_eq!(
            para.sentences[0].range.slice(src),
            "The naïve party’s brief was filed."
        );
        assert_eq!(
            para.sentences[1].range.slice(src),
            "“Justice delayed is justice denied.”"
        );
        let words: Vec<&str> = para.sentences[0]
            .tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Word)
            .map(|t| t.range.slice(src))
            .collect();
        assert_eq!(
            words,
            vec!["The", "naïve", "party’s", "brief", "was", "filed"]
        );
    }

    #[test]
    fn empty_source_parses_to_empty_document() {
        assert!(parse("", false).blocks.is_empty());
        assert!(parse("", true).blocks.is_empty());
        assert!(parse("\n \n", false).blocks.is_empty());
    }
}
