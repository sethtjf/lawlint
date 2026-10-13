// Rule interface, RuleMeta, Ctx — mirrors rule.rs.

import type { Block, Document, Sentence, Token } from "./document";
import type { RubricFragment } from "./judge";
import type { Fix, Intent, Scope, Severity, TextRange, Tier } from "./types";

export interface Interests {
  tokens?: boolean;
  sentences?: boolean;
  blocks?: boolean;
  documentExit?: boolean;
}

export interface RuleExample {
  bad: string;
  good: string;
}

export interface RuleMeta {
  id: string;
  tier: Tier;
  scope: Scope;
  severity: Severity;
  intent: Intent;
  description: string;
  docs_url: string;
  rationale?: string;
  explanation?: string;
  examples: RuleExample[];
}

export interface Report {
  span: TextRange;
  message: string;
  suggestion?: string;
  weight?: number;
  fix?: Fix;
}

export class Ctx {
  readonly source: string;
  readonly srcBytes: import("./utf8").Src;
  readonly wordCount: number;
  private thresholdOverride?: number;
  private reports: Report[] = [];

  constructor(source: import("./utf8").Src, wordCount: number) {
    this.srcBytes = source;
    this.source = source.text;
    this.wordCount = wordCount;
  }

  setThresholdOverride(value: number | undefined) {
    this.thresholdOverride = value;
  }

  report(r: Report) {
    this.reports.push(r);
  }

  threshold(dflt: number): number {
    return this.thresholdOverride ?? dflt;
  }

  takeReports(): Report[] {
    const r = this.reports;
    this.reports = [];
    return r;
  }

  /** Slice the source by byte range. */
  slice(range: TextRange): string {
    return this.srcBytes.slice(range.start, range.end);
  }
}

export interface Rule {
  meta(): RuleMeta;
  interests(): Interests;
  checkToken?(t: Token, ctx: Ctx): void;
  checkSentence?(s: Sentence, ctx: Ctx): void;
  checkBlock?(b: Block, ctx: Ctx): void;
  onDocumentExit?(doc: Document, ctx: Ctx): void;
  rubric?(): RubricFragment | undefined;
}
