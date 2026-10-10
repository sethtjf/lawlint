// Finalize: line/col/excerpt, stats, score — mirrors scoring.rs.

import type { Document } from "./document";
import { compileRustRegex } from "./regex";
import type { Diagnostic, LintResult, Severity } from "./types";
import { type Src, wsTrim, wsTrimEnd, wsTrimStart } from "./utf8";

const WORD_RE = compileRustRegex(String.raw`(?u)\b[\w'’-]+\b`, "g");

/** Scope-aware word count: \b[\w'’-]+\b with code-block ranges blanked. */
export function wordCount(src: Src, doc: Document): number {
  const code = doc.blocks.filter((b) => b.kind === "codeblock").map((b) => b.range);
  let count = 0;
  WORD_RE.lastIndex = 0;
  for (const m of src.text.matchAll(WORD_RE)) {
    const s = src.byte(m.index ?? 0);
    const e = src.byte((m.index ?? 0) + m[0].length);
    if (!code.some((r) => s >= r.start && e <= r.end)) count += 1;
  }
  return count;
}

/** Byte offsets of the start of every line (line 1 starts at 0). */
export function lineStarts(src: Src): number[] {
  const starts = [0];
  for (let i = 0; i < src.text.length; i++) {
    if (src.text[i] === "\n") starts.push(src.byte(i + 1));
  }
  return starts;
}

/** (line, column): 1-based; column in UTF-16 code units. Byte offset in. */
export function location(src: Src, starts: number[], byteOffset: number): [number, number] {
  let idx = 0;
  while (idx < starts.length && starts[idx] <= byteOffset) idx += 1;
  const line = Math.max(1, idx);
  const lineStartByte = starts[line - 1];
  const u16Start = src.u16(lineStartByte);
  const u16Offset = src.u16(byteOffset);
  const column = u16Offset - u16Start + 1;
  return [line, column];
}

function points(severity: Severity): number {
  if (severity === "error") return 5;
  if (severity === "warning") return 3;
  return 1;
}

/** Tier-3 gate: drop below-floor findings, cap inferential severity at Warning. */
export function gateTier3(diagnostics: Diagnostic[], floor: number): Diagnostic[] {
  return diagnostics
    .filter((d) => d.tier !== "inferential" || (d.confidence ?? 0) >= floor)
    .map((d) => {
      if (d.tier === "inferential" && d.severity === "error") {
        return { ...d, severity: "warning" as Severity };
      }
      return d;
    });
}

export function finalize(src: Src, diagnostics: Diagnostic[], doc: Document): LintResult {
  diagnostics.sort((a, b) => a.span.start - b.span.start || a.span.end - b.span.end);
  const starts = lineStarts(src);
  const lines = src.text.split("\n");
  for (const d of diagnostics) {
    const [line, column] = location(src, starts, d.span.start);
    const [endLine, endColumn] = location(src, starts, d.span.end);
    d.line = line;
    d.column = column;
    d.endLine = endLine;
    d.endColumn = endColumn;
    d.excerpt = wsTrim(lines[line - 1] ?? "");
  }
  const words = wordCount(src, doc);
  const sentenceCount = doc.blocks.reduce((n, b) => n + b.sentences.length, 0);
  const penalty = diagnostics
    .filter((d) => d.intent === "detection")
    .reduce((sum, d) => {
      const confidence = d.tier === "inferential" ? (d.confidence ?? 1) : 1;
      return sum + points(d.severity) * (d.weight ?? 1) * confidence;
    }, 0);
  const density = (penalty / Math.max(words, 1)) * 1000;
  const score = Math.min(100, Math.max(0, Math.round(100 * Math.exp(-density / 100))));
  return { diagnostics, stats: { wordCount: words, sentenceCount, score } };
}
