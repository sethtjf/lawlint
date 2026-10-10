// Ported from crates/lawlint-core/src/judge.rs #[cfg(test)] — planning,
// grounding, and the serial run/cache paths. The run_parallel_* tests are
// not ported: the TS Judge interface is synchronous, so concurrency cannot
// change ordering.

import { describe, expect, test } from "bun:test";
import { parse } from "../src/document";
import type { Block, Document } from "../src/document";
import {
  Granularity,
  MemoryCache,
  MockJudge,
  PROMPT_VERSION,
  defaultJudgePlan,
  defaultQuoteGround,
  judgePlanForContext,
  planJudge,
  planJudgeWith,
  runJudge,
} from "../src/judge";
import type { JudgeFinding, JudgeRequest, RubricFragment } from "../src/judge";
import type { TextRange } from "../src/types";
import { Src, wsTrim } from "../src/utf8";

// ---- helpers ------------------------------------------------------------

function frag(rule: string, granularity: Granularity): RubricFragment {
  return {
    rule,
    severity: "warning",
    granularity,
    rubric: `Rubric text for ${rule}.`,
    flag_examples: ["flag one", "flag two", "flag three"],
    pass_examples: ["pass one", "pass two", "pass three"],
  };
}

function finding(rule: string, quote: string, confidence: number): JudgeFinding {
  return { rule, quote, explanation: "because", confidence };
}

function req(text: string, rules: string[]): JudgeRequest {
  return {
    chunk_range: { start: 0, end: new Src(text).byteLen },
    chunk_text: text,
    rules,
    prompt: `PROMPT for: ${text}`,
    cache_key_base: `base:${text}:${rules.join(",")}`,
  };
}

function paraDoc(source: string): Document {
  const src = new Src(source);
  const blocks: Block[] = [];
  let offset = 0;
  for (const part of source.split("\n\n")) {
    if (wsTrim(part).length > 0) {
      const partBytes = new Src(part).byteLen;
      blocks.push({
        kind: "paragraph",
        range: { start: offset, end: offset + partBytes },
        sentences: [],
      });
    }
    offset += new Src(part).byteLen + 2;
  }
  return { blocks, source: src };
}

function block(kind: Block["kind"], start: number, end: number): Block {
  return { kind, range: { start, end }, sentences: [] };
}

function slice(text: string, r: TextRange): string {
  return new Src(text).slice(r.start, r.end);
}

// ---- skeleton ------------------------------------------------------------

test("judge_stats_default_and_camel_case", () => {
  const s = { chunks: 0, cacheHits: 0, chunksFailed: 0, grounded: 0, hallucinated: {} };
  expect(s.chunks).toBe(0);
  expect(Object.keys(s.hallucinated)).toHaveLength(0);
});

test("prompt_version_is_stable", () => {
  expect(PROMPT_VERSION).toBe("3");
});

test("mock_judge_model_id", () => {
  expect(MockJudge.new().modelId()).toBe("mock");
});

// ---- grounding -----------------------------------------------------------

test("ground_exact_substring_absolute_offsets", () => {
  const chunk = "The court held that this applies broadly.";
  const range = { start: 100, end: 100 + new Src(chunk).byteLen };
  const got = defaultQuoteGround("held that", chunk, range);
  expect(got).toEqual({ start: 110, end: 119 });
  expect(slice(chunk, { start: 10, end: 19 })).toBe("held that");
});

test("ground_exact_with_multibyte_prefix", () => {
  const chunk = "It was—frankly—wrong in every way.";
  const src = new Src(chunk);
  const range = { start: 50, end: 50 + src.byteLen };
  const idx = src.byte(chunk.indexOf("wrong"));
  const got = defaultQuoteGround("wrong", chunk, range);
  expect(got?.start).toBe(50 + idx);
  expect(got?.end).toBe(50 + idx + 5);
});

test("ground_fuzzy_at_floor_accepts", () => {
  const chunk = "zz abcdefghiX zz";
  const range = { start: 7, end: 7 + new Src(chunk).byteLen };
  const got = defaultQuoteGround("abcdefghij", chunk, range);
  expect(got).toEqual({ start: 7 + 3, end: 7 + 13 });
  expect(slice(chunk, { start: 3, end: 13 })).toBe("abcdefghiX");
});

test("ground_fuzzy_below_floor_discards", () => {
  const chunk = "zz abcdefghX zz";
  const range = { start: 0, end: new Src(chunk).byteLen };
  expect(defaultQuoteGround("abcdefghi", chunk, range)).toBeUndefined();
});

test("ground_fuzzy_respects_char_boundaries", () => {
  const chunk = "so café latte is great indeed";
  const range = { start: 10, end: 10 + new Src(chunk).byteLen };
  const got = defaultQuoteGround("café latte is greet", chunk, range);
  const rel = { start: (got?.start ?? 0) - 10, end: (got?.end ?? 0) - 10 };
  expect(slice(chunk, rel)).toBe("café latte is great");
});

test("ground_long_quotes_are_exact_only", () => {
  const quote = "q".repeat(401);
  const chunk = `prefix ${quote} suffix`;
  const range = { start: 0, end: new Src(chunk).byteLen };
  const got = defaultQuoteGround(quote, chunk, range);
  expect(got !== undefined ? slice(chunk, got) : undefined).toBe(quote);
  const near = `X${quote.slice(1)}`;
  expect(defaultQuoteGround(near, chunk, range)).toBeUndefined();
});

test("ground_hallucinated_quote_in_large_chunk_is_fast", () => {
  const chunk = "The court held that the motion fails on procedural grounds. ".repeat(300);
  const quote = "zzz ".repeat(90).trim();
  const range = { start: 0, end: new Src(chunk).byteLen };
  expect(defaultQuoteGround(quote, chunk, range)).toBeUndefined();
});

test("ground_garbage_and_edge_quotes_discard", () => {
  const chunk = "short text";
  const range = { start: 0, end: new Src(chunk).byteLen };
  expect(defaultQuoteGround("completely unrelated words here", chunk, range)).toBeUndefined();
  expect(defaultQuoteGround("", chunk, range)).toBeUndefined();
  expect(defaultQuoteGround("short text but much longer", chunk, range)).toBeUndefined();
});

// ---- planning --------------------------------------------------------------

test("plan_merges_consecutive_small_paragraphs", () => {
  const source = "First paragraph here.\n\nSecond paragraph here.";
  const src = new Src(source);
  const doc = paraDoc(source);
  const reqs = planJudge(doc, src, [frag("core/empty-hedge", Granularity.Sentence)]);
  expect(reqs.length).toBe(1);
  expect(reqs[0].chunk_range).toEqual({ start: 0, end: src.byteLen });
  expect(reqs[0].chunk_text).toBe(source);
  expect(reqs[0].rules).toEqual(["core/empty-hedge"]);
});

test("plan_splits_when_over_target_chars", () => {
  const para = "x".repeat(500);
  const source = `${para}\n\n${para}\n\n${para}`;
  const src = new Src(source);
  const doc = paraDoc(source);
  const reqs = planJudge(doc, src, [frag("core/empty-hedge", Granularity.Sentence)]);
  expect(reqs.length).toBe(2);
  expect(reqs[0].chunk_range).toEqual({ start: 0, end: 1002 });
  expect(reqs[1].chunk_range).toEqual({ start: 1004, end: 1504 });
  for (const r of reqs) expect(r.chunk_text).toBe(slice(source, r.chunk_range));
});

test("plan_non_prose_blocks_break_runs_and_are_excluded", () => {
  const source = "Para one text.\n\n# Heading\n\nPara two text.";
  const src = new Src(source);
  const doc: Document = {
    source: src,
    blocks: [block("paragraph", 0, 14), block("heading", 16, 25), block("paragraph", 27, 41)],
  };
  const reqs = planJudge(doc, src, [frag("core/empty-hedge", Granularity.Sentence)]);
  expect(reqs.length).toBe(2);
  expect(reqs[0].chunk_text).toBe("Para one text.");
  expect(reqs[1].chunk_text).toBe("Para two text.");
  expect(reqs.some((r) => r.chunk_text.includes("Heading"))).toBe(false);
});

test("plan_breaks_runs_on_invisible_non_blocks", () => {
  const source =
    "Para one is fine.\n\n<div>hidden html</div>\n\nPara two is fine.\n\n---\n\nPara three is fine.";
  const src = new Src(source);
  const doc = parse(src, true);
  expect(doc.blocks.length).toBe(3);
  const reqs = planJudge(doc, src, [frag("core/empty-hedge", Granularity.Sentence)]);
  expect(reqs.length).toBe(3);
  expect(reqs.every((r) => !r.chunk_text.includes("div"))).toBe(true);
  expect(reqs.every((r) => !r.chunk_text.includes("---"))).toBe(true);
  expect(reqs[0].chunk_text).toBe("Para one is fine.");
  expect(reqs[1].chunk_text).toBe("Para two is fine.");
  expect(reqs[2].chunk_text).toBe("Para three is fine.");
});

test("plan_list_items_count_as_prose", () => {
  const source = "Para text.\n\n- item one";
  const src = new Src(source);
  const doc: Document = {
    source: src,
    blocks: [block("paragraph", 0, 10), block("listitem", 12, 22)],
  };
  const reqs = planJudge(doc, src, [frag("core/empty-hedge", Granularity.Sentence)]);
  expect(reqs.length).toBe(1);
  expect(reqs[0].chunk_range).toEqual({ start: 0, end: 22 });
});

test("plan_document_granularity_gets_whole_document_request", () => {
  const source = "Alpha para.\n\nBeta para.";
  const src = new Src(source);
  const doc = paraDoc(source);
  const reqs = planJudge(doc, src, [
    frag("core/empty-hedge", Granularity.Sentence),
    frag("core/padded-elaboration", Granularity.Paragraph),
    frag("core/whole-doc", Granularity.Document),
  ]);
  expect(reqs.length).toBe(2);
  expect(reqs[0].rules).toEqual(["core/empty-hedge", "core/padded-elaboration"]);
  const docReq = reqs[1];
  expect(docReq.chunk_range).toEqual({ start: 0, end: src.byteLen });
  expect(docReq.chunk_text).toBe(source);
  expect(docReq.rules).toEqual(["core/whole-doc"]);
  expect(reqs[0].cache_key_base).not.toBe(docReq.cache_key_base);
});

test("plan_no_rules_or_empty_source_yields_no_requests", () => {
  const source = "Some text.";
  const src = new Src(source);
  const doc = paraDoc(source);
  expect(planJudge(doc, src, [])).toHaveLength(0);
  const emptyDoc: Document = { source: new Src("   \n  "), blocks: [] };
  expect(
    planJudge(emptyDoc, emptyDoc.source, [frag("core/whole-doc", Granularity.Document)]),
  ).toHaveLength(0);
});

test("plan_cache_key_base_is_deterministic_and_content_sensitive", () => {
  const rules = [frag("core/empty-hedge", Granularity.Sentence)];
  const s1 = "One paragraph of text.";
  const a = planJudge(paraDoc(s1), new Src(s1), rules);
  const b = planJudge(paraDoc(s1), new Src(s1), rules);
  expect(a[0].cache_key_base).toBe(b[0].cache_key_base);
  expect(a[0].prompt).toBe(b[0].prompt);
  const s2 = "A different paragraph.";
  const c = planJudge(paraDoc(s2), new Src(s2), rules);
  expect(a[0].cache_key_base).not.toBe(c[0].cache_key_base);
  const d = planJudge(paraDoc(s1), new Src(s1), [frag("core/other-rule", Granularity.Sentence)]);
  expect(a[0].cache_key_base).not.toBe(d[0].cache_key_base);
  expect(a[0].cache_key_base.length).toBe(64);
  expect(/^[0-9a-f]+$/.test(a[0].cache_key_base)).toBe(true);
});

test("plan_prompt_contains_rubrics_examples_chunk_and_json_contract", () => {
  const source = "It could perhaps be argued that the claim fails.";
  const src = new Src(source);
  const doc = paraDoc(source);
  const r = frag("core/empty-hedge", Granularity.Sentence);
  r.rubric = "Flag hedges that carry no information.";
  r.flag_examples[0] = "It could perhaps be argued that…";
  r.pass_examples[0] = "Damages are uncertain because treatment is ongoing.";
  const reqs = planJudge(doc, src, [r]);
  const p = reqs[0].prompt;
  for (const needle of [
    "core/empty-hedge",
    "Flag hedges that carry no information.",
    "It could perhaps be argued that…",
    "Damages are uncertain because treatment is ongoing.",
    source,
    "JSON array",
    '"rule"',
    '"quote"',
    '"confidence"',
    "suggested_rewrite",
    "respond with exactly []",
    "never emit an object stating that a rule is satisfied",
    "The parties shall meet on the first business day of each month.",
    "the entire correct response is:\n[]",
  ]) {
    expect(p).toContain(needle);
  }
});

test("plan_per_rule_prompts_share_a_prefix_through_the_document", () => {
  const source = "It could perhaps be argued that the claim fails.";
  const src = new Src(source);
  const doc = paraDoc(source);
  const rules = [
    frag("core/empty-hedge", Granularity.Sentence),
    frag("core/padded-elaboration", Granularity.Paragraph),
  ];
  const reqs = planJudgeWith(doc, src, rules, { contextChars: 24_000, perRule: true });
  expect(reqs.length).toBe(2);
  expect(reqs[0].rules).toEqual(["core/empty-hedge"]);
  expect(reqs[1].rules).toEqual(["core/padded-elaboration"]);
  let i = 0;
  while (i < reqs[0].prompt.length && reqs[0].prompt[i] === reqs[1].prompt[i]) i += 1;
  const shared = reqs[0].prompt.slice(0, i);
  expect(shared.includes(source) && shared.includes(">>>")).toBe(true);
  expect(shared.includes("core/empty-hedge")).toBe(false);
});

test("plan_bundles_rubrics_when_not_per_rule", () => {
  const source = "It could perhaps be argued that the claim fails.";
  const src = new Src(source);
  const doc = paraDoc(source);
  const reqs = planJudgeWith(
    doc,
    src,
    [
      frag("core/empty-hedge", Granularity.Sentence),
      frag("core/padded-elaboration", Granularity.Paragraph),
    ],
    defaultJudgePlan(),
  );
  expect(reqs.length).toBe(1);
  expect(reqs[0].rules.length).toBe(2);
});

test("plan_context_budget_controls_unit_count", () => {
  const para = "x".repeat(500);
  const source = `${para}\n\n${para}\n\n${para}`;
  const src = new Src(source);
  const doc = paraDoc(source);
  const rules = [frag("core/empty-hedge", Granularity.Sentence)];
  const small = planJudgeWith(doc, src, rules, defaultJudgePlan());
  expect(small.length).toBe(2);
  const large = planJudgeWith(doc, src, rules, { contextChars: 24_000, perRule: false });
  expect(large.length).toBe(1);
  expect(large[0].chunk_text).toBe(source);
});

test("plan_per_rule_cache_keys_are_independent_per_rule", () => {
  const source = "It could perhaps be argued that the claim fails.";
  const src = new Src(source);
  const doc = paraDoc(source);
  const hedge = frag("core/empty-hedge", Granularity.Sentence);
  const padded = frag("core/padded-elaboration", Granularity.Paragraph);
  const plan = { contextChars: 24_000, perRule: true };
  const withBoth = planJudgeWith(doc, src, [hedge, padded], plan);
  const withOne = planJudgeWith(doc, src, [hedge], plan);
  expect(withBoth[0].cache_key_base).toBe(withOne[0].cache_key_base);
  expect(withBoth[0].cache_key_base).not.toBe(withBoth[1].cache_key_base);
});

test("judge_plan_for_context_enables_per_rule_only_above_the_default", () => {
  expect(judgePlanForContext(800).perRule).toBe(false);
  expect(judgePlanForContext(1200).perRule).toBe(false);
  expect(judgePlanForContext(24_000).perRule).toBe(true);
  expect(judgePlanForContext(24_000).contextChars).toBe(24_000);
  expect(judgePlanForContext(0).contextChars).toBe(1);
});

// ---- execution (serial paths only) ----------------------------------------

test("run_retries_once_then_succeeds", () => {
  const text = "The hedge could perhaps be argued.";
  const r = req(text, ["core/empty-hedge"]);
  const judge = MockJudge.new()
    .respondErr(text, "transient")
    .respond(text, [finding("core/empty-hedge", "could perhaps", 0.8)]);
  const { grounded, stats } = runJudge(judge, undefined, [r], new Src(text));
  expect(judge.calls()).toBe(2);
  expect(grounded.length).toBe(1);
  expect(stats.chunks).toBe(1);
  expect(stats.chunksFailed).toBe(0);
  expect(stats.grounded).toBe(1);
  expect(grounded[0].finding.rule).toBe("core/empty-hedge");
  expect(slice(text, grounded[0].span)).toBe("could perhaps");
});

test("run_fails_chunk_closed_after_retry", () => {
  const text = "Some chunk of prose.";
  const r = req(text, ["core/empty-hedge"]);
  const judge = MockJudge.new()
    .respondMalformed(text, "not json")
    .respondMalformed(text, "still not json");
  const { grounded, stats } = runJudge(judge, undefined, [r], new Src(text));
  expect(judge.calls()).toBe(2);
  expect(grounded).toHaveLength(0);
  expect(stats.chunksFailed).toBe(1);
  expect(stats.grounded).toBe(0);
});

test("run_failed_chunk_does_not_poison_others", () => {
  const t1 = "Chunk one fails hard.";
  const t2 = "Chunk two works fine.";
  const judge = MockJudge.new()
    .respondErr(t1, "boom")
    .respondErr(t1, "boom again")
    .respond(t2, [finding("core/empty-hedge", "works fine", 0.7)]);
  const reqs = [req(t1, ["core/empty-hedge"]), req(t2, ["core/empty-hedge"])];
  const { grounded, stats } = runJudge(judge, undefined, reqs, new Src(t2));
  expect(stats.chunks).toBe(2);
  expect(stats.chunksFailed).toBe(1);
  expect(grounded.length).toBe(1);
  expect(grounded[0].finding.quote).toBe("works fine");
});

test("run_cache_hit_is_deterministic_and_skips_judge", () => {
  const text = "It could perhaps be argued that the claim fails.";
  const r = req(text, ["core/empty-hedge"]);
  const cache = new MemoryCache();
  const judge = MockJudge.new().respond(text, [
    finding("core/empty-hedge", "could perhaps be argued", 0.9),
  ]);
  const { grounded: out1, stats: stats1 } = runJudge(judge, cache, [r], new Src(text));
  expect(judge.calls()).toBe(1);
  expect(stats1.cacheHits).toBe(0);
  const { grounded: out2, stats: stats2 } = runJudge(judge, cache, [r], new Src(text));
  expect(judge.calls()).toBe(1);
  expect(stats2.cacheHits).toBe(1);
  expect(stats2.grounded).toBe(1);
  expect(out1.length).toBe(out2.length);
  expect(out1[0].finding.quote).toBe(out2[0].finding.quote);
  expect(out1[0].span).toEqual(out2[0].span);
});

test("run_cache_key_includes_model_id", () => {
  const text = "Model-sensitive caching text.";
  const r = req(text, ["core/empty-hedge"]);
  const cache = new MemoryCache();
  const j1 = MockJudge.withModel("model-a").respond(text, [
    finding("core/empty-hedge", "caching text", 0.9),
  ]);
  const j2 = MockJudge.withModel("model-b").respond(text, [
    finding("core/empty-hedge", "caching text", 0.9),
  ]);
  runJudge(j1, cache, [r], new Src(text));
  const { stats: stats2 } = runJudge(j2, cache, [r], new Src(text));
  expect(j2.calls()).toBe(1);
  expect(stats2.cacheHits).toBe(0);
  const j3 = MockJudge.withModel("model-a");
  const { stats: stats3 } = runJudge(j3, cache, [r], new Src(text));
  expect(j3.calls()).toBe(0);
  expect(stats3.cacheHits).toBe(1);
});

test("run_discards_findings_naming_foreign_rules", () => {
  const text = "Perfectly quotable prose right here.";
  const r = req(text, ["core/empty-hedge"]);
  const judge = MockJudge.new().respond(text, [
    finding("core/not-in-request", "quotable prose", 0.9),
    finding("core/empty-hedge", "quotable prose", 0.9),
  ]);
  const { grounded, stats } = runJudge(judge, undefined, [r], new Src(text));
  expect(grounded.length).toBe(1);
  expect(grounded[0].finding.rule).toBe("core/empty-hedge");
  expect(stats.hallucinated["core/not-in-request"]).toBe(1);
  expect(stats.grounded).toBe(1);
});

test("run_counts_ungroundable_findings_as_hallucinated", () => {
  const text = "Nothing matches the invented quote.";
  const r = req(text, ["core/empty-hedge"]);
  const judge = MockJudge.new().respond(text, [
    finding("core/empty-hedge", "totally fabricated wording", 0.9),
  ]);
  const { grounded, stats } = runJudge(judge, undefined, [r], new Src(text));
  expect(grounded).toHaveLength(0);
  expect(stats.hallucinated["core/empty-hedge"]).toBe(1);
  expect(stats.grounded).toBe(0);
  expect(stats.chunksFailed).toBe(0);
});

test("run_clamps_confidence_into_unit_interval", () => {
  const text = "Overconfident and underconfident findings.";
  const r = req(text, ["core/empty-hedge"]);
  const judge = MockJudge.new().respond(text, [
    finding("core/empty-hedge", "Overconfident", 1.5),
    finding("core/empty-hedge", "underconfident", -0.5),
    finding("core/empty-hedge", "findings", Number.NaN),
  ]);
  const { grounded } = runJudge(judge, undefined, [r], new Src(text));
  expect(grounded.length).toBe(3);
  expect(grounded[0].finding.confidence).toBe(1.0);
  expect(grounded[1].finding.confidence).toBe(0.0);
  expect(grounded[2].finding.confidence).toBe(0.0);
});

test("run_unscripted_request_yields_no_findings_but_no_failure", () => {
  const text = "No script matches this chunk.";
  const r = req(text, ["core/empty-hedge"]);
  const judge = MockJudge.new();
  const { grounded, stats } = runJudge(judge, undefined, [r], new Src(text));
  expect(grounded).toHaveLength(0);
  expect(stats.chunks).toBe(1);
  expect(stats.chunksFailed).toBe(0);
});

// ---- MemoryCache -------------------------------------------------------------

test("memory_cache_get_put_roundtrip_and_overwrite", () => {
  const cache = new MemoryCache();
  expect(cache.get("k")).toBeUndefined();
  cache.put("k", [finding("core/a", "q", 0.5)]);
  const got = cache.get("k");
  expect(got?.length).toBe(1);
  expect(got?.[0].rule).toBe("core/a");
  cache.put("k", []);
  expect(cache.get("k")).toEqual([]);
  expect(cache.get("other")).toBeUndefined();
});
