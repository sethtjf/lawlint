//! Read `.docx` into lawlint's plain-text model and write fixes back as Word
//! tracked changes (`<w:ins>`/`<w:del>`) plus review comments.
//!
//! The `.docx` is treated as an OPC (zip) package. We project `document.xml`
//! into a flat string, lint that with the unchanged `lawlint-core` engine, then
//! splice the resulting byte-range fixes back as revisions — editing only the
//! affected runs and leaving every other part of the package untouched. This
//! avoids the fidelity loss of round-tripping through a typed docx model.

use std::collections::HashMap;

use lawlint_core::{Applicability, Diagnostic};

mod pkg;
mod project;
mod revise;

use pkg::Package;
use project::{Projection, RunText};
use revise::{Ids, RunEdit};

const DOCUMENT: &str = "word/document.xml";
const COMMENTS: &str = "word/comments.xml";
const CONTENT_TYPES: &str = "[Content_Types].xml";
const DOC_RELS: &str = "word/_rels/document.xml.rels";
const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const COMMENTS_CT: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml";
const COMMENTS_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments";

#[derive(Debug, thiserror::Error)]
pub enum DocxError {
    #[error("not a valid .docx (zip) file: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("xml error: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("not a Word document: missing {0}")]
    MissingPart(&'static str),
    #[error("malformed document.xml: {0}")]
    Malformed(String),
}

/// How to attribute the revisions and comments.
#[derive(Debug, Clone)]
pub struct ReviseOptions {
    pub author: String,
    /// xsd:dateTime, e.g. `2026-07-19T01:11:00Z`. Defaults to now (UTC).
    pub date: Option<String>,
    /// Include `MaybeIncorrect` fixes — in practice the AI rules' suggested
    /// rewrites, which are judgment calls rather than mechanical
    /// substitutions. On by default here because a `.docx` revision is a
    /// *proposal*: it lands as a tracked change the author accepts or rejects
    /// run-by-run in Word, so withholding it costs them the suggestion and
    /// saves them nothing. Plain text has no such review layer, which is why
    /// the CLI gates it there.
    pub include_ai_rewrites: bool,
    /// Anchor a comment over every finding that has no applicable fix, so a
    /// rule that can only explain a problem still reaches the author. On by
    /// default: a review copy that silently omits most of the review is worse
    /// than one that is merely wordy.
    pub annotate_findings: bool,
}

impl Default for ReviseOptions {
    fn default() -> Self {
        Self {
            author: "lawlint".to_string(),
            date: Some(iso8601_utc_now()),
            include_ai_rewrites: true,
            annotate_findings: true,
        }
    }
}

pub struct ReviseResult {
    pub bytes: Vec<u8>,
    /// Fixes turned into tracked changes (del+ins+comment).
    pub applied: usize,
    /// Fixes whose text could not be safely rewritten in place — the edit
    /// crosses a run boundary (redlines never do, see revise.rs), or an
    /// earlier fix already claimed overlapping text for its own redline —
    /// but whose message is still surfaced as a comment anchored to the
    /// original, unchanged span rather than being dropped outright.
    pub annotated: usize,
    /// How many of `applied` came from AI rules (`MaybeIncorrect`), so callers
    /// can tell the author which redlines warrant the closer read.
    pub ai_applied: usize,
    /// Edits dropped entirely: no run at all contains the span (e.g. it
    /// lands in a paragraph gap), or the run(s) it resolved to turned out
    /// unusable (already inside a revision, or shaped in a way the emitter
    /// refuses to touch — see revise.rs).
    pub skipped: usize,
}

/// Extract the plain text of a `.docx` for linting.
pub fn extract(docx_bytes: &[u8]) -> Result<String, DocxError> {
    let pkg = Package::read(docx_bytes)?;
    let xml = pkg
        .get_str(DOCUMENT)?
        .ok_or(DocxError::MissingPart(DOCUMENT))?;
    Ok(project::project(&xml)?.text)
}

/// Apply MachineApplicable fixes from `diagnostics` to the `.docx` as tracked
/// changes + comments, returning the new package bytes.
pub fn apply_tracked_changes(
    docx_bytes: &[u8],
    diagnostics: &[Diagnostic],
    opts: &ReviseOptions,
) -> Result<ReviseResult, DocxError> {
    let mut pkg = Package::read(docx_bytes)?;
    let xml = pkg
        .get_str(DOCUMENT)?
        .ok_or(DocxError::MissingPart(DOCUMENT))?;
    let projection = project::project(&xml)?;

    let existing_comments = pkg.get_str(COMMENTS)?;
    let comment_base = existing_comments
        .as_deref()
        .map(max_comment_id)
        .unwrap_or(0)
        + 1;
    let mut ids = Ids {
        next_rev: 900_000,
        next_comment: comment_base,
    };

    let selection = select_edits(&projection, diagnostics, opts, &mut ids);
    let edits_by_ordinal = selection.edits_by_ordinal;
    let mut skipped = selection.skipped;

    if edits_by_ordinal.is_empty() {
        return Ok(ReviseResult {
            bytes: docx_bytes.to_vec(),
            applied: 0,
            annotated: 0,
            ai_applied: 0,
            skipped,
        });
    }

    let out = revise::apply(
        &xml,
        &edits_by_ordinal,
        &opts.author,
        opts.date.as_deref(),
        &mut ids,
    )?;
    skipped += out.skipped;
    let annotated = out.annotated;
    let applied = out.comments.len() - annotated;
    // The emitter can drop redlines it refuses to touch and does not report
    // which ones, so cap rather than over-report the AI share.
    let ai_applied = selection.ai_redlines.min(applied);

    pkg.set(DOCUMENT, out.document_xml);

    if !out.comments.is_empty() {
        let comments_xml = build_comments_xml(existing_comments.as_deref(), &out.comments);
        pkg.set(COMMENTS, comments_xml.into_bytes());
        wire_comments_part(&mut pkg)?;
    }

    Ok(ReviseResult {
        bytes: pkg.write()?,
        applied,
        annotated,
        ai_applied,
        skipped,
    })
}

struct EditSelection {
    edits_by_ordinal: HashMap<usize, Vec<RunEdit>>,
    skipped: usize,
    /// Redline pieces originating from an AI rule, before the emitter has had
    /// its say — capped against the emitter's real count by the caller.
    ai_redlines: usize,
}

/// Resolve each MachineApplicable fix's edit to the run(s) it lands in and
/// decide whether it becomes a full redline or downgrades to a comment-only
/// anchor, in span order:
///   - the whole span sits in one run, and no earlier (span-order) redline
///     already claimed overlapping text → a self-contained redline;
///   - otherwise, it sits in one run but lost the redline slot to an
///     earlier overlapping redline → a self-contained comment-only anchor
///     (the emitter can nest/interleave comment ranges freely; it just
///     can't apply two conflicting text replacements to the same words);
///   - the span crosses a run boundary → a comment-only anchor split into
///     an opens-only piece (start run) and a closes-only piece (end run)
///     sharing one `comment_id` (redlines never cross runs — see
///     revise.rs's module doc for why that's a deliberate scope limit, not
///     an oversight).
///
/// A span that isn't contained in any run at all (e.g. it lands in a
/// paragraph gap) is dropped and counted in `skipped`.
fn select_edits(
    projection: &Projection,
    diagnostics: &[Diagnostic],
    opts: &ReviseOptions,
    ids: &mut Ids,
) -> EditSelection {
    // A candidate per finding. One with a usable fix wants a redline; every
    // other finding wants a comment anchored over the text that triggered it,
    // so a rule that can explain a problem without mechanically repairing it
    // still leaves a mark in the document — which is most rules.
    struct Candidate {
        start: usize,
        end: usize,
        replacement: Option<String>,
        message: String,
        is_ai: bool,
    }

    // The comment carries the rule's advice too, not just its complaint: for a
    // finding with no fix, the suggestion is the only actionable thing there
    // is.
    let text_for = |d: &Diagnostic| match &d.suggestion {
        Some(suggestion) if suggestion != &d.message => {
            format!("{} — {}", d.message, suggestion)
        }
        _ => d.message.clone(),
    };
    let in_bounds = |start: usize, end: usize| start <= end && end <= projection.text.len();

    let mut rewrites: Vec<Candidate> = Vec::new();
    let mut annotations: Vec<Candidate> = Vec::new();
    for d in diagnostics {
        let usable_fix = d.fix.as_ref().filter(|fix| match fix.applicability {
            Applicability::MachineApplicable => true,
            Applicability::MaybeIncorrect => opts.include_ai_rewrites,
        });
        match usable_fix {
            Some(fix) => {
                let is_ai = fix.applicability == Applicability::MaybeIncorrect;
                for e in &fix.edits {
                    if in_bounds(e.range.start, e.range.end) {
                        rewrites.push(Candidate {
                            start: e.range.start,
                            end: e.range.end,
                            replacement: Some(e.replacement.clone()),
                            message: text_for(d),
                            is_ai,
                        });
                    }
                }
            }
            None if opts.annotate_findings
                && in_bounds(d.span.start, d.span.end)
                && d.span.start < d.span.end =>
            {
                annotations.push(Candidate {
                    start: d.span.start,
                    end: d.span.end,
                    replacement: None,
                    message: text_for(d),
                    is_ai: false,
                });
            }
            None => {}
        }
    }
    // Rewrites are resolved first so they get first claim on the redline slot;
    // comment anchors may overlap anything, including each other.
    rewrites.sort_by_key(|c| (c.start, c.end));
    annotations.sort_by_key(|c| (c.start, c.end));
    let candidates = rewrites.into_iter().chain(annotations);

    let mut edits_by_ordinal: HashMap<usize, Vec<RunEdit>> = HashMap::new();
    let mut skipped = 0usize;
    let mut ai_redlines = 0usize;
    let mut redline_guard = 0usize;

    for candidate in candidates {
        let Candidate {
            start,
            end,
            replacement,
            message,
            is_ai,
        } = candidate;
        let Some((start_run, end_run)) = find_runs(projection, start, end) else {
            skipped += 1;
            continue;
        };
        let same_run = start_run.ordinal == end_run.ordinal;
        // A redline needs its whole span inside one run and no earlier redline
        // on the same words. Failing either, the finding is not dropped — it
        // degrades to a comment over the untouched original, which still tells
        // the author what and where.
        let wants_redline = replacement.is_some() && same_run && start >= redline_guard;

        let comment_id = ids.next_comment;
        ids.next_comment += 1;

        if wants_redline {
            redline_guard = end;
            if is_ai {
                ai_redlines += 1;
            }
            edits_by_ordinal
                .entry(start_run.ordinal)
                .or_default()
                .push(RunEdit {
                    start: start - start_run.start,
                    end: end - start_run.start,
                    replacement,
                    message,
                    comment_id,
                    opens_here: true,
                    closes_here: true,
                });
        } else if same_run {
            edits_by_ordinal
                .entry(start_run.ordinal)
                .or_default()
                .push(RunEdit {
                    start: start - start_run.start,
                    end: end - start_run.start,
                    replacement: None,
                    message,
                    comment_id,
                    opens_here: true,
                    closes_here: true,
                });
        } else {
            // Split anchor: the two halves share one comment_id so the emitter
            // can pair them, and `classify` can void them together.
            edits_by_ordinal
                .entry(start_run.ordinal)
                .or_default()
                .push(RunEdit {
                    start: start - start_run.start,
                    end: start_run.end - start_run.start,
                    replacement: None,
                    message: message.clone(),
                    comment_id,
                    opens_here: true,
                    closes_here: false,
                });
            edits_by_ordinal
                .entry(end_run.ordinal)
                .or_default()
                .push(RunEdit {
                    start: 0,
                    end: end - end_run.start,
                    replacement: None,
                    message,
                    comment_id,
                    opens_here: false,
                    closes_here: true,
                });
        }
    }

    EditSelection {
        edits_by_ordinal,
        skipped,
        ai_redlines,
    }
}

/// Resolve a `[start, end)` span to the run containing its start and the run
/// containing its end, which may differ. `start == end` (a pure insertion)
/// is a special case: the asymmetric half-open lookups below would miss a
/// zero-width span sitting exactly on a run boundary (e.g. an empty
/// `<w:t/>`), so it instead uses the old combined inclusive-both-ends
/// predicate against a single run.
fn find_runs(projection: &Projection, start: usize, end: usize) -> Option<(&RunText, &RunText)> {
    if start == end {
        let r = projection
            .runs
            .iter()
            .find(|r| start >= r.start && end <= r.end)?;
        return Some((r, r));
    }
    let start_run = projection
        .runs
        .iter()
        .find(|r| r.start <= start && start < r.end)?;
    let end_run = projection
        .runs
        .iter()
        .find(|r| r.start < end && end <= r.end)?;
    Some((start_run, end_run))
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

fn initials(author: &str) -> String {
    let inits: String = author
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .collect();
    if inits.is_empty() {
        "L".to_string()
    } else {
        inits.to_uppercase()
    }
}

fn comment_block(c: &revise::CommentOut) -> String {
    let mut attrs = format!(
        r#" w:id="{}" w:author="{}" w:initials="{}""#,
        c.id,
        xml_escape(&c.author),
        xml_escape(&initials(&c.author)),
    );
    if let Some(date) = &c.date {
        attrs.push_str(&format!(r#" w:date="{}""#, xml_escape(date)));
    }
    format!(
        r#"<w:comment{attrs}><w:p><w:r><w:t xml:space="preserve">{}</w:t></w:r></w:p></w:comment>"#,
        xml_escape(&c.text),
    )
}

fn build_comments_xml(existing: Option<&str>, comments: &[revise::CommentOut]) -> String {
    let blocks: String = comments.iter().map(comment_block).collect();
    match existing {
        Some(existing) if existing.contains("</w:comments>") => {
            existing.replacen("</w:comments>", &format!("{blocks}</w:comments>"), 1)
        }
        _ => format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:comments xmlns:w="{W_NS}">{blocks}</w:comments>"#,
        ),
    }
}

fn max_comment_id(comments_xml: &str) -> usize {
    let mut max = 0usize;
    let mut rest = comments_xml;
    while let Some(pos) = rest.find("<w:comment ") {
        rest = &rest[pos..];
        if let Some(id) = attr_value(rest, "w:id=\"") {
            max = max.max(id.parse().unwrap_or(0));
        }
        rest = &rest[1..];
    }
    max
}

/// Read the value of `needle` (e.g. `w:id="`) starting near the front of `s`.
fn attr_value(s: &str, needle: &str) -> Option<String> {
    let start = s.find(needle)? + needle.len();
    let end = s[start..].find('"')? + start;
    Some(s[start..end].to_string())
}

/// Declare the comments part in `[Content_Types].xml` and relate it from
/// `document.xml.rels` if not already present.
fn wire_comments_part(pkg: &mut Package) -> Result<(), DocxError> {
    let ct = pkg
        .get_str(CONTENT_TYPES)?
        .ok_or(DocxError::MissingPart("[Content_Types].xml"))?;
    if !ct.contains("/word/comments.xml") {
        let override_el =
            format!(r#"<Override PartName="/word/comments.xml" ContentType="{COMMENTS_CT}"/>"#);
        let ct = ct.replacen("</Types>", &format!("{override_el}</Types>"), 1);
        pkg.set(CONTENT_TYPES, ct.into_bytes());
    }

    let rels = pkg
        .get_str(DOC_RELS)?
        .ok_or(DocxError::MissingPart("word/_rels/document.xml.rels"))?;
    if !rels.contains(COMMENTS_REL) {
        let rid = next_rel_id(&rels);
        let rel =
            format!(r#"<Relationship Id="{rid}" Type="{COMMENTS_REL}" Target="comments.xml"/>"#,);
        let rels = rels.replacen("</Relationships>", &format!("{rel}</Relationships>"), 1);
        pkg.set(DOC_RELS, rels.into_bytes());
    }
    Ok(())
}

fn next_rel_id(rels: &str) -> String {
    let mut max = 0usize;
    let mut rest = rels;
    while let Some(pos) = rest.find("Id=\"rId") {
        let start = pos + "Id=\"rId".len();
        let digits: String = rest[start..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(n) = digits.parse::<usize>() {
            max = max.max(n);
        }
        rest = &rest[pos + 4..];
    }
    format!("rId{}", max + 1)
}

/// Current UTC time as an xsd:dateTime (`YYYY-MM-DDThh:mm:ssZ`).
fn iso8601_utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lawlint_core::{Fix, RuleId, Severity, TextRange, Tier};

    fn diag(start: usize, end: usize, replacement: &str, message: &str) -> Diagnostic {
        Diagnostic {
            rule_id: RuleId("core/x".into()),
            severity: Severity::Warning,
            tier: Tier::Static,
            intent: Default::default(),
            span: TextRange { start, end },
            message: message.into(),
            line: 0,
            column: 0,
            end_line: None,
            end_column: None,
            excerpt: String::new(),
            suggestion: None,
            weight: None,
            confidence: None,
            fix: Some(Fix {
                edits: vec![lawlint_core::Edit {
                    range: TextRange { start, end },
                    replacement: replacement.into(),
                }],
                applicability: Applicability::MachineApplicable,
            }),
        }
    }

    fn ids() -> Ids {
        Ids {
            next_rev: 900_000,
            next_comment: 1,
        }
    }

    /// Three contiguous runs: [0,5) [5,10) [10,15).
    fn three_runs() -> Projection {
        Projection {
            text: "x".repeat(15),
            runs: vec![
                RunText {
                    ordinal: 0,
                    start: 0,
                    end: 5,
                },
                RunText {
                    ordinal: 1,
                    start: 5,
                    end: 10,
                },
                RunText {
                    ordinal: 2,
                    start: 10,
                    end: 15,
                },
            ],
        }
    }

    #[test]
    fn select_edits_allows_overlapping_annotations() {
        // Both spans sit in run 0 and overlap; the first wins the redline,
        // the second downgrades to a comment-only anchor rather than being
        // dropped (goal A: the emitter can nest comment ranges now).
        let projection = three_runs();
        let diags = vec![diag(0, 3, "A", "first"), diag(1, 4, "B", "second")];
        let mut ids = ids();
        let sel = select_edits(&projection, &diags, &ReviseOptions::default(), &mut ids);

        assert_eq!(sel.skipped, 0);
        let pieces = sel.edits_by_ordinal.get(&0).expect("ordinal 0 has edits");
        assert_eq!(pieces.len(), 2);
        assert!(pieces.iter().any(|p| p.replacement.as_deref() == Some("A")));
        assert!(pieces
            .iter()
            .any(|p| p.replacement.is_none() && p.message == "second"));
        // Distinct comment ids for both pieces.
        assert_ne!(pieces[0].comment_id, pieces[1].comment_id);
    }

    #[test]
    fn select_edits_downgrades_multi_run_fix_to_split_comment() {
        // Span [2,12) starts in run 0 and ends in run 2: a redline can't
        // cross runs, so this becomes a two-piece comment-only anchor
        // instead of being dropped (goal B's explicit escape hatch).
        let projection = three_runs();
        let diags = vec![diag(2, 12, "Y", "spans runs")];
        let mut ids = ids();
        let sel = select_edits(&projection, &diags, &ReviseOptions::default(), &mut ids);

        assert_eq!(sel.skipped, 0);
        let opens = &sel.edits_by_ordinal[&0];
        let closes = &sel.edits_by_ordinal[&2];
        assert_eq!(opens.len(), 1);
        assert_eq!(closes.len(), 1);
        assert!(opens[0].opens_here && !opens[0].closes_here);
        assert!(!closes[0].opens_here && closes[0].closes_here);
        assert!(opens[0].replacement.is_none());
        assert!(closes[0].replacement.is_none());
        assert_eq!(opens[0].comment_id, closes[0].comment_id);
        // Run 1 (the middle run) gets no entry at all.
        assert!(!sel.edits_by_ordinal.contains_key(&1));
    }

    #[test]
    fn select_edits_skips_span_not_contained_in_any_run() {
        let mut projection = three_runs();
        // Open a gap between run 0 and run 1 (e.g. a tab/paragraph break).
        projection.runs[1].start = 7;
        let diags = vec![diag(5, 6, "Z", "in the gap")];
        let mut ids = ids();
        let sel = select_edits(&projection, &diags, &ReviseOptions::default(), &mut ids);

        assert_eq!(sel.skipped, 1);
        assert!(sel.edits_by_ordinal.is_empty());
    }

    #[test]
    fn accounting_reconciles_across_overlap_and_multi_run_candidates() {
        // A mix of: an ordinary single-run fix, an overlap loser downgraded
        // to a comment, and a multi-run fix downgraded to a split comment.
        // applied + annotated + skipped must equal the number of candidates
        // considered, and out.comments.len() must equal applied + annotated.
        let inner = concat!(
            r#"<w:p><w:r><w:t>xxxxx</w:t></w:r>"#,
            r#"<w:r><w:t>xxxxx</w:t></w:r>"#,
            r#"<w:r><w:t>xxxxx</w:t></w:r></w:p>"#,
        );
        let xml = format!(
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{inner}</w:body></w:document>"#
        );
        let projection = three_runs();
        let diags = vec![
            diag(0, 3, "A", "ordinary fix"),   // run 0, applied
            diag(1, 4, "B", "overlap loser"),  // run 0, downgraded
            diag(2, 12, "Y", "multi-run fix"), // runs 0..2, downgraded (2 pieces)
        ];
        let mut ids = ids();
        let sel = select_edits(&projection, &diags, &ReviseOptions::default(), &mut ids);
        assert_eq!(sel.skipped, 0);

        let out = revise::apply(&xml, &sel.edits_by_ordinal, "lawlint", None, &mut ids).unwrap();
        assert_eq!(out.skipped, 0);
        let applied = out.comments.len() - out.annotated;
        // One redline applied (the first fix); two comment-only findings
        // (overlap loser + multi-run fix) annotated.
        assert_eq!(applied, 1);
        assert_eq!(out.annotated, 2);
        assert_eq!(out.comments.len(), applied + out.annotated);
    }
}
