// Ported from crates/lawlint-docx/src/lib.rs #[cfg(test)].

import { expect, test } from "bun:test";
import type { Diagnostic } from "@lawlint/core";
import { unzipSync } from "fflate";
import { DocxError, applyTrackedChanges, defaultReviseOptions } from "../src/index";
import { Package, strToBytes } from "../src/pkg";
import type { Projection } from "../src/project";
import { type Ids, apply } from "../src/revise";
import { selectEdits } from "../src/select";
import { assertWordNamespaces } from "./helpers";

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
function makeDocx(extraParts: [string, string][] = [], docXml: string = DOC_XML): Uint8Array {
  const pkg = new Package();
  pkg.set("[Content_Types].xml", strToBytes(CONTENT_TYPES));
  pkg.set("word/_rels/document.xml.rels", strToBytes(RELS));
  pkg.set("word/document.xml", strToBytes(docXml));
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

function spanDiag(start: number, end: number, replacement: string): Diagnostic {
  return {
    ruleId: "test/rule",
    severity: "warning",
    tier: "static",
    intent: "style",
    span: { start, end },
    message: "m",
    line: 0,
    column: 0,
    excerpt: "",
    fix: {
      edits: [{ range: { start, end }, replacement }],
      applicability: "machine-applicable",
    },
  };
}

function partText(bytes: Uint8Array, name: string): string {
  const data = unzipSync(bytes)[name];
  return data === undefined ? "" : new TextDecoder().decode(data);
}

// A run containing a text box (<w:drawing>) plus a plain body run. The
// nested run's <w:t> still consumes ordinal 0 in the projection; "main" is
// ordinal 1 at byte offset 5 ("box" + the inner paragraph's "\n\n").
const TXBX_DOC = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="${W_NS}"><w:body><w:p><w:r><w:drawing><w:txbxContent><w:p><w:r><w:t>box</w:t></w:r></w:p></w:txbxContent></w:drawing></w:r><w:r><w:t>main</w:t></w:r></w:p></w:body></w:document>`;

test("fix_inside_nested_textbox_run_is_untouchable", () => {
  const docx = makeDocx([], TXBX_DOC);
  const out = applyTrackedChanges(docx, [spanDiag(0, 3, "BOX")], FIXED_OPTS);
  expect(out.applied).toBe(0);
  // The nested run is inside a non-simple outer run: the whole document
  // passes through verbatim.
  expect(partText(out.bytes, "word/document.xml")).toBe(TXBX_DOC);
});

test("fix_on_body_run_after_textbox_lands_correctly", () => {
  const docx = makeDocx([], TXBX_DOC);
  // "mai" is at byte offset 5 in the projected "box\n\nmain".
  const out = applyTrackedChanges(docx, [spanDiag(5, 8, "MAI")], FIXED_OPTS);
  expect(out.applied).toBe(1);
  const doc = partText(out.bytes, "word/document.xml");
  expect(doc).toContain('<w:delText xml:space="preserve">mai</w:delText>');
  // The text-box run is byte-for-byte unchanged.
  expect(doc).toContain(
    "<w:drawing><w:txbxContent><w:p><w:r><w:t>box</w:t></w:r></w:p></w:txbxContent></w:drawing>",
  );
});

test("comments_xml_with_other_prefix_stays_consistent", () => {
  // Existing comments.xml binds W_NS to "x": appended blocks use x: too.
  const existing = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<x:comments xmlns:x="${W_NS}"><x:comment x:id="4" x:author="a"><x:p><x:r><x:t>old</x:t></x:r></x:p></x:comment></x:comments>`;
  const docx = makeDocx([["word/comments.xml", existing]]);
  const out = applyTrackedChanges(docx, [delveDiag("m")], FIXED_OPTS);
  const comments = commentsXmlOf(out.bytes);
  expect(comments).toContain('<x:comment x:id="5"');
  expect(comments).not.toContain("<w:comment ");
});

test("comments_xml_without_prefix_declares_w_locally", () => {
  const existing = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<comments xmlns="${W_NS}"><comment id="2" author="a"><p><r><t>old</t></r></p></comment></comments>`;
  const docx = makeDocx([["word/comments.xml", existing]]);
  const out = applyTrackedChanges(docx, [delveDiag("m")], FIXED_OPTS);
  const comments = commentsXmlOf(out.bytes);
  // The root is not patched; the generated block declares w: locally.
  expect(comments).toContain('<w:comment w:id="3"');
  expect(comments).toContain(`xmlns:w="${W_NS}"`);
  assertWordNamespaces(comments);
});

test("comments_xml_prolog_comment_does_not_capture_the_splice", () => {
  const existing = `<!-- <comments xmlns="${W_NS}"> --><?xml version="1.0"?>
<comments xmlns="${W_NS}"><comment id="2" author="a"><p><r><t>old</t></r></p></comment></comments>`;
  const docx = makeDocx([["word/comments.xml", existing]]);
  const out = applyTrackedChanges(docx, [delveDiag("m")], FIXED_OPTS);
  const comments = commentsXmlOf(out.bytes);
  // The new block lands inside the real root, after the prolog comment
  // and before the root's closing tag.
  const commentEnd = comments.indexOf("-->");
  const blockAt = comments.indexOf('<w:comment w:id="3"');
  const closeAt = comments.indexOf("</comments>");
  expect(blockAt).toBeGreaterThan(commentEnd);
  expect(blockAt).toBeLessThan(closeAt);
  assertWordNamespaces(comments);
});

test("self_closing_rels_with_mimic_prolog_comment_expands_real_root", () => {
  const rels = `<!-- <Relationships/> --><?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>`;
  const pkg = new Package();
  pkg.set("[Content_Types].xml", strToBytes(CONTENT_TYPES));
  pkg.set("word/_rels/document.xml.rels", strToBytes(rels));
  pkg.set("word/document.xml", strToBytes(DOC_XML));
  const out = applyTrackedChanges(pkg.write(), [delveDiag("m")], FIXED_OPTS);
  const relsOut = partText(out.bytes, "word/_rels/document.xml.rels");
  // The relationship is inside the real root, not the prolog comment.
  const start = relsOut.indexOf("<Relationships ");
  const comment = relsOut.indexOf("<!-- <Relationships/> -->");
  expect(relsOut).toContain('Target="comments.xml"');
  expect(comment).toBe(0);
  expect(relsOut.indexOf('Target="comments.xml"')).toBeGreaterThan(start);
});

test("self_closing_rels_and_types_roots_are_expanded", () => {
  const docx = makeDocx();
  const out = applyTrackedChanges(docx, [delveDiag("m")], FIXED_OPTS);
  // The fixture's document.xml.rels root is self-closing; it must be
  // expanded (keeping attributes) with the comments relationship inside.
  const rels = partText(out.bytes, "word/_rels/document.xml.rels");
  expect(rels).toContain('Target="comments.xml"');
  expect(rels).toContain("</Relationships>");
  const ct = partText(out.bytes, "[Content_Types].xml");
  expect(ct).toContain("/word/comments.xml");
});

test("self_closing_types_root_is_expanded", () => {
  const typesSelfClosed = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>`;
  const pkg = new Package();
  pkg.set("[Content_Types].xml", strToBytes(typesSelfClosed));
  pkg.set("word/_rels/document.xml.rels", strToBytes(RELS));
  pkg.set("word/document.xml", strToBytes(DOC_XML));
  const out = applyTrackedChanges(pkg.write(), [delveDiag("m")], FIXED_OPTS);
  const ct = partText(out.bytes, "[Content_Types].xml");
  expect(ct).toContain("/word/comments.xml");
  expect(ct).toContain("</Types>");
});

test("zip_entry_over_size_limit_is_rejected", () => {
  const bytes = makeDocx();
  // Patch the first central-directory record's uncompressed-size field
  // (signature PK\x01\x02, size at +24, little-endian) past the 256 MiB cap.
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  for (let i = 0; i + 4 < bytes.length; i++) {
    if (
      bytes[i] === 0x50 &&
      bytes[i + 1] === 0x4b &&
      bytes[i + 2] === 0x01 &&
      bytes[i + 3] === 0x02
    ) {
      view.setUint32(i + 24, 256 * 1024 * 1024 + 1, true);
      break;
    }
  }
  try {
    Package.read(bytes);
    expect.unreachable("should have thrown");
  } catch (e) {
    expect(e).toBeInstanceOf(DocxError);
    expect((e as DocxError).kind).toBe("zip");
  }
});

test("zip_over_max_entries_is_rejected", () => {
  const pkg = new Package();
  for (let i = 0; i < 10_001; i++) pkg.set(`f${i}`, strToBytes("x"));
  try {
    Package.read(pkg.write());
    expect.unreachable("should have thrown");
  } catch (e) {
    expect(e).toBeInstanceOf(DocxError);
    expect((e as DocxError).kind).toBe("zip");
  }
});

test("self_closing_comments_root_is_expanded", () => {
  // An existing self-closing <w:comments/> must gain the new block inside
  // it — not have the block dropped while document.xml references its id.
  const existing = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:comments xmlns:w="${W_NS}"/>`;
  const docx = makeDocx([["word/comments.xml", existing]]);
  const out = applyTrackedChanges(docx, [delveDiag("m")], FIXED_OPTS);
  const comments = commentsXmlOf(out.bytes);
  expect(comments).toContain('<w:comment w:id="1"');
  expect(comments).toContain("</w:comments>");
  const doc = partText(out.bytes, "word/document.xml");
  expect(doc).toContain('<w:commentReference w:id="1"/>');
});

test("wrong_part_root_throws_malformed", () => {
  const badTypes = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Foo xmlns="http://example.com"/>`;
  const pkg = new Package();
  pkg.set("[Content_Types].xml", strToBytes(badTypes));
  pkg.set("word/_rels/document.xml.rels", strToBytes(RELS));
  pkg.set("word/document.xml", strToBytes(DOC_XML));
  try {
    applyTrackedChanges(pkg.write(), [delveDiag("m")], FIXED_OPTS);
    expect.unreachable("should have thrown");
  } catch (e) {
    expect(e).toBeInstanceOf(DocxError);
    expect((e as DocxError).kind).toBe("malformed");
  }
});
