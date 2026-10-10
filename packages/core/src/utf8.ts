// UTF-8 byte offsets on top of a JS string. Ranges in the public output are
// UTF-8 byte offsets (Rust parity); internally we index UTF-16 code units
// and convert at the boundaries.

export class Src {
  readonly text: string;
  /** u16 index → byte offset (length text.length + 1). */
  readonly u16ToByte: Uint32Array;
  /** byte offset → u16 index; defined at UTF-8 char boundaries. */
  readonly byteToU16: Map<number, number>;
  readonly byteLen: number;

  constructor(text: string) {
    this.text = text;
    this.u16ToByte = new Uint32Array(text.length + 1);
    this.byteToU16 = new Map();
    let u16 = 0;
    let byte = 0;
    for (const ch of text) {
      this.byteToU16.set(byte, u16);
      const w = ch.length; // 1 or 2 UTF-16 units
      this.u16ToByte.fill(byte, u16, u16 + w);
      byte += utf8Len(ch);
      u16 += w;
    }
    this.u16ToByte[u16] = byte;
    this.byteToU16.set(byte, u16);
    this.byteLen = byte;
  }

  /** Slice by byte range (bounds must be char boundaries). */
  slice(start: number, end: number): string {
    return this.text.slice(this.u16(start), this.u16(end));
  }

  /** Byte offset → UTF-16 index. Throws on a non-boundary offset. */
  u16(byte: number): number {
    const v = this.byteToU16.get(byte);
    if (v === undefined) throw new Error(`offset ${byte} is not a UTF-8 char boundary`);
    return v;
  }

  /** UTF-16 index → byte offset. */
  byte(u16: number): number {
    return this.u16ToByte[u16];
  }

  /** True when `byte` falls on a char boundary. */
  isBoundary(byte: number): boolean {
    return byte === 0 || byte === this.byteLen || this.byteToU16.has(byte);
  }
}

/** UTF-8 byte length of a JS string. */
export function utf8Len(s: string): number {
  let n = 0;
  for (const ch of s) {
    const cp = ch.codePointAt(0) ?? 0;
    n += cp < 0x80 ? 1 : cp < 0x800 ? 2 : cp < 0x10000 ? 3 : 4;
  }
  return n;
}

// Rust char-class shorthands (Unicode scalar semantics).
const ALPHABETIC = /\p{Alphabetic}/u;
const ALPHANUMERIC = /[\p{Alphabetic}\p{Nd}\p{Nl}\p{No}]/u;
const WHITESPACE = /\p{White_Space}/u;
const UPPERCASE = /\p{Uppercase}/u;

export function isAlphabetic(c: string): boolean {
  return ALPHABETIC.test(c);
}
export function isAlphanumeric(c: string): boolean {
  return ALPHANUMERIC.test(c);
}
export function isWhitespace(c: string): boolean {
  return WHITESPACE.test(c);
}
export function isUppercase(c: string): boolean {
  return UPPERCASE.test(c);
}
export function isAsciiDigit(c: string): boolean {
  return c >= "0" && c <= "9";
}
export function isAsciiUppercase(c: string): boolean {
  return c >= "A" && c <= "Z";
}

/** Iterate code points of `s` with their UTF-16 indices, like char_indices. */
export function* charIndices(s: string): Generator<[number, string]> {
  let i = 0;
  for (const ch of s) {
    yield [i, ch];
    i += ch.length;
  }
}

/** strsim::normalized_levenshtein over Unicode scalar values (JS code points). */
export function normalizedLevenshtein(a: string, b: string): number {
  const ac = [...a];
  const bc = [...b];
  if (ac.length === 0 && bc.length === 0) return 1.0;
  if (ac.length === 0 || bc.length === 0) return 0.0;
  let prev: number[] = Array.from({ length: bc.length + 1 }, (_, j) => j);
  let cur: number[] = new Array(bc.length + 1);
  for (let i = 1; i <= ac.length; i++) {
    cur[0] = i;
    for (let j = 1; j <= bc.length; j++) {
      const cost = ac[i - 1] === bc[j - 1] ? 0 : 1;
      cur[j] = Math.min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + cost);
    }
    [prev, cur] = [cur, prev];
  }
  const dist = prev[bc.length];
  return 1.0 - dist / Math.max(ac.length, bc.length);
}

// Rust str::trim* — White_Space property only (JS trim() also strips U+FEFF).
const WS_LEAD = /^\p{White_Space}+/u;
const WS_TAIL = /\p{White_Space}+$/u;
export function wsTrimStart(s: string): string {
  return s.replace(WS_LEAD, "");
}
export function wsTrimEnd(s: string): string {
  return s.replace(WS_TAIL, "");
}
export function wsTrim(s: string): string {
  return s.replace(WS_LEAD, "").replace(WS_TAIL, "");
}
