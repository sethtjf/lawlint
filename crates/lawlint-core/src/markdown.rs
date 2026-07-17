//! Markdown block structure via pulldown-cmark. [agent A]
//!
//! Uses `pulldown_cmark::Parser::new(...).into_offset_iter()` to classify
//! blocks (heading, list item, block quote, fenced/indented code) with byte
//! ranges into the ORIGINAL source. Code blocks get no sentences.
//!
//! Text-bearing blocks are built from the ranges of their *inline* events,
//! so markers (`# `, `- `, `> `, fences) stay outside the linted range while
//! every offset still indexes the original source. Code blocks keep their
//! full container range (fences included) so `Scope::All` masking covers the
//! whole construct.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::document::BlockKind;
use crate::types::TextRange;

/// Container context while walking events; the innermost container decides
/// how a paragraph (or tight-item inline run) is classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    Item,
    Quote,
}

fn classify(containers: &[Container]) -> BlockKind {
    match containers.last() {
        Some(Container::Item) => BlockKind::ListItem,
        Some(Container::Quote) => BlockKind::BlockQuote,
        None => BlockKind::Paragraph,
    }
}

/// An open text block being accumulated: (kind, start, end). `start` begins
/// at `usize::MAX` so the first inline event snaps it to its real offset.
type Open = Option<(BlockKind, usize, usize)>;

fn flush(current: &mut Open, blocks: &mut Vec<(BlockKind, TextRange)>) {
    if let Some((kind, s, e)) = current.take() {
        if e > s {
            blocks.push((kind, TextRange { start: s, end: e }));
        }
    }
}

fn extend(current: &mut Open, containers: &[Container], range: std::ops::Range<usize>) {
    match current {
        Some((_, s, e)) => {
            *s = (*s).min(range.start);
            *e = (*e).max(range.end);
        }
        // Tight list items (and any other bare inline run) have no Paragraph
        // event: open an implicit block classified by container context.
        None => *current = Some((classify(containers), range.start, range.end)),
    }
}

/// Extract classified block ranges from markdown source. Ranges are absolute
/// byte offsets into `source`. [agent A]
pub fn markdown_blocks(source: &str) -> Vec<(BlockKind, TextRange)> {
    let mut blocks: Vec<(BlockKind, TextRange)> = Vec::new();
    let mut containers: Vec<Container> = Vec::new();
    let mut current: Open = None;
    let mut code_depth = 0usize;
    let mut table_depth = 0usize;

    // ENABLE_TABLES so GFM pipe tables emit Table events (and are excluded
    // below) instead of parsing as one giant terminator-free Paragraph that
    // trips sentence-length and phrase rules on table syntax.
    for (event, range) in Parser::new_ext(source, Options::ENABLE_TABLES).into_offset_iter() {
        if code_depth > 0 {
            // Swallow everything inside a code block; its full range was
            // already recorded at Start.
            match event {
                Event::Start(Tag::CodeBlock(_)) => code_depth += 1,
                Event::End(TagEnd::CodeBlock) => code_depth -= 1,
                _ => {}
            }
            continue;
        }
        if table_depth > 0 {
            // Tables are not prose: swallow head/rows/cells entirely, like
            // HTML blocks and thematic breaks they produce no text blocks.
            match event {
                Event::Start(Tag::Table(_)) => table_depth += 1,
                Event::End(TagEnd::Table) => table_depth -= 1,
                _ => {}
            }
            continue;
        }
        match event {
            Event::Start(tag) => match tag {
                Tag::Table(_) => {
                    flush(&mut current, &mut blocks);
                    table_depth += 1;
                }
                Tag::CodeBlock(_) => {
                    flush(&mut current, &mut blocks);
                    blocks.push((
                        BlockKind::CodeBlock,
                        TextRange {
                            start: range.start,
                            end: range.end,
                        },
                    ));
                    code_depth += 1;
                }
                Tag::Heading { .. } => {
                    flush(&mut current, &mut blocks);
                    current = Some((BlockKind::Heading, usize::MAX, 0));
                }
                Tag::Paragraph => {
                    flush(&mut current, &mut blocks);
                    current = Some((classify(&containers), usize::MAX, 0));
                }
                Tag::Item => {
                    flush(&mut current, &mut blocks);
                    containers.push(Container::Item);
                }
                Tag::BlockQuote(_) => {
                    flush(&mut current, &mut blocks);
                    containers.push(Container::Quote);
                }
                Tag::List(_) | Tag::HtmlBlock => flush(&mut current, &mut blocks),
                // Inline containers: their range covers the whole element.
                Tag::Emphasis
                | Tag::Strong
                | Tag::Strikethrough
                | Tag::Link { .. }
                | Tag::Image { .. } => extend(&mut current, &containers, range),
                // Extensions we don't enable (footnote definitions, metadata,
                // definition lists…): treat as block boundaries.
                _ => flush(&mut current, &mut blocks),
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Heading(_) | TagEnd::Paragraph => flush(&mut current, &mut blocks),
                TagEnd::Item => {
                    flush(&mut current, &mut blocks);
                    containers.pop();
                }
                TagEnd::BlockQuote(_) => {
                    flush(&mut current, &mut blocks);
                    containers.pop();
                }
                TagEnd::List(_) | TagEnd::HtmlBlock => flush(&mut current, &mut blocks),
                TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Link
                | TagEnd::Image => extend(&mut current, &containers, range),
                TagEnd::CodeBlock => {} // handled by code_depth branch
                _ => flush(&mut current, &mut blocks),
            },
            // Inline content: accumulate into the current (possibly implicit)
            // text block.
            Event::Text(_)
            | Event::Code(_)
            | Event::InlineHtml(_)
            | Event::SoftBreak
            | Event::HardBreak
            | Event::FootnoteReference(_)
            | Event::TaskListMarker(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_) => extend(&mut current, &containers, range),
            // Block-level HTML is not prose.
            Event::Html(_) => {}
            Event::Rule => flush(&mut current, &mut blocks),
        }
    }
    flush(&mut current, &mut blocks);
    blocks
}

/// Plain-text fallback: paragraphs split on blank lines, all
/// `BlockKind::Paragraph`. Ranges are trimmed to the paragraph's first and
/// last non-whitespace bytes. [agent A]
pub fn plain_blocks(source: &str) -> Vec<(BlockKind, TextRange)> {
    let mut blocks = Vec::new();
    let mut pos = 0usize;
    let mut block_start: Option<usize> = None;
    let mut block_end = 0usize;
    for line in source.split_inclusive('\n') {
        let line_start = pos;
        pos += line.len();
        if line.trim().is_empty() {
            if let Some(s) = block_start.take() {
                blocks.push((
                    BlockKind::Paragraph,
                    TextRange {
                        start: s,
                        end: block_end,
                    },
                ));
            }
            continue;
        }
        let content_start = line_start + (line.len() - line.trim_start().len());
        let content_end = line_start + line.trim_end().len();
        if block_start.is_none() {
            block_start = Some(content_start);
        }
        block_end = content_end;
    }
    if let Some(s) = block_start {
        blocks.push((
            BlockKind::Paragraph,
            TextRange {
                start: s,
                end: block_end,
            },
        ));
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slices(source: &str) -> Vec<(BlockKind, String)> {
        markdown_blocks(source)
            .into_iter()
            .map(|(k, r)| (k, r.slice(source).to_string()))
            .collect()
    }

    // ---- plain_blocks ----------------------------------------------------

    #[test]
    fn plain_splits_on_blank_lines() {
        let src = "First paragraph line one.\nLine two.\n\nSecond paragraph.\n\n\nThird.";
        let blocks = plain_blocks(src);
        assert_eq!(blocks.len(), 3);
        assert!(blocks.iter().all(|(k, _)| *k == BlockKind::Paragraph));
        assert_eq!(
            blocks[0].1.slice(src),
            "First paragraph line one.\nLine two."
        );
        assert_eq!(blocks[1].1.slice(src), "Second paragraph.");
        assert_eq!(blocks[2].1.slice(src), "Third.");
    }

    #[test]
    fn plain_treats_whitespace_only_lines_as_blank() {
        let src = "One.\n   \t \nTwo.";
        let blocks = plain_blocks(src);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].1.slice(src), "One.");
        assert_eq!(blocks[1].1.slice(src), "Two.");
    }

    #[test]
    fn plain_trims_leading_and_trailing_whitespace() {
        let src = "   indented start.  \n";
        let blocks = plain_blocks(src);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].1.slice(src), "indented start.");
    }

    #[test]
    fn plain_handles_crlf() {
        let src = "One.\r\n\r\nTwo.\r\n";
        let blocks = plain_blocks(src);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].1.slice(src), "One.");
        assert_eq!(blocks[1].1.slice(src), "Two.");
    }

    #[test]
    fn plain_empty_source() {
        assert!(plain_blocks("").is_empty());
        assert!(plain_blocks("\n\n  \n").is_empty());
    }

    // ---- markdown_blocks -------------------------------------------------

    #[test]
    fn heading_and_paragraph() {
        let src = "# The Title\n\nBody paragraph here.\n";
        assert_eq!(
            slices(src),
            vec![
                (BlockKind::Heading, "The Title".to_string()),
                (BlockKind::Paragraph, "Body paragraph here.".to_string()),
            ]
        );
    }

    #[test]
    fn tight_list_items() {
        let src = "- first point\n- second point\n";
        assert_eq!(
            slices(src),
            vec![
                (BlockKind::ListItem, "first point".to_string()),
                (BlockKind::ListItem, "second point".to_string()),
            ]
        );
    }

    #[test]
    fn loose_list_items_with_paragraphs() {
        let src = "1. first item\n\n2. second item\n";
        assert_eq!(
            slices(src),
            vec![
                (BlockKind::ListItem, "first item".to_string()),
                (BlockKind::ListItem, "second item".to_string()),
            ]
        );
    }

    #[test]
    fn block_quote() {
        let src = "> Quoted holding of the court.\n";
        assert_eq!(
            slices(src),
            vec![(
                BlockKind::BlockQuote,
                "Quoted holding of the court.".to_string()
            )]
        );
    }

    #[test]
    fn fenced_code_block_keeps_full_range() {
        let src = "Intro text.\n\n```rust\nlet x = 1;\n```\n\nOutro.\n";
        let blocks = markdown_blocks(src);
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].0, BlockKind::Paragraph);
        assert_eq!(blocks[1].0, BlockKind::CodeBlock);
        assert!(blocks[1].1.slice(src).contains("let x = 1;"));
        assert!(blocks[1].1.slice(src).starts_with("```"));
        assert_eq!(blocks[2].0, BlockKind::Paragraph);
        assert_eq!(blocks[2].1.slice(src), "Outro.");
    }

    #[test]
    fn indented_code_block() {
        let src = "Para.\n\n    let indented = true;\n\nAfter.\n";
        let blocks = markdown_blocks(src);
        let kinds: Vec<BlockKind> = blocks.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                BlockKind::Paragraph,
                BlockKind::CodeBlock,
                BlockKind::Paragraph
            ]
        );
        assert!(blocks[1].1.slice(src).contains("let indented = true;"));
    }

    #[test]
    fn nested_list_inside_quote_classifies_innermost() {
        let src = "> - nested point\n";
        assert_eq!(
            slices(src),
            vec![(BlockKind::ListItem, "nested point".to_string())]
        );
    }

    #[test]
    fn quote_inside_list_classifies_innermost() {
        let src = "- item\n  > inner quote\n";
        assert_eq!(
            slices(src),
            vec![
                (BlockKind::ListItem, "item".to_string()),
                (BlockKind::BlockQuote, "inner quote".to_string()),
            ]
        );
    }

    #[test]
    fn inline_emphasis_and_links_stay_in_one_block() {
        let src = "This has *emphasis* and a [link](https://example.com) inline.\n";
        let blocks = markdown_blocks(src);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].0, BlockKind::Paragraph);
        assert_eq!(
            blocks[0].1.slice(src),
            "This has *emphasis* and a [link](https://example.com) inline."
        );
    }

    #[test]
    fn multiline_paragraph_is_one_block() {
        let src = "Line one continues\nonto line two.\n\nNext.\n";
        let blocks = markdown_blocks(src);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].1.slice(src), "Line one continues\nonto line two.");
    }

    #[test]
    fn heading_inside_blockquote_is_heading() {
        let src = "> # Quoted Heading\n";
        assert_eq!(
            slices(src),
            vec![(BlockKind::Heading, "Quoted Heading".to_string())]
        );
    }

    #[test]
    fn multibyte_utf8_offsets() {
        let src = "# Café—Ruling\n\nThe naïve—“quoted”—text.\n";
        let blocks = markdown_blocks(src);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].1.slice(src), "Café—Ruling");
        assert_eq!(blocks[1].1.slice(src), "The naïve—“quoted”—text.");
    }

    #[test]
    fn thematic_break_and_html_block_produce_no_text_blocks() {
        let src = "Before.\n\n---\n\n<div>\nraw html\n</div>\n\nAfter.\n";
        let blocks = markdown_blocks(src);
        let kinds: Vec<BlockKind> = blocks.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, vec![BlockKind::Paragraph, BlockKind::Paragraph]);
        assert_eq!(blocks[0].1.slice(src), "Before.");
        assert_eq!(blocks[1].1.slice(src), "After.");
    }

    #[test]
    fn gfm_tables_produce_no_text_blocks() {
        // Regression: without ENABLE_TABLES the whole table parsed as ONE
        // terminator-free Paragraph spanning every row — a 56-word "sentence"
        // that tripped core/sentence-length and fed table syntax to phrase
        // and density rules.
        let src = "Before.\n\n\
                   | col one | col two |\n\
                   |---------|---------|\n\
                   | a1      | b1      |\n\
                   | a2      | b2      |\n\
                   | a3      | b3      |\n\
                   | a4      | b4      |\n\n\
                   After.\n";
        let blocks = markdown_blocks(src);
        let kinds: Vec<BlockKind> = blocks.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, vec![BlockKind::Paragraph, BlockKind::Paragraph]);
        assert_eq!(blocks[0].1.slice(src), "Before.");
        assert_eq!(blocks[1].1.slice(src), "After.");
    }

    #[test]
    fn blocks_are_in_source_order() {
        let src = "# H\n\npara one\n\n- li\n\n> q\n\n```\ncode\n```\n";
        let blocks = markdown_blocks(src);
        let starts: Vec<usize> = blocks.iter().map(|(_, r)| r.start).collect();
        let mut sorted = starts.clone();
        sorted.sort_unstable();
        assert_eq!(starts, sorted);
        let kinds: Vec<BlockKind> = blocks.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                BlockKind::Heading,
                BlockKind::Paragraph,
                BlockKind::ListItem,
                BlockKind::BlockQuote,
                BlockKind::CodeBlock,
            ]
        );
    }
}
