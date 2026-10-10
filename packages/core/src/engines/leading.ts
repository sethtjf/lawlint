// Sentence-opener engine — mirrors engines/leading.rs.

import type { Sentence } from "../document";
import { LoadError } from "../error";
import type { RuleDef } from "../loader";
import { compileRustRegex } from "../regex";
import type { Ctx, Interests, Rule, RuleMeta } from "../rule";
import { utf8Len, wsTrimStart } from "../utf8";

interface LeadingItem {
  regex: RegExp;
  message?: string;
  suggestion?: string;
}

export class LeadingEngine implements Rule {
  private readonly meta_: RuleMeta;
  private readonly items: LeadingItem[];
  private readonly defaultMessage: string;

  private constructor(meta: RuleMeta, items: LeadingItem[], defaultMessage: string) {
    this.meta_ = meta;
    this.items = items;
    this.defaultMessage = defaultMessage;
  }

  static fromDef(meta: RuleMeta, def: RuleDef, file: string): LeadingEngine {
    const items: LeadingItem[] = [];
    for (let i = 0; i < def.patterns.length; i++) {
      const needle = def.patterns[i].pattern;
      let regex: RegExp;
      try {
        regex = compileRustRegex(`(?i)^(?:${needle})`);
      } catch (e) {
        throw LoadError.invalidRegex(
          file,
          `patterns[${i}]`,
          needle,
          e instanceof Error ? e.message : String(e),
        );
      }
      const p = def.patterns[i];
      items.push({
        regex,
        message: p.kind === "detailed" ? p.message : undefined,
        suggestion: p.kind === "detailed" ? p.suggestion : undefined,
      });
    }
    return new LeadingEngine(meta, items, def.message ?? meta.description);
  }

  meta(): RuleMeta {
    return this.meta_;
  }
  interests(): Interests {
    return { sentences: true };
  }

  checkSentence(s: Sentence, ctx: Ctx): void {
    const slice = ctx.slice(s.range);
    const trimmed = wsTrimStart(slice);
    const base = s.range.start + utf8Len(slice.slice(0, slice.length - trimmed.length));
    for (const item of this.items) {
      const m = item.regex.exec(trimmed);
      if (m !== null && m.index === 0) {
        if (m[0].length === 0) continue;
        ctx.report({
          span: { start: base, end: base + utf8Len(m[0]) },
          message: item.message ?? this.defaultMessage,
          suggestion: item.suggestion,
        });
        return;
      }
    }
  }
}
