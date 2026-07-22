//! Rewrite `word/document.xml`, turning each fix into a Word revision
//! (`<w:del>` + `<w:ins>`) wrapped in a comment range that carries the rule
//! message, or — when a full rewrite isn't safe to apply — a comment-only
//! anchor that leaves the text untouched. Everything else in the document is
//! streamed through byte-for-byte (modulo XML-equivalent escaping), so no
//! formatting is lost.
//!
//! Scope: a fix is applied as a redline only when its whole span sits inside
//! a single "simple" run (optional `<w:rPr>` + exactly one `<w:t>`) that is
//! not already inside an existing revision; redlines never cross a run
//! boundary and never overlap another redline (both are real content
//! conflicts, not just an XML limitation). Comment-only anchors have no such
//! restriction: `<w:commentRangeStart>`/`<w:commentRangeEnd>` are
//! independent point markers (paragraph-level siblings of runs, not nested
//! inside them), so OOXML allows them to nest, cross, and straddle run
//! boundaries — including landing inside another anchor's own redline,
//! which splits that redline's `<w:del>` into multiple pieces so the
//! marker can sit at the right textual position.
//!
//! `RunEdit` pieces come in three shapes, distinguished by `opens_here` /
//! `closes_here`:
//!   - self-contained (`opens_here && closes_here`): the whole anchor lives
//!     in one run. Every redline is this shape.
//!   - opens-only: the first half of a comment-only anchor that continues
//!     into a later run.
//!   - closes-only: the second half, in the run where it ends.
//!
//! A run strictly between the two halves needs no `RunEdit` at all — with no
//! entry in `edits_by_ordinal` it takes the empty-edits fast path below and
//! streams through untouched, whatever its own shape.
//!
//! Emission is a two-pass process:
//!   1. `classify` walks the whole document (read-only) and decides, for
//!      every run that has edits, whether it is "simple" enough to touch.
//!      Any `RunEdit` whose run isn't simple has its `comment_id` voided.
//!      This has to happen *before* any output is written: a multi-run
//!      comment's `commentRangeStart` is written into the (logically
//!      append-only) output stream before its `commentRangeEnd` run is even
//!      reached, so if that later run turns out unusable we cannot erase the
//!      already-written start marker. Voiding by shared `comment_id` — both
//!      halves carry the same id — guarantees both halves are dropped
//!      together, so an orphaned marker can never be emitted. Do not
//!      reintroduce a purely local "is this run simple" re-check in
//!      `finish_run` in place of consulting `voided_ids`: that would
//!      silently reintroduce this orphan risk.
//!   2. The read+write pass streams the document, and for each run whose
//!      surviving (non-voided) pieces are non-empty, replaces the old
//!      single-cursor walk with a boundary-event sweep: collect every
//!      piece's relevant open/close offsets into a sorted set of cut
//!      points, and walk the segments between them, emitting markers at
//!      each cut point and either a kept run or (inside an active redline)
//!      a `<w:del>` piece for each segment.

use std::collections::{HashMap, HashSet};

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::name::QName;
use quick_xml::{Reader, Writer};

use crate::DocxError;

/// A fix (or comment-only anchor) resolved to a run: `start`/`end` are byte
/// offsets *within that run's text*, and `message` becomes the comment body.
#[derive(Debug, Clone)]
pub struct RunEdit {
    /// Meaningful when `opens_here`; otherwise callers set it to 0 (unused).
    pub start: usize,
    /// Meaningful when `closes_here`; otherwise callers set it to this
    /// run's full text length (unused, but keeps bounds checks trivially
    /// satisfied for the rest-of-run tail an opens-only piece implies).
    pub end: usize,
    /// `Some(text)`: a redline — the span is deleted and `text` is
    /// inserted. `None`: a comment-only anchor — the span's text is left
    /// untouched, only wrapped in a comment range. Redlines are always
    /// self-contained within one run; only comment-only anchors may be
    /// split across a run boundary (enforced defensively below, decided by
    /// lib.rs).
    pub replacement: Option<String>,
    pub message: String,
    /// Shared by both pieces of a comment split across a run boundary, so
    /// the emitter can pair a `commentRangeStart` written in one run with
    /// the `commentRangeEnd` + reference written in another. Assigned by
    /// the caller (lib.rs) once per finding, never once per piece — two
    /// pieces of the same finding must carry the same id or the halves
    /// can't be paired (and `classify` can't void them together).
    pub comment_id: usize,
    /// This piece contains the anchor's start boundary: emit
    /// `commentRangeStart` (and, for a redline, start deleting) at `start`.
    pub opens_here: bool,
    /// This piece contains the anchor's end boundary: emit
    /// `commentRangeEnd` + the reference run (and, for a redline, the
    /// `<w:ins>`) at `end`.
    pub closes_here: bool,
}

#[derive(Debug, Clone)]
pub struct CommentOut {
    pub id: usize,
    pub author: String,
    pub date: Option<String>,
    pub text: String,
}

pub struct RevisionOutput {
    pub document_xml: Vec<u8>,
    pub comments: Vec<CommentOut>,
    /// Of `comments`, how many are comment-only anchors (`replacement:
    /// None`) rather than the comment attached to an applied redline.
    /// `comments.len() - annotated` is the applied-redline count.
    pub annotated: usize,
    /// Findings that could not be marked at all (run-shape ineligibility,
    /// defensive overlap, or bounds). Counted per finding, not per emitted
    /// piece, so a dropped multi-run comment counts once.
    pub skipped: usize,
}

struct WtInfo {
    text: String,
}

struct RunInfo {
    rpr: Vec<Event<'static>>,
    wts: Vec<WtInfo>,
    other_children: usize,
}

fn local(name: QName<'_>) -> Vec<u8> {
    name.local_name().as_ref().to_vec()
}

/// Break a buffered `<w:r>…</w:r>` into its `<w:rPr>`, its `<w:t>` texts, and a
/// count of any other direct children (drawings, tabs, breaks, field codes…).
fn analyze_run(buffer: &[Event<'static>]) -> RunInfo {
    let mut rpr = Vec::new();
    let mut wts = Vec::new();
    let mut other_children = 0usize;

    // Walk only direct children of the run (buffer[0] is <w:r>, last is </w:r>).
    let inner = &buffer[1..buffer.len().saturating_sub(1)];
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < inner.len() {
        match &inner[i] {
            Event::Start(e) if depth == 0 => {
                let name = local(e.name());
                // Find the matching End for this child subtree.
                let mut j = i + 1;
                let mut d = 1i32;
                while j < inner.len() && d > 0 {
                    match &inner[j] {
                        Event::Start(_) => d += 1,
                        Event::End(_) => d -= 1,
                        _ => {}
                    }
                    if d == 0 {
                        break;
                    }
                    j += 1;
                }
                match name.as_slice() {
                    b"rPr" => {
                        rpr = inner[i..=j]
                            .iter()
                            .map(|e| e.clone().into_owned())
                            .collect()
                    }
                    b"t" => {
                        let mut text = String::new();
                        for ev in &inner[i + 1..j] {
                            if let Event::Text(t) = ev {
                                if let Ok(s) = t.unescape() {
                                    text.push_str(&s);
                                }
                            }
                        }
                        wts.push(WtInfo { text });
                    }
                    _ => other_children += 1,
                }
                i = j + 1;
                continue;
            }
            Event::Empty(e) if depth == 0 => match local(e.name()).as_slice() {
                b"rPr" => rpr = vec![inner[i].clone().into_owned()],
                b"t" => wts.push(WtInfo {
                    text: String::new(),
                }),
                _ => other_children += 1,
            },
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            _ => {}
        }
        i += 1;
    }

    RunInfo {
        rpr,
        wts,
        other_children,
    }
}

/// A run is safe to touch when it's not already inside a revision and has
/// exactly one `<w:t>` and no other direct children. Used both by `classify`
/// (to decide which `comment_id`s to void) and conceptually mirrors the old
/// per-run gate — the difference is that eligibility now lives entirely in
/// `classify`, run once before any output is written, rather than being
/// re-derived locally while emitting (see the module doc for why).
fn is_simple_run(run_in_revision: bool, info: &RunInfo) -> bool {
    !run_in_revision && info.wts.len() == 1 && info.other_children == 0
}

struct Emitter<'w> {
    writer: &'w mut Writer<Vec<u8>>,
}

impl Emitter<'_> {
    fn rpr(&mut self, rpr: &[Event<'static>]) -> Result<(), DocxError> {
        for ev in rpr {
            self.writer.write_event(ev.clone())?;
        }
        Ok(())
    }

    fn text_el(&mut self, tag: &str, text: &str) -> Result<(), DocxError> {
        let mut start = BytesStart::new(tag);
        start.push_attribute(("xml:space", "preserve"));
        self.writer.write_event(Event::Start(start))?;
        self.writer.write_event(Event::Text(BytesText::new(text)))?;
        self.writer.write_event(Event::End(BytesEnd::new(tag)))?;
        Ok(())
    }

    /// A plain (unchanged) run carrying `text`.
    fn keep_run(&mut self, rpr: &[Event<'static>], text: &str) -> Result<(), DocxError> {
        self.writer
            .write_event(Event::Start(BytesStart::new("w:r")))?;
        self.rpr(rpr)?;
        self.text_el("w:t", text)?;
        self.writer.write_event(Event::End(BytesEnd::new("w:r")))?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn revision(
        &mut self,
        wrap: &str, // "w:ins" | "w:del"
        run_text_tag: &str,
        rpr: &[Event<'static>],
        text: &str,
        id: usize,
        author: &str,
        date: Option<&str>,
    ) -> Result<(), DocxError> {
        let mut start = BytesStart::new(wrap);
        start.push_attribute(("w:id", id.to_string().as_str()));
        start.push_attribute(("w:author", author));
        if let Some(date) = date {
            start.push_attribute(("w:date", date));
        }
        self.writer.write_event(Event::Start(start))?;
        self.writer
            .write_event(Event::Start(BytesStart::new("w:r")))?;
        self.rpr(rpr)?;
        self.text_el(run_text_tag, text)?;
        self.writer.write_event(Event::End(BytesEnd::new("w:r")))?;
        self.writer.write_event(Event::End(BytesEnd::new(wrap)))?;
        Ok(())
    }

    fn comment_marker(&mut self, tag: &str, id: usize) -> Result<(), DocxError> {
        let mut el = BytesStart::new(tag);
        el.push_attribute(("w:id", id.to_string().as_str()));
        self.writer.write_event(Event::Empty(el))?;
        Ok(())
    }

    fn comment_reference_run(
        &mut self,
        rpr: &[Event<'static>],
        id: usize,
    ) -> Result<(), DocxError> {
        self.writer
            .write_event(Event::Start(BytesStart::new("w:r")))?;
        self.rpr(rpr)?;
        let mut el = BytesStart::new("w:commentReference");
        el.push_attribute(("w:id", id.to_string().as_str()));
        self.writer.write_event(Event::Empty(el))?;
        self.writer.write_event(Event::End(BytesEnd::new("w:r")))?;
        Ok(())
    }
}

pub struct Ids {
    pub next_rev: usize,
    pub next_comment: usize,
}

/// Pass 1: decide, for every run that has at least one candidate `RunEdit`,
/// whether that run is eligible to be touched. Returns the set of
/// `comment_id`s that touch an ineligible run — every piece carrying one of
/// these ids must be dropped in pass 2, regardless of which run it's in,
/// so a multi-run comment's two halves are always voided together (see the
/// module doc: this is the whole point of doing this as a separate pass
/// before any writing starts).
fn classify(
    document_xml: &str,
    edits_by_ordinal: &HashMap<usize, Vec<RunEdit>>,
) -> Result<HashSet<usize>, DocxError> {
    let mut reader = Reader::from_str(document_xml);
    reader.config_mut().trim_text(false);

    let mut voided = HashSet::new();
    let mut wt_counter = 0usize;
    let mut revision_depth = 0i32;

    let mut buffering = false;
    let mut buffer: Vec<Event<'static>> = Vec::new();
    let mut run_in_revision = false;

    loop {
        let ev = reader
            .read_event()
            .map_err(|e| DocxError::Malformed(e.to_string()))?;
        match ev {
            Event::Eof => break,
            Event::Start(e) => {
                if buffering {
                    buffer.push(Event::Start(e).into_owned());
                    continue;
                }
                match local(e.name()).as_slice() {
                    b"r" => {
                        buffering = true;
                        run_in_revision = revision_depth > 0;
                        buffer.clear();
                        buffer.push(Event::Start(e).into_owned());
                    }
                    b"ins" | b"del" => revision_depth += 1,
                    _ => {}
                }
            }
            Event::End(e) => {
                if buffering {
                    if local(e.name()).as_slice() == b"r" {
                        buffer.push(Event::End(e).into_owned());
                        let info = analyze_run(&buffer);
                        let base_ordinal = wt_counter;
                        wt_counter += info.wts.len();
                        if !is_simple_run(run_in_revision, &info) {
                            for k in 0..info.wts.len() {
                                if let Some(es) = edits_by_ordinal.get(&(base_ordinal + k)) {
                                    for piece in es {
                                        voided.insert(piece.comment_id);
                                    }
                                }
                            }
                        }
                        buffering = false;
                    } else {
                        buffer.push(Event::End(e).into_owned());
                    }
                    continue;
                }
                if local(e.name()).as_slice() == b"ins" || local(e.name()).as_slice() == b"del" {
                    revision_depth -= 1;
                }
            }
            other => {
                if buffering {
                    buffer.push(other.into_owned());
                }
            }
        }
    }

    Ok(voided)
}

pub fn apply(
    document_xml: &str,
    edits_by_ordinal: &HashMap<usize, Vec<RunEdit>>,
    author: &str,
    date: Option<&str>,
    ids: &mut Ids,
) -> Result<RevisionOutput, DocxError> {
    let voided_ids = classify(document_xml, edits_by_ordinal)?;

    let mut reader = Reader::from_str(document_xml);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::new());

    let mut comments: Vec<CommentOut> = Vec::new();
    let mut annotated = 0usize;
    // Findings whose anchor could not be written, keyed by comment id. A
    // multi-run comment is two pieces but one finding; counting pieces made a
    // single dropped anchor report as two, and these counters are user-visible.
    let mut dropped: std::collections::HashSet<usize> = std::collections::HashSet::new();

    let mut wt_counter = 0usize;

    // Run buffering state. Pass 2 no longer needs to track w:ins/w:del depth
    // itself — eligibility was already fully decided by `classify` above —
    // so ins/del Start/End events fall through to the generic passthrough
    // arms below like any other element.
    let mut buffering = false;
    let mut buffer: Vec<Event<'static>> = Vec::new();

    loop {
        let ev = reader
            .read_event()
            .map_err(|e| DocxError::Malformed(e.to_string()))?;
        match ev {
            Event::Eof => break,
            Event::Start(e) => {
                if buffering {
                    buffer.push(Event::Start(e).into_owned());
                    continue;
                }
                match local(e.name()).as_slice() {
                    b"r" => {
                        buffering = true;
                        buffer.clear();
                        buffer.push(Event::Start(e).into_owned());
                    }
                    _ => writer.write_event(Event::Start(e))?,
                }
            }
            Event::End(e) => {
                if buffering {
                    if local(e.name()).as_slice() == b"r" {
                        buffer.push(Event::End(e).into_owned());
                        let mut em = Emitter {
                            writer: &mut writer,
                        };
                        finish_run(
                            &buffer,
                            &mut wt_counter,
                            edits_by_ordinal,
                            &voided_ids,
                            author,
                            date,
                            ids,
                            &mut em,
                            &mut comments,
                            &mut annotated,
                            &mut dropped,
                        )?;
                        buffering = false;
                    } else {
                        buffer.push(Event::End(e).into_owned());
                    }
                    continue;
                }
                writer.write_event(Event::End(e))?;
            }
            other => {
                if buffering {
                    buffer.push(other.into_owned());
                } else {
                    writer.write_event(other)?;
                }
            }
        }
    }

    Ok(RevisionOutput {
        document_xml: writer.into_inner(),
        comments,
        annotated,
        skipped: dropped.len(),
    })
}

/// Write this piece's opening marker. For a redline this also arms
/// `active_redline` so the segment walk between here and its close treats
/// the text as deleted; the actual deletion text is emitted by the caller's
/// segment walk (in possibly several pieces, if another anchor's boundary
/// falls inside this redline's span), not here.
fn open_piece(
    em: &mut Emitter<'_>,
    e: &RunEdit,
    active_redline: &mut Option<RunEdit>,
) -> Result<(), DocxError> {
    em.comment_marker("w:commentRangeStart", e.comment_id)?;
    if e.replacement.is_some() {
        *active_redline = Some(e.clone());
    }
    Ok(())
}

/// Write this piece's closing marker + reference and record its comment.
/// For a redline this first flushes the (single) `<w:ins>`, then disarms
/// `active_redline` before writing the comment range's own end — matching
/// the original single-piece ordering (Start, del(s), ins, End, reference)
/// even when the del was split by an interior anchor.
#[allow(clippy::too_many_arguments)]
fn close_piece(
    em: &mut Emitter<'_>,
    e: &RunEdit,
    rpr: &[Event<'static>],
    author: &str,
    date: Option<&str>,
    ids: &mut Ids,
    comments: &mut Vec<CommentOut>,
    annotated: &mut usize,
    active_redline: &mut Option<RunEdit>,
) -> Result<(), DocxError> {
    if let Some(replacement) = &e.replacement {
        if !replacement.is_empty() {
            let rid = ids.next_rev;
            ids.next_rev += 1;
            em.revision("w:ins", "w:t", rpr, replacement, rid, author, date)?;
        }
        *active_redline = None;
    } else {
        *annotated += 1;
    }
    em.comment_marker("w:commentRangeEnd", e.comment_id)?;
    em.comment_reference_run(rpr, e.comment_id)?;
    comments.push(CommentOut {
        id: e.comment_id,
        author: author.to_string(),
        date: date.map(|d| d.to_string()),
        text: e.message.clone(),
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn finish_run(
    buffer: &[Event<'static>],
    wt_counter: &mut usize,
    edits_by_ordinal: &HashMap<usize, Vec<RunEdit>>,
    voided_ids: &HashSet<usize>,
    author: &str,
    date: Option<&str>,
    ids: &mut Ids,
    em: &mut Emitter<'_>,
    comments: &mut Vec<CommentOut>,
    annotated: &mut usize,
    dropped: &mut std::collections::HashSet<usize>,
) -> Result<(), DocxError> {
    let info = analyze_run(buffer);
    let base_ordinal = *wt_counter;
    *wt_counter += info.wts.len();

    // Which of this run's <w:t> ordinals have edits?
    let mut run_edits: Vec<RunEdit> = Vec::new();
    for k in 0..info.wts.len() {
        if let Some(es) = edits_by_ordinal.get(&(base_ordinal + k)) {
            run_edits.extend(es.iter().cloned());
        }
    }

    if run_edits.is_empty() {
        return verbatim(buffer, em);
    }

    let text_len = info.wts.first().map(|w| w.text.len()).unwrap_or(0);

    // Filter: ids voided by classify() (run-shape ineligibility, on either
    // end of a multi-run comment), out-of-bounds pieces, and any piece that
    // isn't self-contained despite being a redline (should never happen —
    // lib.rs only ever produces self-contained redlines — but this is the
    // load-bearing defensive check the segment-coverage sweep below relies
    // on: it assumes every redline's start/end are both meaningful).
    let mut selected: Vec<RunEdit> = Vec::new();
    for e in run_edits {
        if voided_ids.contains(&e.comment_id) {
            dropped.insert(e.comment_id);
            continue;
        }
        if e.start > e.end || e.end > text_len {
            dropped.insert(e.comment_id);
            continue;
        }
        if !e.opens_here && !e.closes_here {
            dropped.insert(e.comment_id);
            continue;
        }
        if e.replacement.is_some() && !(e.opens_here && e.closes_here) {
            dropped.insert(e.comment_id);
            continue;
        }
        selected.push(e);
    }
    if selected.is_empty() {
        return verbatim(buffer, em);
    }

    // Redlines must never overlap each other within a run — that's a real
    // content conflict (two proposed replacements of intersecting text),
    // not just an emitter limitation, so unlike comment overlap it is not
    // relaxed by this change. lib.rs is expected to already guarantee this;
    // this is a narrow defensive backstop restricted to redline pieces.
    selected.sort_by_key(|e| (e.start, e.end, e.comment_id));
    let mut redline_guard = 0usize;
    let mut final_pieces: Vec<RunEdit> = Vec::new();
    for e in selected {
        if e.replacement.is_some() {
            if e.start < redline_guard {
                dropped.insert(e.comment_id);
                continue;
            }
            redline_guard = e.end;
        }
        final_pieces.push(e);
    }
    if final_pieces.is_empty() {
        return verbatim(buffer, em);
    }

    let text = &info.wts[0].text;
    let rpr = &info.rpr;

    // Boundary-event sweep: cut points are every offset at which some piece
    // opens or closes, plus the run's own start/end. Walking segments
    // between consecutive cut points, rather than a single cursor, is what
    // lets two (or more) anchors overlap, nest, or interleave freely.
    let mut cuts: Vec<usize> = vec![0, text.len()];
    for e in &final_pieces {
        if e.opens_here {
            cuts.push(e.start);
        }
        if e.closes_here {
            cuts.push(e.end);
        }
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mut active_redline: Option<RunEdit> = None;

    for i in 0..cuts.len() {
        let o = cuts[i];

        // Non-zero-width closes at this offset (the common case: opened at
        // an earlier cut point).
        for e in final_pieces
            .iter()
            .filter(|e| e.closes_here && e.end == o && !(e.opens_here && e.start == o))
        {
            close_piece(
                em,
                e,
                rpr,
                author,
                date,
                ids,
                comments,
                annotated,
                &mut active_redline,
            )?;
        }

        // Zero-width pieces (open == close, e.g. a pure insertion): open
        // then immediately close at this same point, so no segment walk
        // ever sees them as "active" across real text.
        for e in final_pieces
            .iter()
            .filter(|e| e.opens_here && e.closes_here && e.start == o && e.end == o)
        {
            open_piece(em, e, &mut active_redline)?;
            close_piece(
                em,
                e,
                rpr,
                author,
                date,
                ids,
                comments,
                annotated,
                &mut active_redline,
            )?;
        }

        // Opens at this offset (excluding the zero-width ones just handled).
        for e in final_pieces
            .iter()
            .filter(|e| e.opens_here && e.start == o && !(e.closes_here && e.end == o))
        {
            open_piece(em, e, &mut active_redline)?;
        }

        if i + 1 < cuts.len() {
            let next_o = cuts[i + 1];
            let seg = &text[o..next_o];
            if !seg.is_empty() {
                if active_redline.is_some() {
                    let rid = ids.next_rev;
                    ids.next_rev += 1;
                    em.revision("w:del", "w:delText", rpr, seg, rid, author, date)?;
                } else {
                    em.keep_run(rpr, seg)?;
                }
            }
        }
    }

    Ok(())
}

fn verbatim(buffer: &[Event<'static>], em: &mut Emitter<'_>) -> Result<(), DocxError> {
    for ev in buffer {
        em.writer.write_event(ev.clone())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NS: &str = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>"#;

    fn run(inner: &str, edits: HashMap<usize, Vec<RunEdit>>) -> (String, RevisionOutput) {
        let xml = format!("{NS}{inner}</w:body></w:document>");
        let mut ids = Ids {
            next_rev: 100,
            next_comment: 1,
        };
        let out = apply(
            &xml,
            &edits,
            "lawlint",
            Some("2020-01-01T00:00:00Z"),
            &mut ids,
        )
        .unwrap();
        (String::from_utf8(out.document_xml.clone()).unwrap(), out)
    }

    /// A self-contained redline piece.
    fn edit(id: usize, start: usize, end: usize, replacement: &str) -> RunEdit {
        RunEdit {
            start,
            end,
            replacement: Some(replacement.to_string()),
            message: "rule msg".to_string(),
            comment_id: id,
            opens_here: true,
            closes_here: true,
        }
    }

    /// A self-contained comment-only anchor.
    fn annotation(id: usize, start: usize, end: usize, message: &str) -> RunEdit {
        RunEdit {
            start,
            end,
            replacement: None,
            message: message.to_string(),
            comment_id: id,
            opens_here: true,
            closes_here: true,
        }
    }

    /// The opening half of a comment-only anchor that continues into a
    /// later run. `end` should be this run's full text length.
    fn open_only(id: usize, start: usize, end_of_run: usize, message: &str) -> RunEdit {
        RunEdit {
            start,
            end: end_of_run,
            replacement: None,
            message: message.to_string(),
            comment_id: id,
            opens_here: true,
            closes_here: false,
        }
    }

    /// The closing half of a comment-only anchor that opened in an earlier
    /// run.
    fn close_only(id: usize, end: usize, message: &str) -> RunEdit {
        RunEdit {
            start: 0,
            end,
            replacement: None,
            message: message.to_string(),
            comment_id: id,
            opens_here: false,
            closes_here: true,
        }
    }

    #[test]
    fn simple_run_becomes_del_ins_and_comment() {
        // "Pursuant to X" -> replace "Pursuant to" (bytes 0..11) with "Under".
        let inner = r#"<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Pursuant to X</w:t></w:r></w:p>"#;
        let mut edits = HashMap::new();
        edits.insert(0, vec![edit(1, 0, 11, "Under")]);
        let (xml, out) = run(inner, edits);

        assert_eq!(out.skipped, 0);
        assert_eq!(out.annotated, 0);
        assert_eq!(out.comments.len(), 1);
        assert_eq!(out.comments[0].text, "rule msg");
        assert!(xml.contains(r#"<w:delText xml:space="preserve">Pursuant to</w:delText>"#));
        assert!(xml.contains(r#"<w:t xml:space="preserve">Under</w:t>"#));
        assert!(xml.contains("<w:commentRangeStart"));
        assert!(xml.contains("<w:commentReference"));
        // The untouched tail " X" is kept as a plain run.
        assert!(xml.contains(r#"<w:t xml:space="preserve"> X</w:t>"#));
        // rPr is carried onto the revision runs.
        assert!(xml.contains("<w:b/>"));
    }

    #[test]
    fn pure_insertion_emits_no_deltext() {
        // Empty span (start==end): insertion only.
        let inner = r#"<w:p><w:r><w:t>ab</w:t></w:r></w:p>"#;
        let mut edits = HashMap::new();
        edits.insert(0, vec![edit(1, 1, 1, "X")]);
        let (xml, out) = run(inner, edits);
        assert_eq!(out.comments.len(), 1);
        assert!(!xml.contains("w:delText"));
        assert!(xml.contains(r#"<w:t xml:space="preserve">X</w:t>"#));
    }

    #[test]
    fn multi_text_run_is_skipped_not_corrupted() {
        // A run with two <w:t> is not "simple"; the edit is skipped and the run
        // is emitted verbatim.
        let inner = r#"<w:p><w:r><w:t>foo</w:t><w:t>bar</w:t></w:r></w:p>"#;
        let mut edits = HashMap::new();
        edits.insert(0, vec![edit(1, 0, 3, "FOO")]);
        let (xml, out) = run(inner, edits);
        assert_eq!(out.skipped, 1);
        assert!(out.comments.is_empty());
        assert!(xml.contains("<w:t>foo</w:t><w:t>bar</w:t>"));
        assert!(!xml.contains("w:ins"));
    }

    #[test]
    fn edit_inside_existing_revision_is_skipped() {
        // ordinal 0 is inside <w:ins>; edits there are not applied.
        let inner = r#"<w:p><w:ins><w:r><w:t>added</w:t></w:r></w:ins></w:p>"#;
        let mut edits = HashMap::new();
        edits.insert(0, vec![edit(1, 0, 5, "gone")]);
        let (xml, out) = run(inner, edits);
        assert_eq!(out.skipped, 1);
        assert!(xml.contains("<w:t>added</w:t>"));
    }

    #[test]
    fn run_without_edits_is_untouched() {
        let inner = r#"<w:p><w:r><w:t>clean</w:t></w:r></w:p>"#;
        let (xml, out) = run(inner, HashMap::new());
        assert_eq!(out.skipped, 0);
        assert!(out.comments.is_empty());
        assert!(xml.contains("<w:t>clean</w:t>"));
    }

    #[test]
    fn annotation_only_edit_comments_without_changing_text() {
        let inner = r#"<w:p><w:r><w:t>Pursuant to X</w:t></w:r></w:p>"#;
        let mut edits = HashMap::new();
        edits.insert(0, vec![annotation(1, 0, 11, "consider rewording")]);
        let (xml, out) = run(inner, edits);

        assert_eq!(out.skipped, 0);
        assert_eq!(out.annotated, 1);
        assert_eq!(out.comments.len(), 1);
        assert_eq!(out.comments[0].text, "consider rewording");
        assert!(!xml.contains("w:delText"));
        assert!(!xml.contains("<w:ins"));
        assert!(xml.contains("<w:commentRangeStart"));
        assert!(xml.contains("<w:commentReference"));
        // The original text survives untouched, just split into kept-run
        // pieces around the anchor.
        assert!(xml.contains("Pursuant to"));
        assert!(xml.contains(" X"));
    }

    #[test]
    fn two_overlapping_comment_anchors_on_one_run() {
        // Crossing (not nested) spans: A=[0,5), B=[3,8).
        let inner = r#"<w:p><w:r><w:t>0123456789</w:t></w:r></w:p>"#;
        let mut edits = HashMap::new();
        edits.insert(
            0,
            vec![
                annotation(1, 0, 5, "first finding"),
                annotation(2, 3, 8, "second finding"),
            ],
        );
        let (xml, out) = run(inner, edits);

        assert_eq!(out.skipped, 0);
        assert_eq!(out.comments.len(), 2);
        assert_eq!(
            out.comments.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![1, 2]
        );

        // Interleaved order: Start(1), Start(2), End(1)+Ref(1), End(2)+Ref(2).
        let start1 = xml.find(r#"<w:commentRangeStart w:id="1"/>"#).unwrap();
        let start2 = xml.find(r#"<w:commentRangeStart w:id="2"/>"#).unwrap();
        let end1 = xml.find(r#"<w:commentRangeEnd w:id="1"/>"#).unwrap();
        let end2 = xml.find(r#"<w:commentRangeEnd w:id="2"/>"#).unwrap();
        assert!(start1 < start2, "Start(1) before Start(2)");
        assert!(
            start2 < end1,
            "Start(2) before End(1): true overlap, not nesting"
        );
        assert!(end1 < end2, "End(1) before End(2)");
    }

    #[test]
    fn nested_comment_anchor() {
        // A=[0,10) fully contains B=[3,7).
        let inner = r#"<w:p><w:r><w:t>0123456789</w:t></w:r></w:p>"#;
        let mut edits = HashMap::new();
        edits.insert(
            0,
            vec![annotation(1, 0, 10, "outer"), annotation(2, 3, 7, "inner")],
        );
        let (xml, out) = run(inner, edits);

        assert_eq!(out.skipped, 0);
        assert_eq!(out.comments.len(), 2);

        let start1 = xml.find(r#"<w:commentRangeStart w:id="1"/>"#).unwrap();
        let start2 = xml.find(r#"<w:commentRangeStart w:id="2"/>"#).unwrap();
        let end2 = xml.find(r#"<w:commentRangeEnd w:id="2"/>"#).unwrap();
        let end1 = xml.find(r#"<w:commentRangeEnd w:id="1"/>"#).unwrap();
        // Start(A), Start(B), End(B)+Ref(B), End(A)+Ref(A).
        assert!(start1 < start2);
        assert!(start2 < end2);
        assert!(end2 < end1);
    }

    #[test]
    fn identical_span_comments_both_survive() {
        // Two findings covering exactly the same words must not collapse.
        let inner = r#"<w:p><w:r><w:t>0123456789</w:t></w:r></w:p>"#;
        let mut edits = HashMap::new();
        edits.insert(
            0,
            vec![
                annotation(1, 2, 6, "finding A"),
                annotation(2, 2, 6, "finding B"),
            ],
        );
        let (xml, out) = run(inner, edits);
        assert_eq!(out.skipped, 0);
        assert_eq!(out.comments.len(), 2);
        assert!(xml.contains(r#"<w:commentRangeStart w:id="1"/>"#));
        assert!(xml.contains(r#"<w:commentRangeStart w:id="2"/>"#));
        assert!(xml.contains(r#"<w:commentRangeEnd w:id="1"/>"#));
        assert!(xml.contains(r#"<w:commentRangeEnd w:id="2"/>"#));
    }

    #[test]
    fn comment_spanning_three_runs() {
        // Ordinal 0 (opens), ordinal 1 (middle, untouched, deliberately
        // non-simple to prove it doesn't matter), ordinal 2 (closes).
        let inner = r#"<w:p><w:r><w:t>Alpha </w:t></w:r><w:r><w:t>Beta</w:t><w:br/></w:r><w:r><w:t> Gamma</w:t></w:r></w:p>"#;
        let mut edits = HashMap::new();
        edits.insert(0, vec![open_only(9, 0, 6, "spans three runs")]);
        edits.insert(2, vec![close_only(9, 3, "spans three runs")]);
        let (xml, out) = run(inner, edits);

        assert_eq!(out.skipped, 0);
        assert_eq!(out.annotated, 1);
        assert_eq!(out.comments.len(), 1);
        assert_eq!(out.comments[0].id, 9);

        // The middle run passes through byte-for-byte, untouched.
        assert!(xml.contains("<w:r><w:t>Beta</w:t><w:br/></w:r>"));

        let start = xml.find(r#"<w:commentRangeStart w:id="9"/>"#).unwrap();
        let end = xml.find(r#"<w:commentRangeEnd w:id="9"/>"#).unwrap();
        assert!(start < end);
        // No delText/ins anywhere: this is comment-only.
        assert!(!xml.contains("w:delText"));
        assert!(!xml.contains("<w:ins"));
    }

    #[test]
    fn multi_run_comment_dropped_when_a_boundary_run_is_not_simple() {
        // The end run has two <w:t> children, so it's not simple; neither
        // half of the comment may be emitted (no orphaned marker).
        let inner =
            r#"<w:p><w:r><w:t>Alpha </w:t></w:r><w:r><w:t>Beta</w:t><w:t>!</w:t></w:r></w:p>"#;
        let mut edits = HashMap::new();
        edits.insert(0, vec![open_only(9, 0, 6, "not simple end run")]);
        edits.insert(1, vec![close_only(9, 2, "not simple end run")]);
        let (xml, out) = run(inner, edits);

        assert!(out.comments.is_empty());
        assert_eq!(out.annotated, 0);
        assert_eq!(
            out.skipped, 1,
            "one finding was dropped, even though it was two emitter pieces"
        );
        assert!(!xml.contains("commentRangeStart"));
        assert!(!xml.contains("commentRangeEnd"));
        // Both affected runs pass through verbatim.
        assert!(xml.contains("<w:t>Alpha </w:t>"));
        assert!(xml.contains("<w:t>Beta</w:t><w:t>!</w:t>"));
    }

    #[test]
    fn multi_run_comment_dropped_when_boundary_run_already_in_revision() {
        let inner =
            r#"<w:p><w:r><w:t>Alpha </w:t></w:r><w:ins><w:r><w:t>Beta</w:t></w:r></w:ins></w:p>"#;
        let mut edits = HashMap::new();
        edits.insert(0, vec![open_only(9, 0, 6, "end run in revision")]);
        edits.insert(1, vec![close_only(9, 2, "end run in revision")]);
        let (xml, out) = run(inner, edits);

        assert!(out.comments.is_empty());
        assert_eq!(out.skipped, 1, "one finding, not two pieces");
        assert!(!xml.contains("commentRangeStart"));
        assert!(!xml.contains("commentRangeEnd"));
        assert!(xml.contains("<w:ins><w:r><w:t>Beta</w:t></w:r></w:ins>"));
    }

    #[test]
    fn redline_adjacent_to_overlapping_comment() {
        // Redline R=[5,10) replacing with "X"; comment C=[7,12) starts
        // inside R's span, forcing R's <w:del> to split.
        let inner = r#"<w:p><w:r><w:t>0123456789AB</w:t></w:r></w:p>"#;
        let mut edits = HashMap::new();
        edits.insert(
            0,
            vec![
                edit(1, 5, 10, "X"),
                annotation(2, 7, 12, "interior comment"),
            ],
        );
        let (xml, out) = run(inner, edits);

        assert_eq!(out.skipped, 0);
        assert_eq!(out.comments.len(), 2);
        assert_eq!(out.annotated, 1);

        // Deleted text is recoverable by concatenating the (split) delText
        // pieces: "56" + "789" == the original [5,10) span "56789".
        let del_texts: Vec<&str> = xml
            .match_indices("<w:delText")
            .map(|(i, _)| {
                let start = xml[i..].find('>').unwrap() + i + 1;
                let end = xml[start..].find("</w:delText>").unwrap() + start;
                &xml[start..end]
            })
            .collect();
        assert_eq!(del_texts.concat(), "56789");
        assert_eq!(del_texts.len(), 2, "split into two delText pieces");

        // Exactly one <w:ins>, appearing after the last del piece.
        assert_eq!(xml.matches("<w:ins").count(), 1);
        let last_del_end = xml.rfind("</w:del>").unwrap();
        let ins_pos = xml.find("<w:ins").unwrap();
        assert!(ins_pos > last_del_end);

        // Comment 2's Start marker sits between the two del pieces.
        let first_del_end = xml.find("</w:del>").unwrap();
        let comment2_start = xml.find(r#"<w:commentRangeStart w:id="2"/>"#).unwrap();
        assert!(comment2_start > first_del_end && comment2_start < last_del_end);

        // Comment 2's End+reference come after comment 1's own End.
        let end1 = xml.find(r#"<w:commentRangeEnd w:id="1"/>"#).unwrap();
        let end2 = xml.find(r#"<w:commentRangeEnd w:id="2"/>"#).unwrap();
        assert!(end1 < end2);
    }

    #[test]
    fn redlines_never_overlap_within_a_run() {
        let inner = r#"<w:p><w:r><w:t>0123456789</w:t></w:r></w:p>"#;
        let mut edits = HashMap::new();
        edits.insert(0, vec![edit(1, 0, 5, "A"), edit(2, 3, 8, "B")]);
        let (xml, out) = run(inner, edits);

        // Only the first (in span order) redline survives; the overlapping
        // second is defensively dropped, not corrupted into the stream.
        assert_eq!(out.skipped, 1);
        assert_eq!(out.comments.len(), 1);
        assert_eq!(out.comments[0].id, 1);
        assert!(xml.contains(r#"<w:t xml:space="preserve">A</w:t>"#));
        assert!(!xml.contains(r#"<w:t xml:space="preserve">B</w:t>"#));
    }
}
