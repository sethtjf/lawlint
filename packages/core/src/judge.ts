// Tier-3 (inferential) pipeline — mirrors judge.rs.

import { sha256 } from "@noble/hashes/sha256";
import type { Document } from "./document";
import { JudgeError } from "./error";
import type { Severity, TextRange } from "./types";
import { type Src, normalizedLevenshtein, utf8Len, wsTrim } from "./utf8";

export const PROMPT_VERSION = "3";
const TARGET_CHUNK_CHARS = 1200;
const GROUND_FLOOR = 0.9;
const MAX_FUZZY_QUOTE_CHARS = 400;

export enum Granularity {
  Sentence = "sentence",
  Paragraph = "paragraph",
  Document = "document",
}

export interface RubricFragment {
  rule: string;
  severity: Severity;
  granularity: Granularity;
  rubric: string;
  flag_examples: string[];
  pass_examples: string[];
}

export interface JudgeRequest {
  chunk_range: TextRange;
  chunk_text: string;
  rules: string[];
  prompt: string;
  cache_key_base: string;
}

export interface JudgeFinding {
  rule: string;
  quote: string;
  explanation: string;
  confidence: number;
  suggested_rewrite?: string;
}

export interface JudgeStats {
  chunks: number;
  cacheHits: number;
  chunksFailed: number;
  grounded: number;
  hallucinated: Record<string, number>;
  firstFailure?: string;
}

export interface Judge {
  evaluate(req: JudgeRequest): JudgeFinding[];
  modelId(): string;
  maxConcurrency?(): number;
}

export interface JudgeCache {
  get(key: string): JudgeFinding[] | undefined;
  put(key: string, findings: JudgeFinding[]): void;
}

function sha256Hex(input: string): string {
  return [...sha256(new TextEncoder().encode(input))]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

function rubricSetHash(rubrics: RubricFragment[]): string {
  // serde_json of &RubricFragment: fields in declaration order, snake_case.
  const json = JSON.stringify(
    rubrics.map((r) => ({
      rule: r.rule,
      severity: r.severity,
      granularity: r.granularity,
      rubric: r.rubric,
      flag_examples: r.flag_examples,
      pass_examples: r.pass_examples,
    })),
  );
  return sha256Hex(json);
}

function fullCacheKey(base: string, modelId: string): string {
  return sha256Hex(`${base}${modelId}`);
}

const SEV_NAME: Record<Severity, string> = {
  error: "error",
  warning: "warning",
  suggestion: "suggestion",
};

function buildPrompt(chunkText: string, rubrics: RubricFragment[]): string {
  let p =
    "You are a strict legal-writing reviewer. Evaluate the text below against each rule that follows it. Only report real violations.\n\n";
  p += "Text to evaluate:\n<<<\n";
  p += chunkText;
  p += "\n>>>\n\n";
  for (const r of rubrics) {
    p += `Rule \`${r.rule}\` (severity: ${SEV_NAME[r.severity]}, granularity: ${r.granularity}):\n`;
    p += `Rubric: ${r.rubric}\n`;
    p += "Flag examples (violations):\n";
    for (const ex of r.flag_examples) p += `- ${ex}\n`;
    p += "Pass examples (acceptable, do NOT flag):\n";
    for (const ex of r.pass_examples) p += `- ${ex}\n`;
    p += "\n";
  }
  p +=
    'Respond with ONLY a JSON array (no prose, no code fences). Each element must be an object of the form {"rule": "<one of the rule ids above>", "quote": "<excerpt copied VERBATIM from the text>", "explanation": "<one short sentence stating the violation>", "confidence": <number between 0.0 and 1.0>, "suggested_rewrite": "<replacement text>" or null}. The quote must appear verbatim in the text. Report ONLY violations: never emit an object stating that a rule is satisfied, not violated, or not present. If nothing violates any rule, respond with exactly []. Example — for the clean text "The parties shall meet on the first business day of each month." the entire correct response is:\n[]';
  return p;
}

function buildRequest(range: TextRange, text: string, rubrics: RubricFragment[]): JudgeRequest {
  const setHash = rubricSetHash(rubrics);
  const cache_key_base = sha256Hex(`${text}${setHash}${PROMPT_VERSION}`);
  return {
    chunk_range: range,
    chunk_text: text,
    rules: rubrics.map((r) => r.rule),
    prompt: buildPrompt(text, rubrics),
    cache_key_base,
  };
}

export interface JudgePlan {
  contextChars: number;
  perRule: boolean;
}

export function defaultJudgePlan(): JudgePlan {
  return { contextChars: TARGET_CHUNK_CHARS, perRule: false };
}

export function judgePlanForContext(contextChars: number): JudgePlan {
  return { contextChars: Math.max(1, contextChars), perRule: contextChars > TARGET_CHUNK_CHARS };
}

function chunkProseBlocks(doc: Document, src: Src, contextChars: number): TextRange[] {
  const chunks: TextRange[] = [];
  let current: TextRange | undefined;
  let currentChars = 0;
  for (const block of doc.blocks) {
    const prose = block.kind === "paragraph" || block.kind === "listitem";
    if (!prose) {
      if (current) chunks.push(current);
      current = undefined;
      currentChars = 0;
      continue;
    }
    const blockChars = [...src.slice(block.range.start, block.range.end)].length;
    const gapIsBlank = (r: TextRange) =>
      [...src.slice(r.end, block.range.start)].every((c) => /\p{White_Space}/u.test(c));
    if (current !== undefined && currentChars + blockChars <= contextChars && gapIsBlank(current)) {
      current.end = block.range.end;
      currentChars += blockChars;
    } else if (current !== undefined) {
      chunks.push(current);
      current = { ...block.range };
      currentChars = blockChars;
    } else {
      current = { ...block.range };
      currentChars = blockChars;
    }
  }
  if (current) chunks.push(current);
  return chunks;
}

export function planJudge(doc: Document, src: Src, rules: RubricFragment[]): JudgeRequest[] {
  return planJudgeWith(doc, src, rules, defaultJudgePlan());
}

export function planJudgeWith(
  doc: Document,
  src: Src,
  rules: RubricFragment[],
  plan: JudgePlan,
): JudgeRequest[] {
  const chunkRubrics = rules.filter(
    (r) => r.granularity === Granularity.Sentence || r.granularity === Granularity.Paragraph,
  );
  const docRubrics = rules.filter((r) => r.granularity === Granularity.Document);
  const split = (rs: RubricFragment[]): RubricFragment[][] =>
    plan.perRule ? rs.map((r) => [r]) : [rs];

  const reqs: JudgeRequest[] = [];
  if (chunkRubrics.length > 0) {
    for (const range of chunkProseBlocks(doc, src, plan.contextChars)) {
      const text = src.slice(range.start, range.end);
      if (wsTrim(text).length === 0) continue;
      for (const group of split(chunkRubrics)) {
        reqs.push(buildRequest(range, text, group));
      }
    }
  }
  if (docRubrics.length > 0 && wsTrim(src.text).length > 0) {
    const range = { start: 0, end: src.byteLen };
    for (const group of split(docRubrics)) {
      reqs.push(buildRequest(range, src.text, group));
    }
  }
  return reqs;
}

type Fetched =
  | { kind: "cached"; findings: JudgeFinding[] }
  | { kind: "fresh"; findings: JudgeFinding[] }
  | { kind: "failed"; error: string };

function fetchOne(judge: Judge, cache: JudgeCache | undefined, req: JudgeRequest): Fetched {
  const key = fullCacheKey(req.cache_key_base, judge.modelId());
  const hit = cache?.get(key);
  if (hit !== undefined) return { kind: "cached", findings: hit };
  try {
    const findings = judge.evaluate(req);
    cache?.put(key, findings);
    return { kind: "fresh", findings };
  } catch {
    try {
      const findings = judge.evaluate(req); // one retry
      cache?.put(key, findings);
      return { kind: "fresh", findings };
    } catch (e) {
      return { kind: "failed", error: e instanceof Error ? e.message : String(e) };
    }
  }
}

export type GroundedFinding = { req: JudgeRequest; finding: JudgeFinding; span: TextRange };

export function runJudge(
  judge: Judge,
  cache: JudgeCache | undefined,
  reqs: JudgeRequest[],
  src: Src,
): { grounded: GroundedFinding[]; stats: JudgeStats } {
  const stats: JudgeStats = {
    chunks: reqs.length,
    cacheHits: 0,
    chunksFailed: 0,
    grounded: 0,
    hallucinated: {},
  };
  const out: GroundedFinding[] = [];
  for (const req of reqs) {
    const fetched = fetchOne(judge, cache, req);
    let findings: JudgeFinding[];
    if (fetched.kind === "cached") {
      stats.cacheHits += 1;
      findings = fetched.findings;
    } else if (fetched.kind === "fresh") {
      findings = fetched.findings;
    } else {
      stats.chunksFailed += 1;
      if (stats.firstFailure === undefined) stats.firstFailure = fetched.error;
      continue;
    }
    for (const finding of findings) {
      if (!req.rules.includes(finding.rule)) {
        stats.hallucinated[finding.rule] = (stats.hallucinated[finding.rule] ?? 0) + 1;
        continue;
      }
      const confidence = Number.isNaN(finding.confidence)
        ? 0
        : Math.min(1, Math.max(0, finding.confidence));
      const f = { ...finding, confidence: Math.fround(confidence) };
      const span = defaultQuoteGround(f.quote, req.chunk_text, req.chunk_range, src);
      if (span !== undefined) {
        stats.grounded += 1;
        out.push({ req, finding: f, span });
      } else {
        stats.hallucinated[f.rule] = (stats.hallucinated[f.rule] ?? 0) + 1;
      }
    }
  }
  return { grounded: out, stats };
}

/** Ground a finding's quote inside its chunk; absolute byte range out. */
export function defaultQuoteGround(
  quote: string,
  chunkText: string,
  chunkRange: TextRange,
  src?: Src,
): TextRange | undefined {
  if (quote.length === 0) return undefined;
  // Exact substring (UTF-16 index → byte offset within the chunk).
  const idx = chunkText.indexOf(quote);
  if (idx !== -1) {
    const byteStart = chunkRange.start + utf8Len(chunkText.slice(0, idx));
    return { start: byteStart, end: byteStart + utf8Len(quote) };
  }
  const quoteChars = [...quote].length;
  if (quoteChars > MAX_FUZZY_QUOTE_CHARS) return undefined;
  const chunkCharsArr = [...chunkText];
  const chunkChars = chunkCharsArr.length;
  if (quoteChars > chunkChars) return undefined;
  // char boundaries in bytes within chunkText
  const boundaries: number[] = [0];
  {
    let b = 0;
    for (const ch of chunkCharsArr.slice(0, -1)) {
      b += utf8Len(ch);
      boundaries.push(b);
    }
    boundaries.push(b + utf8Len(chunkCharsArr[chunkCharsArr.length - 1] ?? ""));
    // boundaries now has chunkChars+1 entries
  }
  // Rolling char-multiset prefilter.
  const diff = new Map<string, number>();
  for (const c of quote) diff.set(c, (diff.get(c) ?? 0) - 1);
  let surplus = 0;
  for (let i = 0; i < quoteChars; i++) {
    const e = (diff.get(chunkCharsArr[i]) ?? 0) + 1;
    diff.set(chunkCharsArr[i], e);
    if (e > 0) surplus += 1;
  }
  let best: { score: number; start: number } | undefined;
  for (let start = 0; start <= chunkChars - quoteChars; start++) {
    if (start > 0) {
      const out = chunkCharsArr[start - 1];
      let e = diff.get(out) ?? 0;
      if (e > 0) surplus -= 1;
      diff.set(out, e - 1);
      const inc = chunkCharsArr[start + quoteChars - 1];
      e = (diff.get(inc) ?? 0) + 1;
      diff.set(inc, e);
      if (e > 0) surplus += 1;
    }
    const bound = 1 - surplus / quoteChars;
    if (bound < GROUND_FLOOR) continue;
    const window = chunkCharsArr.slice(start, start + quoteChars).join("");
    const score = normalizedLevenshtein(quote, window);
    if (best === undefined || score > best.score) best = { score, start };
  }
  if (best === undefined || best.score < GROUND_FLOOR) return undefined;
  return {
    start: chunkRange.start + boundaries[best.start],
    end: chunkRange.start + boundaries[best.start + quoteChars],
  };
}

// ---- MockJudge ----------------------------------------------------------

type ScriptedResponse =
  | { kind: "findings"; findings: JudgeFinding[] }
  | { kind: "backend"; message: string }
  | { kind: "malformed"; raw: string };

export class MockJudge implements Judge {
  private readonly model: string;
  private readonly script: [string, ScriptedResponse[]][] = [];
  private callCount = 0;

  static new(): MockJudge {
    return MockJudge.withModel("mock");
  }
  static withModel(model: string): MockJudge {
    return new MockJudge(model);
  }
  constructor(model: string) {
    this.model = model;
  }
  respond(matcher: string, findings: JudgeFinding[]): this {
    this.push(matcher, { kind: "findings", findings });
    return this;
  }
  respondErr(matcher: string, message: string): this {
    this.push(matcher, { kind: "backend", message });
    return this;
  }
  respondMalformed(matcher: string, raw: string): this {
    this.push(matcher, { kind: "malformed", raw });
    return this;
  }
  calls(): number {
    return this.callCount;
  }
  private push(matcher: string, response: ScriptedResponse) {
    const entry = this.script.find(([m]) => m === matcher);
    if (entry) entry[1].push(response);
    else this.script.push([matcher, [response]]);
  }
  evaluate(req: JudgeRequest): JudgeFinding[] {
    this.callCount += 1;
    for (const [matcher, queue] of this.script) {
      if (matcher.length === 0 || req.chunk_text.includes(matcher)) {
        const response = queue.shift();
        if (response !== undefined) {
          if (response.kind === "findings") return response.findings;
          if (response.kind === "backend") throw new JudgeError("backend", response.message);
          throw new JudgeError("malformed", response.raw);
        }
      }
    }
    return [];
  }
  modelId(): string {
    return this.model;
  }
}

// ---- MemoryCache ---------------------------------------------------------

export class MemoryCache implements JudgeCache {
  private readonly inner = new Map<string, JudgeFinding[]>();
  get(key: string): JudgeFinding[] | undefined {
    const v = this.inner.get(key);
    return v === undefined ? undefined : [...v];
  }
  put(key: string, findings: JudgeFinding[]): void {
    this.inner.set(key, [...findings]);
  }
}
