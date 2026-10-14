// Read `.docx` into lawlint's plain-text model and write fixes back as Word
// tracked changes (`<w:ins>`/`<w:del>`) plus review comments.
//
// The `.docx` is treated as an OPC (zip) package. We project `document.xml`
// into a flat string, lint that with the unchanged engine, then splice the
// resulting byte-range fixes back as revisions — editing only the affected
// runs and leaving every other part of the package untouched.

import type { Diagnostic } from "@lawlint/core";
import { Package, strToBytes } from "./pkg";
import { project } from "./project";
import { type CommentOut, type Ids, apply } from "./revise";
import { selectEdits } from "./select";

const DOCUMENT = "word/document.xml";
const COMMENTS = "word/comments.xml";
const CONTENT_TYPES = "[Content_Types].xml";
const DOC_RELS = "word/_rels/document.xml.rels";
const W_NS = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const COMMENTS_CT = "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml";
const COMMENTS_REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments";

export type DocxErrorKind = "zip" | "xml" | "missing_part" | "malformed";

export class DocxError extends Error {
  readonly kind: DocxErrorKind;
  constructor(kind: DocxErrorKind, message: string) {
    super(message);
    this.name = "DocxError";
    this.kind = kind;
  }
}

export interface ReviseOptions {
  author: string;
  /** xsd:dateTime, e.g. `2026-07-19T01:11:00Z`. Defaults to now (UTC). */
  date: string | null;
  /** Include `maybe-incorrect` fixes (AI rules' suggested rewrites). */
  includeAiRewrites: boolean;
  /** Anchor a comment over every finding that has no applicable fix. */
  annotateFindings: boolean;
}

export function defaultReviseOptions(): ReviseOptions {
  return {
    author: "lawlint",
    date: iso8601UtcNow(),
    includeAiRewrites: true,
    annotateFindings: true,
  };
}

export interface ReviseResult {
  bytes: Uint8Array;
  /** Fixes turned into tracked changes (del+ins+comment). */
  applied: number;
  /** Fixes that could not be safely rewritten but got a comment anchor. */
  annotated: number;
  /** How many of `applied` came from AI rules (`maybe-incorrect`). */
  aiApplied: number;
  /** Edits dropped entirely (no containing run, or unusable run). */
  skipped: number;
}

/** Extract the plain text of a `.docx` for linting. */
export function extract(docx: Uint8Array): string {
  const pkg = Package.read(docx);
  const xml = pkg.getStr(DOCUMENT);
  if (xml === undefined) throw new DocxError("missing_part", DOCUMENT);
  return project(xml).text;
}

/**
 * Apply MachineApplicable fixes from `diagnostics` to the `.docx` as tracked
 * changes + comments, returning the new package bytes.
 */
export function applyTrackedChanges(
  docx: Uint8Array,
  diagnostics: readonly Diagnostic[],
  opts: ReviseOptions = defaultReviseOptions(),
): ReviseResult {
  const pkg = Package.read(docx);
  const xml = pkg.getStr(DOCUMENT);
  if (xml === undefined) throw new DocxError("missing_part", DOCUMENT);
  const projection = project(xml);

  const existingComments = pkg.getStr(COMMENTS);
  const commentBase = (existingComments !== undefined ? maxCommentId(existingComments) : 0) + 1;
  const ids: Ids = { nextRev: 900_000, nextComment: commentBase };

  const selection = selectEdits(projection, diagnostics, opts, ids);
  const editsByOrdinal = selection.editsByOrdinal;
  let skipped = selection.skipped;

  if (editsByOrdinal.size === 0) {
    return { bytes: docx, applied: 0, annotated: 0, aiApplied: 0, skipped };
  }

  const out = apply(xml, editsByOrdinal, opts.author, opts.date, ids);
  skipped += out.skipped;
  const annotated = out.annotated;
  const applied = out.comments.length - annotated;
  // Cap rather than over-report the AI share — the emitter can drop
  // redlines it refuses to touch without reporting which.
  const aiApplied = Math.min(selection.aiRedlines, applied);

  pkg.set(DOCUMENT, out.documentXml);

  if (out.comments.length > 0) {
    const commentsXml = buildCommentsXml(existingComments, out.comments);
    pkg.set(COMMENTS, strToBytes(commentsXml));
    wireCommentsPart(pkg);
  }

  return {
    bytes: pkg.write(),
    applied,
    annotated,
    aiApplied,
    skipped,
  };
}

/** lib.rs's own xml_escape — escapes `& < > "` only (not `'`). */
function xmlEscape(s: string): string {
  let out = "";
  for (const c of s) {
    switch (c) {
      case "&":
        out += "&amp;";
        break;
      case "<":
        out += "&lt;";
        break;
      case ">":
        out += "&gt;";
        break;
      case '"':
        out += "&quot;";
        break;
      default:
        out += c;
    }
  }
  return out;
}

function initials(author: string): string {
  // Rust: split_whitespace (Unicode White_Space — \p{White_Space} matches it
  // exactly, unlike JS \s which adds U+FEFF and drops U+0085), then the first
  // char (code point, not UTF-16 unit) of each word.
  const inits = author
    .split(/\p{White_Space}+/u)
    .filter((w) => w.length > 0)
    .map((w) => [...w][0])
    .join("");
  return inits === "" ? "L" : inits.toUpperCase();
}

function commentBlock(c: CommentOut): string {
  let attrs = ` w:id="${c.id}" w:author="${xmlEscape(c.author)}" w:initials="${xmlEscape(initials(c.author))}"`;
  if (c.date !== null) attrs += ` w:date="${xmlEscape(c.date)}"`;
  return `<w:comment${attrs}><w:p><w:r><w:t xml:space="preserve">${xmlEscape(c.text)}</w:t></w:r></w:p></w:comment>`;
}

function buildCommentsXml(existing: string | undefined, comments: CommentOut[]): string {
  const blocks = comments.map(commentBlock).join("");
  if (existing?.includes("</w:comments>")) {
    return replaceFirst(existing, "</w:comments>", `${blocks}</w:comments>`);
  }
  return `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<w:comments xmlns:w="${W_NS}">${blocks}</w:comments>`;
}

function maxCommentId(commentsXml: string): number {
  let max = 0;
  let rest = commentsXml;
  let pos = rest.indexOf("<w:comment ");
  while (pos !== -1) {
    rest = rest.slice(pos);
    const id = attrValue(rest, 'w:id="');
    // Rust id.parse::<usize>().unwrap_or(0): all digits (optional leading
    // `+`), nothing else — "12abc" and " 5" both parse to 0 there.
    if (id !== undefined && /^\+?[0-9]+$/.test(id)) max = Math.max(max, Number(id));
    rest = rest.slice(1);
    pos = rest.indexOf("<w:comment ");
  }
  return max;
}

/** Read the value of `needle` (e.g. `w:id="`) starting near the front of `s`. */
function attrValue(s: string, needle: string): string | undefined {
  const start = s.indexOf(needle);
  if (start === -1) return undefined;
  const from = start + needle.length;
  const end = s.indexOf('"', from);
  if (end === -1) return undefined;
  return s.slice(from, end);
}

/**
 * Rust `replacen(needle, replacement, 1)`: first occurrence, replacement is
 * literal (String.replace would interpret `$&`, `$'`, `$$`, …).
 */
function replaceFirst(haystack: string, needle: string, replacement: string): string {
  const at = haystack.indexOf(needle);
  if (at === -1) return haystack;
  return haystack.slice(0, at) + replacement + haystack.slice(at + needle.length);
}

/**
 * Declare the comments part in `[Content_Types].xml` and relate it from
 * `document.xml.rels` if not already present.
 */
function wireCommentsPart(pkg: Package): void {
  const ct = pkg.getStr(CONTENT_TYPES);
  if (ct === undefined) throw new DocxError("missing_part", CONTENT_TYPES);
  if (!ct.includes("/word/comments.xml")) {
    const overrideEl = `<Override PartName="/word/comments.xml" ContentType="${COMMENTS_CT}"/>`;
    pkg.set(CONTENT_TYPES, strToBytes(replaceFirst(ct, "</Types>", `${overrideEl}</Types>`)));
  }

  const rels = pkg.getStr(DOC_RELS);
  if (rels === undefined) throw new DocxError("missing_part", DOC_RELS);
  if (!rels.includes(COMMENTS_REL)) {
    const rid = nextRelId(rels);
    const rel = `<Relationship Id="${rid}" Type="${COMMENTS_REL}" Target="comments.xml"/>`;
    pkg.set(DOC_RELS, strToBytes(replaceFirst(rels, "</Relationships>", `${rel}</Relationships>`)));
  }
}

function nextRelId(rels: string): string {
  let max = 0;
  let rest = rels;
  let pos = rest.indexOf('Id="rId');
  while (pos !== -1) {
    const start = pos + 'Id="rId'.length;
    let digits = "";
    while (start + digits.length < rest.length && /\d/.test(rest[start + digits.length])) {
      digits += rest[start + digits.length];
    }
    const n = Number.parseInt(digits, 10);
    if (Number.isFinite(n)) max = Math.max(max, n);
    rest = rest.slice(pos + 4);
    pos = rest.indexOf('Id="rId');
  }
  return `rId${max + 1}`;
}

/** Current UTC time as an xsd:dateTime (`YYYY-MM-DDThh:mm:ssZ`). */
function iso8601UtcNow(): string {
  return `${new Date().toISOString().slice(0, 19)}Z`;
}
