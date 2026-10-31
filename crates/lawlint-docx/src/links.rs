//! Strip AI-assistant referral tags from hyperlink targets.
//!
//! ChatGPT appends `utm_source=chatgpt.com` to every link it formats. In a
//! `.docx` the URL lives in a relationship part (`word/_rels/*.rels`), not in
//! the text the engine lints, so `core/no-ai-tracking-links` never sees it:
//! the reader only finds the tag by hovering the link. This pass rewrites the
//! `Target` of each external hyperlink relationship directly. Word does not
//! track edits to relationship targets, so these changes cannot be redlines;
//! the caller reports them instead.

use crate::pkg::Package;
use crate::DocxError;

/// `utm_source` values that mark a link as AI-formatted, compared
/// case-insensitively. Keep in step with the value list in
/// `builtin/rules/no-ai-tracking-links.md`.
pub const AI_UTM_SOURCES: &[&str] = &["chatgpt.com", "openai.com", "openai"];

const HYPERLINK_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";

/// A hyperlink target carrying an AI referral tag, and what it becomes once
/// the tag is removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedLink {
    /// Relationship part the link lives in, e.g. `word/_rels/document.xml.rels`.
    pub part: String,
    pub url: String,
    pub cleaned: String,
}

/// `url` without its AI `utm_source` parameters, or `None` when it has none.
/// Other query parameters and the fragment are kept in order.
pub fn strip_ai_tracking(url: &str) -> Option<String> {
    let (rest, fragment) = match url.find('#') {
        Some(i) => url.split_at(i),
        None => (url, ""),
    };
    let (base, query) = rest.split_once('?')?;
    let params: Vec<&str> = query.split('&').collect();
    let kept: Vec<&str> = params
        .iter()
        .copied()
        .filter(|param| !is_ai_tag(param))
        .collect();
    if kept.len() == params.len() {
        return None;
    }
    let mut out = base.to_string();
    if kept.iter().any(|p| !p.is_empty()) {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    out.push_str(fragment);
    Some(out)
}

fn is_ai_tag(param: &str) -> bool {
    let Some((key, value)) = param.split_once('=') else {
        return false;
    };
    key.eq_ignore_ascii_case("utm_source")
        && AI_UTM_SOURCES
            .iter()
            .any(|source| value.eq_ignore_ascii_case(source))
}

/// Every AI-tagged hyperlink target in the package, in part order.
pub fn find(pkg: &Package) -> Result<Vec<TrackedLink>, DocxError> {
    let mut found = Vec::new();
    for part in rels_parts(pkg) {
        let xml = rels_str(pkg, &part)?;
        for target in hyperlink_targets(&xml) {
            if let Some(cleaned) = strip_ai_tracking(&target.value) {
                found.push(TrackedLink {
                    part: part.clone(),
                    url: target.value,
                    cleaned,
                });
            }
        }
    }
    Ok(found)
}

/// Rewrite every AI-tagged hyperlink target in place. Returns the links
/// changed; parts with none are left byte-identical.
pub fn strip(pkg: &mut Package) -> Result<Vec<TrackedLink>, DocxError> {
    let mut changed = Vec::new();
    for part in rels_parts(pkg) {
        let xml = rels_str(pkg, &part)?;
        let mut out = String::with_capacity(xml.len());
        let mut cursor = 0;
        for target in hyperlink_targets(&xml) {
            let Some(cleaned) = strip_ai_tracking(&target.value) else {
                continue;
            };
            out.push_str(&xml[cursor..target.start]);
            out.push_str(&escape_attr(&cleaned));
            cursor = target.end;
            changed.push(TrackedLink {
                part: part.clone(),
                url: target.value,
                cleaned,
            });
        }
        if cursor > 0 {
            out.push_str(&xml[cursor..]);
            pkg.set(&part, out.into_bytes());
        }
    }
    Ok(changed)
}

/// Relationship parts under `word/`: the main document plus headers,
/// footers, footnotes, endnotes, and comments, which can all hold links.
fn rels_parts(pkg: &Package) -> Vec<String> {
    pkg.parts
        .iter()
        .map(|p| p.name.clone())
        .filter(|name| name.starts_with("word/_rels/") && name.ends_with(".rels"))
        .collect()
}

fn rels_str(pkg: &Package, part: &str) -> Result<String, DocxError> {
    Ok(pkg.get_str(part)?.unwrap_or_default())
}

/// A hyperlink `Target` attribute: its unescaped value and the byte range of
/// the raw (escaped) value between the quotes.
struct Target {
    value: String,
    start: usize,
    end: usize,
}

/// Scan `<Relationship …/>` elements for external hyperlink targets. A
/// relationships part is flat and machine-written, so an attribute scan is
/// enough and lets the rewrite splice only the bytes it changes.
fn hyperlink_targets(xml: &str) -> Vec<Target> {
    let mut targets = Vec::new();
    let mut from = 0;
    while let Some(offset) = xml[from..].find("<Relationship") {
        let open = from + offset;
        let Some(len) = xml[open..].find('>') else {
            break;
        };
        let close = open + len;
        from = close + 1;
        let tag = &xml[open..close];
        // `<Relationships` is the root element, not a relationship.
        if !tag["<Relationship".len()..].starts_with(|c: char| c.is_whitespace()) {
            continue;
        }
        let is_hyperlink = attr(tag, "Type").is_some_and(|(_, _, v)| v == HYPERLINK_REL);
        let external = attr(tag, "TargetMode").is_some_and(|(_, _, v)| v == "External");
        if !(is_hyperlink && external) {
            continue;
        }
        if let Some((start, end, raw)) = attr(tag, "Target") {
            targets.push(Target {
                value: unescape_attr(raw),
                start: open + start,
                end: open + end,
            });
        }
    }
    targets
}

/// Find attribute `name` in a start tag: (value start, value end, raw value),
/// offsets relative to `tag`.
fn attr<'a>(tag: &'a str, name: &str) -> Option<(usize, usize, &'a str)> {
    let bytes = tag.as_bytes();
    let mut from = 0;
    while let Some(offset) = tag[from..].find(name) {
        let at = from + offset;
        from = at + name.len();
        let preceded = at > 0 && bytes[at - 1].is_ascii_whitespace();
        let rest = tag[from..].trim_start();
        if !preceded || !rest.starts_with('=') {
            continue;
        }
        let after_eq = rest[1..].trim_start();
        let quote = after_eq.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let start = tag.len() - after_eq.len() + 1;
        let end = start + tag[start..].find(quote)?;
        return Some((start, end, &tag[start..end]));
    }
    None
}

fn unescape_attr(raw: &str) -> String {
    quick_xml::escape::unescape(raw)
        .map(|v| v.into_owned())
        .unwrap_or_else(|_| raw.to_string())
}

fn escape_attr(value: &str) -> String {
    quick_xml::escape::escape(value).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_only_ai_utm_source() {
        let cases = [
            (
                "https://a.test/x?utm_source=chatgpt.com",
                "https://a.test/x",
            ),
            (
                "https://a.test/x?utm_source=chatgpt.com&page=2",
                "https://a.test/x?page=2",
            ),
            (
                "https://a.test/x?page=2&utm_source=chatgpt.com#s3",
                "https://a.test/x?page=2#s3",
            ),
            ("https://a.test/x?UTM_SOURCE=OpenAI", "https://a.test/x"),
            (
                "https://a.test/x?utm_source=openai.com&utm_medium=x",
                "https://a.test/x?utm_medium=x",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(strip_ai_tracking(input).as_deref(), Some(want), "{input}");
        }
        for clean in [
            "https://a.test/x",
            "https://a.test/x?utm_source=newsletter",
            "https://a.test/x?utm_source=chatgpt.com.example",
            "https://a.test/x#utm_source=chatgpt.com",
        ] {
            assert_eq!(strip_ai_tracking(clean), None, "{clean}");
        }
    }

    const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://a.test/case?x=1&amp;utm_source=chatgpt.com" TargetMode="External"/><Relationship Id="rId6" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://a.test/clean" TargetMode="External"/></Relationships>"#;

    fn pkg_with(rels: &str) -> Package {
        let mut pkg = Package { parts: Vec::new() };
        pkg.set("word/_rels/document.xml.rels", rels.as_bytes().to_vec());
        pkg.set("word/document.xml", b"<w:document/>".to_vec());
        pkg
    }

    #[test]
    fn finds_and_strips_hyperlink_targets_only() {
        let mut pkg = pkg_with(RELS);
        let found = find(&pkg).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].url,
            "https://a.test/case?x=1&utm_source=chatgpt.com"
        );
        assert_eq!(found[0].cleaned, "https://a.test/case?x=1");

        let changed = strip(&mut pkg).unwrap();
        assert_eq!(changed, found);
        let out = pkg
            .get_str("word/_rels/document.xml.rels")
            .unwrap()
            .unwrap();
        assert_eq!(out, RELS.replace("?x=1&amp;utm_source=chatgpt.com", "?x=1"));
        assert!(find(&pkg).unwrap().is_empty());
    }

    #[test]
    fn untouched_parts_stay_byte_identical() {
        let rels = RELS.replace("&amp;utm_source=chatgpt.com", "");
        let mut pkg = pkg_with(&rels);
        assert!(strip(&mut pkg).unwrap().is_empty());
        assert_eq!(
            pkg.get("word/_rels/document.xml.rels").unwrap(),
            rels.as_bytes()
        );
    }

    #[test]
    fn single_quoted_attributes_and_internal_targets() {
        let rels = r#"<Relationships><Relationship Target='https://a.test/?utm_source=chatgpt.com' TargetMode='External' Id='r1' Type='http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink'/><Relationship Id="r2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://a.test/?utm_source=chatgpt.com"/></Relationships>"#;
        let mut pkg = pkg_with(rels);
        let changed = strip(&mut pkg).unwrap();
        // Only the External one: an internal target is a package part, not a URL.
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].cleaned, "https://a.test/");
    }
}
