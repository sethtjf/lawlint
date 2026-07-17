import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { builtInRules, lint, lintFile } from "../src/index.js";

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
  ] satisfies [string, string, string][])("%s flags its target", (ruleId, bad, clean) => {
    expect(has(bad, ruleId)).toBe(true);
    expect(has(clean, ruleId)).toBe(false);
  });

  it("supports threshold overrides for density rules", () => {
    const text = "Arguably this is a claim.";
    expect(has(text, "no-hedging")).toBe(true);
    expect(has(text, "no-hedging", { thresholds: { "no-hedging": 1000 } })).toBe(false);
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

  it("keeps the registry at twelve built-in rules", () => {
    expect(builtInRules).toHaveLength(12);
  });
});
