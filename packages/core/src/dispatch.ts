// Single-pass dispatcher: traversal, scope mask, suppression — mirrors dispatch.rs.

import type { LintOptions } from "./config";
import type { BlockKind, Document, Sentence } from "./document";
import type { RuleSet } from "./registry";
import { Ctx, type Rule } from "./rule";
import { lineStarts, wordCount } from "./scoring";
import type { Diagnostic, Scope, TextRange } from "./types";
import { type Src, wsTrim, wsTrimEnd, wsTrimStart } from "./utf8";

function blockAdmitted(scope: Scope, kind: BlockKind): boolean {
  if (scope === "all") return true;
  if (scope === "text") return kind !== "codeblock";
  return kind === "paragraph" || kind === "listitem";
}

function sentenceAdmitted(scope: Scope, kind: BlockKind, s: Sentence): boolean {
  if (scope === "all") return true;
  if (scope === "text") return kind !== "codeblock";
  return (kind === "paragraph" || kind === "listitem") && !s.isCitation;
}

export function scopeMask(scope: Scope, src: Src, doc: Document): TextRange[] {
  if (scope === "all") return [{ start: 0, end: src.byteLen }];
  if (scope === "text") return doc.blocks.filter((b) => b.kind !== "codeblock").map((b) => b.range);
  return doc.blocks
    .filter((b) => b.kind === "paragraph" || b.kind === "listitem")
    .flatMap((b) => b.sentences)
    .filter((s) => !s.isCitation)
    .map((s) => s.range);
}

export function maskContains(mask: TextRange[], span: TextRange): boolean {
  // partition_point: first index with r.start > span.start
  let idx = 0;
  while (idx < mask.length && mask[idx].start <= span.start) idx += 1;
  if (idx === 0) return false;
  const r = mask[idx - 1];
  return r.start <= span.start && span.end <= r.end;
}

// ---- Suppression comments ----------------------------------------------

interface LineSuppression {
  all: boolean;
  ids: Set<string>;
}
const emptySupp = (): LineSuppression => ({ all: false, ids: new Set() });

function suppSuppresses(s: LineSuppression, id: string): boolean {
  return s.all || s.ids.has(id);
}
function suppMerge(a: LineSuppression, b: LineSuppression) {
  a.all = a.all || b.all;
  for (const id of b.ids) a.ids.add(id);
}

export class Suppressions {
  private readonly starts: number[];
  private readonly perLine: LineSuppression[];
  private readonly file: LineSuppression;

  private constructor(starts: number[], perLine: LineSuppression[], file: LineSuppression) {
    this.starts = starts;
    this.perLine = perLine;
    this.file = file;
  }

  static create(src: Src, ruleSet: RuleSet): Suppressions {
    const starts = lineStarts(src);
    const lines = src.text.split("\n");
    const perLine: LineSuppression[] = lines.map(emptySupp);
    const file = emptySupp();
    const active = emptySupp();
    const nextLine: [number, LineSuppression][] = [];

    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];
      let directiveLine = false;
      const posFile = findDirective(line, "lawlint-disable-file");
      const posNext = posFile === -1 ? findDirective(line, "lawlint-disable-next-line") : -1;
      const posDisable =
        posFile === -1 && posNext === -1 ? findDirective(line, "lawlint-disable") : -1;
      const posEnable =
        posFile === -1 && posNext === -1 && posDisable === -1
          ? findDirective(line, "lawlint-enable")
          : -1;
      if (posFile !== -1) {
        directiveLine = true;
        const { all, ids } = parseIds(line.slice(posFile + "lawlint-disable-file".length), ruleSet);
        suppMerge(file, { all, ids });
      } else if (posNext !== -1) {
        directiveLine = true;
        const { all, ids } = parseIds(
          line.slice(posNext + "lawlint-disable-next-line".length),
          ruleSet,
        );
        for (let j = i + 1; j < lines.length; j++) {
          if (wsTrim(lines[j]).length > 0) {
            nextLine.push([j, { all, ids }]);
            break;
          }
        }
      } else if (posDisable !== -1) {
        directiveLine = true;
        const { all, ids } = parseIds(line.slice(posDisable + "lawlint-disable".length), ruleSet);
        if (all) active.all = true;
        else for (const id of ids) active.ids.add(id);
      } else if (posEnable !== -1) {
        directiveLine = true;
        const { all, ids } = parseIds(line.slice(posEnable + "lawlint-enable".length), ruleSet);
        if (all) {
          active.all = false;
          active.ids.clear();
        } else {
          active.all = false;
          for (const id of ids) active.ids.delete(id);
        }
      }
      perLine[i] = { all: active.all, ids: new Set(active.ids) };
      if (directiveLine) perLine[i].all = true;
    }
    for (const [target, supp] of nextLine) suppMerge(perLine[target], supp);
    return new Suppressions(starts, perLine, file);
  }

  suppressed(id: string, byteOffset: number): boolean {
    if (suppSuppresses(this.file, id)) return true;
    let idx = 0;
    while (idx < this.starts.length && this.starts[idx] <= byteOffset) idx += 1;
    const line = Math.max(1, idx) - 1;
    const l = this.perLine[line];
    return l !== undefined && suppSuppresses(l, id);
  }
}

function parseIds(rest: string, ruleSet: RuleSet): { all: boolean; ids: Set<string> } {
  const cleaned = rest.replaceAll("-->", " ").replaceAll("*/", " ").replaceAll(",", " ");
  const tokens = cleaned.split(/\s+/).filter((t) => t.length > 0);
  if (tokens.length === 0) return { all: true, ids: new Set() };
  const ids = new Set<string>();
  for (const t of tokens) {
    const id = ruleSet.resolve(t) ?? ruleSet.resolve(t.toLowerCase());
    if (id !== undefined) ids.add(id);
  }
  return { all: false, ids };
}

/** Byte offset of the first ASCII-case-insensitive occurrence of needle. */
function findDirective(line: string, needle: string): number {
  const lower = line.toLowerCase();
  // needle is pure ASCII; toLowerCase on the line could shift offsets for
  // non-ASCII chars (e.g. İ). Scan byte-wise instead.
  const bytes = new TextEncoder().encode(line);
  const n = new TextEncoder().encode(needle.toLowerCase());
  outer: for (let i = 0; i + n.length <= bytes.length; i++) {
    for (let j = 0; j < n.length; j++) {
      let b = bytes[i + j];
      if (b >= 65 && b <= 90) b += 32; // ASCII lowercase
      if (b !== n[j]) continue outer;
    }
    // i is a byte offset into the line's UTF-8
    return byteOffsetToU16(line, i);
  }
  return -1;
}

function byteOffsetToU16(s: string, byte: number): number {
  // needle/delimiters are ASCII; count bytes over code points.
  let u16 = 0;
  let b = 0;
  for (const ch of s) {
    if (b >= byte) return u16;
    const cp = ch.codePointAt(0) ?? 0;
    b += cp < 0x80 ? 1 : cp < 0x800 ? 2 : cp < 0x10000 ? 3 : 4;
    u16 += ch.length;
  }
  return u16;
}

function thresholdFor(options: LintOptions, ruleSet: RuleSet, id: string): number | undefined {
  const thresholds = options.thresholds;
  if (thresholds === undefined) return undefined;
  if (thresholds[id] !== undefined) return thresholds[id];
  for (const [k, v] of Object.entries(thresholds)) {
    if (ruleSet.resolve(k) === id) return v;
  }
  return undefined;
}

interface Slot {
  rule: Rule;
  threshold?: number;
  diagnostics: Diagnostic[];
}

function collect(slot: Slot, ctx: Ctx) {
  const meta = slot.rule.meta();
  for (const r of ctx.takeReports()) {
    const d: Diagnostic = {
      ruleId: meta.id,
      severity: meta.severity,
      tier: meta.tier,
      intent: meta.intent,
      span: r.span,
      message: r.message,
      line: 0,
      column: 0,
      excerpt: "",
    };
    if (r.suggestion !== undefined) d.suggestion = r.suggestion;
    if (r.weight !== undefined) d.weight = r.weight;
    if (r.fix !== undefined) d.fix = r.fix;
    slot.diagnostics.push(d);
  }
}

export function run(
  src: Src,
  doc: Document,
  rules: Rule[],
  options: LintOptions,
  ruleSet: RuleSet,
): Diagnostic[] {
  const ctx = new Ctx(src, wordCount(src, doc));
  const slots: Slot[] = rules.map((rule) => ({
    rule,
    threshold: thresholdFor(options, ruleSet, rule.meta().id),
    diagnostics: [],
  }));

  for (const block of doc.blocks) {
    for (const slot of slots) {
      if (slot.rule.interests().blocks && blockAdmitted(slot.rule.meta().scope, block.kind)) {
        ctx.setThresholdOverride(slot.threshold);
        slot.rule.checkBlock?.(block, ctx);
        collect(slot, ctx);
      }
    }
    for (const sentence of block.sentences) {
      for (const slot of slots) {
        if (
          slot.rule.interests().sentences &&
          sentenceAdmitted(slot.rule.meta().scope, block.kind, sentence)
        ) {
          ctx.setThresholdOverride(slot.threshold);
          slot.rule.checkSentence?.(sentence, ctx);
          collect(slot, ctx);
        }
      }
      for (const token of sentence.tokens) {
        for (const slot of slots) {
          if (
            slot.rule.interests().tokens &&
            sentenceAdmitted(slot.rule.meta().scope, block.kind, sentence)
          ) {
            ctx.setThresholdOverride(slot.threshold);
            slot.rule.checkToken?.(token, ctx);
            collect(slot, ctx);
          }
        }
      }
    }
  }
  for (const slot of slots) {
    if (slot.rule.interests().documentExit) {
      ctx.setThresholdOverride(slot.threshold);
      slot.rule.onDocumentExit?.(doc, ctx);
      collect(slot, ctx);
    }
  }

  const suppressions = Suppressions.create(src, ruleSet);
  const out: Diagnostic[] = [];
  for (const slot of slots) {
    if (slot.diagnostics.length === 0) continue;
    const mask = scopeMask(slot.rule.meta().scope, src, doc);
    for (const d of slot.diagnostics) {
      if (!maskContains(mask, d.span)) continue;
      if (suppressions.suppressed(d.ruleId, d.span.start)) continue;
      out.push(d);
    }
  }
  return out;
}
