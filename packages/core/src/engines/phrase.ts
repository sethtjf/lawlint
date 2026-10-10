// Phrase engine — mirrors engines/phrase.rs.

import type { Block } from "../document";
import { LoadError } from "../error";
import { type RuleDef, scanCaptureRefs, validateFixCaptureRefs } from "../loader";
import { compileRustRegex, rustFindIter } from "../regex";
import type { Ctx, Interests, Report, Rule, RuleMeta } from "../rule";
import { Src, isAlphabetic, isUppercase, utf8Len } from "../utf8";

type FixSpec =
  | { kind: "literal"; text: string }
  | { kind: "expand"; template: string; hasRefs: boolean };

interface PhraseItem {
  regex: RegExp;
  message?: string;
  suggestion?: string;
  fix?: FixSpec;
}

/** Iterate matches of `re` over decoded `text`, byte offsets. */
function* matches(
  re: RegExp,
  text: string,
): Generator<{ start: number; end: number; m: RegExpExecArray }> {
  let prefixBytes = 0;
  let prefixU16 = 0;
  for (const m of rustFindIter(re, text)) {
    // Convert match UTF-16 index to bytes relative to text.
    const byteStart = prefixBytes + utf8Len(text.slice(prefixU16, m.index));
    const byteEnd = byteStart + utf8Len(m[0]);
    yield { start: byteStart, end: byteEnd, m };
    prefixBytes = byteEnd;
    prefixU16 = m.index + m[0].length;
  }
}

function matchCase(matched: string, replacement: string): string {
  let letters = 0;
  let allUpper = true;
  let firstUpper: boolean | undefined;
  for (const c of matched) {
    if (!isAlphabetic(c)) continue;
    letters += 1;
    const upper = isUppercase(c);
    if (!upper) allUpper = false;
    if (firstUpper === undefined) firstUpper = upper;
  }
  if (letters >= 2 && allUpper) return replacement.toUpperCase();
  if (firstUpper === true && replacement.length > 0) {
    const first = [...replacement][0];
    return first.toUpperCase() + replacement.slice(first.length);
  }
  return replacement;
}

/** regex Captures::expand grammar: $1/${1}, $name/${name}, $$ → $. */
export function expandTemplate(template: string, m: RegExpExecArray): string {
  let out = "";
  let i = 0;
  while (i < template.length) {
    if (template[i] !== "$") {
      out += template[i];
      i += 1;
      continue;
    }
    if (template[i + 1] === "$") {
      out += "$";
      i += 2;
      continue;
    }
    let name: string | undefined;
    if (template[i + 1] === "{") {
      const end = template.indexOf("}", i + 2);
      if (end === -1) {
        out += "$";
        i += 1;
        continue;
      }
      name = template.slice(i + 2, end);
      i = end + 1;
    } else {
      let j = i + 1;
      while (j < template.length && /[0-9A-Za-z_]/.test(template[j])) j += 1;
      if (j === i + 1) {
        out += "$";
        i += 1;
        continue;
      }
      name = template.slice(i + 1, j);
      i = j;
    }
    if (/^\d+$/.test(name)) {
      const n = Number.parseInt(name, 10);
      out += m[n] ?? "";
    } else {
      out += m.groups?.[name] ?? "";
    }
  }
  return out;
}

export class PhraseEngine implements Rule {
  private readonly meta_: RuleMeta;
  private readonly items: PhraseItem[];
  private readonly defaultMessage: string;
  private readonly allowContext?: { re: RegExp; window: number };

  private constructor(
    meta: RuleMeta,
    items: PhraseItem[],
    defaultMessage: string,
    allowContext?: { re: RegExp; window: number },
  ) {
    this.meta_ = meta;
    this.items = items;
    this.defaultMessage = defaultMessage;
    this.allowContext = allowContext;
  }

  static fromDef(meta: RuleMeta, def: RuleDef, file: string): PhraseEngine {
    const items: PhraseItem[] = [];
    for (let i = 0; i < def.patterns.length; i++) {
      const p = def.patterns[i];
      const field = `patterns[${i}]`;
      let regex: RegExp;
      try {
        regex = compileRustRegex(p.pattern);
      } catch (e) {
        throw LoadError.invalidRegex(
          file,
          field,
          p.pattern,
          e instanceof Error ? e.message : String(e),
        );
      }
      const fixLit = p.kind === "detailed" ? p.fix : undefined;
      const fixTmpl = p.kind === "detailed" ? p.fixTemplate : undefined;
      let fix: FixSpec | undefined;
      if (fixLit !== undefined && fixTmpl !== undefined) {
        throw LoadError.invalidField(
          file,
          `${field}.fixTemplate`,
          "a pattern may set `fix` (literal) or `fixTemplate` (capture-group interpolation), not both",
        );
      }
      if (fixLit !== undefined) {
        fix = { kind: "literal", text: fixLit };
      } else if (fixTmpl !== undefined) {
        validateFixCaptureRefs(file, `${field}.fixTemplate`, fixTmpl, regex);
        fix = { kind: "expand", template: fixTmpl, hasRefs: scanCaptureRefs(fixTmpl).length > 0 };
      }
      items.push({
        regex,
        message: p.kind === "detailed" ? p.message : undefined,
        suggestion: p.kind === "detailed" ? p.suggestion : undefined,
        fix,
      });
    }
    let allowContext: { re: RegExp; window: number } | undefined;
    if (def.allowContext !== undefined) {
      try {
        allowContext = {
          re: compileRustRegex(def.allowContext.pattern),
          window: def.allowContext.window,
        };
      } catch (e) {
        throw LoadError.invalidRegex(
          file,
          "allow_context.pattern",
          def.allowContext.pattern,
          e instanceof Error ? e.message : String(e),
        );
      }
    }
    const defaultMessage = def.message ?? meta.description;
    return new PhraseEngine(meta, items, defaultMessage, allowContext);
  }

  meta(): RuleMeta {
    return this.meta_;
  }
  interests(): Interests {
    return { blocks: true };
  }

  private allowContextSkips(ctx: Ctx, start: number, end: number): boolean {
    const ac = this.allowContext;
    if (!ac) return false;
    const src = ctx.srcBytes;
    let lo = Math.max(0, start - ac.window);
    while (lo > 0 && !src.isBoundary(lo)) lo -= 1;
    let hi = Math.min(src.byteLen, end + ac.window);
    while (hi < src.byteLen && !src.isBoundary(hi)) hi += 1;
    return ac.re.test(src.slice(lo, hi));
  }

  checkBlock(b: Block, ctx: Ctx): void {
    const src = ctx.srcBytes;
    const text = src.slice(b.range.start, b.range.end);
    for (const item of this.items) {
      for (const m of matches(item.regex, text)) {
        if (m.start === m.end) continue;
        const start = b.range.start + m.start;
        const end = b.range.start + m.end;
        if (this.allowContextSkips(ctx, start, end)) continue;
        const span = { start, end };
        const message = item.message ?? this.defaultMessage;
        const matched = src.slice(start, end);
        let fix: Report["fix"];
        if (item.fix?.kind === "literal") {
          fix = {
            edits: [{ range: span, replacement: matchCase(matched, item.fix.text) }],
            applicability: "machine-applicable",
          };
        } else if (item.fix?.kind === "expand") {
          const expanded = expandTemplate(item.fix.template, m.m);
          const replacement = item.fix.hasRefs ? expanded : matchCase(matched, expanded);
          fix = { edits: [{ range: span, replacement }], applicability: "machine-applicable" };
        }
        ctx.report({ span, message, suggestion: item.suggestion, fix });
      }
    }
  }
}
