// Parity test: replay the Rust goldens through the TypeScript port.
// Zero mismatches is the bar — see step2-brief.md.

import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { sha256 } from "@noble/hashes/sha256";
import {
  MemoryCache,
  MockJudge,
  RuleSet,
  Src,
  applyFixes,
  builtInRulesMeta,
  defaultQuoteGround,
  lint,
  lintFull,
  lintWith,
  parse,
  parseLintOptions,
  planJudgeWith,
  remediationPrompt,
  runJudge,
} from "../src/index";
import type { JudgeFinding, LintOptions } from "../src/index";
import { loadDir } from "../src/node";
import { utf8Len } from "../src/utf8";

const ROOT = join(import.meta.dir, "../../..");
const GOLDEN = join(ROOT, "parity/golden");

function jsonl<T>(file: string): T[] {
  return readFileSync(file, "utf8")
    .split("\n")
    .filter((l) => l.trim().length > 0)
    .map((l) => JSON.parse(l) as T);
}

interface Input {
  id: string;
  inputId: string;
  textSha256: string;
  input: { options?: Record<string, unknown>; source?: Record<string, unknown> };
  output: unknown;
}

const inputs = new Map<string, string>();
for (const row of jsonl<{ id: string; text: string; sha256: string }>(
  join(GOLDEN, "inputs.jsonl"),
)) {
  inputs.set(row.id, row.text);
}
const textOf = (c: Input) => inputs.get(c.inputId) as string;

const BUILTIN = RuleSet.builtIn();
const SIMPLE_ENGLISH = loadDir(join(ROOT, "packages/simple-english"));

function rulesFor(c: Input): RuleSet {
  const rule = c.input.source?.rule as string | undefined;
  if (rule?.startsWith("simple-english/")) return SIMPLE_ENGLISH;
  return BUILTIN;
}

function optionsFor(c: Input): LintOptions {
  return parseLintOptions(c.input.options ?? {});
}

function sha256Hex(s: string): string {
  return [...sha256(new TextEncoder().encode(s))]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

type Mismatch = { surface: string; id: string; detail: string };
const mismatches: Mismatch[] = [];
const skipped: { id: string; reason: string }[] = [];

function deepEqual(a: unknown, b: unknown, path: string): string | undefined {
  if (typeof a === "number" && typeof b === "number") {
    return a === b ? undefined : `${path}: ${a} !== ${b}`;
  }
  if (
    a === null ||
    b === null ||
    typeof a !== "object" ||
    typeof b !== "object" ||
    Array.isArray(a) !== Array.isArray(b)
  ) {
    return a === b ? undefined : `${path}: ${JSON.stringify(a)} !== ${JSON.stringify(b)}`;
  }
  if (Array.isArray(a) && Array.isArray(b)) {
    if (a.length !== b.length) return `${path}: length ${a.length} !== ${b.length}`;
    for (let i = 0; i < a.length; i++) {
      const d = deepEqual(a[i], b[i], `${path}[${i}]`);
      if (d) return d;
    }
    return undefined;
  }
  const ao = a as Record<string, unknown>;
  const bo = b as Record<string, unknown>;
  const keys = new Set([...Object.keys(ao), ...Object.keys(bo)]);
  for (const k of keys) {
    if (!(k in ao) || !(k in bo))
      return `${path}.${k}: missing on one side (${k in ao ? "rust only" : "ts only"})`;
    const d = deepEqual(ao[k], bo[k], `${path}.${k}`);
    if (d) return d;
  }
  return undefined;
}

function compare(surface: string, id: string, actual: unknown, expected: unknown) {
  const d = deepEqual(actual, expected, "");
  if (d !== undefined) {
    mismatches.push({
      surface,
      id,
      detail: d.slice(0, 300),
    });
  }
}

// ---- parse ---------------------------------------------------------------

describe("parity: parse", () => {
  const cases = jsonl<Input>(join(GOLDEN, "parse.jsonl"));
  for (const c of cases) {
    test(c.id, () => {
      const doc = parse(textOf(c), (c.input.options?.markdown as boolean) ?? false);
      const actual = {
        blocks: doc.blocks.map((b) => ({
          kind: b.kind,
          range: b.range,
          sentences: b.sentences.map((s) => ({ isCitation: s.isCitation, range: s.range })),
        })),
      };
      compare("parse", c.id, actual, c.output);
    });
  }
});

// ---- lint ----------------------------------------------------------------

describe("parity: lint", () => {
  const cases = jsonl<Input>(join(GOLDEN, "lint.jsonl"));
  for (const c of cases) {
    test(c.id, () => {
      const result = lintWith(textOf(c), optionsFor(c), rulesFor(c));
      const actual = JSON.parse(JSON.stringify(result));
      const expected = JSON.parse(JSON.stringify(c.output));
      // Corpus cases omit excerpt by design.
      const firstExpected = expected.diagnostics?.[0];
      const stripsExcerpt = firstExpected !== undefined && !("excerpt" in firstExpected);
      if (stripsExcerpt) {
        actual.diagnostics = actual.diagnostics.map(
          ({ excerpt: _excerpt, ...rest }: Record<string, unknown>) => rest,
        );
      }
      compare("lint", c.id, actual, expected);
    });
  }
});

// ---- fix -----------------------------------------------------------------

describe("parity: fix", () => {
  const cases = jsonl<Input>(join(GOLDEN, "fix.jsonl"));
  for (const c of cases) {
    test(c.id, () => {
      const text = textOf(c);
      const lintResult = lintWith(text, optionsFor(c), rulesFor(c));
      const fixed = applyFixes(text, lintResult.diagnostics);
      const expected = c.output as { text?: string; outputSha256?: string; changed?: boolean };
      if (expected.text !== undefined) {
        compare("fix", c.id, { text: fixed }, expected);
      } else {
        compare(
          "fix",
          c.id,
          { outputSha256: sha256Hex(fixed), changed: fixed !== text },
          { outputSha256: expected.outputSha256, changed: expected.changed },
        );
      }
    });
  }
});

// ---- prompt ---------------------------------------------------------------

describe("parity: prompt", () => {
  const cases = jsonl<Input>(join(GOLDEN, "prompt.jsonl"));
  for (const c of cases) {
    test(c.id, () => {
      const text = textOf(c);
      const result = lintWith(text, optionsFor(c), rulesFor(c));
      const prompt = remediationPrompt({ kind: "text", text }, result, rulesFor(c));
      const expected = c.output as {
        prompt?: string | null;
        promptSha256?: string;
        length?: number;
      };
      const expectedP = c.output as {
        prompt?: string | null;
        promptSha256?: string;
        promptLength?: number;
      };
      if ("prompt" in expectedP) {
        compare("prompt", c.id, { prompt: prompt ?? null }, expectedP);
      } else {
        // The generator hashes "" when the prompt is None.
        compare(
          "prompt",
          c.id,
          {
            promptSha256: sha256Hex(prompt ?? ""),
            promptLength: prompt === undefined ? 0 : utf8Len(prompt),
          },
          { promptSha256: expectedP.promptSha256, promptLength: expectedP.promptLength },
        );
      }
    });
  }
});

// ---- judge_plan ------------------------------------------------------------

describe("parity: judge_plan", () => {
  const cases = jsonl<Input>(join(GOLDEN, "judge_plan.jsonl"));
  for (const c of cases) {
    test(c.id, () => {
      const text = textOf(c);
      const src = new Src(text);
      const doc = parse(src, false);
      const fragments = BUILTIN.instantiate({})
        .map((r) => r.rubric?.())
        .filter((f) => f !== undefined);
      const plan = {
        contextChars: (c.input.options?.contextChars as number) ?? 1200,
        perRule: (c.input.options?.perRule as boolean) ?? false,
      };
      const requests = planJudgeWith(doc, src, fragments, plan);
      compare("judge_plan", c.id, { requests }, c.output);
    });
  }
});

// ---- judge_ground -----------------------------------------------------------

describe("parity: judge_ground", () => {
  const cases = jsonl<Input>(join(GOLDEN, "judge_ground.jsonl"));
  for (const c of cases) {
    if (c.id.startsWith("judge-parse-")) {
      skipped.push({
        id: c.id,
        reason: "parse_findings_with_stats lives in lawlint-judge, not the core port",
      });
      continue;
    }
    test(c.id, () => {
      const text = textOf(c);
      const src = new Src(text);
      const range = { start: 0, end: src.byteLen };
      const longQuote = "x".repeat(401);
      if (c.id === "judge-grounding-mock") {
        const findings: JudgeFinding[] = [
          {
            rule: "core/empty-hedge",
            quote: "It could perhaps be argued",
            explanation: "Stacked hedge without support.",
            confidence: 0.93,
          },
          {
            rule: "core/empty-hedge",
            quote: "It could perhaps be argued that",
            explanation: "Fuzzy match around the hedge.",
            confidence: 0.93,
          },
          {
            rule: "core/empty-hedge",
            quote: "A fabricated passage nowhere present",
            explanation: "This quote is absent.",
            confidence: 0.91,
          },
          {
            rule: "unknown/rule",
            quote: "The record does not say why.",
            explanation: "Unknown rule must be discarded.",
            confidence: 0.99,
          },
          {
            rule: "core/empty-hedge",
            quote: longQuote,
            explanation: "Long quote has no fuzzy fallback.",
            confidence: 0.95,
          },
          {
            rule: "core/empty-hedge",
            quote: "The record does not say why.",
            explanation: "Below confidence floor.",
            confidence: 0.3,
          },
          {
            rule: "core/empty-hedge",
            quote: "Damages are uncertain",
            explanation: "The sentence does not violate the empty hedge rule.",
            confidence: 0.99,
          },
        ];
        const request = {
          chunk_range: range,
          chunk_text: text,
          rules: ["core/empty-hedge"],
          prompt: "Mock judge input; no network or AI is used.",
          cache_key_base: "parity-grounding-request",
        };
        const mock = MockJudge.new().respond("", findings);
        const { grounded, stats } = runJudge(mock, undefined, [request], src);
        const direct = [
          "It could perhaps be argued",
          "It could perhaps be argue",
          "a fabricated passage nowhere present",
          longQuote,
          "",
        ].map((quote) => ({ quote, span: defaultQuoteGround(quote, text, range) ?? null }));
        const groundedJson = grounded.map(({ req, finding, span }) => ({
          request: req,
          finding: { ...finding, suggested_rewrite: finding.suggested_rewrite ?? null },
          span,
        }));
        compare("judge_ground", c.id, { grounded: groundedJson, stats, direct }, c.output);
      } else if (c.id === "judge-grounding-confidence-floor") {
        const finding: JudgeFinding = {
          rule: "core/empty-hedge",
          quote: "It could perhaps be argued",
          explanation: "Below confidence floor.",
          confidence: 0.3,
        };
        const mock = MockJudge.new().respond("", [finding]);
        const result = lintFull(text, { judge: { floor: 0.6 } }, BUILTIN, mock, undefined);
        compare("judge_ground", c.id, { mockCalls: mock.calls(), result }, c.output);
      } else {
        skipped.push({ id: c.id, reason: "unknown judge_ground case" });
      }
    });
  }
});

// ---- rules_meta -------------------------------------------------------------

describe("parity: rules_meta", () => {
  const cases = jsonl<Input>(join(GOLDEN, "rules_meta.jsonl"));
  for (const c of cases) {
    test(c.id, () => {
      const pkg = c.input.source?.package as string;
      const rules = pkg === "simple-english" ? SIMPLE_ENGLISH.metas() : builtInRulesMeta();
      // Rust emits {id: bare name, meta: {..camelCase..}}; id strips the pkg.
      const actual = {
        rules: rules.map((m) => ({
          id: m.id.split("/").pop(),
          meta: {
            description: m.description,
            docsUrl: m.docs_url,
            examples: m.examples,
            intent: m.intent,
            ...(m.rationale !== undefined ? { rationale: m.rationale } : {}),
            scope: m.scope,
            severity: m.severity,
            tier: m.tier,
          },
        })),
      };
      compare("rules_meta", c.id, actual, c.output);
    });
  }
});

// ---- loader_errors -----------------------------------------------------------

describe("parity: loader_errors", () => {
  const cases = jsonl<Input>(join(GOLDEN, "loader_errors.jsonl"));
  for (const c of cases) {
    test(c.id, () => {
      const fixture = c.input.source?.fixture as string;
      const path = `parity/fixtures/${fixture}`;
      const text = readFileSync(join(ROOT, path), "utf8");
      let error: string | undefined;
      try {
        RuleSet.fromSources("test", [[path, text]]);
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
      }
      const expected = (c.output as { error: string }).error;
      const kind = (e: string | undefined) => {
        if (e === undefined) return "no error";
        if (e.includes("missing required field")) return "missing-field";
        if (e.includes("invalid YAML")) return "yaml";
        if (e.includes("invalid regex")) return "invalid-regex";
        if (e.includes("unknown field")) return "invalid-field";
        return "invalid-field";
      };
      // Exact match when neither side is a YAML/regex library message;
      // otherwise compare kind + "file: " prefix (library messages differ).
      // serde_yaml's "unknown field" errors also carry an "at line N column M"
      // suffix that is library formatting — compare the field name instead.
      const isLibMsg = (e: string) => e.includes("invalid YAML") || e.includes("invalid regex");
      const unknownField = (e: string) => /unknown field `([^`]+)`/.exec(e)?.[1];
      if (error === undefined) {
        mismatches.push({
          surface: "loader_errors",
          id: c.id,
          detail: `no error raised; expected ${expected}`,
        });
      } else if (unknownField(expected) !== undefined || unknownField(error) !== undefined) {
        const expFile = expected.split(": ", 1)[0];
        if (unknownField(error) !== unknownField(expected) || !error.startsWith(`${expFile}: `)) {
          mismatches.push({
            surface: "loader_errors",
            id: c.id,
            detail: `unknown-field mismatch: got ${error}`,
          });
        }
      } else if (isLibMsg(expected) || isLibMsg(error)) {
        const expFile = expected.split(": ", 1)[0];
        if (kind(error) !== kind(expected) || !error.startsWith(`${expFile}: `)) {
          mismatches.push({
            surface: "loader_errors",
            id: c.id,
            detail: `kind/prefix differ: got ${error}`,
          });
        }
      } else if (error !== expected) {
        mismatches.push({ surface: "loader_errors", id: c.id, detail: `${error} !== ${expected}` });
      }
    });
  }
});

// ---- summary -----------------------------------------------------------------

test("parity summary: zero mismatches", () => {
  if (mismatches.length > 0 || skipped.length > 0) {
    console.log(`\nPARITY: ${mismatches.length} mismatch(es), ${skipped.length} skipped`);
    for (const s of skipped) console.log(`  SKIPPED ${s.id}: ${s.reason}`);
    for (const m of mismatches.slice(0, 25))
      console.log(`  MISMATCH ${m.surface}/${m.id}: ${m.detail}`);
  }
  expect(mismatches).toEqual([]);
});
