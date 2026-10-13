// Legal-aware sentence segmentation + tokenization.
// All ranges are UTF-8 byte offsets into `source`.

import type { Sentence, Token } from "./document";
import { compileRustRegex } from "./regex";
import type { TextRange } from "./types";
import {
  charIndices,
  isAlphabetic,
  isAlphanumeric,
  isAsciiDigit,
  isAsciiUppercase,
  isUppercase,
  isWhitespace,
  utf8Len,
  wsTrim,
  wsTrimEnd,
} from "./utf8";

const ABBREVIATIONS = new Set([
  // case style / citation
  "v.",
  "vs.",
  "id.",
  "ibid.",
  "no.",
  "nos.",
  "fed.",
  "r.",
  "civ.",
  "crim.",
  "p.",
  "proc.",
  "evid.",
  "cir.",
  "ct.",
  "cl.",
  "u.s.",
  "s.",
  "ed.",
  "l.",
  "rev.",
  "stat.",
  "reg.",
  "sec.",
  "art.",
  "para.",
  "pp.",
  "seq.",
  // latin
  "e.g.",
  "i.e.",
  "etc.",
  "cf.",
  // corporate
  "inc.",
  "corp.",
  "ltd.",
  "co.",
  // honorifics
  "mr.",
  "mrs.",
  "ms.",
  "dr.",
  "prof.",
  "hon.",
  // months
  "jan.",
  "feb.",
  "mar.",
  "apr.",
  "jun.",
  "jul.",
  "aug.",
  "sep.",
  "sept.",
  "oct.",
  "nov.",
  "dec.",
]);

const ORDINAL_RE = compileRustRegex(String.raw`(?i)^\d+(d|st|nd|rd|th)$`);
const ENUM_RE = compileRustRegex(String.raw`^\(?([0-9]{1,3}|[A-Za-z]|[ivxlcIVXLC]{1,3})\)?$`);
const REPORTER_RE = compileRustRegex(String.raw`\d+\s+[A-Z][A-Za-z.]{0,15}\s+\d+`);
const CASE_STYLE_RE = compileRustRegex(String.raw`[A-Z][\w.'’-]*\s+vs?\.\s+[A-Z]`);

const CITATION_SIGNALS = ["See also", "See", "But see", "Cf.", "Accord", "E.g.,"];

/** Build a u16→byte offset table for a decoded string. */
export function u16ToByteTable(s: string): Uint32Array {
  const t = new Uint32Array(s.length + 1);
  let u16 = 0;
  let byte = 0;
  for (const ch of s) {
    t.fill(byte, u16, u16 + ch.length);
    u16 += ch.length;
    byte += utf8Len(ch);
  }
  t[u16] = byte;
  return t;
}

/** Split the block slice `range` (byte offsets into `src`) into sentences. */
export function splitSentences(slice: string, base: number, source: string): Sentence[] {
  // `slice` is the decoded block text; `base` is its absolute byte start.
  const chars: [number, string][] = [...charIndices(slice)];
  const u16ToByte = u16ToByteTable(slice);
  const byteOf = (u16: number) => u16ToByte[u16];
  const sentences: Sentence[] = [];
  let start: number | null = null; // u16 offset of sentence start
  let i = 0;

  const push = (s: number, e: number) => {
    if (e > s) {
      const abs: TextRange = { start: base + byteOf(s), end: base + byteOf(e) };
      sentences.push(makeSentence(source, abs, slice.slice(s, e)));
    }
  };

  while (i < chars.length) {
    const [off, c] = chars[i];
    if (start === null) {
      if (isWhitespace(c)) {
        i += 1;
        continue;
      }
      start = off;
    }
    if (c === "." || c === "!" || c === "?") {
      // Absorb a run of terminators, then trailing closers, then whitespace.
      let j = i + 1;
      while (
        j < chars.length &&
        (chars[j][1] === "." || chars[j][1] === "!" || chars[j][1] === "?")
      )
        j += 1;
      let k = j;
      while (
        k < chars.length &&
        (chars[k][1] === '"' ||
          chars[k][1] === "'" ||
          chars[k][1] === "”" ||
          chars[k][1] === "’" ||
          chars[k][1] === ")" ||
          chars[k][1] === "]")
      )
        k += 1;
      let m = k;
      while (m < chars.length && isWhitespace(chars[m][1])) m += 1;
      const endRel = k < chars.length ? chars[k][0] : slice.length;
      if (m >= chars.length) {
        push(start ?? off, endRel);
        start = null;
        i = m;
        continue;
      }
      const hasWs = m > k;
      const blocked = c === "." && j === i + 1 && isBlockedPeriod(slice, chars, i);
      if (hasWs && isLikelySentenceStart(chars[m][1]) && !blocked) {
        push(start ?? off, endRel);
        start = null;
        i = m;
        continue;
      }
      i = j;
      continue;
    }
    i += 1;
  }
  if (start !== null) {
    const trimmed = wsTrimEnd(slice.slice(start));
    push(start, start + trimmed.length);
  }
  return sentences;
}

function makeSentence(source: string, range: TextRange, text: string): Sentence {
  return { range, tokens: tokenize(text, range.start), isCitation: isCitationSentence(text) };
}

function isLikelySentenceStart(c: string): boolean {
  return (
    isUppercase(c) ||
    isAsciiDigit(c) ||
    c === '"' ||
    c === "“" ||
    c === "‘" ||
    c === "'" ||
    c === "("
  );
}

function isBlockedPeriod(text: string, chars: [number, string][], i: number): boolean {
  let idx = i;
  while (idx > 0) {
    const ch = chars[idx - 1][1];
    if (
      isAlphanumeric(ch) ||
      ch === "." ||
      ch === "(" ||
      ch === ")" ||
      ch === "'" ||
      ch === "’" ||
      ch === "-"
    ) {
      idx -= 1;
    } else {
      break;
    }
  }
  if (idx === i) return false; // no word attached
  const wstart = chars[idx][0];
  const word = text.slice(wstart, chars[i][0]);

  const lowered = `${word.toLowerCase()}.`;
  if (ABBREVIATIONS.has(lowered)) return true;
  // Single capital initial: "J." — word is a single char.
  if (word.length === 1 && isAsciiUppercase(word)) return true;
  if (ORDINAL_RE.test(word)) return true;
  if (ENUM_RE.test(word) && atLineStart(text, wstart)) return true;
  return false;
}

function atLineStart(text: string, pos: number): boolean {
  const before = text.slice(0, pos);
  const nl = before.lastIndexOf("\n");
  const lineBegin = nl === -1 ? 0 : nl + 1;
  return wsTrim(before.slice(lineBegin)).length === 0;
}

/** Tokenize `text` (decoded slice starting at byte `base`) into tokens. */
export function tokenize(text: string, base: number): Token[] {
  const chars: [number, string][] = [...charIndices(text)];
  const u16ToByte = u16ToByteTable(text);
  const tokens: Token[] = [];
  let i = 0;
  while (i < chars.length) {
    const [off, c] = chars[i];
    if (isWhitespace(c)) {
      i += 1;
      continue;
    }
    if (isAlphabetic(c)) {
      let j = i + 1;
      while (j < chars.length) {
        const ch = chars[j][1];
        if (isAlphanumeric(ch)) {
          j += 1;
        } else if (
          (ch === "'" || ch === "’" || ch === "-") &&
          j + 1 < chars.length &&
          isAlphanumeric(chars[j + 1][1])
        ) {
          j += 1;
        } else {
          break;
        }
      }
      const end = j < chars.length ? chars[j][0] : text.length;
      tokens.push({
        range: { start: base + u16ToByte[off], end: base + u16ToByte[end] },
        kind: "word",
      });
      i = j;
    } else if (isAsciiDigit(c)) {
      let j = i + 1;
      while (j < chars.length) {
        const ch = chars[j][1];
        if (isAsciiDigit(ch)) {
          j += 1;
        } else if (
          (ch === "." || ch === ",") &&
          j + 1 < chars.length &&
          isAsciiDigit(chars[j + 1][1])
        ) {
          j += 1;
        } else {
          break;
        }
      }
      const end = j < chars.length ? chars[j][0] : text.length;
      tokens.push({
        range: { start: base + u16ToByte[off], end: base + u16ToByte[end] },
        kind: "number",
      });
      i = j;
    } else {
      const endU16 = off + c.length;
      tokens.push({
        range: { start: base + u16ToByte[off], end: base + u16ToByte[endU16] },
        kind: "punct",
      });
      i += 1;
    }
  }
  return tokens;
}

/** Citation heuristic on the decoded sentence text. */
export function isCitationSentence(sentenceText: string): boolean {
  let t = sentenceText;
  while (
    t.length > 0 &&
    (isWhitespace(t[0]) || t[0] === '"' || t[0] === "“" || t[0] === "‘" || t[0] === "'")
  ) {
    t = t.slice(1);
  }
  if (REPORTER_RE.test(t)) return true;
  for (const signal of CITATION_SIGNALS) {
    if (t.startsWith(signal)) {
      const rest = t.slice(signal.length);
      if (rest.length > 0 && isWhitespace(rest[0]) && CASE_STYLE_RE.test(rest)) return true;
    }
  }
  return false;
}
