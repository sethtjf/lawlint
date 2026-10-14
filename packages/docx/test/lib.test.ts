// Ported from crates/lawlint-docx/src/lib.rs #[cfg(test)].

import { expect, test } from "bun:test";
import type { Diagnostic } from "@lawlint/core";
import { unzipSync } from "fflate";
import { applyTrackedChanges, defaultReviseOptions } from "../src/index";
import { Package, strToBytes } from "../src/pkg";
import type { Projection } from "../src/project";
import { type Ids, apply } from "../src/revise";
import { selectEdits } from "../src/select";

function diag(start: number, end: number, replacement: string, message: string): Diagnostic {
  return {
    ruleId: "core/x",
    severity: "warning",
    tier: "static",
    intent: "style",
    span: { start, end },
    message,
    line: 0,
    column: 0,
    excerpt: "",
    fix: {
      edits: [{ range: { start, end }, replacement }],
      applicability: "machine-applicable",
    },
  };
}

function ids(): Ids {
  return { nextRev: 900_000, nextComment: 1 };
}

/** Three contiguous runs: [0,5) [5,10) [10,15). */
function threeRuns(): Projection {
  return {
    text: "x".repeat(15),
    byteLen: 15,
    runs: [
      { ordinal: 0, start: 0, end: 5 },
      { ordinal: 1, start: 5, end: 10 },
      { ordinal: 2, start: 10, end: 15 },
    ],
  };
}

test("select_edits_allows_overlapping_annotations", () => {
  // Both spans sit in run 0 and overlap; the first wins the redline, the
  // second downgrades to a comment-only anchor rather than being dropped.
  const projection = threeRuns();
  const diags = [diag(0, 3, "A", "first"), diag(1, 4, "B", "second")];
  const sel = selectEdits(projection, diags, defaultReviseOptions(), ids());

  expect(sel.skipped).toBe(0);
  const pieces = sel.editsByOrdinal.get(0);
  expect(pieces?.length).toBe(2);
  expect(pieces?.some((p) => p.replacement === "A")).toBe(true);
  expect(pieces?.some((p) => p.replacement === null && p.message === "second")).toBe(true);
  expect(pieces?.[0].commentId).not.toBe(pieces?.[1].commentId);
});

test("select_edits_downgrades_multi_run_fix_to_split_comment", () => {
  // Span [2,12) starts in run 0 and ends in run 2: a redline can't cross
  // runs, so this becomes a two-piece comment-only anchor.
  const projection = threeRuns();
  const diags = [diag(2, 12, "Y", "spans runs")];
  const sel = selectEdits(projection, diags, defaultReviseOptions(), ids());

  expect(sel.skipped).toBe(0);
  const opens = sel.editsByOrdinal.get(0);
  const closes = sel.editsByOrdinal.get(2);
  expect(opens?.length).toBe(1);
  expect(closes?.length).toBe(1);
  expect(opens?.[0].opensHere).toBe(true);
  expect(opens?.[0].closesHere).toBe(false);
  expect(closes?.[0].opensHere).toBe(false);
  expect(closes?.[0].closesHere).toBe(true);
  expect(opens?.[0].replacement).toBeNull();
  expect(closes?.[0].replacement).toBeNull();
  expect(opens?.[0].commentId).toBe(closes?.[0].commentId);
  // Run 1 (the middle run) gets no entry at all.
  expect(sel.editsByOrdinal.has(1)).toBe(false);
});

test("select_edits_skips_span_not_contained_in_any_run", () => {
  const projection = threeRuns();
  // Open a gap between run 0 and run 1 (e.g. a tab/paragraph break).
  projection.runs[1].start = 7;
  const diags = [diag(5, 6, "Z", "in the gap")];
  const sel = selectEdits(projection, diags, defaultReviseOptions(), ids());

  expect(sel.skipped).toBe(1);
  expect(sel.editsByOrdinal.size).toBe(0);
});

test("accounting_reconciles_across_overlap_and_multi_run_candidates", () => {
  // A mix of: an ordinary single-run fix, an overlap loser downgraded to a
  // comment, and a multi-run fix downgraded to a split comment.
  const inner =
    "<w:p><w:r><w:t>xxxxx</w:t></w:r>" +
    "<w:r><w:t>xxxxx</w:t></w:r>" +
    "<w:r><w:t>xxxxx</w:t></w:r></w:p>";
  const xml = `<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>${inner}</w:body></w:document>`;
  const projection = threeRuns();
  const diags = [
    diag(0, 3, "A", "ordinary fix"), // run 0, applied
    diag(1, 4, "B", "overlap loser"), // run 0, downgraded
    diag(2, 12, "Y", "multi-run fix"), // runs 0..2, downgraded (2 pieces)
  ];
  const ids0 = ids();
  const sel = selectEdits(projection, diags, defaultReviseOptions(), ids0);
  expect(sel.skipped).toBe(0);

  const out = apply(xml, sel.editsByOrdinal, "lawlint", null, ids0);
  expect(out.skipped).toBe(0);
  const applied = out.comments.length - out.annotated;
  // One redline applied (the first fix); two comment-only findings
  // (overlap loser + multi-run fix) annotated.
  expect(applied).toBe(1);
  expect(out.annotated).toBe(2);
  expect(out.comments.length).toBe(applied + out.annotated);
});

const W_NS = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const DOC_XML = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="${W_NS}"><w:body><w:p><w:r><w:t>delve into it</w:t></w:r></w:p></w:body></w:document>`;
const CONTENT_TYPES = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>`;
const RELS = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>`;

/** Minimal .docx with one run ("delve into it"); "delve" is bytes 0..5. */
function makeDocx(extraParts: [string, string][] = []): Uint8Array {
  const pkg = new Package();
  pkg.set("[Content_Types].xml", strToBytes(CONTENT_TYPES));
  pkg.set("word/_rels/document.xml.rels", strToBytes(RELS));
  pkg.set("word/document.xml", strToBytes(DOC_XML));
  for (const [name, content] of extraParts) pkg.set(name, strToBytes(content));
  return pkg.write();
}

function delveDiag(message: string): Diagnostic {
  return {
    ruleId: "test/rule",
    severity: "warning",
    tier: "static",
    intent: "style",
    span: { start: 0, end: 5 },
    message,
    line: 0,
    column: 0,
    excerpt: "delve",
    fix: {
      edits: [{ range: { start: 0, end: 5 }, replacement: "examine" }],
      applicability: "machine-applicable",
    },
  };
}

function commentsXmlOf(bytes: Uint8Array): string {
  const data = unzipSync(bytes)["word/comments.xml"];
  expect(data).toBeDefined();
  return new TextDecoder().decode(data);
}

const FIXED_OPTS = { ...defaultReviseOptions(), date: "2020-01-01T00:00:00Z" };

test("dollar_signs_in_messages_are_not_replacement_patterns", () => {
  const existing = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:comments xmlns:w="${W_NS}"><w:comment w:id="1" w:author="x"><w:p><w:r><w:t>old</w:t></w:r></w:p></w:comment></w:comments>`;
  const docx = makeDocx([["word/comments.xml", existing]]);
  const out = applyTrackedChanges(docx, [delveDiag("costs $& and $' and $$5")], FIXED_OPTS);
  const comments = commentsXmlOf(out.bytes);
  // The message must survive literally (only & is xml-escaped): with
  // String.replace, "$&" would expand to the matched "</w:comments>".
  expect(comments).toContain("costs $&amp; and $' and $$5");
});

test("initials_uses_rust_whitespace_and_first_code_point", () => {
  // "\u{1F600}mma Stone" → first code points "\u{1F600}" and "S" → "\u{1F600}S"
  // (w[0] would emit a lone surrogate). U+FEFF is JS whitespace but not Rust
  // White_Space, so "a\uFEFFb c" is two words → "AC".
  const docx = makeDocx();
  const emoji = applyTrackedChanges(docx, [delveDiag("m")], {
    ...FIXED_OPTS,
    author: "\u{1F600}mma Stone",
  });
  expect(commentsXmlOf(emoji.bytes)).toContain('w:initials="\u{1F600}S"');

  const feff = applyTrackedChanges(docx, [delveDiag("m")], {
    ...FIXED_OPTS,
    author: "a\uFEFFb c",
  });
  expect(commentsXmlOf(feff.bytes)).toContain('w:initials="AC"');
});

test("max_comment_id_matches_rust_usize_parse", () => {
  // "12abc" parses to 0 in Rust; "+3" parses to 3. Max is 7 → next 8.
  const existing = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:comments xmlns:w="${W_NS}"><w:comment w:id="7" w:author="x"/><w:comment w:id="12abc" w:author="x"/><w:comment w:id="+3" w:author="x"/></w:comments>`;
  const docx = makeDocx([["word/comments.xml", existing]]);
  const out = applyTrackedChanges(docx, [delveDiag("m")], FIXED_OPTS);
  expect(commentsXmlOf(out.bytes)).toContain('<w:comment w:id="8"');
});

test("zip_entry_order_and_new_parts_last", () => {
  // Integer-like entry names would reorder to the front through a plain
  // object; "0" must keep its position and new parts must come last.
  const docx = makeDocx([["0", "integer-named part"]]);
  const out = applyTrackedChanges(docx, [delveDiag("m")], FIXED_OPTS);

  const names: string[] = [];
  unzipSync(out.bytes, {
    filter: (f) => {
      names.push(f.name);
      return true;
    },
  });
  expect(names).toEqual([
    "[Content_Types].xml",
    "word/_rels/document.xml.rels",
    "word/document.xml",
    "0",
    "word/comments.xml",
  ]);
});
