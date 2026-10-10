// Markdown rule parsing + validation — mirrors loader.rs.
// Frontmatter parsed with the `yaml` package; validation produces the same
// LoadError messages as the Rust serde-based loader.

import { type Document, parseDocument } from "yaml";
import { LoadError } from "./error";
import { Granularity } from "./judge";
import { InvalidRegexError, compileRustRegex } from "./regex";
import type { RuleExample } from "./rule";
import type { Intent, Scope, Severity, Tier } from "./types";
import { wsTrim } from "./utf8";

export interface PackageManifest {
  name: string;
  version: string;
  description?: string;
}

export interface PatternDefBare {
  kind: "bare";
  pattern: string;
}
export interface PatternDefDetailed {
  kind: "detailed";
  pattern: string;
  message?: string;
  suggestion?: string;
  fix?: string;
  fixTemplate?: string;
}
export type PatternDef = PatternDefBare | PatternDefDetailed;

export interface AllowContextDef {
  pattern: string;
  window: number;
}

export interface RuleDef {
  id: string;
  engine: string;
  scope?: string;
  severity?: string;
  intent?: string;
  description?: string;
  rationale?: string;
  docs?: string;
  message?: string;
  examples: RuleExample[];
  patterns: PatternDef[];
  allowContext?: AllowContextDef;
  threshold?: number;
  metric?: string;
  params?: Record<string, number>;
  direction?: string;
  granularity?: string;
  rubric?: string;
  flagExamples: string[];
  passExamples: string[];
  explanation?: string;
}

// ---- Capture-ref scanning (mirrors scan_capture_refs) -------------------

export type CaptureRef = { kind: "number"; n: number } | { kind: "named"; name: string };

export function scanCaptureRefs(replacement: string): CaptureRef[] {
  const isCapLetter = (b: string) => /[0-9A-Za-z_]/.test(b);
  const refs: CaptureRef[] = [];
  let i = 0;
  while (i < replacement.length) {
    if (replacement[i] !== "$") {
      i += 1;
      continue;
    }
    if (replacement[i + 1] === "$") {
      i += 2;
      continue;
    }
    if (replacement[i + 1] === "{") {
      const start = i + 2;
      let j = start;
      while (j < replacement.length && replacement[j] !== "}") j += 1;
      if (j >= replacement.length) {
        i += 1;
        continue;
      }
      const name = replacement.slice(start, j);
      const n = /^\d+$/.test(name) ? Number.parseInt(name, 10) : Number.NaN;
      refs.push(Number.isNaN(n) ? { kind: "named", name } : { kind: "number", n });
      i = j + 1;
      continue;
    }
    const start = i + 1;
    let j = start;
    while (j < replacement.length && isCapLetter(replacement[j])) j += 1;
    if (j === start) {
      i += 1;
      continue;
    }
    const name = replacement.slice(start, j);
    const n = /^\d+$/.test(name) ? Number.parseInt(name, 10) : Number.NaN;
    refs.push(Number.isNaN(n) ? { kind: "named", name } : { kind: "number", n });
    i = j;
  }
  return refs;
}

/** Capture groups a translated regex exposes. */
export function captureInfo(re: RegExp): { count: number; names: Set<string> } {
  // Probe against an empty match: exec can't be used portably, so compile a
  // twin that always matches and count groups + collect names.
  const probe = new RegExp(`(?:)|${re.source}`, re.flags);
  const m = probe.exec("");
  const count = m ? m.length - 1 : 0;
  const names = new Set(Object.keys(m?.groups ?? {}));
  return { count, names };
}

export function validateFixCaptureRefs(file: string, field: string, fix: string, re: RegExp): void {
  const { count, names } = captureInfo(re);
  for (const ref of scanCaptureRefs(fix)) {
    if (ref.kind === "number") {
      if (ref.n >= count + 1) {
        throw LoadError.invalidField(
          file,
          field,
          `fix references capture group $${ref.n}, but the pattern only defines groups 0..${count} (0 is the whole match)`,
        );
      }
    } else if (!names.has(ref.name)) {
      const known = [...names].map((n) => `"${n}"`).join(", ");
      throw LoadError.invalidField(
        file,
        field,
        `fix references named capture group "${ref.name}", which the pattern does not define — known named groups: [${known}]`,
      );
    }
  }
}

// ---- Enum-ish helpers ----------------------------------------------------

export function parseSeverity(s: string): Severity | undefined {
  if (s === "error") return "error";
  if (s === "warning") return "warning";
  if (s === "suggestion" || s === "info") return "suggestion";
  return undefined;
}
export function parseIntent(s: string): Intent | undefined {
  if (s === "style") return "style";
  if (s === "detection") return "detection";
  return undefined;
}
export function parseScope(s: string): Scope | undefined {
  if (s === "prose" || s === "text" || s === "all") return s;
  return undefined;
}
export function parseGranularity(s: string): Granularity | undefined {
  if (s === "sentence") return Granularity.Sentence;
  if (s === "paragraph") return Granularity.Paragraph;
  if (s === "document") return Granularity.Document;
  return undefined;
}
export function engineTier(engine: string): Tier | undefined {
  if (engine === "phrase" || engine === "leading") return "static";
  if (engine === "density" || engine === "statistical") return "statistical";
  if (engine === "inferential") return "inferential";
  return undefined;
}

const ENGINES = new Set(["phrase", "leading", "density", "statistical", "inferential"]);

const RULEDEF_FIELDS = [
  "id",
  "engine",
  "scope",
  "severity",
  "intent",
  "description",
  "rationale",
  "docs",
  "message",
  "examples",
  "patterns",
  "allow_context",
  "threshold",
  "metric",
  "params",
  "direction",
  "granularity",
  "rubric",
  "flag_examples",
  "pass_examples",
  "explanation",
];
const MANIFEST_FIELDS = ["name", "version", "description"];

function yamlError(file: string, e: unknown): LoadError {
  const msg = e instanceof Error ? e.message : String(e);
  return LoadError.yaml(file, msg);
}

function compileCheck(file: string, field: string, pattern: string): RegExp {
  try {
    return compileRustRegex(pattern);
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    throw LoadError.invalidRegex(file, field, pattern, msg);
  }
}

/** Parse + validate a single Markdown rule file. */
export function parseRule(file: string, markdown: string): RuleDef {
  const def = parseRuleMarkdown(file, markdown);
  validateRule(file, def);
  return def;
}

function parseRuleMarkdown(file: string, markdown: string): RuleDef {
  const lines = markdown.split("\n");
  // lines() in Rust drops the trailing empty piece; mimic by iterating.
  let li = 0;
  const first = lines[li++];
  if (first === undefined || wsTrim(first) !== "---") {
    throw LoadError.invalidField(
      file,
      "frontmatter",
      "a rule file must start with YAML frontmatter between --- fences",
    );
  }
  let frontmatter = "";
  let foundEnd = false;
  for (; li < lines.length; li++) {
    if (wsTrim(lines[li]) === "---") {
      foundEnd = true;
      li++;
      break;
    }
    frontmatter += `${lines[li]}\n`;
  }
  if (!foundEnd) {
    throw LoadError.invalidField(file, "frontmatter", "missing closing --- fence");
  }
  const body = lines.slice(li);
  const def = parseFrontmatter(file, frontmatter);
  const [rubric, bodyFlags, bodyPasses] = extractSkillSections(body);
  if (def.engine === "inferential") {
    if (def.rubric === undefined && rubric.length > 0) def.rubric = rubric;
    if (def.flagExamples.length === 0) def.flagExamples = bodyFlags;
    if (def.passExamples.length === 0) def.passExamples = bodyPasses;
  } else {
    const explanation = wsTrim(body.join("\n"));
    if (explanation.length > 0) def.explanation = explanation;
  }
  return def;
}

function parseFrontmatter(file: string, frontmatter: string): RuleDef {
  let doc: Document;
  try {
    doc = parseDocument(frontmatter, { schema: "core" });
  } catch (e) {
    throw yamlError(file, e);
  }
  if (doc.errors.length > 0) {
    throw LoadError.yaml(file, doc.errors[0].message);
  }
  const raw = doc.toJS();
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) {
    throw LoadError.yaml(file, "frontmatter must be a mapping");
  }
  const keys = Object.keys(raw);
  for (const k of keys) {
    if (!RULEDEF_FIELDS.includes(k)) {
      throw LoadError.invalidField(
        file,
        k,
        `unknown field \`${k}\`, expected one of ${RULEDEF_FIELDS.map((f) => `\`${f}\``).join(", ")}`,
      );
    }
  }
  const str = (k: string): string | undefined => {
    const v = raw[k];
    if (v === undefined || v === null) return undefined;
    if (typeof v !== "string") {
      throw LoadError.invalidField(file, k, "invalid type: expected a string");
    }
    return v;
  };
  const num = (k: string): number | undefined => {
    const v = raw[k];
    if (v === undefined || v === null) return undefined;
    if (typeof v !== "number") {
      throw LoadError.invalidField(file, k, "invalid type: expected a number");
    }
    return v;
  };
  const strList = (k: string): string[] => {
    const v = raw[k];
    if (v === undefined || v === null) return [];
    if (!Array.isArray(v) || v.some((x) => typeof x !== "string")) {
      throw LoadError.invalidField(file, k, "invalid type: expected a list of strings");
    }
    return v as string[];
  };

  if (raw.id === undefined) throw LoadError.missingField(file, "id");
  if (raw.engine === undefined) throw LoadError.missingField(file, "engine");

  // patterns: bare string or detailed object.
  const patterns: PatternDef[] = [];
  const rawPatterns = raw.patterns;
  if (rawPatterns !== undefined && rawPatterns !== null) {
    if (!Array.isArray(rawPatterns)) {
      throw LoadError.invalidField(file, "patterns", "invalid type: expected a list");
    }
    for (const item of rawPatterns) {
      if (typeof item === "string") {
        patterns.push({ kind: "bare", pattern: item });
      } else if (item !== null && typeof item === "object" && !Array.isArray(item)) {
        const o = item as Record<string, unknown>;
        if (typeof o.pattern !== "string") {
          throw LoadError.invalidField(
            file,
            "patterns",
            "invalid type: pattern entry needs a `pattern` string",
          );
        }
        const s = (k: string) => (o[k] === undefined || o[k] === null ? undefined : String(o[k]));
        patterns.push({
          kind: "detailed",
          pattern: o.pattern,
          message: s("message"),
          suggestion: s("suggestion"),
          fix: s("fix"),
          fixTemplate: s("fixTemplate") ?? s("fix_template"),
        });
      } else {
        throw LoadError.invalidField(
          file,
          "patterns",
          "invalid type: pattern entries are strings or objects",
        );
      }
    }
  }

  // examples: single {bad, good} or list.
  const examples: RuleExample[] = [];
  const rawExamples = raw.examples;
  if (rawExamples !== undefined && rawExamples !== null) {
    const list = Array.isArray(rawExamples) ? rawExamples : [rawExamples];
    for (const ex of list) {
      if (ex === null || typeof ex !== "object" || Array.isArray(ex)) {
        throw LoadError.invalidField(file, "examples", "invalid type: expected {bad, good}");
      }
      const o = ex as Record<string, unknown>;
      if (typeof o.bad !== "string" || typeof o.good !== "string") {
        throw LoadError.invalidField(
          file,
          "examples",
          "invalid type: expected {bad, good} strings",
        );
      }
      examples.push({ bad: o.bad, good: o.good });
    }
  }

  const ac = raw.allow_context;
  let allowContext: AllowContextDef | undefined;
  if (ac !== undefined && ac !== null) {
    if (
      typeof ac !== "object" ||
      Array.isArray(ac) ||
      typeof ac.pattern !== "string" ||
      typeof ac.window !== "number"
    ) {
      throw LoadError.invalidField(
        file,
        "allow_context",
        "invalid type: expected {pattern, window}",
      );
    }
    allowContext = { pattern: ac.pattern, window: ac.window };
  }

  const params = raw.params;
  if (params !== undefined && params !== null) {
    if (
      typeof params !== "object" ||
      Array.isArray(params) ||
      Object.values(params).some((v) => typeof v !== "number")
    ) {
      throw LoadError.invalidField(file, "params", "invalid type: expected a map of numbers");
    }
  }

  return {
    id: str("id") as string,
    engine: str("engine") as string,
    scope: str("scope"),
    severity: str("severity"),
    intent: str("intent"),
    description: str("description"),
    rationale: str("rationale"),
    docs: str("docs"),
    message: str("message"),
    examples,
    patterns,
    allowContext,
    threshold: num("threshold"),
    metric: str("metric"),
    params: params as Record<string, number> | undefined,
    direction: str("direction"),
    granularity: str("granularity"),
    rubric: str("rubric"),
    flagExamples: strList("flag_examples"),
    passExamples: strList("pass_examples"),
  };
}

function extractSkillSections(lines: string[]): [string, string[], string[]] {
  const rubric: string[] = [];
  const flags: string[] = [];
  const passes: string[] = [];
  let section: "flags" | "passes" | null = null;
  for (const line of lines) {
    if (line.startsWith("## ")) {
      const normalized = wsTrim(line.slice(3)).toLowerCase();
      if (normalized === "flag examples") {
        section = "flags";
      } else if (normalized === "pass examples") {
        section = "passes";
      } else {
        section = null;
        rubric.push(line);
      }
      continue;
    }
    if (section !== null) {
      const t = wsTrim(line);
      const item = t.startsWith("- ")
        ? wsTrim(t.slice(2))
        : t.startsWith("* ")
          ? wsTrim(t.slice(2))
          : undefined;
      if (item !== undefined && item.length > 0) {
        if (section === "flags") flags.push(item);
        else passes.push(item);
      }
    } else {
      rubric.push(line);
    }
  }
  return [wsTrim(rubric.join("\n")), flags, passes];
}

// Rust's {:?} debug format for strings = JSON.stringify, close enough for
// the error messages that embed the given value.
const dq = JSON.stringify;

function validateRule(file: string, def: RuleDef): void {
  const id = wsTrim(def.id);
  if (id.length === 0 || id.includes("/") || /\s/.test(id)) {
    throw LoadError.invalidField(
      file,
      "id",
      `${dq(def.id)} is not a valid rule name — use a bare name like no-em-dash (no slashes or spaces; the package prefix is added automatically)`,
    );
  }
  if (!ENGINES.has(def.engine)) {
    throw LoadError.invalidField(
      file,
      "engine",
      `${dq(def.engine)} is not an engine — use phrase, leading, density, statistical, or inferential`,
    );
  }
  if (def.scope !== undefined && parseScope(def.scope) === undefined) {
    throw LoadError.invalidField(
      file,
      "scope",
      `${dq(def.scope)} is not a scope — use prose, text, or all`,
    );
  }
  if (def.severity !== undefined && parseSeverity(def.severity) === undefined) {
    throw LoadError.invalidField(
      file,
      "severity",
      `${dq(def.severity)} is not a severity — use error, warning, or suggestion`,
    );
  }
  if (def.intent !== undefined && parseIntent(def.intent) === undefined) {
    throw LoadError.invalidField(
      file,
      "intent",
      `${dq(def.intent)} is not an intent — use style or detection`,
    );
  }
  if (def.granularity !== undefined && parseGranularity(def.granularity) === undefined) {
    throw LoadError.invalidField(
      file,
      "granularity",
      `${dq(def.granularity)} is not a granularity — use sentence, paragraph, or document`,
    );
  }
  if (def.allowContext !== undefined && def.engine !== "phrase") {
    throw LoadError.invalidField(
      file,
      "allow_context",
      `allow_context only applies to phrase rules — this rule uses the ${def.engine} engine`,
    );
  }
  if (def.direction !== undefined && def.engine !== "statistical") {
    throw LoadError.invalidField(
      file,
      "direction",
      `direction only applies to statistical rules — this rule uses the ${def.engine} engine`,
    );
  }

  switch (def.engine) {
    case "phrase": {
      if (def.patterns.length === 0) {
        throw LoadError.invalidField(
          file,
          "patterns",
          "a phrase rule needs at least one pattern — add a patterns list",
        );
      }
      for (let i = 0; i < def.patterns.length; i++) {
        const p = def.patterns[i];
        const field = `patterns[${i}]`;
        const re = compileCheck(file, field, p.pattern);
        const fix = p.kind === "detailed" ? p.fix : undefined;
        const fixTemplate = p.kind === "detailed" ? p.fixTemplate : undefined;
        if (fix !== undefined && fixTemplate !== undefined) {
          throw LoadError.invalidField(
            file,
            `${field}.fixTemplate`,
            "a pattern may set `fix` (literal) or `fixTemplate` (capture-group interpolation), not both",
          );
        }
        if (fixTemplate !== undefined) {
          validateFixCaptureRefs(file, `${field}.fixTemplate`, fixTemplate, re);
        }
      }
      if (def.allowContext !== undefined) {
        compileCheck(file, "allow_context.pattern", def.allowContext.pattern);
      }
      break;
    }
    case "leading": {
      if (def.patterns.length === 0) {
        throw LoadError.invalidField(
          file,
          "patterns",
          "a leading rule needs at least one sentence-opener pattern — add a patterns list",
        );
      }
      for (let i = 0; i < def.patterns.length; i++) {
        const needle = def.patterns[i].pattern;
        try {
          compileRustRegex(`(?i)^(?:${needle})`);
        } catch (e) {
          const msg =
            e instanceof InvalidRegexError ? e.message : e instanceof Error ? e.message : String(e);
          throw LoadError.invalidRegex(file, `patterns[${i}]`, needle, msg);
        }
      }
      break;
    }
    case "density": {
      const threshold = def.threshold;
      if (threshold === undefined) throw LoadError.missingField(file, "threshold");
      if (!Number.isFinite(threshold) || threshold < 0) {
        throw LoadError.invalidField(
          file,
          "threshold",
          `${threshold} is not a valid threshold — use a non-negative number of matches per 1000 words, like 8`,
        );
      }
      if (def.patterns.length !== 1) {
        throw LoadError.invalidField(
          file,
          "patterns",
          `a density rule needs exactly one pattern — found ${def.patterns.length}`,
        );
      }
      compileCheck(file, "patterns[0]", def.patterns[0].pattern);
      break;
    }
    case "statistical": {
      const metricStr = def.metric;
      if (metricStr === undefined) throw LoadError.missingField(file, "metric");
      const metric = metricFromString(metricStr);
      if (metric === undefined) {
        throw LoadError.invalidField(
          file,
          "metric",
          `${dq(metricStr)} is not a known metric — use ${metricAlternatives()}`,
        );
      }
      const param = (name: string) => def.params?.[name];
      const runLength = param("run_length");
      if (
        runLength !== undefined &&
        (!Number.isFinite(runLength) || runLength < 1 || runLength % 1 !== 0)
      ) {
        throw LoadError.invalidField(
          file,
          "params.run_length",
          `${runLength} is not a valid run length — use a whole number of at least 1`,
        );
      }
      const maxWords = param("max_words");
      if (maxWords !== undefined && (!Number.isFinite(maxWords) || maxWords < 1)) {
        throw LoadError.invalidField(
          file,
          "params.max_words",
          `${maxWords} is not a valid sentence length — use a positive number of words like 45`,
        );
      }
      if (!isDocumentLevel(metric) && def.direction !== undefined) {
        throw LoadError.invalidField(
          file,
          "direction",
          `direction only applies to document-level metrics, not ${metricStr}`,
        );
      }
      if (isDocumentLevel(metric)) {
        const threshold = def.threshold;
        if (threshold === undefined) throw LoadError.missingField(file, "threshold");
        if (!Number.isFinite(threshold)) {
          throw LoadError.invalidField(
            file,
            "threshold",
            `${threshold} is not a valid threshold — use a finite number`,
          );
        }
        const direction = def.direction;
        if (direction === undefined) throw LoadError.missingField(file, "direction");
        if (direction !== "above" && direction !== "below") {
          throw LoadError.invalidField(
            file,
            "direction",
            `${dq(direction)} is not a direction — use above or below`,
          );
        }
      }
      break;
    }
    case "inferential": {
      if (def.rubric === undefined || wsTrim(def.rubric).length === 0) {
        throw LoadError.missingField(file, "rubric");
      }
      if (def.flagExamples.length < 3) {
        throw LoadError.invalidField(
          file,
          "flag_examples",
          `an inferential rule needs at least 3 flag_examples — found ${def.flagExamples.length}`,
        );
      }
      if (def.passExamples.length < 3) {
        throw LoadError.invalidField(
          file,
          "pass_examples",
          `an inferential rule needs at least 3 pass_examples — found ${def.passExamples.length}`,
        );
      }
      if (def.severity !== undefined && parseSeverity(def.severity) === "error") {
        throw LoadError.invalidField(
          file,
          "severity",
          `"error" is too severe for an inferential rule — judge findings are advisory; use warning or suggestion`,
        );
      }
      break;
    }
  }
}

// Kept here to avoid a circular import with engines/statistical.ts; the
// values are duplicated in Metric::parse there.
export type MetricName =
  | "sentence-length"
  | "repetitive-openers"
  | "sentence-length-variance"
  | "cadence-autocorrelation"
  | "repeated-opener-density"
  | "triad-density"
  | "paired-adjective-rate";

const METRICS: MetricName[] = [
  "sentence-length",
  "repetitive-openers",
  "sentence-length-variance",
  "cadence-autocorrelation",
  "repeated-opener-density",
  "triad-density",
  "paired-adjective-rate",
];

export function metricFromString(s: string): MetricName | undefined {
  return (METRICS as string[]).includes(s) ? (s as MetricName) : undefined;
}

export function metricAlternatives(): string {
  return "sentence-length, repetitive-openers, sentence-length-variance, cadence-autocorrelation, repeated-opener-density, triad-density, or paired-adjective-rate";
}

export function isDocumentLevel(m: MetricName): boolean {
  return m !== "sentence-length" && m !== "repetitive-openers";
}

export function validatePackageName(file: string, name: string): void {
  const trimmed = wsTrim(name);
  if (trimmed.length === 0 || trimmed.includes("/") || /\s/.test(trimmed)) {
    throw LoadError.invalidField(
      file,
      "name",
      `${dq(name)} is not a valid package name — use a bare name like core (no slashes or spaces)`,
    );
  }
}

export function parseManifest(file: string, yaml: string): PackageManifest {
  let doc: Document;
  try {
    doc = parseDocument(yaml, { schema: "core" });
  } catch (e) {
    throw yamlError(file, e);
  }
  if (doc.errors.length > 0) throw LoadError.yaml(file, doc.errors[0].message);
  const raw = doc.toJS();
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) {
    throw LoadError.yaml(file, "manifest must be a mapping");
  }
  for (const k of Object.keys(raw)) {
    if (!MANIFEST_FIELDS.includes(k)) {
      throw LoadError.invalidField(
        file,
        k,
        `unknown field \`${k}\`, expected one of ${MANIFEST_FIELDS.map((f) => `\`${f}\``).join(", ")}`,
      );
    }
  }
  if (raw.name === undefined) throw LoadError.missingField(file, "name");
  if (raw.version === undefined) throw LoadError.missingField(file, "version");
  const manifest: PackageManifest = {
    name: String(raw.name),
    version: String(raw.version),
  };
  if (raw.description !== undefined && raw.description !== null) {
    manifest.description = String(raw.description);
  }
  validatePackageName(file, manifest.name);
  return manifest;
}
