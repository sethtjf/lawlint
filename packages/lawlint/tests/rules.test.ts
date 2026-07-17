import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { builtInRules, findConfig, lint, lintFile, mergeOptions } from "../src/index.js";

const has = (text: string, ruleId: string, options = {}) =>
  lint(text, options).diagnostics.some((diagnostic) => diagnostic.ruleId === ruleId);

describe("built-in rules", () => {
  it("flags AI clichés and legalese", () => {
    const result = lint(
      "It is important to note that we delve into the landscape pursuant to the aforementioned rule.",
    );
    expect(result.diagnostics.map((d) => d.ruleId)).toEqual(
      expect.arrayContaining(["no-ai-cliches", "no-legalese"]),
    );
  });

  it("supports disabling and severity overrides", () => {
    const result = lint("It is important to note this.", {
      disable: ["no-ai-cliches"],
      severity: { "no-ai-cliches": "error" },
    });
    expect(result.diagnostics).toHaveLength(0);
  });

  it("computes useful stats and flags long sentences", () => {
    const result = lint(`${"word ".repeat(50)}.`);
    expect(result.stats.wordCount).toBe(50);
    expect(result.diagnostics.some((d) => d.ruleId === "sentence-length")).toBe(true);
  });

  it("accepts third-party rules", () => {
    const result = lint("hello", {
      rules: [
        {
          id: "custom",
          meta: { description: "test", docsUrl: "https://example.test" },
          check: (c) => [c.diagnostic(0, 5, "custom issue")],
        },
      ],
    });
    expect(result.diagnostics[0]?.ruleId).toBe("custom");
  });

  it.each([
    ["no-ai-cliches", "We should delve into this issue.", "We should examine this issue."],
    ["no-robotic-transitions", "Moreover, one. Furthermore, two.", "One point. Another point."],
    ["no-legalese", "Pursuant to herein, the party acts.", "Under this rule, the party acts."],
    ["no-em-dash-overuse", "a — b — c — d — e — f — g — h — i", "A short sentence."],
    [
      "no-rule-of-three",
      "red, blue, and green; red, blue, and green.",
      "The red and blue options remain.",
    ],
    ["no-not-only", "It is not only fast but also clear.", "It is fast and clear."],
    ["sentence-length", `${"word ".repeat(50)}.`, "A short sentence."],
    [
      "no-repetitive-openers",
      "The contract ends. The contract changes. The contract renews.",
      "The contract ends. It changes. Parties renew it.",
    ],
    [
      "no-passive-overuse",
      "is made, is used, is signed, is filed",
      "The clerk files and signs documents.",
    ],
    ["no-hedging", "Arguably, perhaps this is likely correct.", "This is correct."],
    [
      "no-empty-emphasis",
      "This is very really significantly crucially important.",
      "This is important.",
    ],
    ["no-doublets", "The parties shall cease and desist.", "The parties shall stop."],
    [
      "no-em-dash",
      "The clause — read narrowly — controls.",
      "The clause, read narrowly, controls.",
    ],
    ["no-en-dash", "The pre–trial motion was denied.", "The range spans 2020–2024."],
    [
      "no-semicolons",
      "One clause governs; another does not.",
      "One clause governs. Another does not.",
    ],
    [
      "oxford-comma",
      "The parties are Alice, Bob and Carol.",
      "The parties are Alice, Bob, and Carol.",
    ],
    ["no-marketing-language", "We leverage powerful, seamless tools.", "We use two small tools."],
    ["no-sycophantic-openers", "Great question. The rule bars it.", "The rule bars it."],
    ["no-throat-clearing", "Let me think about this. The rule bars it.", "The rule bars it."],
    ["no-parenthetical-asides", "(one) (two) (three)", "A plain sentence."],
  ] satisfies [string, string, string][])("%s flags its target", (ruleId, bad, clean) => {
    expect(has(bad, ruleId)).toBe(true);
    expect(has(clean, ruleId)).toBe(false);
  });

  it("supports threshold overrides for density rules", () => {
    const text = "Arguably this is a claim.";
    expect(has(text, "no-hedging")).toBe(true);
    expect(has(text, "no-hedging", { thresholds: { "no-hedging": 1000 } })).toBe(false);
  });

  it("reports density diagnostics on the trigger's actual line", () => {
    const result = lint(`A clean introduction.\n\n${"word — ".repeat(10)}word`);
    const diagnostic = result.diagnostics.find((item) => item.ruleId === "no-em-dash-overuse");
    expect(diagnostic?.line).toBe(3);
    expect(diagnostic?.column).toBeGreaterThan(1);
  });

  it("merges discovered config underneath explicit options", () => {
    const result = mergeOptions(
      {
        disable: ["no-legalese"],
        severity: { "no-hedging": "info" },
        thresholds: { "no-hedging": 20 },
      },
      {
        disable: ["no-ai-cliches"],
        severity: { "no-hedging": "error" },
        thresholds: { "no-hedging": 30 },
      },
    );
    expect(result.disable).toEqual(["no-ai-cliches"]);
    expect(result.severity?.["no-hedging"]).toBe("error");
    expect(result.thresholds?.["no-hedging"]).toBe(30);
  });

  it("discovers config for a stdin-equivalent working directory", async () => {
    const directory = await mkdtemp(join(tmpdir(), "lawlint-config-"));
    await writeFile(
      join(directory, "lawlint.config.json"),
      JSON.stringify({
        disable: ["no-ai-cliches", "no-legalese", "no-marketing-language"],
        severity: { "no-legalese": "error" },
      }),
    );
    const config = await findConfig(directory);
    const options = mergeOptions(config);
    const result = lint("We delve pursuant to the rule.", options);
    expect(result.diagnostics).toHaveLength(0);
  });

  it("skips fenced Markdown code blocks in lintFile", async () => {
    const directory = await mkdtemp(join(tmpdir(), "lawlint-"));
    const path = join(directory, "sample.md");
    await writeFile(
      path,
      "Prose is clear.\n\n```ts\nconst text = 'delve pursuant to herein';\n```\n",
    );
    const result = await lintFile(path);
    expect(result.diagnostics).toHaveLength(0);
  });

  it("keeps the registry at twenty built-in rules", () => {
    expect(builtInRules).toHaveLength(20);
  });

  it("exposes an explicit severity for every built-in rule", () => {
    for (const rule of builtInRules) {
      expect(rule.meta.severity).toBeDefined();
    }
  });
});

describe("scoring", () => {
  // Sentence openers must be distinct alphabetic words: the opener regex only
  // captures [A-Za-z']+, so numbered filler like "w10" would read as "w" every time.
  const openers = [
    "Alpha",
    "Bravo",
    "Cedar",
    "Delta",
    "Ember",
    "Fjord",
    "Grove",
    "Harbor",
    "Inlet",
    "Juniper",
  ];
  const filler = (i: number) => (i % 10 === 0 ? (openers[i / 10] ?? `Opener${i}`) : `w${i}`);

  const sentences = (count: number) =>
    Array.from(
      { length: count },
      (_, s) => `${Array.from({ length: 10 }, (_, w) => filler(s * 10 + w)).join(" ")}.`,
    ).join(" ");

  // Swaps the first `count` non-opener filler words for "perhaps", which only
  // the no-hedging density rule matches.
  const hedged = (count: number) => {
    let remaining = count;
    return Array.from({ length: 100 }, (_, i) => {
      const boundary = i === 99 ? "." : (i + 1) % 10 === 0 ? ". " : " ";
      if (i % 10 !== 0 && remaining > 0) {
        remaining--;
        return `perhaps${boundary}`;
      }
      return `${filler(i)}${boundary}`;
    }).join("");
  };

  it("gives clean text a perfect score", () => {
    const result = lint(sentences(10));
    expect(result.diagnostics).toHaveLength(0);
    expect(result.stats.score).toBe(100);
  });

  it("charges penalty per 1,000 words, not per word", () => {
    const result = lint(`${sentences(9)} We map the landscape of this matter briefly here today.`);
    expect(result.diagnostics.map((d) => d.ruleId)).toEqual(["no-ai-cliches"]);
    expect(result.stats.wordCount).toBe(100);
    // One warning (3 penalty) per 100 words = density 30 -> 100 * e^-0.3.
    expect(result.stats.score).toBe(74);
  });

  it("weights density diagnostics by how far past the threshold they run", () => {
    const mild = lint(hedged(3));
    const heavy = lint(hedged(12));
    expect(mild.diagnostics.map((d) => d.ruleId)).toEqual(["no-hedging"]);
    expect(mild.diagnostics[0]?.weight).toBe(2);
    expect(heavy.diagnostics[0]?.weight).toBe(11);
    expect(mild.stats.score).toBe(55);
    expect(heavy.stats.score).toBe(4);
  });
});
