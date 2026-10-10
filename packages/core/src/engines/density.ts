// Density engine — mirrors engines/density.rs.

import type { Block, Document } from "../document";
import { LoadError } from "../error";
import type { RuleDef } from "../loader";
import { compileRustRegex } from "../regex";
import type { Ctx, Interests, Rule, RuleMeta } from "../rule";
import type { TextRange } from "../types";
import { utf8Len } from "../utf8";

export class DensityEngine implements Rule {
  private readonly meta_: RuleMeta;
  private readonly re: RegExp;
  private readonly defaultThreshold: number;
  private readonly message: string;
  private count = 0;
  private firstSpan?: TextRange;

  private constructor(meta: RuleMeta, re: RegExp, threshold: number, message: string) {
    this.meta_ = meta;
    this.re = re;
    this.defaultThreshold = threshold;
    this.message = message;
  }

  static fromDef(meta: RuleMeta, def: RuleDef, file: string): DensityEngine {
    if (def.threshold === undefined) throw LoadError.missingField(file, "threshold");
    if (def.patterns.length !== 1) {
      throw LoadError.invalidField(
        file,
        "patterns",
        `a density rule needs exactly one pattern, but ${def.patterns.length} were given`,
      );
    }
    const pattern = def.patterns[0].pattern;
    let re: RegExp;
    try {
      re = compileRustRegex(pattern);
    } catch (e) {
      throw LoadError.invalidRegex(
        file,
        "patterns[0]",
        pattern,
        e instanceof Error ? e.message : String(e),
      );
    }
    return new DensityEngine(meta, re, def.threshold, def.message ?? meta.description);
  }

  meta(): RuleMeta {
    return this.meta_;
  }
  interests(): Interests {
    return { blocks: true, documentExit: true };
  }

  checkBlock(b: Block, ctx: Ctx): void {
    const text = ctx.slice(b.range);
    const g = new RegExp(
      this.re.source,
      this.re.flags.includes("g") ? this.re.flags : `${this.re.flags}g`,
    );
    let m = g.exec(text);
    while (m !== null) {
      this.count += 1;
      if (this.firstSpan === undefined) {
        const group = m[1] !== undefined ? { s: m.index + m[0].indexOf(m[1]), e: 0 } : undefined;
        // Capture group 1 marks the reportable part; find its offset inside m[0].
        let startU16 = m.index;
        let endU16 = m.index + m[0].length;
        if (m[1] !== undefined) {
          const idx = m.indices?.[1];
          if (idx) {
            startU16 = idx[0];
            endU16 = idx[1];
          } else {
            const inner = m[0].indexOf(m[1]);
            startU16 = m.index + inner;
            endU16 = startU16 + m[1].length;
          }
        }
        const prefixBytes = utf8Len(text.slice(0, startU16));
        const start = b.range.start + prefixBytes;
        const end = Math.max(
          b.range.start +
            prefixBytes +
            utf8Len(
              m.indices?.[1]
                ? text.slice(m.indices[1][0], m.indices[1][1])
                : m[1] !== undefined
                  ? m[1]
                  : m[0],
            ),
          start + 1,
        );
        this.firstSpan = { start, end };
      }
      if (m[0].length === 0) g.lastIndex = m.index + 1;
      m = g.exec(text);
    }
  }

  onDocumentExit(_doc: Document, ctx: Ctx): void {
    const span = this.firstSpan;
    if (span === undefined) return;
    const words = Math.max(ctx.wordCount, 1);
    const threshold = ctx.threshold(this.defaultThreshold);
    if ((this.count / words) * 1000 <= threshold) return;
    const weight = Math.max(1, Math.ceil(this.count - (threshold * words) / 1000));
    ctx.report({
      span,
      message: `${this.message} (${this.count} occurrences in ${words} words)`,
      weight,
    });
  }
}
