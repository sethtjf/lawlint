// RuleSet: packages, defs, aliases — mirrors registry.rs.

import type { LintOptions } from "./config";
import { DensityEngine } from "./engines/density";
import { LeadingEngine } from "./engines/leading";
import { PhraseEngine } from "./engines/phrase";
import { StatisticalEngine } from "./engines/statistical";
import { LoadError } from "./error";
import { BUILTIN_FILES } from "./generated/builtin";
import { Granularity } from "./judge";
import type { RubricFragment } from "./judge";
import * as loader from "./loader";
import type { PackageManifest, RuleDef } from "./loader";
import type { Interests, Rule, RuleMeta } from "./rule";
import type { Intent, Scope, Severity, Tier } from "./types";
import { wsTrimStart } from "./utf8";

export class InferentialRule implements Rule {
  private readonly meta_: RuleMeta;
  private readonly fragment: RubricFragment;
  private constructor(meta: RuleMeta, fragment: RubricFragment) {
    this.meta_ = meta;
    this.fragment = fragment;
  }
  static fromDef(meta: RuleMeta, def: RuleDef): InferentialRule {
    const fragment: RubricFragment = {
      rule: meta.id,
      severity: meta.severity,
      granularity:
        def.granularity !== undefined
          ? (loader.parseGranularity(def.granularity) ?? Granularity.Sentence)
          : Granularity.Sentence,
      rubric: def.rubric ?? "",
      flag_examples: def.flagExamples,
      pass_examples: def.passExamples,
    };
    return new InferentialRule(meta, fragment);
  }
  meta(): RuleMeta {
    return this.meta_;
  }
  interests(): Interests {
    return {};
  }
  rubric(): RubricFragment {
    return this.fragment;
  }
}

function buildMeta(pkg: string, def: RuleDef): RuleMeta {
  const meta: RuleMeta = {
    id: `${pkg}/${def.id}`,
    tier: loader.engineTier(def.engine) as Tier,
    scope: def.scope !== undefined ? (loader.parseScope(def.scope) as Scope) : "text",
    severity:
      def.severity !== undefined ? (loader.parseSeverity(def.severity) as Severity) : "warning",
    intent: def.intent !== undefined ? (loader.parseIntent(def.intent) as Intent) : "detection",
    description: def.description ?? "",
    docs_url: def.docs ?? `https://lawlint.com/rules/${def.id}`,
    examples: def.examples,
  };
  if (def.rationale !== undefined) meta.rationale = def.rationale;
  if (def.explanation !== undefined) meta.explanation = def.explanation;
  return meta;
}

export class RuleSet {
  private readonly defs: [RuleMeta, RuleDef, string][] = [];
  private aliases = new Map<string, string>();

  static fromParts(manifest: PackageManifest, rules: [string, RuleDef][]): RuleSet {
    const set = new RuleSet();
    for (const [file, def] of rules) {
      const meta = buildMeta(manifest.name, def);
      const first = set.defs.find(([m]) => m.id === meta.id);
      if (first !== undefined) {
        throw LoadError.duplicateId(meta.id, first[2], file);
      }
      set.defs.push([meta, def, file]);
    }
    set.rebuildAliases();
    return set;
  }

  static builtIn(): RuleSet {
    const manifestText = BUILTIN_FILES["style.yaml"];
    if (manifestText === undefined)
      throw LoadError.io("builtin/style.yaml", "embedded file not found");
    const manifest = loader.parseManifest("builtin/style.yaml", manifestText);
    const rules: [string, RuleDef][] = [];
    for (const name of Object.keys(BUILTIN_FILES).sort()) {
      if (!name.startsWith("rules/") || !name.endsWith(".md")) continue;
      rules.push([`builtin/${name}`, loader.parseRule(`builtin/${name}`, BUILTIN_FILES[name])]);
    }
    return RuleSet.fromParts(manifest, rules);
  }

  static fromSources(packageName: string, files: [string, string][]): RuleSet {
    loader.validatePackageName("style.yaml", packageName);
    const manifest: PackageManifest = { name: packageName, version: "0.0.0" };
    const rules: [string, RuleDef][] = [];
    for (const [name, yaml] of files) {
      const markdown = wsTrimStart(yaml).startsWith("---") ? yaml : `---\n${yaml}\n---\n`;
      rules.push([name, loader.parseRule(name, markdown)]);
    }
    return RuleSet.fromParts(manifest, rules);
  }

  merge(other: RuleSet): void {
    for (const [meta, , file] of other.defs) {
      const first = this.defs.find(([m]) => m.id === meta.id);
      if (first !== undefined) {
        throw LoadError.duplicateId(meta.id, first[2], file);
      }
    }
    this.defs.push(...other.defs);
    this.rebuildAliases();
  }

  private rebuildAliases() {
    const counts = new Map<string, number>();
    for (const [meta] of this.defs) {
      const bare = meta.id.split("/").pop() as string;
      counts.set(bare, (counts.get(bare) ?? 0) + 1);
    }
    this.aliases = new Map();
    for (const [meta] of this.defs) {
      const bare = meta.id.split("/").pop() as string;
      if (counts.get(bare) === 1) this.aliases.set(bare, meta.id);
    }
  }

  resolve(idOrAlias: string): string | undefined {
    if (this.defs.some(([m]) => m.id === idOrAlias)) return idOrAlias;
    return this.aliases.get(idOrAlias);
  }

  private namesRule(name: string, fullId: string): boolean {
    return this.resolve(name) === fullId;
  }

  instantiate(options: LintOptions): Rule[] {
    const out: Rule[] = [];
    for (const [metaRaw, def, file] of this.defs) {
      if (options.disable?.some((d) => this.namesRule(d, metaRaw.id))) continue;
      if (
        options.enable !== undefined &&
        !options.enable.some((e) => this.namesRule(e, metaRaw.id))
      )
        continue;
      const meta = { ...metaRaw };
      if (options.severity !== undefined) {
        for (const [name, severity] of Object.entries(options.severity)) {
          if (this.namesRule(name, meta.id)) meta.severity = severity;
        }
      }
      let rule: Rule;
      switch (def.engine) {
        case "phrase":
          rule = PhraseEngine.fromDef(meta, def, file);
          break;
        case "leading":
          rule = LeadingEngine.fromDef(meta, def, file);
          break;
        case "density":
          rule = DensityEngine.fromDef(meta, def, file);
          break;
        case "statistical":
          rule = StatisticalEngine.fromDef(meta, def, file);
          break;
        case "inferential":
          rule = InferentialRule.fromDef(meta, def);
          break;
        default:
          throw new Error(`loader validated engine, got ${def.engine}`);
      }
      out.push(rule);
    }
    return out;
  }

  metas(): RuleMeta[] {
    return this.defs.map(([m]) => m);
  }
}
