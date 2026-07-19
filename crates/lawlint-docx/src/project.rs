//! Project `word/document.xml` into a flat plain-text string that the lawlint
//! engine can lint, recording where each `<w:t>` run text lands in that string.
//!
//! The projection is deterministic: the tracked-change writer re-runs the exact
//! same builder so diagnostic byte offsets line up with the runs they came from.
//!
//! Rules:
//! - Each `<w:t>` contributes its (unescaped) text; the byte range it occupies
//!   in the projected string is recorded as a [`RunText`] keyed by its document
//!   order (`ordinal`).
//! - `<w:tab/>` → `\t`, `<w:br/>`/`<w:cr/>` → `\n` (structural gaps, never
//!   editable).
//! - Every `</w:p>` appends a `\n\n` paragraph separator, so blank-line block
//!   segmentation in the core engine sees paragraph boundaries.

use quick_xml::events::Event;
use quick_xml::name::QName;
use quick_xml::Reader;

use crate::DocxError;

/// One `<w:t>` text node and the byte range it occupies in the projected text.
#[derive(Debug, Clone)]
pub struct RunText {
    /// Document-order index of the `<w:t>` element (0-based).
    pub ordinal: usize,
    pub start: usize,
    pub end: usize,
}

pub struct Projection {
    pub text: String,
    pub runs: Vec<RunText>,
}

fn local(name: QName<'_>) -> Vec<u8> {
    name.local_name().as_ref().to_vec()
}

pub fn project(document_xml: &str) -> Result<Projection, DocxError> {
    let mut reader = Reader::from_str(document_xml);
    reader.config_mut().trim_text(false);

    let mut text = String::new();
    let mut runs = Vec::new();
    let mut ordinal = 0usize;

    // State while inside a <w:t>…</w:t>.
    let mut in_t = false;
    let mut t_start = 0usize;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                if local(e.name()).as_slice() == b"t" {
                    in_t = true;
                    t_start = text.len();
                }
            }
            Ok(Event::End(e)) => match local(e.name()).as_slice() {
                b"t" => {
                    if in_t {
                        runs.push(RunText {
                            ordinal,
                            start: t_start,
                            end: text.len(),
                        });
                        ordinal += 1;
                        in_t = false;
                    }
                }
                b"p" => text.push_str("\n\n"),
                _ => {}
            },
            Ok(Event::Empty(e)) => match local(e.name()).as_slice() {
                b"tab" => text.push('\t'),
                b"br" | b"cr" => text.push('\n'),
                // An empty <w:t/> still consumes an ordinal so counts align.
                b"t" => {
                    runs.push(RunText {
                        ordinal,
                        start: text.len(),
                        end: text.len(),
                    });
                    ordinal += 1;
                }
                _ => {}
            },
            Ok(Event::Text(e)) => {
                if in_t {
                    let unescaped = e
                        .unescape()
                        .map_err(|err| DocxError::Malformed(err.to_string()))?;
                    text.push_str(&unescaped);
                }
            }
            Ok(Event::CData(e)) => {
                if in_t {
                    text.push_str(&String::from_utf8_lossy(&e.into_inner()));
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(err) => return Err(DocxError::Malformed(err.to_string())),
        }
    }

    Ok(Projection { text, runs })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NS: &str = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>"#;

    fn proj(inner: &str) -> Projection {
        project(&format!("{NS}{inner}</w:body></w:document>")).unwrap()
    }

    #[test]
    fn maps_run_byte_ranges_and_paragraph_breaks() {
        let p = proj(r#"<w:p><w:r><w:t>Hello</w:t></w:r><w:r><w:t> world</w:t></w:r></w:p>"#);
        assert_eq!(p.text, "Hello world\n\n");
        assert_eq!(p.runs.len(), 2);
        // First run occupies "Hello", second " world"; ordinals are document order.
        assert_eq!(
            (p.runs[0].ordinal, p.runs[0].start, p.runs[0].end),
            (0, 0, 5)
        );
        assert_eq!(
            (p.runs[1].ordinal, p.runs[1].start, p.runs[1].end),
            (1, 5, 11)
        );
        assert_eq!(&p.text[p.runs[1].start..p.runs[1].end], " world");
    }

    #[test]
    fn tabs_and_breaks_are_structural_gaps() {
        let p = proj(r#"<w:p><w:r><w:tab/><w:t>a</w:t><w:br/><w:t>b</w:t></w:r></w:p>"#);
        assert_eq!(p.text, "\ta\nb\n\n");
        assert_eq!(&p.text[p.runs[0].start..p.runs[0].end], "a");
        assert_eq!(&p.text[p.runs[1].start..p.runs[1].end], "b");
    }

    #[test]
    fn utf8_offsets_are_byte_indices() {
        let p = proj(r#"<w:p><w:r><w:t>café</w:t></w:r><w:r><w:t>!</w:t></w:r></w:p>"#);
        // "café" is 5 bytes (é = 2), so the second run starts at byte 5.
        assert_eq!(p.runs[1].start, 5);
        assert_eq!(&p.text[p.runs[1].start..p.runs[1].end], "!");
    }

    #[test]
    fn empty_text_node_still_consumes_an_ordinal() {
        let p = proj(r#"<w:p><w:r><w:t/></w:r><w:r><w:t>x</w:t></w:r></w:p>"#);
        assert_eq!(p.runs.len(), 2);
        assert_eq!(p.runs[0].start, p.runs[0].end); // empty span
        assert_eq!((p.runs[1].ordinal, p.runs[1].start), (1, 0));
    }

    #[test]
    fn xml_entities_are_unescaped() {
        let p = proj(r#"<w:p><w:r><w:t>A &amp; B &lt; C</w:t></w:r></w:p>"#);
        assert_eq!(p.text, "A & B < C\n\n");
        assert_eq!(&p.text[p.runs[0].start..p.runs[0].end], "A & B < C");
    }

    #[test]
    fn existing_tracked_changes_contribute_their_ordinals() {
        // A run inside <w:ins> still holds a <w:t> and must keep counting so the
        // revise pass agrees with these ordinals.
        let p = proj(
            r#"<w:p><w:r><w:t>keep</w:t></w:r><w:ins><w:r><w:t>added</w:t></w:r></w:ins><w:r><w:t>tail</w:t></w:r></w:p>"#,
        );
        assert_eq!(p.text, "keepaddedtail\n\n");
        assert_eq!(p.runs.len(), 3);
        assert_eq!(p.runs[2].ordinal, 2);
        assert_eq!(&p.text[p.runs[2].start..p.runs[2].end], "tail");
    }
}
