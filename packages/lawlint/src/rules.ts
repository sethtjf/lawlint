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
      severity,
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
    severity: "warning",
    rationale:
      "Use this signal as a prompt to revise rhythm and density, not as a hard prohibition.",
  },
  check: (context) => {
    const matches = [...context.text.matchAll(pattern)];
    const count = matches.length;
    const words = Math.max(1, context.text.trim().split(/\s+/).length);
    const threshold = context.options.thresholds?.[id] ?? defaultThreshold;
    if ((count / words) * 1000 <= threshold || count === 0) return [];
    const match = matches[0];
    const start = match?.index ?? 0;
    return [context.diagnostic(start, start + (match?.[0].length ?? 1), message)];
  },
});

// Flags matches only when they open the document, a line, or a sentence.
const leadingPhraseRule = (
  id: string,
  description: string,
  needles: RegExp[],
  message: string,
  suggestion?: string,
): Rule => ({
  id,
  meta: {
    description,
    docsUrl: `https://lawlint.dev/rules/${id}`,
    severity: "error",
    rationale: "Start with the substance. Openers that add no information should be cut.",
  },
  check: (context) => {
    const out: Diagnostic[] = [];
    for (const needle of needles) {
      const re = new RegExp(`(^|[.!?]["')\\]]?\\s+|\\n\\s*)(${needle.source})`, "gi");
      for (const match of context.text.matchAll(re)) {
        const start = (match.index ?? 0) + (match[1]?.length ?? 0);
        out.push(
          context.diagnostic(start, start + (match[2]?.length ?? 0), message, suggestion, "error"),
        );
      }
    }
    return out;
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
      severity: "warning",
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
      severity: "warning",
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
  phraseRule(
    "no-em-dash",
    "Flags every em dash.",
    [
      rx(
        /—/g,
        "Never use em dashes.",
        "Substitute a comma, period, colon, or parentheses depending on the relationship.",
      ),
    ],
    "error",
  ),
  {
    id: "no-en-dash",
    meta: {
      description: "Flags en dashes outside numeric ranges.",
      docsUrl: "https://lawlint.dev/rules/no-en-dash",
      severity: "error",
      rationale:
        "En dashes belong only in numeric ranges such as 2020–2024. Elsewhere they read as stray punctuation.",
    },
    check: (c) => {
      const out: Diagnostic[] = [];
      for (const match of c.text.matchAll(/–/g)) {
        const start = match.index ?? 0;
        const before = c.text[start - 1] ?? "";
        const after = c.text[start + 1] ?? "";
        if (/\d/.test(before) && /\d/.test(after)) continue;
        out.push(
          c.diagnostic(
            start,
            start + 1,
            "Avoid en dashes except in numeric ranges (e.g. 2020–2024).",
            "Use a hyphen, or reword the sentence.",
            "error",
          ),
        );
      }
      return out;
    },
  },
  phraseRule(
    "no-semicolons",
    "Flags semicolons.",
    [
      rx(
        /;/g,
        "Prefer periods over semicolons.",
        "Two short sentences beat one stitched-together one.",
      ),
    ],
    "error",
  ),
  phraseRule(
    "oxford-comma",
    "Flags lists that omit the Oxford comma.",
    [
      rx(
        /\w+,\s+\w+(?:\s+\w+){0,3}\s+(?:and|or)\s+\w+/gi,
        "Use the Oxford comma before the final item in a list.",
        "Add a comma before the closing “and” or “or”.",
      ),
    ],
    "error",
  ),
  phraseRule(
    "no-marketing-language",
    "Flags marketing language, hype, and filler.",
    [
      rx(/\bleverage\b/gi, "Avoid the marketing verb “leverage”.", "Use “use” or a concrete verb."),
      rx(/\bunlock\b/gi, "Avoid the hype verb “unlock”.", "Describe the actual outcome."),
      rx(
        /\bpowerful\b/gi,
        "Avoid the filler adjective “powerful”.",
        "State the specific capability.",
      ),
      rx(
        /\bseamless(?:ly)?\b/gi,
        "Avoid the hype word “seamless”.",
        "Describe what actually happens.",
      ),
      rx(/\brobust\b/gi, "Avoid the filler adjective “robust”.", "Name the concrete property."),
      rx(/\bcutting[- ]edge\b/gi, "Avoid the hype phrase “cutting-edge”.", "Say what it is."),
      rx(/\bdelve\b/gi, "Avoid “delve”.", "Use a direct verb such as “examine”."),
      rx(/\btapestry\b/gi, "Avoid the metaphor “tapestry”."),
      rx(/\bin the realm of\b/gi, "Avoid “in the realm of”.", "Name the subject directly."),
      rx(
        /\bnavigate the landscape of\b/gi,
        "Avoid “navigate the landscape of”.",
        "Describe the task.",
      ),
      rx(
        /\bit['’]s worth noting that\b/gi,
        "State the point directly.",
        "Drop the throat-clearing.",
      ),
      rx(/\bat the end of the day\b/gi, "Avoid the filler phrase “at the end of the day”."),
    ],
    "error",
  ),
  leadingPhraseRule(
    "no-sycophantic-openers",
    "Flags sycophantic openers.",
    [
      /(?:great|good|excellent|fantastic|wonderful) question/,
      /what a (?:great|fascinating|wonderful|excellent|interesting) (?:question|problem|point)/,
      /that['’]s a (?:great|fascinating|wonderful|excellent) (?:question|point)/,
    ],
    "Skip the sycophantic opener and start with the substance.",
  ),
  leadingPhraseRule(
    "no-throat-clearing",
    "Flags throat-clearing openers.",
    [
      /let me think(?: about this)?/,
      /here['’]s my take/,
      /here['’]s what i think/,
      /i think it['’]s worth/,
      /before (?:i|we) (?:begin|start|dive in)/,
    ],
    "Cut the throat-clearing and lead with the point.",
  ),
  densityRule(
    "no-parenthetical-asides",
    "Flags frequent parenthetical asides.",
    /\([^)]*\)/g,
    15,
    "Parenthetical asides appear frequently; integrate important clauses into the sentence.",
  ),
];
