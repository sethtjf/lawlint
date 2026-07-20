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
}

impl Default for ReviseOptions {
    fn default() -> Self {
        Self {
            author: "lawlint".to_string(),
            date: Some(iso8601_utc_now()),
        }
    }
}

pub struct ReviseResult {
    pub bytes: Vec<u8>,
    /// Fixes turned into tracked changes.
    pub applied: usize,
    /// Fixes that could not be applied (span multiple/complex runs).
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

    // Collect (edit, message), select non-overlapping in span order (core's rule).
    let mut candidates: Vec<(usize, usize, String, String)> = Vec::new();
    for d in diagnostics {
        let Some(fix) = &d.fix else { continue };
        if fix.applicability != Applicability::MachineApplicable {
            continue;
        }
        for e in &fix.edits {
            if e.range.start <= e.range.end && e.range.end <= projection.text.len() {
                candidates.push((
                    e.range.start,
                    e.range.end,
                    e.replacement.clone(),
                    d.message.clone(),
                ));
            }
        }
    }
    candidates.sort_by_key(|c| (c.0, c.1));

    let mut edits_by_ordinal: HashMap<usize, Vec<RunEdit>> = HashMap::new();
    let mut skipped = 0usize;
    let mut guard = 0usize;
    for (start, end, replacement, message) in candidates {
        if start < guard {
            continue; // overlaps an already-selected edit
        }
        guard = end;
        // The whole span must sit inside one run's text.
        match projection
            .runs
            .iter()
            .find(|r| start >= r.start && end <= r.end)
        {
            Some(run) => edits_by_ordinal
                .entry(run.ordinal)
                .or_default()
                .push(RunEdit {
                    start: start - run.start,
                    end: end - run.start,
                    replacement,
                    message,
                }),
            None => skipped += 1,
        }
    }

    if edits_by_ordinal.is_empty() {
        return Ok(ReviseResult {
            bytes: docx_bytes.to_vec(),
            applied: 0,
            skipped,
        });
    }

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

    let out = revise::apply(
        &xml,
        &edits_by_ordinal,
        &opts.author,
        opts.date.as_deref(),
        &mut ids,
    )?;
    skipped += out.skipped;
    let applied = out.comments.len();

    pkg.set(DOCUMENT, out.document_xml);

    if !out.comments.is_empty() {
        let comments_xml = build_comments_xml(existing_comments.as_deref(), &out.comments);
        pkg.set(COMMENTS, comments_xml.into_bytes());
        wire_comments_part(&mut pkg)?;
    }

    Ok(ReviseResult {
        bytes: pkg.write()?,
        applied,
        skipped,
    })
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
