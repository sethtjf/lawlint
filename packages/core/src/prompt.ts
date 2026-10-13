// Remediation prompt — mirrors prompt.rs.

import type { RuleSet } from "./registry";
import type { LintResult } from "./types";

export type PromptSource = { kind: "text"; text: string } | { kind: "file"; path: string };

export function remediationPrompt(
  source: PromptSource,
  result: LintResult,
  rules: RuleSet,
): string | undefined {
  if (result.diagnostics.length === 0) return undefined;

  const metas = rules.metas();
  const metaFor = (id: string) => metas.find((m) => m.id === id);

  const order: string[] = [];
  for (const d of result.diagnostics) {
    if (!order.includes(d.ruleId)) order.push(d.ruleId);
  }

  let out = "";
  if (source.kind === "text") {
    out +=
      "Revise the document that follows. It was flagged by lawlint, a linter for legal and general prose. Apply the fixes described below and return the corrected document.\n";
  } else {
    out += `Revise the document at \`${source.path}\`. It was flagged by lawlint, a linter for legal and general prose. Read the file, apply the fixes described below, and write the corrected document back to the same path.\n`;
  }
  out += CONSTRAINTS;
  if (source.kind === "text") {
    out +=
      "- Return only the revised document text. Do not add commentary, explanations, or code fences.\n";
  } else {
    out += "- Line numbers in the findings refer to the file as it exists now, before any edits.\n";
  }
  out +=
    "\nEach section names a violated rule, explains it, gives before/after examples, then lists the findings to fix.\n";

  for (const ruleId of order) {
    out += `\n## ${ruleId}\n`;
    const meta = metaFor(ruleId);
    if (meta !== undefined) {
      out += `${meta.description}\n`;
      if (meta.rationale !== undefined) out += `${meta.rationale}\n`;
      for (const ex of meta.examples) {
        out += `Before: ${ex.bad}\n`;
        out += `After: ${ex.good}\n`;
      }
    }
    out += "Findings:\n";
    for (const d of result.diagnostics.filter((d) => d.ruleId === ruleId)) {
      out += `- line ${d.line}: "${d.excerpt}"`;
      if (d.suggestion !== undefined) out += ` (suggestion: ${d.suggestion})`;
      out += "\n";
    }
  }

  if (source.kind === "text") {
    out += "\n---\nDocument to revise:\n\n";
    out += source.text;
    if (!source.text.endsWith("\n")) out += "\n";
    out += "---\n\n";
    out +=
      "Return only the revised document text, with every listed finding fixed and nothing else changed.\n";
  } else {
    out += `\nEdit \`${source.path}\` in place, fixing every listed finding and changing nothing else.\n`;
  }
  return out;
}

const CONSTRAINTS = `
Hard constraints:
- Preserve the document's meaning and legal precision exactly.
- Change only the text a finding covers. Leave every other character byte-for-byte identical, including whitespace, capitalization, and punctuation.
- Do not introduce new stylistic problems while fixing the listed ones.
`;
