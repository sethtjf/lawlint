// Resolve each fix's edit to the run(s) it lands in and decide whether it
// becomes a full redline or downgrades to a comment-only anchor.

import type { Applicability, Diagnostic } from "@lawlint/core";
import type { ReviseOptions } from "./index";
import type { Projection, RunText } from "./project";
import type { Ids, RunEdit } from "./revise";

export interface EditSelection {
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
 * In span order:
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
export function findRuns(
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
