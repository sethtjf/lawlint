// Rewrite `word/document.xml`, turning each fix into a Word revision
// (`<w:del>` + `<w:ins>`) wrapped in a comment range, or — when a rewrite
// isn't safe — a comment-only anchor that leaves the text untouched.
// Everything else is streamed through byte-for-byte.
//
// Scope: a fix is applied as a redline only when its whole span sits inside
// a single "simple" run (optional `<w:rPr>` + exactly one `<w:t>`) that is
// not already inside an existing revision; redlines never cross a run
// boundary and never overlap another redline. Comment-only anchors have no
// such restriction: `<w:commentRangeStart>`/`<w:commentRangeEnd>` are
// independent point markers.
//
// All `start`/`end` offsets are UTF-8 byte offsets into the run's text.
//
// Emission is two passes:
//   1. `classify` walks the document read-only and decides, for every run
//      that has edits, whether it is simple enough to touch; pieces on an
//      ineligible run have their `comment_id` voided (both halves of a
//      split comment share the id, so they drop together and can never
//      leave an orphaned marker).
//   2. The read+write pass streams the document and, for each run with
//      surviving pieces, does a boundary-event sweep over sorted cut
//      points: markers at each cut point, then either a kept run or (inside
//      an active redline) a `<w:del>` piece per segment.

import { DocxError } from "./index";
import {
  type XmlEvent,
  escapeAttr,
  escapeText,
  local,
  resolveCharRef,
  resolvePredefinedEntity,
  tokenize,
} from "./xml";

/**
 * A fix (or comment-only anchor) resolved to a run: `start`/`end` are byte
 * offsets *within that run's text*, and `message` becomes the comment body.
 */
export interface RunEdit {
  /** Meaningful when `opensHere`; otherwise callers set it to 0 (unused). */
  start: number;
  /** Meaningful when `closesHere`; else the run's full text byte length. */
  end: number;
  /** `string`: a redline. `null`: comment-only anchor. */
  replacement: string | null;
  message: string;
  /** Shared by both pieces of a split comment so they pair and void together. */
  commentId: number;
  opensHere: boolean;
  closesHere: boolean;
}

export interface CommentOut {
  id: number;
  author: string;
  date: string | null;
  text: string;
}

export interface RevisionOutput {
  documentXml: Uint8Array;
  comments: CommentOut[];
  /** Of `comments`, how many are comment-only anchors. */
  annotated: number;
  /** Findings that could not be marked at all, counted per finding. */
  skipped: number;
}

interface WtInfo {
  /** The run text as UTF-8 bytes; all edit offsets index into this. */
  text: Uint8Array;
}

interface RunInfo {
  /** Buffered `<w:rPr>…</w:rPr>` events (raw slices), or an empty element. */
  rpr: XmlEvent[];
  wts: WtInfo[];
  otherChildren: number;
}

const encoder = new TextEncoder();
const fatal = new TextDecoder("utf-8", { fatal: true });

/**
 * Decode a byte range of run text. Mirrors Rust's `&text[o..next]` slicing,
 * which panics on a non-char boundary — a byte offset that lands inside a
 * multi-byte char is a bug, and we throw rather than emit mojibake.
 */
function decodeRange(bytes: Uint8Array, start: number, end: number): string {
  return fatal.decode(bytes.subarray(start, end));
}

/**
 * Break a buffered `<w:r>…</w:r>` into its `<w:rPr>`, its `<w:t>` texts, and
 * a count of any other direct children.
 */
function analyzeRun(buffer: XmlEvent[]): RunInfo {
  const rpr: XmlEvent[] = [];
  const wts: WtInfo[] = [];
  let otherChildren = 0;

  // Walk only direct children of the run (buffer[0] is <w:r>, last is </w:r>).
  const inner = buffer.slice(1, Math.max(buffer.length - 1, 1));
  let i = 0;
  while (i < inner.length) {
    const ev = inner[i];
    if (ev.kind === "start") {
      // Find the matching End for this child subtree.
      let j = i + 1;
      let d = 1;
      while (j < inner.length && d > 0) {
        const k = inner[j].kind;
        if (k === "start") d++;
        else if (k === "end") d--;
        if (d === 0) break;
        j++;
      }
      switch (local(ev.name)) {
        case "rPr":
          rpr.length = 0;
          for (let k = i; k <= j; k++) rpr.push(inner[k]);
          break;
        case "t": {
          let text = "";
          for (let k = i + 1; k < j; k++) {
            const child = inner[k];
            if (child.kind === "text") {
              text += child.raw;
            } else if (child.kind === "generalRef") {
              const ch = resolveCharRef(child.name);
              if (ch !== undefined) {
                text += ch;
              } else {
                const value = resolvePredefinedEntity(child.name);
                if (value !== undefined) text += value;
              }
            }
          }
          wts.push({ text: encoder.encode(text) });
          break;
        }
        default:
          otherChildren++;
      }
      i = j + 1;
      continue;
    }
    if (ev.kind === "empty") {
      switch (local(ev.name)) {
        case "rPr":
          rpr.length = 0;
          rpr.push(ev);
          break;
        case "t":
          wts.push({ text: new Uint8Array(0) });
          break;
        default:
          otherChildren++;
      }
    }
    // Text, refs, comments and other non-element content at depth 0 inside
    // the run is ignored for shape analysis (as in Rust) but still passes
    // through verbatim.
    i++;
  }

  return { rpr, wts, otherChildren };
}

/** A run is safe to touch when it isn't inside a revision and has exactly
 * one `<w:t>` and no other direct children. */
function isSimpleRun(runInRevision: boolean, info: RunInfo): boolean {
  return !runInRevision && info.wts.length === 1 && info.otherChildren === 0;
}

// -- Generated markup (quick-xml serialization semantics) ------------------

function emitRpr(out: string[], rpr: XmlEvent[]): void {
  for (const ev of rpr) out.push(ev.raw);
}

function textEl(tag: string, text: string): string {
  return `<${tag} xml:space="preserve">${escapeText(text)}</${tag}>`;
}

function keepRun(rpr: XmlEvent[], text: string): string {
  const parts = ["<w:r>"];
  emitRpr(parts, rpr);
  parts.push(textEl("w:t", text), "</w:r>");
  return parts.join("");
}

function revision(
  wrap: string, // "w:ins" | "w:del"
  runTextTag: string,
  rpr: XmlEvent[],
  text: string,
  id: number,
  author: string,
  date: string | null,
): string {
  let start = `<${wrap} w:id="${id}" w:author="${escapeAttr(author)}"`;
  if (date !== null) start += ` w:date="${escapeAttr(date)}"`;
  start += ">";
  const parts = [start, "<w:r>"];
  emitRpr(parts, rpr);
  parts.push(textEl(runTextTag, text), "</w:r>", `</${wrap}>`);
  return parts.join("");
}

function commentMarker(tag: string, id: number): string {
  return `<${tag} w:id="${id}"/>`;
}

function commentReferenceRun(rpr: XmlEvent[], id: number): string {
  const parts = ["<w:r>"];
  emitRpr(parts, rpr);
  parts.push(`<w:commentReference w:id="${id}"/>`, "</w:r>");
  return parts.join("");
}

export interface Ids {
  nextRev: number;
  nextComment: number;
}

interface Ctx {
  author: string;
  date: string | null;
  ids: Ids;
  comments: CommentOut[];
  annotated: number;
  dropped: Set<number>;
}

/**
 * Pass 1: decide, for every run that has at least one candidate `RunEdit`,
 * whether that run is eligible to be touched. Returns the set of voided
 * `comment_id`s — every piece carrying one is dropped in pass 2, so a
 * multi-run comment's halves are always voided together.
 */
function classify(documentXml: string, editsByOrdinal: Map<number, RunEdit[]>): Set<number> {
  const events = tokenize(documentXml);

  const voided = new Set<number>();
  let wtCounter = 0;
  let revisionDepth = 0;

  let buffering = false;
  let buffer: XmlEvent[] = [];
  let runInRevision = false;

  for (const ev of events) {
    switch (ev.kind) {
      case "start":
        if (buffering) {
          buffer.push(ev);
          continue;
        }
        switch (local(ev.name)) {
          case "r":
            buffering = true;
            runInRevision = revisionDepth > 0;
            buffer = [ev];
            break;
          case "ins":
          case "del":
            revisionDepth++;
            break;
        }
        break;
      case "end":
        if (buffering) {
          buffer.push(ev);
          if (local(ev.name) === "r") {
            const info = analyzeRun(buffer);
            const baseOrdinal = wtCounter;
            wtCounter += info.wts.length;
            if (!isSimpleRun(runInRevision, info)) {
              for (let k = 0; k < info.wts.length; k++) {
                const es = editsByOrdinal.get(baseOrdinal + k);
                if (es) for (const piece of es) voided.add(piece.commentId);
              }
            }
            buffering = false;
          }
          continue;
        }
        if (local(ev.name) === "ins" || local(ev.name) === "del") revisionDepth--;
        break;
      default:
        if (buffering) buffer.push(ev);
    }
  }

  return voided;
}

export function apply(
  documentXml: string,
  editsByOrdinal: Map<number, RunEdit[]>,
  author: string,
  date: string | null,
  ids: Ids,
): RevisionOutput {
  const voidedIds = classify(documentXml, editsByOrdinal);

  const events = tokenize(documentXml);
  const out: string[] = [];

  const ctx: Ctx = {
    author,
    date,
    ids,
    comments: [],
    annotated: 0,
    dropped: new Set(),
  };

  const wt = { v: 0 };
  let buffering = false;
  let buffer: XmlEvent[] = [];

  for (const ev of events) {
    switch (ev.kind) {
      case "start":
        if (buffering) {
          buffer.push(ev);
          continue;
        }
        if (local(ev.name) === "r") {
          buffering = true;
          buffer = [ev];
        } else {
          out.push(ev.raw);
        }
        break;
      case "end":
        if (buffering) {
          buffer.push(ev);
          if (local(ev.name) === "r") {
            finishRun(buffer, editsByOrdinal, voidedIds, ctx, out, wt);
            buffering = false;
          }
          continue;
        }
        out.push(ev.raw);
        break;
      default:
        if (buffering) buffer.push(ev);
        else out.push(ev.raw);
    }
  }

  return {
    documentXml: encoder.encode(out.join("")),
    comments: ctx.comments,
    annotated: ctx.annotated,
    skipped: ctx.dropped.size,
  };
}

/**
 * Write this piece's opening marker. For a redline this also arms
 * `activeRedline` so the segment walk treats the text as deleted.
 */
function openPiece(out: string[], e: RunEdit, state: { activeRedline: RunEdit | null }): void {
  out.push(commentMarker("w:commentRangeStart", e.commentId));
  if (e.replacement !== null) state.activeRedline = e;
}

/**
 * Write this piece's closing marker + reference and record its comment.
 * For a redline this first flushes the (single) `<w:ins>`, then disarms
 * `activeRedline` — matching the ordering Start, del(s), ins, End, reference.
 */
function closePiece(
  out: string[],
  e: RunEdit,
  rpr: XmlEvent[],
  ctx: Ctx,
  state: { activeRedline: RunEdit | null },
): void {
  if (e.replacement !== null) {
    if (e.replacement !== "") {
      const rid = ctx.ids.nextRev++;
      out.push(revision("w:ins", "w:t", rpr, e.replacement, rid, ctx.author, ctx.date));
    }
    state.activeRedline = null;
  } else {
    ctx.annotated++;
  }
  out.push(commentMarker("w:commentRangeEnd", e.commentId));
  out.push(commentReferenceRun(rpr, e.commentId));
  ctx.comments.push({
    id: e.commentId,
    author: ctx.author,
    date: ctx.date,
    text: e.message,
  });
}

function finishRun(
  buffer: XmlEvent[],
  editsByOrdinal: Map<number, RunEdit[]>,
  voidedIds: Set<number>,
  ctx: Ctx,
  out: string[],
  wt: { v: number },
): void {
  const info = analyzeRun(buffer);
  const baseOrdinal = wt.v;
  wt.v += info.wts.length;

  // Which of this run's <w:t> ordinals have edits?
  const runEdits: RunEdit[] = [];
  for (let k = 0; k < info.wts.length; k++) {
    const es = editsByOrdinal.get(baseOrdinal + k);
    if (es) runEdits.push(...es);
  }

  const verbatim = () => {
    for (const ev of buffer) out.push(ev.raw);
  };

  if (runEdits.length === 0) {
    verbatim();
    return;
  }

  const textLen = info.wts[0]?.text.length ?? 0;

  // Filter: ids voided by classify(), out-of-bounds pieces, pieces that
  // are neither open nor close, and redlines that aren't self-contained.
  const selected: RunEdit[] = [];
  for (const e of runEdits) {
    if (voidedIds.has(e.commentId)) {
      ctx.dropped.add(e.commentId);
      continue;
    }
    if (e.start > e.end || e.end > textLen) {
      ctx.dropped.add(e.commentId);
      continue;
    }
    if (!e.opensHere && !e.closesHere) {
      ctx.dropped.add(e.commentId);
      continue;
    }
    if (e.replacement !== null && !(e.opensHere && e.closesHere)) {
      ctx.dropped.add(e.commentId);
      continue;
    }
    selected.push(e);
  }
  if (selected.length === 0) {
    verbatim();
    return;
  }

  // Redlines must never overlap each other within a run — a defensive
  // backstop restricted to redline pieces.
  selected.sort((a, b) => a.start - b.start || a.end - b.end || a.commentId - b.commentId);
  let redlineGuard = 0;
  const finalPieces: RunEdit[] = [];
  for (const e of selected) {
    if (e.replacement !== null) {
      if (e.start < redlineGuard) {
        ctx.dropped.add(e.commentId);
        continue;
      }
      redlineGuard = e.end;
    }
    finalPieces.push(e);
  }
  if (finalPieces.length === 0) {
    verbatim();
    return;
  }

  const text = info.wts[0].text;
  const rpr = info.rpr;

  // Boundary-event sweep: cut points are every offset at which some piece
  // opens or closes, plus the run's own start/end.
  const cuts = [0, text.length];
  for (const e of finalPieces) {
    if (e.opensHere) cuts.push(e.start);
    if (e.closesHere) cuts.push(e.end);
  }
  cuts.sort((a, b) => a - b);
  const uniqueCuts = cuts.filter((v, i) => i === 0 || v !== cuts[i - 1]);

  const state: { activeRedline: RunEdit | null } = { activeRedline: null };

  for (let i = 0; i < uniqueCuts.length; i++) {
    const o = uniqueCuts[i];

    // Non-zero-width closes at this offset.
    for (const e of finalPieces) {
      if (e.closesHere && e.end === o && !(e.opensHere && e.start === o)) {
        closePiece(out, e, rpr, ctx, state);
      }
    }

    // Zero-width pieces (open == close, e.g. a pure insertion).
    for (const e of finalPieces) {
      if (e.opensHere && e.closesHere && e.start === o && e.end === o) {
        openPiece(out, e, state);
        closePiece(out, e, rpr, ctx, state);
      }
    }

    // Opens at this offset (excluding the zero-width ones just handled).
    for (const e of finalPieces) {
      if (e.opensHere && e.start === o && !(e.closesHere && e.end === o)) {
        openPiece(out, e, state);
      }
    }

    if (i + 1 < uniqueCuts.length) {
      const nextO = uniqueCuts[i + 1];
      if (nextO > o) {
        const seg = decodeRange(text, o, nextO);
        if (state.activeRedline !== null) {
          const rid = ctx.ids.nextRev++;
          out.push(revision("w:del", "w:delText", rpr, seg, rid, ctx.author, ctx.date));
        } else {
          out.push(keepRun(rpr, seg));
        }
      }
    }
  }
}
