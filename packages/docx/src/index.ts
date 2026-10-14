// Read `.docx` into lawlint's plain-text model and write fixes back as Word
// tracked changes (`<w:ins>`/`<w:del>`) plus review comments.
//
// The `.docx` is treated as an OPC (zip) package. We project `document.xml`
// into a flat string, lint that with the unchanged engine, then splice the
// resulting byte-range fixes back as revisions — editing only the affected
// runs and leaving every other part of the package untouched.

import type { Applicability, Diagnostic } from "@lawlint/core";
import { Package, strToBytes } from "./pkg";
import { type Projection, type RunText, project } from "./project";
import { type CommentOut, type Ids, type RunEdit, apply } from "./revise";

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

interface EditSelection {
  editsByOrdinal: Map<number, RunEdit[]>;
  skipped: number;
  aiRedlines: number;
}

interface Candidate {
  start: number;
  end: number;
  replacement: string | null;
  message: string;
  isAi: boolean;
}

/**
 * Resolve each fix's edit to the run(s) it lands in and decide whether it
 * becomes a full redline or downgrades to a comment-only anchor, in span
 * order. See the Rust lib.rs for the full rationale:
 *   - whole span in one run, unclaimed → redline;
 *   - one run but lost the redline slot → self-contained comment anchor;
 *   - spans a run boundary → comment anchor split into an opens-only and a
 *     closes-only piece sharing one comment_id;
 *   - no containing run at all → dropped and counted in `skipped`.
 */
export function selectEdits(
  projection: Projection,
  diagnostics: readonly Diagnostic[],
  opts: ReviseOptions,
  ids: Ids,
): EditSelection {
  // The comment carries the rule's advice too, not just its complaint.
  const textFor = (d: Diagnostic) =>
    d.suggestion !== undefined && d.suggestion !== d.message
      ? `${d.message} — ${d.suggestion}`
      : d.message;
  const inBounds = (start: number, end: number) => start <= end && end <= projection.byteLen;

  const rewrites: Candidate[] = [];
  const annotations: Candidate[] = [];
  for (const d of diagnostics) {
    const usableFix =
      d.fix &&
      (d.fix.applicability === ("machine-applicable" as Applicability) ||
        (d.fix.applicability === ("maybe-incorrect" as Applicability) && opts.includeAiRewrites))
        ? d.fix
        : undefined;
    if (usableFix) {
      const isAi = usableFix.applicability === ("maybe-incorrect" as Applicability);
      for (const e of usableFix.edits) {
        if (inBounds(e.range.start, e.range.end)) {
          rewrites.push({
            start: e.range.start,
            end: e.range.end,
            replacement: e.replacement,
            message: textFor(d),
            isAi,
          });
        }
      }
    } else if (
      opts.annotateFindings &&
      inBounds(d.span.start, d.span.end) &&
      d.span.start < d.span.end
    ) {
      annotations.push({
        start: d.span.start,
        end: d.span.end,
        replacement: null,
        message: textFor(d),
        isAi: false,
      });
    }
  }
  // Rewrites resolve first so they get first claim on the redline slot.
  const cmp = (a: Candidate, b: Candidate) => a.start - b.start || a.end - b.end;
  rewrites.sort(cmp);
  annotations.sort(cmp);
  const candidates = [...rewrites, ...annotations];

  const editsByOrdinal = new Map<number, RunEdit[]>();
  const pushEdit = (ordinal: number, e: RunEdit) => {
    const list = editsByOrdinal.get(ordinal);
    if (list) list.push(e);
    else editsByOrdinal.set(ordinal, [e]);
  };

  let skipped = 0;
  let aiRedlines = 0;
  let redlineGuard = 0;

  for (const { start, end, replacement, message, isAi } of candidates) {
    const runs = findRuns(projection, start, end);
    if (!runs) {
      skipped++;
      continue;
    }
    const [startRun, endRun] = runs;
    const sameRun = startRun.ordinal === endRun.ordinal;
    const wantsRedline = replacement !== null && sameRun && start >= redlineGuard;

    const commentId = ids.nextComment++;

    if (wantsRedline) {
      redlineGuard = end;
      if (isAi) aiRedlines++;
      pushEdit(startRun.ordinal, {
        start: start - startRun.start,
        end: end - startRun.start,
        replacement,
        message,
        commentId,
        opensHere: true,
        closesHere: true,
      });
    } else if (sameRun) {
      pushEdit(startRun.ordinal, {
        start: start - startRun.start,
        end: end - startRun.start,
        replacement: null,
        message,
        commentId,
        opensHere: true,
        closesHere: true,
      });
    } else {
      // Split anchor: one comment_id for both halves.
      pushEdit(startRun.ordinal, {
        start: start - startRun.start,
        end: startRun.end - startRun.start,
        replacement: null,
        message,
        commentId,
        opensHere: true,
        closesHere: false,
      });
      pushEdit(endRun.ordinal, {
        start: 0,
        end: end - endRun.start,
        replacement: null,
        message,
        commentId,
        opensHere: false,
        closesHere: true,
      });
    }
  }

  return { editsByOrdinal, skipped, aiRedlines };
}

/**
 * Resolve a `[start, end)` span to the run containing its start and the run
 * containing its end. `start == end` uses the combined inclusive predicate
 * against a single run (a zero-width span on a run boundary would otherwise
 * be missed).
 */
function findRuns(
  projection: Projection,
  start: number,
  end: number,
): [RunText, RunText] | undefined {
  if (start === end) {
    const r = projection.runs.find((r) => start >= r.start && end <= r.end);
    return r ? [r, r] : undefined;
  }
  const startRun = projection.runs.find((r) => r.start <= start && start < r.end);
  if (!startRun) return undefined;
  const endRun = projection.runs.find((r) => r.start < end && end <= r.end);
  if (!endRun) return undefined;
  return [startRun, endRun];
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
  const inits = author
    .split(/\s+/)
    .filter((w) => w.length > 0)
    .map((w) => w[0])
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
    return existing.replace("</w:comments>", `${blocks}</w:comments>`);
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
    if (id !== undefined) max = Math.max(max, Number.parseInt(id, 10) || 0);
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
 * Declare the comments part in `[Content_Types].xml` and relate it from
 * `document.xml.rels` if not already present.
 */
function wireCommentsPart(pkg: Package): void {
  const ct = pkg.getStr(CONTENT_TYPES);
  if (ct === undefined) throw new DocxError("missing_part", CONTENT_TYPES);
  if (!ct.includes("/word/comments.xml")) {
    const overrideEl = `<Override PartName="/word/comments.xml" ContentType="${COMMENTS_CT}"/>`;
    pkg.set(CONTENT_TYPES, strToBytes(ct.replace("</Types>", `${overrideEl}</Types>`)));
  }

  const rels = pkg.getStr(DOC_RELS);
  if (rels === undefined) throw new DocxError("missing_part", DOC_RELS);
  if (!rels.includes(COMMENTS_REL)) {
    const rid = nextRelId(rels);
    const rel = `<Relationship Id="${rid}" Type="${COMMENTS_REL}" Target="comments.xml"/>`;
    pkg.set(DOC_RELS, strToBytes(rels.replace("</Relationships>", `${rel}</Relationships>`)));
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
