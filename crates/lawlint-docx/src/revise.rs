//! Rewrite `word/document.xml`, turning each fix into a Word revision
//! (`<w:del>` + `<w:ins>`) wrapped in a comment range that carries the rule
//! message. Everything else in the document is streamed through byte-for-byte
//! (modulo XML-equivalent escaping), so no formatting is lost.
//!
//! v1 scope: a fix is applied only when its whole span sits inside a single
//! "simple" run (optional `<w:rPr>` + exactly one `<w:t>`) that is not already
//! inside an existing revision. Anything else is left untouched and counted as
//! skipped, to be handled by cross-run support later.

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::name::QName;
use quick_xml::{Reader, Writer};

use crate::DocxError;

/// A fix resolved to a single run: `start`/`end` are byte offsets *within that
/// run's text*, and `message` becomes the comment body.
#[derive(Debug, Clone)]
pub struct RunEdit {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
    pub message: String,
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
    /// Edits that could not be applied (multi-run / complex runs).
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

#[allow(clippy::too_many_arguments)]
pub fn apply(
    document_xml: &str,
    edits_by_ordinal: &std::collections::HashMap<usize, Vec<RunEdit>>,
    author: &str,
    date: Option<&str>,
    ids: &mut Ids,
) -> Result<RevisionOutput, DocxError> {
    let mut reader = Reader::from_str(document_xml);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::new());

    let mut comments: Vec<CommentOut> = Vec::new();
    let mut skipped = 0usize;

    let mut wt_counter = 0usize;
    let mut revision_depth = 0i32;

    // Run buffering state.
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
                    b"ins" | b"del" => {
                        revision_depth += 1;
                        writer.write_event(Event::Start(e))?;
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
                            run_in_revision,
                            edits_by_ordinal,
                            author,
                            date,
                            ids,
                            &mut em,
                            &mut comments,
                            &mut skipped,
                        )?;
                        buffering = false;
                    } else {
                        buffer.push(Event::End(e).into_owned());
                    }
                    continue;
                }
                match local(e.name()).as_slice() {
                    b"ins" | b"del" => revision_depth -= 1,
                    _ => {}
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
        skipped,
    })
}

#[allow(clippy::too_many_arguments)]
fn finish_run(
    buffer: &[Event<'static>],
    wt_counter: &mut usize,
    run_in_revision: bool,
    edits_by_ordinal: &std::collections::HashMap<usize, Vec<RunEdit>>,
    author: &str,
    date: Option<&str>,
    ids: &mut Ids,
    em: &mut Emitter<'_>,
    comments: &mut Vec<CommentOut>,
    skipped: &mut usize,
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

    // v1: only edit a simple run (rPr? + exactly one w:t, no other children,
    // not already inside a revision).
    let simple = !run_in_revision && info.wts.len() == 1 && info.other_children == 0;
    if !simple {
        *skipped += run_edits.len();
        return verbatim(buffer, em);
    }

    let text = &info.wts[0].text;
    run_edits.sort_by_key(|e| (e.start, e.end));

    // Drop overlaps and out-of-bounds, mirroring core's single-pass rule.
    let mut selected: Vec<RunEdit> = Vec::new();
    let mut guard = 0usize;
    for e in run_edits {
        if e.start >= guard && e.end <= text.len() && e.start <= e.end {
            guard = e.end;
            selected.push(e);
        } else if e.end > text.len() {
            *skipped += 1;
        }
    }
    if selected.is_empty() {
        return verbatim(buffer, em);
    }

    let rpr = &info.rpr;
    let mut cursor = 0usize;
    for e in selected {
        if e.start > cursor {
            em.keep_run(rpr, &text[cursor..e.start])?;
        }
        let deleted = &text[e.start..e.end];
        let comment_id = ids.next_comment;
        ids.next_comment += 1;
        em.comment_marker("w:commentRangeStart", comment_id)?;
        if !deleted.is_empty() {
            let rid = ids.next_rev;
            ids.next_rev += 1;
            em.revision("w:del", "w:delText", rpr, deleted, rid, author, date)?;
        }
        if !e.replacement.is_empty() {
            let rid = ids.next_rev;
            ids.next_rev += 1;
            em.revision("w:ins", "w:t", rpr, &e.replacement, rid, author, date)?;
        }
        em.comment_marker("w:commentRangeEnd", comment_id)?;
        em.comment_reference_run(rpr, comment_id)?;
        comments.push(CommentOut {
            id: comment_id,
            author: author.to_string(),
            date: date.map(|d| d.to_string()),
            text: e.message.clone(),
        });
        cursor = e.end;
    }
    if cursor < text.len() {
        em.keep_run(rpr, &text[cursor..])?;
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
    use std::collections::HashMap;

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

    fn edit(start: usize, end: usize, replacement: &str) -> RunEdit {
        RunEdit {
            start,
            end,
            replacement: replacement.to_string(),
            message: "rule msg".to_string(),
        }
    }

    #[test]
    fn simple_run_becomes_del_ins_and_comment() {
        // "Pursuant to X" -> replace "Pursuant to" (bytes 0..11) with "Under".
        let inner = r#"<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Pursuant to X</w:t></w:r></w:p>"#;
        let mut edits = HashMap::new();
        edits.insert(0, vec![edit(0, 11, "Under")]);
        let (xml, out) = run(inner, edits);

        assert_eq!(out.skipped, 0);
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
        edits.insert(0, vec![edit(1, 1, "X")]);
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
        edits.insert(0, vec![edit(0, 3, "FOO")]);
        let (xml, out) = run(inner, edits);
        assert_eq!(out.skipped, 1);
        assert!(out.comments.is_empty());
        assert!(xml.contains("<w:t>foo</w:t><w:t>bar</w:t>"));
        assert!(!xml.contains("w:ins"));
    }

    #[test]
    fn edit_inside_existing_revision_is_skipped() {
        // ordinal 0 is inside <w:ins>; edits there are not applied in v1.
        let inner = r#"<w:p><w:ins><w:r><w:t>added</w:t></w:r></w:ins></w:p>"#;
        let mut edits = HashMap::new();
        edits.insert(0, vec![edit(0, 5, "gone")]);
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
}
