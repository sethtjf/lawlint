/**
 * Generate the docs rule reference from the built-in rule packages.
 *
 * The rules themselves are already Markdown-with-frontmatter
 * (`crates/lawlint-core/builtin/rules/*.md`), so the reference is generated
 * from those files rather than from `lawlint rules --json`. That keeps the
 * documentation build independent of a Rust toolchain — the website can be
 * built and previewed without compiling the engine — and lets each page carry
 * the rule's own body: explanatory prose for a hard rule, the judge's rubric
 * for a soft one.
 *
 * Output is written to `docs/rules/`, which is generated and gitignored.
 * Run via `bun run prepare:content`.
 */

import { mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";

const ROOT = resolve(import.meta.dirname, "..");
const RULES_SRC = resolve(ROOT, "../../crates/lawlint-core/builtin/rules");
const OUT_DIR = join(ROOT, "docs/rules");

/**
 * Engine → serialized tier. This mirrors `engine_tier()` in
 * `crates/lawlint-core/src/loader.rs`; the three tier names are the engine's
 * stable serialized contract, so the mapping is fixed.
 */
const TIER_BY_ENGINE: Record<string, "static" | "statistical" | "inferential"> = {
  phrase: "static",
  leading: "static",
  density: "statistical",
  statistical: "statistical",
  inferential: "inferential",
};

interface Rule {
  id: string;
  engine: string;
  tier: "static" | "statistical" | "inferential";
  kind: "Hard rule" | "Soft rule";
  severity: string;
  intent: string;
  scope: string;
  description: string;
  rationale?: string;
  message?: string;
  examples: { bad: string; good: string }[];
  /** Prose body: explanation for a hard rule, rubric for a soft one. */
  body: string;
  flagExamples: string[];
  passExamples: string[];
}

/** Strip one layer of YAML quoting from a scalar. */
const unquote = (value: string): string => {
  const trimmed = value.trim();
  if (
    (trimmed.startsWith('"') && trimmed.endsWith('"')) ||
    (trimmed.startsWith("'") && trimmed.endsWith("'"))
  ) {
    return trimmed.slice(1, -1).replace(/\\"/g, '"').replace(/''/g, "'");
  }
  return trimmed;
};

/**
 * Read the handful of scalar frontmatter keys the reference renders, plus the
 * `examples:` list. This is deliberately not a general YAML parser: the rule
 * files also carry regex `patterns:` whose quoting a naive parser would mangle,
 * and none of that is rendered here.
 */
const parseFrontmatter = (yaml: string) => {
  const scalars: Record<string, string> = {};
  const examples: { bad: string; good: string }[] = [];
  let pending: { bad?: string; good?: string } | null = null;
  let inExamples = false;

  for (const line of yaml.split("\n")) {
    if (/^examples:\s*$/.test(line)) {
      inExamples = true;
      continue;
    }
    if (inExamples) {
      // Any new top-level key ends the examples block.
      if (/^\S/.test(line) && !line.startsWith("-")) {
        if (pending?.bad && pending.good) {
          examples.push({ bad: pending.bad, good: pending.good });
        }
        pending = null;
        inExamples = false;
      } else {
        const bad = line.match(/^\s*-\s*bad:\s*(.+)$/);
        const good = line.match(/^\s*good:\s*(.+)$/);
        if (bad) {
          if (pending?.bad && pending.good) {
            examples.push({ bad: pending.bad, good: pending.good });
          }
          pending = { bad: unquote(bad[1]) };
          continue;
        }
        if (good && pending) {
          pending.good = unquote(good[1]);
        }
        continue;
      }
    }
    const scalar = line.match(/^([a-z_]+):\s*(.+)$/);
    if (scalar) {
      scalars[scalar[1]] = unquote(scalar[2]);
    }
  }
  if (pending?.bad && pending.good) {
    examples.push({ bad: pending.bad, good: pending.good });
  }
  return { scalars, examples };
};

/**
 * Split a rule body on its `## ` headings into the prose that precedes them and
 * one entry per section. Rule bodies are small and flat — prose, then at most a
 * `## Flag examples` and a `## Pass examples` list — so splitting beats trying
 * to bound each section with a lookahead.
 */
const splitSections = (body: string) => {
  const [prose, ...rest] = body.split(/^##[ \t]+/m);
  const sections = new Map<string, string>();
  for (const chunk of rest) {
    const newline = chunk.indexOf("\n");
    const heading = (newline === -1 ? chunk : chunk.slice(0, newline)).trim();
    sections.set(heading.toLowerCase(), newline === -1 ? "" : chunk.slice(newline + 1));
  }
  return { prose: prose.trim(), sections };
};

/** The bullet list under a `## Flag examples` / `## Pass examples` heading. */
const bullets = (section: string | undefined): string[] =>
  (section ?? "")
    .split("\n")
    .map((line) => line.match(/^\s*-\s*(.+)$/)?.[1])
    .filter((line): line is string => Boolean(line))
    .map(unquote);

const parseRule = (file: string): Rule | null => {
  const raw = readFileSync(file, "utf-8");
  const match = raw.match(/^---\n([\s\S]*?)\n---\n?([\s\S]*)$/);
  if (!match) {
    return null;
  }
  const [, yaml, rest] = match;
  const { scalars, examples } = parseFrontmatter(yaml);
  const engine = scalars.engine ?? "phrase";
  const tier = TIER_BY_ENGINE[engine] ?? "static";

  // The prose body is everything outside the two example sections.
  const { prose, sections } = splitSections(rest);

  return {
    id: scalars.id ?? basename(file, ".md"),
    engine,
    tier,
    kind: tier === "inferential" ? "Soft rule" : "Hard rule",
    // Defaults mirror the loader: severity `warning`, intent `detection`,
    // scope `text`.
    severity: scalars.severity ?? "warning",
    intent: scalars.intent ?? "detection",
    scope: scalars.scope ?? "text",
    description: scalars.description ?? "",
    rationale: scalars.rationale,
    message: scalars.message,
    examples,
    body: prose,
    flagExamples: bullets(sections.get("flag examples")),
    passExamples: bullets(sections.get("pass examples")),
  };
};

/** Escape the characters MDX would otherwise read as expressions or tags. */
const mdx = (text: string): string => text.replace(/([{}<>])/g, "\\$1");

/** Quote a string for YAML frontmatter. */
const yamlString = (text: string): string => `"${text.replace(/"/g, '\\"')}"`;

const renderRule = (rule: Rule): string => {
  const lines: string[] = [];

  lines.push("---");
  lines.push(`title: ${yamlString(rule.id)}`);
  if (rule.description) {
    lines.push(`description: ${yamlString(rule.description)}`);
  }
  lines.push("sidebar:");
  lines.push(`  label: ${yamlString(rule.id)}`);
  lines.push(`  badge: ${yamlString(rule.kind === "Soft rule" ? "Soft" : "Hard")}`);
  lines.push("search:");
  lines.push(`  tags: [rule, ${rule.tier}, ${rule.intent}]`);
  lines.push("---");
  lines.push("");

  // The description is already rendered as the page subtitle from frontmatter,
  // so the body opens on the metadata rather than repeating it.
  lines.push("| Property | Value |");
  lines.push("| -------- | ----- |");
  lines.push(`| Kind | ${rule.kind} |`);
  lines.push(`| Engine | \`${rule.engine}\` |`);
  lines.push(`| Tier | \`${rule.tier}\` |`);
  lines.push(`| Severity | \`${rule.severity}\` |`);
  lines.push(`| Intent | \`${rule.intent}\` |`);
  lines.push(`| Scope | \`${rule.scope}\` |`);
  lines.push("");

  if (rule.intent === "style") {
    lines.push(
      ":::note\nThis is a style rule. It reports findings and participates in `--fix`, but never moves the [human-likeness score](/docs/concepts/scoring).\n:::",
    );
    lines.push("");
  }

  if (rule.rationale) {
    lines.push("## Why");
    lines.push("");
    lines.push(mdx(rule.rationale));
    lines.push("");
  }

  if (rule.examples.length > 0) {
    lines.push("## Examples");
    lines.push("");
    for (const example of rule.examples) {
      lines.push("<Columns cols={2}>");
      lines.push(`  <Column>\n    **Flagged**\n\n    ${mdx(example.bad)}\n  </Column>`);
      lines.push(`  <Column>\n    **Better**\n\n    ${mdx(example.good)}\n  </Column>`);
      lines.push("</Columns>");
      lines.push("");
    }
  }

  if (rule.body) {
    lines.push(rule.kind === "Soft rule" ? "## Rubric" : "## What it catches");
    lines.push("");
    if (rule.kind === "Soft rule") {
      lines.push(
        "This rule is evaluated by the optional [AI judge](/docs/guides/judge) against the criteria below.",
      );
      lines.push("");
    }
    lines.push(mdx(rule.body));
    lines.push("");
  }

  if (rule.flagExamples.length > 0) {
    lines.push("### Should flag");
    lines.push("");
    for (const example of rule.flagExamples) {
      lines.push(`- ${mdx(example)}`);
    }
    lines.push("");
  }

  if (rule.passExamples.length > 0) {
    lines.push("### Should pass");
    lines.push("");
    for (const example of rule.passExamples) {
      lines.push(`- ${mdx(example)}`);
    }
    lines.push("");
  }

  lines.push("## Turning it off");
  lines.push("");
  lines.push("```sh");
  lines.push(`lawlint --disable ${rule.id} draft.md`);
  lines.push("```");
  lines.push("");
  lines.push(
    "Or durably, in `.lawlint/config.json` — see [Configuration](/docs/reference/configuration).",
  );
  lines.push("");

  return lines.join("\n");
};

const renderIndex = (rules: Rule[]): string => {
  const row = (rule: Rule) =>
    `| [\`${rule.id}\`](/docs/rules/${rule.id}) | ${mdx(rule.description)} | \`${rule.tier}\` | \`${rule.severity}\` |`;

  const detection = rules.filter((rule) => rule.intent !== "style");
  const style = rules.filter((rule) => rule.intent === "style");

  return `---
title: Rules
description: Every built-in rule, what it catches, and whether it charges the human-likeness score.
icon: list
sidebar:
  order: 1
---

${rules.length} built-in rules ship enabled. ${detection.length} are
[detection](/docs/concepts/rules#intent-which-rules-move-the-score) rules that
charge the [score](/docs/concepts/scoring); ${style.length} are style rules that
report and fix without moving it.

\`\`\`sh
lawlint rules --json     # this table, as JSON
\`\`\`

## Detection rules

Corpus-validated to distinguish AI-generated from authentic human legal prose.

| Rule | Flags | Tier | Severity |
| ---- | ----- | ---- | -------- |
${detection.map(row).join("\n")}

## Style rules

Writing advice that says nothing about provenance. Reported and fixable; never
scored.

| Rule | Flags | Tier | Severity |
| ---- | ----- | ---- | -------- |
${style.map(row).join("\n")}

## Writing your own

Rules are Markdown files with YAML frontmatter, exactly like the built-ins. See
[Authoring rules](/docs/guides/authoring-rules).
`;
};

const rules = readdirSync(RULES_SRC)
  .filter((file) => file.endsWith(".md"))
  .map((file) => parseRule(join(RULES_SRC, file)))
  .filter((rule): rule is Rule => rule !== null)
  .sort((a, b) => a.id.localeCompare(b.id));

if (rules.length === 0) {
  throw new Error(`No rules found in ${RULES_SRC}`);
}

rmSync(OUT_DIR, { force: true, recursive: true });
mkdirSync(OUT_DIR, { recursive: true });

for (const rule of rules) {
  writeFileSync(join(OUT_DIR, `${rule.id}.mdx`), renderRule(rule));
}
writeFileSync(join(OUT_DIR, "index.mdx"), renderIndex(rules));
writeFileSync(
  join(OUT_DIR, "meta.ts"),
  `// Generated by scripts/generate-rule-docs.ts. Do not edit.
import { defineMeta } from "blume";

export default defineMeta({
  title: "Rules",
  icon: "list",
  order: 7,
  collapsed: true,
  pages: ${JSON.stringify(["index", ...rules.map((rule) => rule.id)], null, 2).replace(/\n/g, "\n  ")},
});
`,
);

console.log(`[rule-docs] wrote ${rules.length} rule pages to docs/rules/`);
