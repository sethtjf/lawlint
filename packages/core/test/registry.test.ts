// Ported from crates/lawlint-core/src/registry.rs #[cfg(test)].

import { describe, expect, test } from "bun:test";
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Granularity } from "../src/judge";
import { parseManifest, parseRule } from "../src/loader";
import type { RuleDef } from "../src/loader";
import { loadDir } from "../src/node";
import { RuleSet } from "../src/registry";

// ---- helpers -----------------------------------------------------------

function manifest(name: string) {
  return parseManifest("style.yaml", `name: ${name}\nversion: 0.1.0\n`);
}

function rule(file: string, yaml: string): [string, RuleDef] {
  const markdown = yaml.trimStart().startsWith("---") ? yaml : `---\n${yaml}\n---\n`;
  return [file, parseRule(file, markdown)];
}

const phraseYaml = (id: string) =>
  `---\nid: ${id}\nengine: phrase\nseverity: error\npatterns: ["—"]\n---\n`;
const inferentialYaml = (id: string) =>
  `---\nid: ${id}\nengine: inferential\nseverity: warning\ngranularity: paragraph\nflag_examples: [a, b, c]\npass_examples: [x, y, z]\n---\nFlag it.\n`;

function setOf(pkg: string, rules: [string, RuleDef][]): RuleSet {
  return RuleSet.fromParts(manifest(pkg), rules);
}

// ---- built_in / load_dir ------------------------------------------------

test("built_in_loads_and_ids_are_namespaced", () => {
  const rs = RuleSet.builtIn();
  for (const meta of rs.metas()) {
    expect(meta.id.startsWith("core/")).toBe(true);
  }
});

test("load_dir_reads_markdown_rules_and_ignores_yaml", () => {
  const base = join(tmpdir(), `lawlint-registry-load-dir-${process.pid}`);
  mkdirSync(join(base, "rules"), { recursive: true });
  writeFileSync(join(base, "style.yaml"), "name: firm\nversion: 1.0.0\n");
  writeFileSync(join(base, "rules", "no-x.yaml"), phraseYaml("ignored"));
  writeFileSync(
    join(base, "rules", "no-x.md"),
    '---\nid: no-x\nengine: phrase\npatterns: ["—"]\n---\n',
  );
  writeFileSync(join(base, "rules", "notes.txt"), "not a rule");
  const rs = loadDir(base);
  expect(rs.metas().length).toBe(1);
  expect(rs.resolve("no-x")).toBeDefined();
  rmSync(base, { recursive: true, force: true });
});

test("load_dir_wraps_rule_read_failures_in_io_error", () => {
  const base = join(tmpdir(), `lawlint-registry-rule-io-${process.pid}`);
  mkdirSync(join(base, "rules", "bad.md"), { recursive: true });
  writeFileSync(join(base, "style.yaml"), "name: firm\nversion: 1.0.0\n");
  // bad.md is a directory: reading it as a file fails.
  try {
    loadDir(base);
    expect.unreachable();
  } catch (e) {
    expect((e as Error).message).toContain("bad.md");
  }
  rmSync(base, { recursive: true, force: true });
});

test("load_dir_missing_manifest_is_io_error", () => {
  const base = join(tmpdir(), `lawlint-registry-missing-${process.pid}`);
  mkdirSync(base, { recursive: true });
  try {
    loadDir(base);
    expect.unreachable();
  } catch (e) {
    expect((e as Error).message).toContain("style.yaml");
  }
  rmSync(base, { recursive: true, force: true });
});

// ---- from_sources ----------------------------------------------------------

test("from_sources_matches_load_dir_semantics", () => {
  const rs = RuleSet.fromSources("user", [["no-x.yaml", phraseYaml("no-x")]]);
  expect(rs.metas().length).toBe(1);
  expect(rs.metas()[0].id).toBe("user/no-x");
  expect(rs.resolve("no-x")).toBe("user/no-x");
  const merged = RuleSet.builtIn();
  merged.merge(rs);
  expect(merged.resolve("user/no-x")).toBeDefined();
  expect(merged.resolve("core/no-em-dash-overuse")).toBeDefined();
});

test("from_sources_validates_package_name_rules_and_duplicates", () => {
  expect(() => RuleSet.fromSources("bad name", [])).toThrow("package name");
  try {
    RuleSet.fromSources("user", [
      ["bad.yaml", "---\nid: no-x\nengine: phrase\nseverity: high\npatterns: [x]\n---\n"],
    ]);
    expect.unreachable();
  } catch (e) {
    expect((e as Error).message).toContain("bad.yaml");
    expect((e as Error).message).toContain("severity");
  }
  try {
    RuleSet.fromSources("user", [
      ["a.yaml", phraseYaml("no-x")],
      ["b.yaml", phraseYaml("no-x")],
    ]);
    expect.unreachable();
  } catch (e) {
    expect((e as Error).message).toContain("b.yaml");
    expect((e as Error).message).toContain('duplicate rule id "user/no-x"');
  }
});

test("duplicate_id_within_package_errors", () => {
  try {
    RuleSet.fromParts(manifest("core"), [
      rule("a.yaml", phraseYaml("no-x")),
      rule("b.yaml", phraseYaml("no-x")),
    ]);
    expect.unreachable();
  } catch (e) {
    const s = (e as Error).message;
    expect(s).toContain('duplicate rule id "core/no-x"');
    expect(s).toContain("a.yaml");
    expect(s).toContain("b.yaml");
  }
});

// ---- meta derivation ---------------------------------------------------------

test("meta_defaults_scope_severity_and_docs_url", () => {
  const rs = setOf("core", [rule("r.yaml", 'id: no-x\nengine: phrase\npatterns: ["—"]\n')]);
  const meta = rs.metas()[0];
  expect(meta.scope).toBe("text");
  expect(meta.severity).toBe("warning");
  expect(meta.docs_url).toBe("https://lawlint.com/rules/no-x");
  expect(meta.tier).toBe("static");
});

test("meta_respects_explicit_fields_and_tier_derivation", () => {
  const rs = setOf("core", [
    rule(
      "d.yaml",
      'id: dens\nengine: density\nscope: prose\nseverity: suggestion\ndocs: "https://example.com/dens"\nthreshold: 8\npatterns: ["x"]\n',
    ),
    rule("s.yaml", "id: stat\nengine: statistical\nmetric: sentence-length\n"),
    rule("i.yaml", inferentialYaml("inf")),
  ]);
  const byId = (id: string) => rs.metas().find((m) => m.id === id);
  const d = byId("core/dens");
  expect(d?.scope).toBe("prose");
  expect(d?.severity).toBe("suggestion");
  expect(d?.docs_url).toBe("https://example.com/dens");
  expect(d?.tier).toBe("statistical");
  expect(byId("core/stat")?.tier).toBe("statistical");
  expect(byId("core/inf")?.tier).toBe("inferential");
});

test("meta_intent_defaults_detection_and_respects_style", () => {
  const rs = setOf("core", [
    rule("a.yaml", 'id: no-x\nengine: phrase\npatterns: ["—"]\n'),
    rule("b.yaml", 'id: no-y\nengine: phrase\nintent: style\npatterns: [";"]\n'),
  ]);
  const byId = (id: string) => rs.metas().find((m) => m.id === id);
  expect(byId("core/no-x")?.intent).toBe("detection");
  expect(byId("core/no-y")?.intent).toBe("style");
});

// ---- resolve / aliases ---------------------------------------------------------

test("resolve_full_id_and_alias", () => {
  const rs = setOf("core", [rule("r.yaml", phraseYaml("no-x"))]);
  expect(rs.resolve("core/no-x")).toBe("core/no-x");
  expect(rs.resolve("no-x")).toBe("core/no-x");
  expect(rs.resolve("no-y")).toBeUndefined();
  expect(rs.resolve("other/no-x")).toBeUndefined();
});

test("ambiguous_alias_resolves_to_nothing", () => {
  const rs = setOf("core", [rule("a.yaml", phraseYaml("no-x"))]);
  rs.merge(setOf("firm", [rule("b.yaml", phraseYaml("no-x"))]));
  expect(rs.resolve("no-x")).toBeUndefined();
  expect(rs.resolve("core/no-x")).toBe("core/no-x");
  expect(rs.resolve("firm/no-x")).toBe("firm/no-x");
});

test("merge_collision_errors_and_leaves_self_unchanged", () => {
  const rs = setOf("core", [rule("a.yaml", phraseYaml("no-x"))]);
  expect(() => rs.merge(setOf("core", [rule("b.yaml", phraseYaml("no-x"))]))).toThrow(
    'duplicate rule id "core/no-x"',
  );
  expect(rs.metas().length).toBe(1);
  expect(rs.resolve("no-x")).toBe("core/no-x");
});

test("merge_extends_defs_and_aliases", () => {
  const rs = setOf("core", [rule("a.yaml", phraseYaml("no-x"))]);
  rs.merge(setOf("firm", [rule("b.yaml", phraseYaml("no-y"))]));
  expect(rs.metas().length).toBe(2);
  expect(rs.resolve("no-y")).toBe("firm/no-y");
});

// ---- instantiate -------------------------------------------------------------

test("instantiate_applies_disable_by_alias_and_full_id", () => {
  const rs = setOf("core", [
    rule("a.yaml", inferentialYaml("inf-a")),
    rule("b.yaml", inferentialYaml("inf-b")),
  ]);
  let rules = rs.instantiate({ disable: ["inf-a"] });
  expect(rules.length).toBe(1);
  expect(rules[0].meta().id).toBe("core/inf-b");
  rules = rs.instantiate({ disable: ["core/inf-b"] });
  expect(rules.length).toBe(1);
  expect(rules[0].meta().id).toBe("core/inf-a");
});

test("instantiate_enable_is_an_allowlist_and_disable_wins", () => {
  const rs = setOf("core", [
    rule("a.yaml", inferentialYaml("inf-a")),
    rule("b.yaml", inferentialYaml("inf-b")),
  ]);
  const rules = rs.instantiate({ enable: ["inf-a"] });
  expect(rules.length).toBe(1);
  expect(rules[0].meta().id).toBe("core/inf-a");
  expect(rs.instantiate({ enable: ["inf-a"], disable: ["inf-a"] })).toHaveLength(0);
});

test("instantiate_unknown_names_are_silently_ignored", () => {
  const rs = setOf("core", [rule("a.yaml", inferentialYaml("inf-a"))]);
  expect(rs.instantiate({ disable: ["no-such-rule"] }).length).toBe(1);
});

test("instantiate_applies_severity_override_via_alias", () => {
  const rs = setOf("core", [rule("a.yaml", inferentialYaml("inf-a"))]);
  const rules = rs.instantiate({ severity: { "inf-a": "suggestion" } });
  expect(rules[0].meta().severity).toBe("suggestion");
  expect(rules[0].rubric?.()?.severity).toBe("suggestion");
  expect(rs.instantiate({})[0].meta().severity).toBe("warning");
});

test("inferential_rule_carries_rubric_fragment", () => {
  const rs = setOf("core", [rule("a.yaml", inferentialYaml("inf-a"))]);
  const rules = rs.instantiate({});
  const fragment = rules[0].rubric?.();
  expect(fragment?.rule).toBe("core/inf-a");
  expect(fragment?.granularity).toBe(Granularity.Paragraph);
  expect(fragment?.rubric).toBe("Flag it.");
  expect(fragment?.flag_examples.length).toBe(3);
  expect(fragment?.pass_examples.length).toBe(3);
  const interests = rules[0].interests();
  expect(interests.tokens).toBeUndefined();
  expect(interests.sentences).toBeUndefined();
  expect(interests.blocks).toBeUndefined();
  expect(interests.documentExit).toBeUndefined();
});

test("instantiate_happy_path_per_engine_kind", () => {
  const rs = setOf("core", [
    rule("p.yaml", phraseYaml("no-p")),
    rule("l.yaml", 'id: no-l\nengine: leading\npatterns: ["Certainly"]\n'),
    rule("d.yaml", 'id: no-d\nengine: density\nthreshold: 8\npatterns: ["x"]\n'),
    rule(
      "s.yaml",
      "id: no-s\nengine: statistical\nmetric: repetitive-openers\nparams: { run_length: 3 }\n",
    ),
    rule("i.yaml", inferentialYaml("no-i")),
  ]);
  const rules = rs.instantiate({});
  expect(rules.length).toBe(5);
  expect(rules.map((r) => r.meta().id)).toEqual([
    "core/no-p",
    "core/no-l",
    "core/no-d",
    "core/no-s",
    "core/no-i",
  ]);
  expect(rules[0].meta().tier).toBe("static");
  expect(rules[2].meta().tier).toBe("statistical");
  expect(rules[4].meta().tier).toBe("inferential");
});
