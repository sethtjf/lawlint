import { readFile } from "node:fs/promises";
import { findConfig } from "./config.js";
import { stripMarkdownCodeBlocks } from "./markdown.js";
import { builtInRules } from "./rules.js";
import type { Diagnostic, LintOptions, LintResult, Rule, RuleContext, Severity } from "./types.js";
export * from "./types.js";
export { builtInRules };
export { stripMarkdownCodeBlocks };

function location(text: string, offset: number) {
  const before = text.slice(0, offset);
  const line = before.split("\n").length;
  const column = offset - (before.lastIndexOf("\n") + 1) + 1;
  return { line, column };
}

export function lint(text: string, options: LintOptions = {}): LintResult {
  if (options.markdown) return lintText(stripMarkdownCodeBlocks(text), options);
  return lintText(text, options);
}

function lintText(text: string, options: LintOptions): LintResult {
  const rules = options.rules ?? builtInRules;
  const diagnostics: Diagnostic[] = [];
  const lines = text.split("\n");
  const context: RuleContext = {
    text,
    lines,
    options,
    diagnostic(start, end, message, suggestion, severity = "warning") {
      const pos = location(text, start);
      const endPos = location(text, end);
      const excerpt = lines[pos.line - 1]?.trim() ?? "";
      const result: Diagnostic = {
        ruleId: "",
        severity,
        message,
        line: pos.line,
        column: pos.column,
        endLine: endPos.line,
        endColumn: endPos.column,
        excerpt,
      };
      if (suggestion !== undefined) result.suggestion = suggestion;
      return result;
    },
  };
  for (const rule of rules) {
    if (options.disable?.includes(rule.id) || (options.enable && !options.enable.includes(rule.id)))
      continue;
    for (const diagnostic of rule.check(context)) {
      diagnostic.ruleId = rule.id;
      diagnostic.severity = options.severity?.[rule.id] ?? diagnostic.severity;
      diagnostics.push(diagnostic);
    }
  }
  const wordCount = text.match(/\b[\w’'-]+\b/g)?.length ?? 0;
  const sentenceCount = text.split(/[.!?]+/).filter((s) => s.trim()).length;
  const penalty = diagnostics.reduce(
    (sum, d) => sum + (d.severity === "error" ? 5 : d.severity === "warning" ? 3 : 1),
    0,
  );
  return {
    diagnostics,
    stats: {
      wordCount,
      sentenceCount,
      score: Math.max(0, Math.min(100, Math.round(100 - (penalty / Math.max(1, wordCount)) * 100))),
    },
  };
}

export async function lintFile(path: string, options: LintOptions = {}): Promise<LintResult> {
  const config = await findConfig(options.cwd);
  const text = await readFile(path, "utf8");
  const markdown = path.toLowerCase().endsWith(".md");
  return lint(text, {
    ...config,
    ...options,
    severity: { ...config.severity, ...options.severity },
    thresholds: { ...config.thresholds, ...options.thresholds },
    markdown: options.markdown ?? markdown,
  });
}
