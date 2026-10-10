// Public entry for @lawlint/core — mirrors lawlint-core's lib.rs plus the
// wasm binding surface. Browser-safe: no node:* imports.

import type { LintOptions } from "./config";
import * as dispatch from "./dispatch";
import { parse as parseDoc } from "./document";
import {
  type Judge,
  type JudgeCache,
  type JudgePlan,
  type RubricFragment,
  defaultQuoteGround,
  planJudgeWith,
  runJudge,
} from "./judge";
import { type PromptSource, remediationPrompt as remediationPromptFn } from "./prompt";
import { RuleSet } from "./registry";
import * as scoring from "./scoring";
import type { Diagnostic, Fix, LintResult } from "./types";
import { Src, wsTrim } from "./utf8";

export type { AiOptions, JudgeOptions, LearnOptions, LintOptions } from "./config";
export { aiModel, parseLintOptions } from "./config";
export type { Block, BlockKind, Document, Sentence, Token } from "./document";
export { JudgeError, LoadError } from "./error";
export type {
  Judge,
  JudgeCache,
  JudgeFinding,
  JudgeRequest,
  JudgeStats,
  RubricFragment,
} from "./judge";
export {
  defaultJudgePlan,
  defaultQuoteGround,
  Granularity,
  judgePlanForContext,
  MemoryCache,
  MockJudge,
  planJudge,
  planJudgeWith,
  PROMPT_VERSION,
  runJudge,
} from "./judge";
export type { JudgePlan } from "./judge";
export { parseManifest, parseRule, scanCaptureRefs } from "./loader";
export type { PatternDef, RuleDef } from "./loader";
export { remediationPrompt } from "./prompt";
export type { PromptSource } from "./prompt";
export { InvalidRegexError, compileRustRegex, translateRustRegex } from "./regex";
export { RuleSet } from "./registry";
export type { Interests, Report, Rule, RuleExample, RuleMeta } from "./rule";
export { Ctx } from "./rule";
export type {
  Applicability,
  Diagnostic,
  Edit,
  Fix,
  Intent,
  LintResult,
  Scope,
  Severity,
  Stats,
  TextRange,
  Tier,
} from "./types";
export { Src } from "./utf8";

const DEFAULT_CONFIDENCE_FLOOR = Math.fround(0.6);

let BUILTIN: RuleSet | undefined;
function builtInSet(): RuleSet {
  BUILTIN ??= RuleSet.builtIn();
  return BUILTIN;
}

export function parse(text: string | Src, markdown: boolean) {
  return parseDoc(text, markdown);
}

/** Lint with the built-in hard-rule set, tiers 1–2 only. */
export function lint(text: string, options: LintOptions = {}): LintResult {
  return lintWith(text, options, builtInSet());
}

export function lintWith(text: string, options: LintOptions, rules: RuleSet): LintResult {
  const src = new Src(text);
  const doc = parseDoc(src, options.markdown ?? false);
  const instances = rules.instantiate(options);
  const diagnostics = dispatch.run(src, doc, instances, options, rules);
  return scoring.finalize(src, diagnostics, doc);
}

function judgePlan(options: LintOptions): JudgePlan {
  let plan = defaultJudgePlan2();
  const judge = options.judge;
  if (judge === undefined) return plan;
  if (judge.contextChars !== undefined) {
    plan = { contextChars: Math.max(1, judge.contextChars), perRule: judge.contextChars > 1200 };
  }
  if (judge.perRule !== undefined) plan.perRule = judge.perRule;
  return plan;
}
import { defaultJudgePlan as defaultJudgePlan2 } from "./judge";

export function lintFull(
  text: string,
  options: LintOptions,
  rules: RuleSet,
  judge: Judge,
  cache?: JudgeCache,
): LintResult {
  const src = new Src(text);
  const doc = parseDoc(src, options.markdown ?? false);
  const instances = rules.instantiate(options);
  const fragments = instances
    .map((r) => ({ scope: r.meta().scope, intent: r.meta().intent, fragment: r.rubric?.() }))
    .filter(
      (f): f is { scope: never; intent: never; fragment: RubricFragment } =>
        f.fragment !== undefined,
    ) as {
    scope: import("./types").Scope;
    intent: import("./types").Intent;
    fragment: RubricFragment;
  }[];
  const diagnostics = dispatch.run(src, doc, instances, options, rules);

  const reqs = planJudgeWith(
    doc,
    src,
    fragments.map((f) => f.fragment),
    judgePlan(options),
  );
  const { grounded, stats } = runJudge(judge, cache, reqs, src);

  const suppressions = dispatch.Suppressions.create(src, rules);
  const tier3: Diagnostic[] = [];
  for (const { finding, span } of grounded) {
    const frag = fragments.find((f) => f.fragment.rule === finding.rule);
    if (frag === undefined) continue;
    const mask = dispatch.scopeMask(frag.scope, src, doc);
    if (!dispatch.maskContains(mask, span)) continue;
    if (suppressions.suppressed(frag.fragment.rule, span.start)) continue;
    const message =
      wsTrim(finding.explanation).length === 0
        ? `Flagged by ${frag.fragment.rule}.`
        : finding.explanation;
    let fix: Fix | undefined;
    if (finding.suggested_rewrite !== undefined) {
      fix = {
        edits: [{ range: span, replacement: finding.suggested_rewrite }],
        applicability: "maybe-incorrect",
      };
    }
    const d: Diagnostic = {
      ruleId: frag.fragment.rule,
      severity: frag.fragment.severity,
      tier: "inferential",
      intent: frag.intent,
      span,
      message,
      line: 0,
      column: 0,
      excerpt: "",
      confidence: finding.confidence,
    };
    if (finding.suggested_rewrite !== undefined) d.suggestion = finding.suggested_rewrite;
    if (fix !== undefined) d.fix = fix;
    tier3.push(d);
  }
  const floor = options.judge?.floor ?? DEFAULT_CONFIDENCE_FLOOR;
  diagnostics.push(...scoring.gateTier3(tier3, floor));
  const result = scoring.finalize(src, diagnostics, doc);
  result.judge = stats;
  return result;
}

/** Apply MachineApplicable fixes: non-overlapping, span order, single pass. */
export function applyFixes(text: string, diagnostics: Diagnostic[]): string {
  return applyFixesWith(text, diagnostics, false);
}

export function applyFixesWith(
  text: string,
  diagnostics: Diagnostic[],
  includeMaybeIncorrect: boolean,
): string {
  const src = new Src(text);
  const edits = diagnostics
    .map((d) => d.fix)
    .filter((f): f is Fix => f !== undefined)
    .filter(
      (f) =>
        f.applicability === "machine-applicable" ||
        (includeMaybeIncorrect && f.applicability === "maybe-incorrect"),
    )
    .flatMap((f) => f.edits)
    .filter((e) => e.range.start <= e.range.end && e.range.end <= src.byteLen)
    .sort((a, b) => a.range.start - b.range.start || a.range.end - b.range.end);
  let out = "";
  let pos = 0;
  for (const edit of edits) {
    if (edit.range.start < pos) continue;
    out += src.slice(pos, edit.range.start);
    out += edit.replacement;
    pos = edit.range.end;
  }
  out += src.slice(pos, src.byteLen);
  return out;
}

export function builtInRulesMeta() {
  return builtInSet().metas();
}

export function validateRule(name: string, markdown: string) {
  return parseRuleShim(name, markdown);
}

import { parseRule as parseRuleShim } from "./loader";

export { defaultQuoteGround as default_quote_ground };

void defaultQuoteGround;
