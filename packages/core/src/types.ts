// Core data model — JSON shapes identical to the Rust serde output.

export interface TextRange {
  start: number;
  end: number;
}

export function rangeSlice(range: TextRange, text: string): string {
  return text.slice(range.start, range.end);
}
export function rangeContains(outer: TextRange, inner: TextRange): boolean {
  return inner.start >= outer.start && inner.end <= outer.end;
}

export type Severity = "error" | "warning" | "suggestion";
export type Tier = "static" | "statistical" | "inferential";
export type Scope = "prose" | "text" | "all";
export type Intent = "style" | "detection";
export type Applicability = "machine-applicable" | "maybe-incorrect";

export interface Diagnostic {
  ruleId: string;
  severity: Severity;
  tier: Tier;
  intent: Intent;
  span: TextRange;
  message: string;
  line: number;
  column: number;
  endLine?: number;
  endColumn?: number;
  excerpt: string;
  suggestion?: string;
  weight?: number;
  confidence?: number;
  fix?: Fix;
}

export interface Fix {
  edits: Edit[];
  applicability: Applicability;
}

export interface Edit {
  range: TextRange;
  replacement: string;
}

export interface Stats {
  wordCount: number;
  sentenceCount: number;
  score: number;
}

export interface LintResult {
  diagnostics: Diagnostic[];
  stats: Stats;
  judge?: import("./judge").JudgeStats;
}
