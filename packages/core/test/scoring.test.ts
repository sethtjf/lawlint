// Ported from crates/lawlint-core/src/scoring.rs #[cfg(test)].

import { describe, expect, test } from "bun:test";
import { parse } from "../src/document";
import { finalize, gateTier3, lineStarts, location, wordCount } from "../src/scoring";
import type { Diagnostic, Severity, Tier } from "../src/types";
import { Src } from "../src/utf8";

function diag(start: number, end: number, severity: Severity, tier: Tier): Diagnostic {
  return {
    ruleId: "core/x",
    severity,
    tier,
    intent: "detection",
    span: { start, end },
    message: "m",
    line: 0,
    column: 0,
    excerpt: "",
  };
}

test("word_count_matches_old_regex_semantics", () => {
  const doc = parse(new Src("the party’s cross-motion, filed 2024"), false);
  expect(wordCount(new Src("the party’s cross-motion, filed 2024"), doc)).toBe(5);
  const empty = parse(new Src(""), false);
  expect(wordCount(new Src(""), empty)).toBe(0);
});

test("word_count_blanks_code_blocks", () => {
  const text = "One two.\n\n```js\nconst x = delve;\n```\n\nThree four.";
  const src = new Src(text);
  const doc = parse(src, true);
  expect(wordCount(src, doc)).toBe(4);
  const plain = parse(src, false);
  expect(wordCount(src, plain)).toBeGreaterThan(4);
});

test("location_is_one_based_with_utf16_columns", () => {
  const src = new Src("ab\n𝒳cd\nend");
  const starts = lineStarts(src);
  expect(location(src, starts, 0)).toEqual([1, 1]);
  expect(location(src, starts, 2)).toEqual([1, 3]);
  // Line 2 starts at byte 3; 𝒳 is 4 bytes / 2 UTF-16 units.
  expect(location(src, starts, 3)).toEqual([2, 1]);
  expect(location(src, starts, 7)).toEqual([2, 3]);
  expect(location(src, starts, 10)).toEqual([3, 1]);
});

test("finalize_sorts_fills_positions_and_excerpt", () => {
  const src = new Src("first line here\nsecond — line");
  const doc = parse(src, false);
  const d1 = diag(6, 10, "warning", "static");
  const d2 = diag(0, 5, "warning", "static");
  const result = finalize(src, [d1, d2], doc);
  expect(result.diagnostics[0].span.start).toBe(0);
  expect(result.diagnostics[0].line).toBe(1);
  expect(result.diagnostics[0].column).toBe(1);
  expect(result.diagnostics[0].endLine).toBe(1);
  expect(result.diagnostics[0].endColumn).toBe(6);
  expect(result.diagnostics[0].excerpt).toBe("first line here");
  expect(result.diagnostics[1].span.start).toBe(6);
});

test("score_formula_matches_old_engine", () => {
  const text = Array.from({ length: 100 }, (_, i) => `w${i}`).join(" ");
  const src = new Src(text);
  const doc = parse(src, false);
  const d = diag(0, 2, "warning", "static");
  d.weight = 2;
  let result = finalize(src, [d], doc);
  expect(result.stats.wordCount).toBe(100);
  expect(result.stats.score).toBe(55);
  const heavy = diag(0, 2, "warning", "static");
  heavy.weight = 11;
  result = finalize(src, [heavy], doc);
  expect(result.stats.score).toBe(4);
});

test("style_intent_diagnostics_do_not_move_the_score", () => {
  const text = Array.from({ length: 100 }, (_, i) => `w${i}`).join(" ");
  const src = new Src(text);
  const doc = parse(src, false);
  const d = diag(0, 2, "warning", "static");
  d.weight = 2;
  d.intent = "style";
  let result = finalize(src, [d], doc);
  expect(result.diagnostics.length).toBe(1);
  expect(result.stats.score).toBe(100);
  const style = diag(0, 2, "error", "static");
  style.intent = "style";
  style.weight = 100;
  const detection = diag(3, 5, "warning", "static");
  detection.weight = 2;
  result = finalize(src, [style, detection], doc);
  expect(result.stats.score).toBe(55);
});

test("score_empty_text_is_100_and_penalty_clamps", () => {
  const emptyDoc = parse(new Src(""), false);
  expect(finalize(new Src(""), [], emptyDoc).stats.score).toBe(100);
  const src = new Src("a b");
  const doc = parse(src, false);
  const d = diag(0, 1, "error", "static");
  d.weight = 1000;
  expect(finalize(src, [d], doc).stats.score).toBe(0);
});

test("tier3_points_scale_with_confidence", () => {
  const text = Array.from({ length: 100 }, (_, i) => `w${i}`).join(" ");
  const src = new Src(text);
  const doc = parse(src, false);
  const d1 = diag(0, 2, "warning", "static");
  const d2 = diag(3, 5, "warning", "inferential");
  d2.confidence = 0.8;
  const result = finalize(src, [d1, d2], doc);
  expect(result.stats.score).toBe(58);
});

test("gate_tier3_floor_and_severity_cap", () => {
  const low = diag(0, 1, "warning", "inferential");
  low.confidence = 0.5;
  const high = diag(1, 2, "error", "inferential");
  high.confidence = 0.9;
  const staticErr = diag(2, 3, "error", "static");
  const out = gateTier3([low, high, staticErr], 0.6);
  expect(out.length).toBe(2);
  expect(out[0].tier).toBe("inferential");
  expect(out[0].severity).toBe("warning");
  expect(out[1].severity).toBe("error");
  const sug = diag(0, 1, "suggestion", "inferential");
  sug.confidence = 0.9;
  expect(gateTier3([sug], 0.6)[0].severity).toBe("suggestion");
});

test("gate_tier3_boundary_at_floor_keeps", () => {
  const at = diag(0, 1, "warning", "inferential");
  at.confidence = 0.6;
  expect(gateTier3([at], 0.6).length).toBe(1);
  const missing = diag(0, 1, "warning", "inferential");
  expect(gateTier3([missing], 0.6)).toHaveLength(0);
});

test("sentence_count_totals_document_sentences", () => {
  const src = new Src("One. Two. Three.\n\nFour.");
  const doc = parse(src, false);
  const result = finalize(src, [], doc);
  expect(result.stats.sentenceCount).toBe(4);
});
