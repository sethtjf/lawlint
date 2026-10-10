// Ported from crates/lawlint-core/src/markdown.rs #[cfg(test)].

import { describe, expect, test } from "bun:test";
import type { BlockKind } from "../src/document";
import { markdownBlocks, plainBlocks } from "../src/markdown";
import { Src } from "../src/utf8";

function slices(source: string): [BlockKind, string][] {
  const src = new Src(source);
  return markdownBlocks(src).map(([k, r]) => [k, src.slice(r.start, r.end)]);
}
function blocks(source: string) {
  return markdownBlocks(new Src(source));
}
function plain(source: string): [BlockKind, string][] {
  const src = new Src(source);
  return plainBlocks(src).map(([k, r]) => [k, src.slice(r.start, r.end)]);
}

// ---- plain_blocks --------------------------------------------------------

test("plain_splits_on_blank_lines", () => {
  const src = "First paragraph line one.\nLine two.\n\nSecond paragraph.\n\n\nThird.";
  const b = plain(src);
  expect(b.length).toBe(3);
  expect(b.every(([k]) => k === "paragraph")).toBe(true);
  expect(b[0][1]).toBe("First paragraph line one.\nLine two.");
  expect(b[1][1]).toBe("Second paragraph.");
  expect(b[2][1]).toBe("Third.");
});

test("plain_treats_whitespace_only_lines_as_blank", () => {
  const b = plain("One.\n   \t \nTwo.");
  expect(b.length).toBe(2);
  expect(b[0][1]).toBe("One.");
  expect(b[1][1]).toBe("Two.");
});

test("plain_trims_leading_and_trailing_whitespace", () => {
  const b = plain("   indented start.  \n");
  expect(b.length).toBe(1);
  expect(b[0][1]).toBe("indented start.");
});

test("plain_handles_crlf", () => {
  const b = plain("One.\r\n\r\nTwo.\r\n");
  expect(b.length).toBe(2);
  expect(b[0][1]).toBe("One.");
  expect(b[1][1]).toBe("Two.");
});

test("plain_empty_source", () => {
  expect(plain("")).toHaveLength(0);
  expect(plain("\n\n  \n")).toHaveLength(0);
});

// ---- markdown_blocks -----------------------------------------------------

test("heading_and_paragraph", () => {
  expect(slices("# The Title\n\nBody paragraph here.\n")).toEqual([
    ["heading", "The Title"],
    ["paragraph", "Body paragraph here."],
  ]);
});

test("tight_list_items", () => {
  expect(slices("- first point\n- second point\n")).toEqual([
    ["listitem", "first point"],
    ["listitem", "second point"],
  ]);
});

test("loose_list_items_with_paragraphs", () => {
  expect(slices("1. first item\n\n2. second item\n")).toEqual([
    ["listitem", "first item"],
    ["listitem", "second item"],
  ]);
});

test("block_quote", () => {
  expect(slices("> Quoted holding of the court.\n")).toEqual([
    ["blockquote", "Quoted holding of the court."],
  ]);
});

test("fenced_code_block_keeps_full_range", () => {
  const src = "Intro text.\n\n```rust\nlet x = 1;\n```\n\nOutro.\n";
  const b = blocks(src);
  const s = new Src(src);
  expect(b.length).toBe(3);
  expect(b[0][0]).toBe("paragraph");
  expect(b[1][0]).toBe("codeblock");
  expect(s.slice(b[1][1].start, b[1][1].end)).toContain("let x = 1;");
  expect(s.slice(b[1][1].start, b[1][1].end).startsWith("```")).toBe(true);
  expect(b[2][0]).toBe("paragraph");
  expect(s.slice(b[2][1].start, b[2][1].end)).toBe("Outro.");
});

test("indented_code_block", () => {
  const src = "Para.\n\n    let indented = true;\n\nAfter.\n";
  const b = blocks(src);
  const s = new Src(src);
  expect(b.map(([k]) => k)).toEqual(["paragraph", "codeblock", "paragraph"]);
  expect(s.slice(b[1][1].start, b[1][1].end)).toContain("let indented = true;");
});

test("nested_list_inside_quote_classifies_innermost", () => {
  expect(slices("> - nested point\n")).toEqual([["listitem", "nested point"]]);
});

test("quote_inside_list_classifies_innermost", () => {
  expect(slices("- item\n  > inner quote\n")).toEqual([
    ["listitem", "item"],
    ["blockquote", "inner quote"],
  ]);
});

test("inline_emphasis_and_links_stay_in_one_block", () => {
  const src = "This has *emphasis* and a [link](https://example.com) inline.\n";
  const b = blocks(src);
  const s = new Src(src);
  expect(b.length).toBe(1);
  expect(b[0][0]).toBe("paragraph");
  expect(s.slice(b[0][1].start, b[0][1].end)).toBe(
    "This has *emphasis* and a [link](https://example.com) inline.",
  );
});

test("multiline_paragraph_is_one_block", () => {
  const src = "Line one continues\nonto line two.\n\nNext.\n";
  const b = blocks(src);
  const s = new Src(src);
  expect(b.length).toBe(2);
  expect(s.slice(b[0][1].start, b[0][1].end)).toBe("Line one continues\nonto line two.");
});

test("heading_inside_blockquote_is_heading", () => {
  expect(slices("> # Quoted Heading\n")).toEqual([["heading", "Quoted Heading"]]);
});

test("multibyte_utf8_offsets", () => {
  const src = "# Café—Ruling\n\nThe naïve—“quoted”—text.\n";
  const b = blocks(src);
  const s = new Src(src);
  expect(b.length).toBe(2);
  expect(s.slice(b[0][1].start, b[0][1].end)).toBe("Café—Ruling");
  expect(s.slice(b[1][1].start, b[1][1].end)).toBe("The naïve—“quoted”—text.");
});

test("thematic_break_and_html_block_produce_no_text_blocks", () => {
  const src = "Before.\n\n---\n\n<div>\nraw html\n</div>\n\nAfter.\n";
  const b = blocks(src);
  const s = new Src(src);
  expect(b.map(([k]) => k)).toEqual(["paragraph", "paragraph"]);
  expect(s.slice(b[0][1].start, b[0][1].end)).toBe("Before.");
  expect(s.slice(b[1][1].start, b[1][1].end)).toBe("After.");
});

test("gfm_tables_produce_no_text_blocks", () => {
  const src =
    "Before.\n\n| col one | col two |\n|---------|---------|\n| a1      | b1      |\n| a2      | b2      |\n| a3      | b3      |\n| a4      | b4      |\n\nAfter.\n";
  const b = blocks(src);
  const s = new Src(src);
  expect(b.map(([k]) => k)).toEqual(["paragraph", "paragraph"]);
  expect(s.slice(b[0][1].start, b[0][1].end)).toBe("Before.");
  expect(s.slice(b[1][1].start, b[1][1].end)).toBe("After.");
});

test("blocks_are_in_source_order", () => {
  const src = "# H\n\npara one\n\n- li\n\n> q\n\n```\ncode\n```\n";
  const b = blocks(src);
  const starts = b.map(([, r]) => r.start);
  expect([...starts].sort((a, c) => a - c)).toEqual(starts);
  expect(b.map(([k]) => k)).toEqual([
    "heading",
    "paragraph",
    "listitem",
    "blockquote",
    "codeblock",
  ]);
});
