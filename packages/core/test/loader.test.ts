// Ported from crates/lawlint-core/src/loader.rs #[cfg(test)].

import { describe, expect, test } from "bun:test";
import { Granularity } from "../src/judge";
import {
  engineTier,
  parseGranularity,
  parseIntent,
  parseManifest,
  parseRule,
  parseScope,
  parseSeverity,
} from "../src/loader";
import type { RuleDef } from "../src/loader";

const F = "rules/test.md";

function ok(frontmatter: string): RuleDef {
  const markdown = frontmatter.trimStart().startsWith("---")
    ? frontmatter
    : `---\n${frontmatter}\n---\n`;
  return parseRule(F, markdown);
}

function err(frontmatter: string): string {
  const markdown = frontmatter.trimStart().startsWith("---")
    ? frontmatter
    : `---\n${frontmatter}\n---\n`;
  try {
    parseRule(F, markdown);
    throw new Error("rule should not parse");
  } catch (e) {
    if (e instanceof Error && e.message === "rule should not parse") throw e;
    return (e as Error).message;
  }
}

// ---- happy paths, one per engine kind ------------------------------------

test("phrase_rule_full_schema", () => {
  const def = ok(`
id: no-em-dash
engine: phrase
scope: text
severity: error
description: "Em dashes are a hallmark of AI-generated prose."
rationale: "Humans reach for commas."
message: "Avoid em dashes"
examples:
  - { bad: "It was—frankly—wrong.", good: "It was, frankly, wrong." }
patterns:
  - "—"
  - { pattern: "(?i)\\\\bdelve\\\\b", message: "No delving", suggestion: "examine", fix: "examine" }
allow_context: { pattern: '\\d\\s?–\\s?\\d', window: 8 }
`);
  expect(def.id).toBe("no-em-dash");
  expect(def.engine).toBe("phrase");
  expect(def.scope).toBe("text");
  expect(def.severity).toBe("error");
  expect(def.examples.length).toBe(1);
  expect(def.examples[0].bad).toBe("It was—frankly—wrong.");
  expect(def.patterns.length).toBe(2);
  expect(def.patterns[0].pattern).toBe("—");
  const p = def.patterns[1];
  expect(p.kind).toBe("detailed");
  if (p.kind === "detailed") {
    expect(p.fixTemplate).toBeUndefined();
    expect(p.pattern).toBe(String.raw`(?i)\bdelve\b`);
    expect(p.message).toBe("No delving");
    expect(p.suggestion).toBe("examine");
    expect(p.fix).toBe("examine");
  }
  expect(def.allowContext?.pattern).toBe("\\d\\s?–\\s?\\d");
  expect(def.allowContext?.window).toBe(8);
});

test("leading_rule_minimal", () => {
  const def = ok(`
id: no-throat-clearing
engine: leading
severity: error
message: "Get to the point"
patterns: ["It is important to note that", "Needless to say"]
`);
  expect(def.engine).toBe("leading");
  expect(def.patterns.length).toBe(2);
  expect(def.scope).toBeUndefined();
});

test("density_rule_minimal", () => {
  const def = ok(`
id: no-hedging
engine: density
severity: warning
threshold: 10
patterns: ["(?i)\\\\b(arguably|perhaps)\\\\b"]
`);
  expect(def.threshold).toBe(10);
  expect(def.patterns.length).toBe(1);
});

test("statistical_rule_with_params", () => {
  const def = ok(`
id: sentence-length
engine: statistical
metric: sentence-length
params: { max_words: 45 }
`);
  expect(def.metric).toBe("sentence-length");
  expect(def.params?.max_words).toBe(45);
});

test("inferential_rule_full", () => {
  const def = ok(`
id: empty-hedge
engine: inferential
severity: warning
granularity: sentence
rubric: >-
  Flag hedges that carry no information about actual uncertainty.
flag_examples: ["a", "b", "c"]
pass_examples: ["x", "y", "z"]
`);
  expect(def.granularity).toBe("sentence");
  expect(def.rubric).toContain("Flag hedges");
  expect(def.flagExamples.length).toBe(3);
  expect(def.passExamples.length).toBe(3);
});

test("markdown_soft_rule_extracts_frontmatter_and_examples", () => {
  const def = ok(`---
id: empty-hedge
engine: inferential
description: Flags empty hedges.
severity: warning
granularity: paragraph
scope: prose
---
# Rubric

Flag a hedge when it adds no useful uncertainty.

## Flag examples
- Perhaps this is good.
- It may arguably work.
- It could possibly pass.

## Pass examples
- The result may vary with jurisdiction.
- Perhaps the witness misunderstood the question.
- It may be true if the contract says so.
`);
  expect(def.id).toBe("empty-hedge");
  expect(def.granularity).toBe("paragraph");
  expect(def.rubric).toContain("adds no useful");
  expect(def.rubric).not.toContain("Flag examples");
  expect(def.flagExamples.length).toBe(3);
  expect(def.passExamples.length).toBe(3);
});

test("markdown_hard_rule_stores_explanation", () => {
  const def = ok(
    "---\nid: no-em-dash\nengine: phrase\npatterns: ['—']\n---\nAvoid em dashes when a comma is clearer.\n",
  );
  expect(def.explanation).toBe("Avoid em dashes when a comma is clearer.");
});

test("markdown_hard_rule_allows_empty_body", () => {
  const def = ok("---\nid: no-em-dash\nengine: phrase\npatterns: ['—']\n---\n");
  expect(def.explanation).toBeUndefined();
});

test("markdown_frontmatter_rejects_unknown_fields", () => {
  expect(err("---\nid: x\nengine: phrase\npatterns: [x]\nunknown: true\n---\n")).toContain(
    "unknown",
  );
});

test("markdown_rule_requires_explicit_id", () => {
  expect(err("---\nengine: phrase\npatterns: [x]\n---\n")).toContain("missing required field `id`");
});

// ---- schema flexibility --------------------------------------------------

test("examples_accepts_single_object", () => {
  const def = ok(`
id: x
engine: phrase
patterns: ["—"]
examples: { bad: "b", good: "g" }
`);
  expect(def.examples.length).toBe(1);
  expect(def.examples[0].good).toBe("g");
});

test("intent_parses_and_defaults_to_none", () => {
  const def = ok('id: x\nengine: phrase\nintent: style\npatterns: ["—"]\n');
  expect(def.intent).toBe("style");
  expect(parseIntent("style")).toBe("style");
  expect(parseIntent("detection")).toBe("detection");
  expect(parseIntent("lint")).toBeUndefined();
  expect(ok('id: x\nengine: phrase\npatterns: ["—"]\n').intent).toBeUndefined();
});

test("bad_intent_lists_alternatives", () => {
  expect(err('id: x\nengine: phrase\nintent: scoring\npatterns: ["—"]\n')).toBe(
    'rules/test.md: intent: "scoring" is not an intent — use style or detection',
  );
});

test("severity_accepts_legacy_info", () => {
  const def = ok('id: x\nengine: phrase\nseverity: info\npatterns: ["—"]\n');
  expect(parseSeverity(def.severity ?? "")).toBe("suggestion");
});

// ---- validation errors ----------------------------------------------------

test("bad_severity_matches_doc_example", () => {
  const e = (() => {
    try {
      parseRule(
        "builtin/rules/no-em-dash.md",
        '---\nid: no-em-dash\nengine: phrase\nseverity: high\npatterns: ["—"]\n---\n',
      );
      throw new Error("nope");
    } catch (e2) {
      return (e2 as Error).message;
    }
  })();
  expect(e).toBe(
    'builtin/rules/no-em-dash.md: severity: "high" is not a severity — use error, warning, or suggestion',
  );
});

test("bad_engine_lists_alternatives", () => {
  expect(err('id: x\nengine: regexp\npatterns: ["a"]\n')).toBe(
    'rules/test.md: engine: "regexp" is not an engine — use phrase, leading, density, statistical, or inferential',
  );
});

test("bad_scope_lists_alternatives", () => {
  expect(err('id: x\nengine: phrase\nscope: body\npatterns: ["a"]\n')).toBe(
    'rules/test.md: scope: "body" is not a scope — use prose, text, or all',
  );
});

test("bad_granularity_lists_alternatives", () => {
  expect(
    err(
      "id: x\nengine: inferential\ngranularity: word\nrubric: r\nflag_examples: [a, b, c]\npass_examples: [x, y, z]\n",
    ),
  ).toBe(
    'rules/test.md: granularity: "word" is not a granularity — use sentence, paragraph, or document',
  );
});

test("invalid_regex_surfaces_regex_error_text", () => {
  const e = err('id: x\nengine: phrase\npatterns: ["(unclosed"]\n');
  expect(e.startsWith('rules/test.md: patterns[0]: invalid regex "(unclosed":')).toBe(true);
});

test("invalid_allow_context_regex_reports_its_field", () => {
  const e = err(
    'id: x\nengine: phrase\npatterns: ["a"]\nallow_context: { pattern: "[", window: 4 }\n',
  );
  expect(e).toContain("allow_context.pattern");
  expect(e).toContain("invalid regex");
});

test("allow_context_on_non_phrase_is_an_error", () => {
  expect(
    err('id: x\nengine: leading\npatterns: ["a"]\nallow_context: { pattern: "b", window: 4 }\n'),
  ).toBe(
    "rules/test.md: allow_context: allow_context only applies to phrase rules — this rule uses the leading engine",
  );
});

test("fix_referencing_unknown_numbered_group_is_load_error", () => {
  const e = err(
    'id: x\nengine: phrase\npatterns:\n  - { pattern: "(\\\\w+) fee", fixTemplate: "${1} and ${2}" }\n',
  );
  expect(e).toContain("patterns[0].fix");
  expect(e).toContain("$2");
});

test("fix_referencing_unknown_named_group_is_load_error", () => {
  const e = err(
    'id: x\nengine: phrase\npatterns:\n  - { pattern: "(?P<amount>\\\\w+) fee", fixTemplate: "${nope}" }\n',
  );
  expect(e).toContain("patterns[0].fix");
  expect(e).toContain("nope");
});

test("fix_with_valid_numbered_and_named_refs_loads", () => {
  const def = ok(
    'id: x\nengine: phrase\npatterns:\n  - { pattern: "(?P<amount>\\\\w+) fee", fixTemplate: "${0}: ${1} / ${amount}" }\n',
  );
  expect(def.patterns.length).toBe(1);
});

test("fix_with_only_dollar_escape_loads_even_with_zero_groups", () => {
  const def = ok(
    'id: x\nengine: phrase\npatterns:\n  - { pattern: "late fee", fixTemplate: "$$5 fee" }\n',
  );
  expect(def.patterns.length).toBe(1);
});

test("phrase_needs_patterns", () => {
  expect(err("id: x\nengine: phrase\n")).toBe(
    "rules/test.md: patterns: a phrase rule needs at least one pattern — add a patterns list",
  );
});

test("density_needs_threshold", () => {
  expect(err('id: x\nengine: density\npatterns: ["a"]\n')).toBe(
    "rules/test.md: missing required field `threshold`",
  );
});

test("density_needs_exactly_one_pattern", () => {
  expect(err('id: x\nengine: density\nthreshold: 8\npatterns: ["a", "b"]\n')).toBe(
    "rules/test.md: patterns: a density rule needs exactly one pattern — found 2",
  );
  expect(err("id: x\nengine: density\nthreshold: 8\n")).toBe(
    "rules/test.md: patterns: a density rule needs exactly one pattern — found 0",
  );
});

test("density_threshold_must_be_non_negative", () => {
  expect(err('id: x\nengine: density\nthreshold: -3\npatterns: ["a"]\n')).toBe(
    "rules/test.md: threshold: -3 is not a valid threshold — use a non-negative number of matches per 1000 words, like 8",
  );
});

test("statistical_needs_known_metric", () => {
  expect(err("id: x\nengine: statistical\n")).toBe(
    "rules/test.md: missing required field `metric`",
  );
  const e = err("id: x\nengine: statistical\nmetric: word-frequency\n");
  expect(
    e.startsWith(
      'rules/test.md: metric: "word-frequency" is not a known metric — use sentence-length, repetitive-openers,',
    ),
  ).toBe(true);
});

test("statistical_document_metric_full_schema", () => {
  const def = ok(`
id: uniform-sentence-rhythm
engine: statistical
metric: sentence-length-variance
threshold: 105
direction: below
`);
  expect(def.metric).toBe("sentence-length-variance");
  expect(def.threshold).toBe(105);
  expect(def.direction).toBe("below");
  ok(
    "id: x\nengine: statistical\nmetric: cadence-autocorrelation\nthreshold: -0.5\ndirection: below\n",
  );
});

test("statistical_document_metric_needs_threshold_and_direction", () => {
  expect(err("id: x\nengine: statistical\nmetric: triad-density\ndirection: above\n")).toBe(
    "rules/test.md: missing required field `threshold`",
  );
  expect(err("id: x\nengine: statistical\nmetric: triad-density\nthreshold: 2\n")).toBe(
    "rules/test.md: missing required field `direction`",
  );
  expect(
    err("id: x\nengine: statistical\nmetric: triad-density\nthreshold: 2\ndirection: up\n"),
  ).toBe('rules/test.md: direction: "up" is not a direction — use above or below');
  expect(
    err("id: x\nengine: statistical\nmetric: triad-density\nthreshold: .nan\ndirection: above\n"),
  ).toContain("threshold");
});

test("direction_outside_document_metrics_is_an_error", () => {
  expect(err('id: x\nengine: phrase\npatterns: ["a"]\ndirection: above\n')).toBe(
    "rules/test.md: direction: direction only applies to statistical rules — this rule uses the phrase engine",
  );
  expect(err("id: x\nengine: statistical\nmetric: sentence-length\ndirection: above\n")).toBe(
    "rules/test.md: direction: direction only applies to document-level metrics, not sentence-length",
  );
});

test("statistical_params_are_validated_at_load_time", () => {
  expect(
    err("id: rep\nengine: statistical\nmetric: repetitive-openers\nparams: { run_length: 0 }\n"),
  ).toBe(
    "rules/test.md: params.run_length: 0 is not a valid run length — use a whole number of at least 1",
  );
  expect(
    err("id: rep\nengine: statistical\nmetric: repetitive-openers\nparams: { run_length: 2.5 }\n"),
  ).toContain("params.run_length");
  expect(
    err("id: rep\nengine: statistical\nmetric: repetitive-openers\nparams: { run_length: .nan }\n"),
  ).toContain("params.run_length");
  expect(
    err("id: sl\nengine: statistical\nmetric: sentence-length\nparams: { max_words: .nan }\n"),
  ).toContain("params.max_words");
  expect(
    err("id: sl\nengine: statistical\nmetric: sentence-length\nparams: { max_words: 0 }\n"),
  ).toContain("params.max_words");
  ok(
    "id: sl\nengine: statistical\nmetric: sentence-length\nparams: { max_words: 45, run_length: 3 }\n",
  );
});

test("inferential_needs_rubric_and_examples", () => {
  expect(
    err("id: x\nengine: inferential\nflag_examples: [a, b, c]\npass_examples: [x, y, z]\n"),
  ).toBe("rules/test.md: missing required field `rubric`");
  expect(
    err("id: x\nengine: inferential\nrubric: r\nflag_examples: [a, b]\npass_examples: [x, y, z]\n"),
  ).toBe(
    "rules/test.md: flag_examples: an inferential rule needs at least 3 flag_examples — found 2",
  );
  expect(
    err("id: x\nengine: inferential\nrubric: r\nflag_examples: [a, b, c]\npass_examples: []\n"),
  ).toBe(
    "rules/test.md: pass_examples: an inferential rule needs at least 3 pass_examples — found 0",
  );
});

test("inferential_severity_above_warning_is_an_error", () => {
  expect(
    err(
      "id: x\nengine: inferential\nseverity: error\nrubric: r\nflag_examples: [a, b, c]\npass_examples: [x, y, z]\n",
    ),
  ).toBe(
    'rules/test.md: severity: "error" is too severe for an inferential rule — judge findings are advisory; use warning or suggestion',
  );
});

test("unknown_field_is_an_error", () => {
  const e = err('id: x\nengine: phrase\npatterns: ["a"]\nseverityy: error\n');
  expect(e).toContain("rules/test.md");
  expect(e).toContain("unknown field `severityy`");
});

test("missing_required_fields_are_named", () => {
  expect(err('engine: phrase\npatterns: ["a"]\n')).toBe(
    "rules/test.md: missing required field `id`",
  );
  expect(err('id: x\npatterns: ["a"]\n')).toBe("rules/test.md: missing required field `engine`");
});

test("invalid_yaml_reports_file", () => {
  const e = err(": : :");
  expect(e.startsWith("rules/test.md:")).toBe(true);
});

test("bad_rule_id_is_an_error", () => {
  expect(err('id: "core/x"\nengine: phrase\npatterns: ["a"]\n')).toContain(
    '"core/x" is not a valid rule name',
  );
  expect(err('id: ""\nengine: phrase\npatterns: ["a"]\n')).toContain("is not a valid rule name");
});

// ---- manifest --------------------------------------------------------------

test("manifest_happy_path", () => {
  const m = parseManifest("pkg/style.yaml", "name: core\nversion: 0.1.0\ndescription: d\n");
  expect(m.name).toBe("core");
  expect(m.version).toBe("0.1.0");
  expect(m.description).toBe("d");
});

test("manifest_missing_name", () => {
  try {
    parseManifest("pkg/style.yaml", "version: 0.1.0\n");
    expect.unreachable();
  } catch (e) {
    expect((e as Error).message).toBe("pkg/style.yaml: missing required field `name`");
  }
});

test("manifest_bad_name", () => {
  try {
    parseManifest("pkg/style.yaml", 'name: "a/b"\nversion: 0.1.0\n');
    expect.unreachable();
  } catch (e) {
    expect((e as Error).message).toContain('"a/b" is not a valid package name');
  }
});

// ---- helper coverage ---------------------------------------------------------

test("enum_helpers", () => {
  expect(parseSeverity("error")).toBe("error");
  expect(parseSeverity("warning")).toBe("warning");
  expect(parseSeverity("suggestion")).toBe("suggestion");
  expect(parseSeverity("info")).toBe("suggestion");
  expect(parseSeverity("high")).toBeUndefined();
  expect(parseScope("prose")).toBe("prose");
  expect(parseScope("nope")).toBeUndefined();
  expect(parseGranularity("document")).toBe(Granularity.Document);
  expect(engineTier("phrase")).toBe("static");
  expect(engineTier("leading")).toBe("static");
  expect(engineTier("density")).toBe("statistical");
  expect(engineTier("statistical")).toBe("statistical");
  expect(engineTier("inferential")).toBe("inferential");
  expect(engineTier("x")).toBeUndefined();
});

test("detailed_pattern_scalar_fields_must_be_strings", () => {
  // Rust: serde Option<String> rejects non-string scalars with
  // "data did not match any variant of untagged enum PatternDef".
  expect(err('id: x\nengine: phrase\npatterns: [{pattern: "foo", fix: 100}]\n')).toContain(
    "`fix` must be a string",
  );
  expect(err('id: x\nengine: phrase\npatterns: [{pattern: "foo", message: 100}]\n')).toContain(
    "`message` must be a string",
  );
  expect(err('id: x\nengine: phrase\npatterns: [{pattern: "foo", suggestion: true}]\n')).toContain(
    "`suggestion` must be a string",
  );
  expect(err('id: x\nengine: phrase\npatterns: [{pattern: "foo", fix_template: 7}]\n')).toContain(
    "`fix_template` must be a string",
  );
});
