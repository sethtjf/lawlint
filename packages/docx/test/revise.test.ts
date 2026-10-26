// Ported from crates/lawlint-docx/src/revise.rs #[cfg(test)].

import { expect, test } from "bun:test";
import { type Ids, type RevisionOutput, type RunEdit, apply } from "../src/revise";

const NS =
  '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>';

function run(inner: string, edits: Map<number, RunEdit[]>): { xml: string; out: RevisionOutput } {
  const xml = `${NS}${inner}</w:body></w:document>`;
  const ids: Ids = { nextRev: 100, nextComment: 1 };
  const out = apply(xml, edits, "lawlint", "2020-01-01T00:00:00Z", ids);
  return { xml: new TextDecoder().decode(out.documentXml), out };
}

/** A self-contained redline piece. */
function edit(id: number, start: number, end: number, replacement: string): RunEdit {
  return {
    start,
    end,
    replacement,
    message: "rule msg",
    commentId: id,
    opensHere: true,
    closesHere: true,
  };
}

/** A self-contained comment-only anchor. */
function annotation(id: number, start: number, end: number, message: string): RunEdit {
  return {
    start,
    end,
    replacement: null,
    message,
    commentId: id,
    opensHere: true,
    closesHere: true,
  };
}

/** The opening half of a comment-only anchor that continues into a later run. */
function openOnly(id: number, start: number, endOfRun: number, message: string): RunEdit {
  return {
    start,
    end: endOfRun,
    replacement: null,
    message,
    commentId: id,
    opensHere: true,
    closesHere: false,
  };
}

/** The closing half of a comment-only anchor that opened in an earlier run. */
function closeOnly(id: number, end: number, message: string): RunEdit {
  return {
    start: 0,
    end,
    replacement: null,
    message,
    commentId: id,
    opensHere: false,
    closesHere: true,
  };
}

test("simple_run_becomes_del_ins_and_comment", () => {
  // "Pursuant to X" -> replace "Pursuant to" (bytes 0..11) with "Under".
  const inner = "<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Pursuant to X</w:t></w:r></w:p>";
  const edits = new Map([[0, [edit(1, 0, 11, "Under")]]]);
  const { xml, out } = run(inner, edits);

  expect(out.skipped).toBe(0);
  expect(out.annotated).toBe(0);
  expect(out.comments.length).toBe(1);
  expect(out.comments[0].text).toBe("rule msg");
  expect(xml).toContain('<w:delText xml:space="preserve">Pursuant to</w:delText>');
  expect(xml).toContain('<w:t xml:space="preserve">Under</w:t>');
  expect(xml).toContain("<w:commentRangeStart");
  expect(xml).toContain("<w:commentReference");
  // The untouched tail " X" is kept as a plain run.
  expect(xml).toContain('<w:t xml:space="preserve"> X</w:t>');
  // rPr is carried onto the revision runs.
  expect(xml).toContain("<w:b/>");
});

test("pure_insertion_emits_no_deltext", () => {
  // Empty span (start==end): insertion only.
  const inner = "<w:p><w:r><w:t>ab</w:t></w:r></w:p>";
  const edits = new Map([[0, [edit(1, 1, 1, "X")]]]);
  const { xml, out } = run(inner, edits);
  expect(out.comments.length).toBe(1);
  expect(xml).not.toContain("w:delText");
  expect(xml).toContain('<w:t xml:space="preserve">X</w:t>');
});

test("multi_text_run_is_skipped_not_corrupted", () => {
  // A run with two <w:t> is not "simple"; the edit is skipped and the run
  // is emitted verbatim.
  const inner = "<w:p><w:r><w:t>foo</w:t><w:t>bar</w:t></w:r></w:p>";
  const edits = new Map([[0, [edit(1, 0, 3, "FOO")]]]);
  const { xml, out } = run(inner, edits);
  expect(out.skipped).toBe(1);
  expect(out.comments).toEqual([]);
  expect(xml).toContain("<w:t>foo</w:t><w:t>bar</w:t>");
  expect(xml).not.toContain("w:ins");
});

test("edit_inside_existing_revision_is_skipped", () => {
  // ordinal 0 is inside <w:ins>; edits there are not applied.
  const inner = "<w:p><w:ins><w:r><w:t>added</w:t></w:r></w:ins></w:p>";
  const edits = new Map([[0, [edit(1, 0, 5, "gone")]]]);
  const { xml, out } = run(inner, edits);
  expect(out.skipped).toBe(1);
  expect(xml).toContain("<w:t>added</w:t>");
});

test("run_without_edits_is_untouched", () => {
  const inner = "<w:p><w:r><w:t>clean</w:t></w:r></w:p>";
  const { xml, out } = run(inner, new Map());
  expect(out.skipped).toBe(0);
  expect(out.comments).toEqual([]);
  expect(xml).toContain("<w:t>clean</w:t>");
});

test("annotation_only_edit_comments_without_changing_text", () => {
  const inner = "<w:p><w:r><w:t>Pursuant to X</w:t></w:r></w:p>";
  const edits = new Map([[0, [annotation(1, 0, 11, "consider rewording")]]]);
  const { xml, out } = run(inner, edits);

  expect(out.skipped).toBe(0);
  expect(out.annotated).toBe(1);
  expect(out.comments.length).toBe(1);
  expect(out.comments[0].text).toBe("consider rewording");
  expect(xml).not.toContain("w:delText");
  expect(xml).not.toContain("<w:ins");
  expect(xml).toContain("<w:commentRangeStart");
  expect(xml).toContain("<w:commentReference");
  // The original text survives untouched, just split into kept-run pieces.
  expect(xml).toContain("Pursuant to");
  expect(xml).toContain(" X");
});

test("two_overlapping_comment_anchors_on_one_run", () => {
  // Crossing (not nested) spans: A=[0,5), B=[3,8).
  const inner = "<w:p><w:r><w:t>0123456789</w:t></w:r></w:p>";
  const edits = new Map([
    [0, [annotation(1, 0, 5, "first finding"), annotation(2, 3, 8, "second finding")]],
  ]);
  const { xml, out } = run(inner, edits);

  expect(out.skipped).toBe(0);
  expect(out.comments.length).toBe(2);
  expect(out.comments.map((c) => c.id)).toEqual([1, 2]);

  // Interleaved order: Start(1), Start(2), End(1)+Ref(1), End(2)+Ref(2).
  const start1 = xml.indexOf('<w:commentRangeStart w:id="1"/>');
  const start2 = xml.indexOf('<w:commentRangeStart w:id="2"/>');
  const end1 = xml.indexOf('<w:commentRangeEnd w:id="1"/>');
  const end2 = xml.indexOf('<w:commentRangeEnd w:id="2"/>');
  expect(start1).toBeGreaterThanOrEqual(0);
  expect(start1).toBeLessThan(start2);
  expect(start2).toBeLessThan(end1);
  expect(end1).toBeLessThan(end2);
});

test("nested_comment_anchor", () => {
  // A=[0,10) fully contains B=[3,7).
  const inner = "<w:p><w:r><w:t>0123456789</w:t></w:r></w:p>";
  const edits = new Map([[0, [annotation(1, 0, 10, "outer"), annotation(2, 3, 7, "inner")]]]);
  const { xml, out } = run(inner, edits);

  expect(out.skipped).toBe(0);
  expect(out.comments.length).toBe(2);

  const start1 = xml.indexOf('<w:commentRangeStart w:id="1"/>');
  const start2 = xml.indexOf('<w:commentRangeStart w:id="2"/>');
  const end2 = xml.indexOf('<w:commentRangeEnd w:id="2"/>');
  const end1 = xml.indexOf('<w:commentRangeEnd w:id="1"/>');
  // Start(A), Start(B), End(B)+Ref(B), End(A)+Ref(A).
  expect(start1).toBeLessThan(start2);
  expect(start2).toBeLessThan(end2);
  expect(end2).toBeLessThan(end1);
});

test("identical_span_comments_both_survive", () => {
  // Two findings covering exactly the same words must not collapse.
  const inner = "<w:p><w:r><w:t>0123456789</w:t></w:r></w:p>";
  const edits = new Map([
    [0, [annotation(1, 2, 6, "finding A"), annotation(2, 2, 6, "finding B")]],
  ]);
  const { xml, out } = run(inner, edits);
  expect(out.skipped).toBe(0);
  expect(out.comments.length).toBe(2);
  expect(xml).toContain('<w:commentRangeStart w:id="1"/>');
  expect(xml).toContain('<w:commentRangeStart w:id="2"/>');
  expect(xml).toContain('<w:commentRangeEnd w:id="1"/>');
  expect(xml).toContain('<w:commentRangeEnd w:id="2"/>');
});

test("comment_spanning_three_runs", () => {
  // Ordinal 0 (opens), ordinal 1 (middle, untouched, deliberately
  // non-simple to prove it doesn't matter), ordinal 2 (closes).
  const inner =
    "<w:p><w:r><w:t>Alpha </w:t></w:r><w:r><w:t>Beta</w:t><w:br/></w:r><w:r><w:t> Gamma</w:t></w:r></w:p>";
  const edits = new Map<number, RunEdit[]>();
  edits.set(0, [openOnly(9, 0, 6, "spans three runs")]);
  edits.set(2, [closeOnly(9, 3, "spans three runs")]);
  const { xml, out } = run(inner, edits);

  expect(out.skipped).toBe(0);
  expect(out.annotated).toBe(1);
  expect(out.comments.length).toBe(1);
  expect(out.comments[0].id).toBe(9);

  // The middle run passes through byte-for-byte, untouched.
  expect(xml).toContain("<w:r><w:t>Beta</w:t><w:br/></w:r>");

  const start = xml.indexOf('<w:commentRangeStart w:id="9"/>');
  const end = xml.indexOf('<w:commentRangeEnd w:id="9"/>');
  expect(start).toBeLessThan(end);
  // No delText/ins anywhere: this is comment-only.
  expect(xml).not.toContain("w:delText");
  expect(xml).not.toContain("<w:ins");
});

test("multi_run_comment_dropped_when_a_boundary_run_is_not_simple", () => {
  // The end run has two <w:t> children, so it's not simple; neither half of
  // the comment may be emitted (no orphaned marker).
  const inner = "<w:p><w:r><w:t>Alpha </w:t></w:r><w:r><w:t>Beta</w:t><w:t>!</w:t></w:r></w:p>";
  const edits = new Map<number, RunEdit[]>();
  edits.set(0, [openOnly(9, 0, 6, "not simple end run")]);
  edits.set(1, [closeOnly(9, 2, "not simple end run")]);
  const { xml, out } = run(inner, edits);

  expect(out.comments).toEqual([]);
  expect(out.annotated).toBe(0);
  expect(out.skipped).toBe(1); // one finding dropped, not two pieces
  expect(xml).not.toContain("commentRangeStart");
  expect(xml).not.toContain("commentRangeEnd");
  // Both affected runs pass through verbatim.
  expect(xml).toContain("<w:t>Alpha </w:t>");
  expect(xml).toContain("<w:t>Beta</w:t><w:t>!</w:t>");
});

test("multi_run_comment_dropped_when_boundary_run_already_in_revision", () => {
  const inner = "<w:p><w:r><w:t>Alpha </w:t></w:r><w:ins><w:r><w:t>Beta</w:t></w:r></w:ins></w:p>";
  const edits = new Map<number, RunEdit[]>();
  edits.set(0, [openOnly(9, 0, 6, "end run in revision")]);
  edits.set(1, [closeOnly(9, 2, "end run in revision")]);
  const { xml, out } = run(inner, edits);

  expect(out.comments).toEqual([]);
  expect(out.skipped).toBe(1); // one finding, not two pieces
  expect(xml).not.toContain("commentRangeStart");
  expect(xml).not.toContain("commentRangeEnd");
  expect(xml).toContain("<w:ins><w:r><w:t>Beta</w:t></w:r></w:ins>");
});

test("redline_adjacent_to_overlapping_comment", () => {
  // Redline R=[5,10) replacing with "X"; comment C=[7,12) starts inside R's
  // span, forcing R's <w:del> to split.
  const inner = "<w:p><w:r><w:t>0123456789AB</w:t></w:r></w:p>";
  const edits = new Map([[0, [edit(1, 5, 10, "X"), annotation(2, 7, 12, "interior comment")]]]);
  const { xml, out } = run(inner, edits);

  expect(out.skipped).toBe(0);
  expect(out.comments.length).toBe(2);
  expect(out.annotated).toBe(1);

  // Deleted text is recoverable by concatenating the (split) delText pieces:
  // "56" + "789" == the original [5,10) span "56789".
  const delTexts: string[] = [];
  for (const m of xml.matchAll(/<w:delText[^>]*>/g)) {
    const start = m.index + m[0].length;
    const end = xml.indexOf("</w:delText>", start);
    delTexts.push(xml.slice(start, end));
  }
  expect(delTexts.join("")).toBe("56789");
  expect(delTexts.length).toBe(2);

  // Exactly one <w:ins>, appearing after the last del piece.
  expect(xml.match(/<w:ins/g)?.length).toBe(1);
  const lastDelEnd = xml.lastIndexOf("</w:del>");
  const insPos = xml.indexOf("<w:ins");
  expect(insPos).toBeGreaterThan(lastDelEnd);

  // Comment 2's Start marker sits between the two del pieces.
  const firstDelEnd = xml.indexOf("</w:del>");
  const comment2Start = xml.indexOf('<w:commentRangeStart w:id="2"/>');
  expect(comment2Start).toBeGreaterThan(firstDelEnd);
  expect(comment2Start).toBeLessThan(lastDelEnd);

  // Comment 2's End+reference come after comment 1's own End.
  const end1 = xml.indexOf('<w:commentRangeEnd w:id="1"/>');
  const end2 = xml.indexOf('<w:commentRangeEnd w:id="2"/>');
  expect(end1).toBeLessThan(end2);
});

test("redlines_never_overlap_within_a_run", () => {
  const inner = "<w:p><w:r><w:t>0123456789</w:t></w:r></w:p>";
  const edits = new Map([[0, [edit(1, 0, 5, "A"), edit(2, 3, 8, "B")]]]);
  const { xml, out } = run(inner, edits);

  // Only the first (in span order) redline survives; the overlapping second
  // is defensively dropped, not corrupted into the stream.
  expect(out.skipped).toBe(1);
  expect(out.comments.length).toBe(1);
  expect(out.comments[0].id).toBe(1);
  expect(xml).toContain('<w:t xml:space="preserve">A</w:t>');
  expect(xml).not.toContain('<w:t xml:space="preserve">B</w:t>');
});

import { assertWordNamespaces } from "./helpers";

const W_NS = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

function runDoc(
  documentXml: string,
  edits: Map<number, RunEdit[]>,
): { xml: string; out: RevisionOutput } {
  const ids: Ids = { nextRev: 100, nextComment: 1 };
  const out = apply(documentXml, edits, "lawlint", "2020-01-01T00:00:00Z", ids);
  return { xml: new TextDecoder().decode(out.documentXml), out };
}

test("cdata_inside_wt_contributes_text", () => {
  const inner = "<w:p><w:r><w:t><![CDATA[Hello]]></w:t></w:r></w:p>";
  const edits = new Map([[0, [edit(1, 0, 0, "X")]]]);
  const { xml, out } = run(inner, edits);
  expect(out.skipped).toBe(0);
  expect(xml).toContain('<w:t xml:space="preserve">X</w:t>');
  expect(xml).toContain('<w:t xml:space="preserve">Hello</w:t>');
});

test("cdata_run_text_can_be_redlined", () => {
  const inner = "<w:p><w:r><w:t><![CDATA[Hello]]></w:t></w:r></w:p>";
  const edits = new Map([[0, [edit(1, 1, 4, "ELL")]]]);
  const { xml, out } = run(inner, edits);
  expect(out.skipped).toBe(0);
  expect(xml).toContain('<w:t xml:space="preserve">H</w:t>');
  expect(xml).toContain('<w:delText xml:space="preserve">ell</w:delText>');
  expect(xml).toContain('<w:t xml:space="preserve">ELL</w:t>');
  expect(xml).toContain('<w:t xml:space="preserve">o</w:t>');
});

test("unprefixed_document_declares_w_locally", () => {
  const doc = `<document xmlns="${W_NS}"><body><p><r><t>Hello</t></r></p></body></document>`;
  const { xml, out } = runDoc(doc, new Map([[0, [edit(1, 1, 4, "i")]]]));
  expect(out.skipped).toBe(0);
  // The root is not patched; generated top-level elements declare w:
  // locally instead.
  expect(xml).toContain(
    `<w:ins w:id="101" w:author="lawlint" w:date="2020-01-01T00:00:00Z" xmlns:w="${W_NS}">`,
  );
  expect(xml).toContain('<w:delText xml:space="preserve">ell</w:delText>');
  assertWordNamespaces(xml);
});

test("prolog_comment_mimicking_root_is_untouched", () => {
  const doc = `<!-- <document xmlns="${W_NS}"> --><document xmlns="${W_NS}"><body><p><r><t>Hello</t></r></p></body></document>`;
  const { xml, out } = runDoc(doc, new Map([[0, [edit(1, 1, 4, "i")]]]));
  expect(out.skipped).toBe(0);
  // The comment passes through byte-for-byte and the real root is not
  // patched — the declaration lives on the generated elements.
  expect(xml.startsWith(`<!-- <document xmlns="${W_NS}"> -->`)).toBe(true);
  expect(xml).toContain(`<document xmlns="${W_NS}"><body>`);
  assertWordNamespaces(xml);
});

test("descendant_rebinding_of_w_is_respected", () => {
  // <body> rebinds w: to another URI, so the generated prefix must be a
  // free one declared locally.
  const doc = `<document xmlns="${W_NS}"><body xmlns:w="urn:other"><p><r><t>Hello</t></r></p></body></document>`;
  const { xml, out } = runDoc(doc, new Map([[0, [edit(1, 1, 4, "i")]]]));
  expect(out.skipped).toBe(0);
  expect(xml).toContain(
    `<lw:ins lw:id="101" lw:author="lawlint" lw:date="2020-01-01T00:00:00Z" xmlns:lw="${W_NS}">`,
  );
  expect(xml).not.toContain("<w:ins");
  assertWordNamespaces(xml);
});

test("root_bound_w_needs_no_declarations", () => {
  const doc = `<w:document xmlns:w="${W_NS}"><w:body><w:p><w:r><w:t>Hello</w:t></w:r></w:p></w:body></w:document>`;
  const { xml, out } = runDoc(doc, new Map([[0, [edit(1, 1, 4, "i")]]]));
  expect(out.skipped).toBe(0);
  expect(xml).toContain('<w:ins w:id="101" w:author="lawlint" w:date="2020-01-01T00:00:00Z">');
  // No extra xmlns declarations on the generated markup.
  expect(xml.match(/<w:ins[^>]*>/g)?.every((t) => !t.includes("xmlns"))).toBe(true);
  assertWordNamespaces(xml);
});

test("non_w_prefix_is_reused_for_generated_markup", () => {
  const doc = `<x:document xmlns:x="${W_NS}"><x:body><x:p><x:r><x:t>Hello</x:t></x:r></x:p></x:body></x:document>`;
  const { xml, out } = runDoc(doc, new Map([[0, [edit(1, 1, 4, "i")]]]));
  expect(out.skipped).toBe(0);
  expect(xml).toContain("<x:ins");
  expect(xml).toContain('<x:delText xml:space="preserve">ell</x:delText>');
  expect(xml).toContain('<x:commentRangeStart x:id="1"/>');
  expect(xml).not.toContain("w:");
  assertWordNamespaces(xml);
});

test("w_bound_elsewhere_picks_a_free_prefix", () => {
  const doc = `<document xmlns="${W_NS}" xmlns:w="http://other/ns"><body><p><r><t>Hello</t></r></p></body></document>`;
  const { xml, out } = runDoc(doc, new Map([[0, [edit(1, 1, 4, "i")]]]));
  expect(out.skipped).toBe(0);
  expect(xml).toContain(`xmlns:lw="${W_NS}"`);
  expect(xml).toContain("<lw:ins");
  assertWordNamespaces(xml);
});

test("run_tag_binding_does_not_leak_into_generated_markup", () => {
  // The <x:r> tag declares x: itself, but generated siblings replace that
  // tag — the prefix must come from the parent scope, so generated
  // elements declare w: locally.
  const doc = `<document xmlns="${W_NS}"><body><p><x:r xmlns:x="${W_NS}"><x:t>Hello</x:t></x:r></p></body></document>`;
  const { xml, out } = runDoc(doc, new Map([[0, [edit(1, 1, 4, "i")]]]));
  expect(out.skipped).toBe(0);
  expect(xml).toContain("<w:ins ");
  expect(xml).toContain(`xmlns:w="${W_NS}"`);
  assertWordNamespaces(xml);
});

test("run_tag_rebinding_w_does_not_corrupt_scope", () => {
  // <w:r xmlns:w="urn:other"> isn't really a Word run — project/classify
  // use local names so it is still buffered, and the generated markup
  // resolves w: from the parent scope (W_NS), ignoring the rebound tag.
  const doc = `<w:document xmlns:w="${W_NS}"><w:body><w:p><w:r xmlns:w="urn:other"><w:t>Hello</w:t></w:r></w:p></w:body></w:document>`;
  const { xml, out } = runDoc(doc, new Map([[0, [edit(1, 1, 4, "i")]]]));
  expect(out.skipped).toBe(0);
  expect(xml).toContain("<w:ins");
  assertWordNamespaces(xml);
});
