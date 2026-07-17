export type Severity = "error" | "warning" | "info";

export interface Diagnostic {
  ruleId: string;
  severity: Severity;
  message: string;
  suggestion?: string;
  line: number;
  column: number;
  endLine?: number;
  endColumn?: number;
  excerpt: string;
}

export interface LintResult {
  diagnostics: Diagnostic[];
  stats: { wordCount: number; sentenceCount: number; score: number };
}

export interface RuleMeta {
  description: string;
  docsUrl: string;
  rationale?: string;
  examples?: { bad: string; good: string };
}

export interface RuleContext {
  text: string;
  lines: string[];
  options: LintOptions;
  diagnostic(
    start: number,
    end: number,
    message: string,
    suggestion?: string,
    severity?: Severity,
  ): Diagnostic;
}

export interface Rule {
  id: string;
  meta: RuleMeta;
  check(context: RuleContext): Diagnostic[];
}

export interface LintOptions {
  enable?: string[];
  disable?: string[];
  severity?: Partial<Record<string, Severity>>;
  rules?: Rule[];
  thresholds?: Record<string, number>;
  cwd?: string;
  markdown?: boolean;
}
