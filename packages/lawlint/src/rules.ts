import type { Diagnostic, Rule, RuleContext } from "./types.js";

const rx = (pattern: RegExp, message: string, suggestion?: string) => ({
  pattern,
  message,
  suggestion,
});

const phrases = [
  rx(/\bdelve\b/gi, "Avoid the AI-writing cliché “delve”.", "Use a direct verb such as “examine”."),
  rx(/\btapestry\b/gi, "Avoid the metaphor “tapestry” in analytical prose."),
  rx(/\blandscape of\b/gi, "Avoid the vague phrase “landscape of”."),
  rx(/\bin today's fast-paced world\b/gi, "Avoid this generic introductory phrase."),
  rx(/\bit is important to note\b/gi, "State the important point directly."),
  rx(/\bnavigate the complexities\b/gi, "Use a concrete description of the task or issue."),
];

function matches(
  context: RuleContext,
  item: ReturnType<typeof rx>,
  severity?: "error" | "warning" | "info",
) {
  const out: Diagnostic[] = [];
  for (const match of context.text.matchAll(item.pattern)) {
    const start = match.index ?? 0;
    out.push(
      context.diagnostic(start, start + match[0].length, item.message, item.suggestion, severity),
    );
  }
  return out;
}

function phraseRule(
  id: string,
  description: string,
  items: ReturnType<typeof rx>[],
  severity: "error" | "warning" | "info" = "warning",
): Rule {
  return {
    id,
    meta: {
      description,
      docsUrl: `https://lawlint.dev/rules/${id}`,
      rationale:
        "Avoid patterns that can make otherwise clear prose sound formulaic or overworked.",
    },
    check: (c) => items.flatMap((item) => matches(c, item, severity)),
  };
}

const densityRule = (
  id: string,
  description: string,
  pattern: RegExp,
  defaultThreshold: number,
  message: string,
): Rule => ({
  id,
  meta: {
    description,
    docsUrl: `https://lawlint.dev/rules/${id}`,
    rationale:
      "Use this signal as a prompt to revise rhythm and density, not as a hard prohibition.",
  },
  check: (context) => {
    const count = [...context.text.matchAll(pattern)].length;
    const words = Math.max(1, context.text.trim().split(/\s+/).length);
    const threshold = context.options.thresholds?.[id] ?? defaultThreshold;
    if ((count / words) * 1000 <= threshold || count === 0) return [];
    const match = context.text.match(pattern);
    const start = match?.index ?? 0;
    return [context.diagnostic(start, start + (match?.[0].length ?? 1), message)];
  },
});

export const builtInRules: Rule[] = [
  phraseRule("no-ai-cliches", "Flags common AI-writing clichés.", phrases),
  densityRule(
    "no-robotic-transitions",
    "Flags overuse of formulaic transitions.",
    /^\s*(Moreover|Furthermore|Additionally|In conclusion),/gim,
    18,
    "Formulaic sentence transitions are overused.",
  ),
  phraseRule("no-legalese", "Flags archaic or unnecessarily formal legalese.", [
    rx(/\bhereinafter\b/gi, "Avoid “hereinafter”.", "Name the party or concept directly."),
    rx(
      /\baforementioned\b/gi,
      "Avoid “aforementioned”.",
      "Repeat the noun or use a clear reference.",
    ),
    rx(/\bpursuant to\b/gi, "Consider replacing “pursuant to”.", "Use “under” or “by”."),
    rx(
      /\bnotwithstanding the foregoing\b/gi,
      "Avoid “notwithstanding the foregoing”.",
      "State the exception directly.",
    ),
    rx(/\bherein\b|\bthereto\b/gi, "Avoid archaic legalese.", "Use a specific noun or pronoun."),
  ]),
  densityRule(
    "no-em-dash-overuse",
    "Flags excessive em dashes.",
    /—/g,
    8,
    "Em dashes are used too frequently.",
  ),
  densityRule(
    "no-rule-of-three",
    "Flags dense repeated triplet constructions.",
    /\b\w+(?:\s+\w+){0,3},\s+\w+(?:\s+\w+){0,3},\s+and\s+\w+/gi,
    12,
    "Repeated rule-of-three constructions can sound formulaic.",
  ),
  phraseRule("no-not-only", "Flags not-only/but-also constructions.", [
    rx(
      /\bnot only\b[\s\S]{0,120}\bbut also\b/gi,
      "Avoid the formulaic “not only ... but also” construction.",
    ),
  ]),
  {
    id: "sentence-length",
    meta: {
      description: "Flags sentences that are difficult to read.",
      docsUrl: "https://lawlint.dev/rules/sentence-length",
    },
    check: (c) => {
      const out: Diagnostic[] = [];
      for (const match of c.text.matchAll(/[^.!?]+[.!?]+|[^.!?]+$/g)) {
        const words = match[0].trim().split(/\s+/).filter(Boolean);
        if (words.length > (c.options.thresholds?.["sentence-length"] ?? 45)) {
          const start = match.index ?? 0;
          out.push(
            c.diagnostic(
              start,
              start + match[0].length,
              `Sentence is ${words.length} words; consider shortening it.`,
            ),
          );
        }
      }
      return out;
    },
  },
  {
    id: "no-repetitive-openers",
    meta: {
      description: "Flags repeated sentence openings.",
      docsUrl: "https://lawlint.dev/rules/no-repetitive-openers",
    },
    check: (c) => {
      const out: Diagnostic[] = [];
      const sentences = [...c.text.matchAll(/(?:^|[.!?]\s+)([A-Za-z']+)/g)];
      for (let i = 2; i < sentences.length; i++) {
        const a = sentences[i - 2]?.[1]?.toLowerCase();
        const b = sentences[i - 1]?.[1]?.toLowerCase();
        const d = sentences[i]?.[1]?.toLowerCase();
        if (a && a === b && b === d) {
          const start = sentences[i - 2]?.index ?? 0;
          out.push(
            c.diagnostic(start, start + a.length, `Three consecutive sentences begin with “${a}”.`),
          );
        }
      }
      return out;
    },
  },
  densityRule(
    "no-passive-overuse",
    "Flags likely passive-voice overuse.",
    /\b(?:is|are|was|were|be|been|being)\s+\w+(?:ed|en)\b/gi,
    25,
    "Passive voice appears frequently; prefer active constructions where possible.",
  ),
  densityRule(
    "no-hedging",
    "Flags excessive hedging language.",
    /\b(?:arguably|it could be said|generally speaking|perhaps|likely)\b/gi,
    10,
    "Reduce hedging and make the claim more direct.",
  ),
  densityRule(
    "no-empty-emphasis",
    "Flags overused empty emphasis words.",
    /\b(?:very|really|significantly|crucially)\b/gi,
    12,
    "Replace emphasis with a specific fact or omit it.",
  ),
  phraseRule(
    "no-doublets",
    "Flags legal doublets and triplets.",
    [
      rx(
        /\b(?:cease and desist|null and void|any and all)\b/gi,
        "This legal doublet is often unnecessary.",
        "Use one precise term.",
      ),
    ],
    "info",
  ),
];
