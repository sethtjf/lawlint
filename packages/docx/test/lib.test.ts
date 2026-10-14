// Ported from crates/lawlint-docx/src/lib.rs #[cfg(test)].

import { expect, test } from "bun:test";
import type { Diagnostic } from "@lawlint/core";
import { applyTrackedChanges, defaultReviseOptions, selectEdits } from "../src/index";
import type { Projection } from "../src/project";
import { type Ids, apply } from "../src/revise";

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
