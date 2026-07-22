//! End-to-end pipeline test against a real (committed) `.docx` fixture:
//! extract text, lint it with the core engine, write fixes back as tracked
//! changes, and assert both the revisions and the untouched-part fidelity.

use std::io::Read;

use lawlint_core::{lint, LintOptions};
use lawlint_docx::{apply_tracked_changes, extract, ReviseOptions};

const FIXTURE: &[u8] = include_bytes!("fixtures/sample.docx");

fn part(bytes: &[u8], name: &str) -> Option<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut file = zip.by_name(name).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).unwrap();
    Some(buf)
}

fn names(bytes: &[u8]) -> Vec<String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    (0..zip.len())
        .map(|i| zip.by_index(i).unwrap().name().to_string())
        .collect()
}

#[test]
fn extracts_projected_text() {
    let text = extract(FIXTURE).unwrap();
    assert!(text.contains("Motion to Dismiss"));
    assert!(text.contains("pursuant to the schedule"));
    // Paragraph separators keep block segmentation meaningful.
    assert!(text.contains("\n\n"));
}

#[test]
fn writes_tracked_changes_and_preserves_all_other_parts() {
    let text = extract(FIXTURE).unwrap();
    let result = lint(&text, &LintOptions::default());
    assert!(
        result.diagnostics.iter().any(|d| d.fix.is_some()),
        "fixture should surface at least one machine-applicable fix"
    );

    let opts = ReviseOptions {
        author: "unit-test".to_string(),
        date: Some("2020-01-01T00:00:00Z".to_string()),
        include_ai_rewrites: true,
        annotate_findings: true,
    };
    let out = apply_tracked_changes(FIXTURE, &result.diagnostics, &opts).unwrap();
    assert!(out.applied >= 1, "at least one fix should be applied");

    let doc = String::from_utf8(part(&out.bytes, "word/document.xml").unwrap()).unwrap();
    assert!(doc.contains("<w:ins"));
    assert!(doc.contains("<w:del"));
    assert!(doc.contains("<w:commentReference"));

    let comments = String::from_utf8(part(&out.bytes, "word/comments.xml").unwrap()).unwrap();
    assert!(comments.contains(r#"w:author="unit-test""#));

    // comments.xml is declared and related.
    let ct = String::from_utf8(part(&out.bytes, "[Content_Types].xml").unwrap()).unwrap();
    assert!(ct.contains("/word/comments.xml"));
    let rels =
        String::from_utf8(part(&out.bytes, "word/_rels/document.xml.rels").unwrap()).unwrap();
    assert!(rels.contains("comments.xml"));

    // The only new part is comments.xml; every original part survives.
    let before: std::collections::HashSet<_> = names(FIXTURE).into_iter().collect();
    let after: std::collections::HashSet<_> = names(&out.bytes).into_iter().collect();
    for name in &before {
        assert!(after.contains(name), "dropped original part: {name}");
    }
    let added: Vec<_> = after.difference(&before).collect();
    assert_eq!(added, vec![&"word/comments.xml".to_string()]);

    // Unrelated parts are byte-identical (no lossy round-trip).
    for name in [
        "word/theme/theme1.xml",
        "word/styles.xml",
        "docProps/thumbnail.jpeg",
    ] {
        assert_eq!(
            part(FIXTURE, name),
            part(&out.bytes, name),
            "part changed unexpectedly: {name}"
        );
    }
}

#[test]
fn no_applicable_fixes_returns_input_unchanged() {
    // A doc with no diagnostics: apply is a no-op passthrough.
    let out = apply_tracked_changes(FIXTURE, &[], &ReviseOptions::default()).unwrap();
    assert_eq!(out.applied, 0);
    assert_eq!(out.bytes, FIXTURE);
}
