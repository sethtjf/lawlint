//! Phrase engine. [agent B]
//!
//! List of `{ regex, message?, suggestion?, fix? }`. Interest: blocks. Run
//! each regex on `block.range.slice(source)`; report at absolute offsets.
//! Optional `allow_context: { pattern, window }`: expand match by `window`
//! bytes each side (clamped to char boundaries); if the pattern matches the
//! expanded slice, skip. A `fix` string on an item makes a MachineApplicable
//! single-edit Fix.

use regex::Regex;

use crate::document::Block;
use crate::error::LoadError;
use crate::loader::{PatternDef, RuleDef};
use crate::rule::{Ctx, Interests, Report, Rule, RuleMeta};
use crate::types::{Applicability, Edit, Fix, TextRange};

/// One compiled `patterns:` item.
#[derive(Debug)]
struct PhraseItem {
    regex: Regex,
    /// Per-item message override; falls back to the rule's default message.
    message: Option<String>,
    suggestion: Option<String>,
    /// Replacement string → MachineApplicable single-edit Fix.
    fix: Option<String>,
}

#[derive(Debug)]
pub struct PhraseEngine {
    meta: RuleMeta,
    items: Vec<PhraseItem>,
    /// Rule-level default message (`message:` in the def, else description).
    default_message: String,
    /// Compiled `allow_context` (regex, window in bytes).
    allow_context: Option<(Regex, usize)>,
}

impl PhraseEngine {
    /// Build from a validated def. Regex compile errors become `LoadError`
    /// with `file` context.
    pub fn from_def(meta: RuleMeta, def: &RuleDef, file: &str) -> Result<Self, LoadError> {
        let mut items = Vec::with_capacity(def.patterns.len());
        for (i, p) in def.patterns.iter().enumerate() {
            let (pattern, message, suggestion, fix) = match p {
                PatternDef::Bare(s) => (s.as_str(), None, None, None),
                PatternDef::Detailed {
                    pattern,
                    message,
                    suggestion,
                    fix,
                } => (
                    pattern.as_str(),
                    message.clone(),
                    suggestion.clone(),
                    fix.clone(),
                ),
            };
            let regex = Regex::new(pattern).map_err(|e| LoadError::InvalidRegex {
                file: file.to_string(),
                field: format!("patterns[{i}]"),
                pattern: pattern.to_string(),
                message: e.to_string(),
            })?;
            items.push(PhraseItem {
                regex,
                message,
                suggestion,
                fix,
            });
        }

        let allow_context = match &def.allow_context {
            Some(ac) => {
                let re = Regex::new(&ac.pattern).map_err(|e| LoadError::InvalidRegex {
                    file: file.to_string(),
                    field: "allow_context.pattern".to_string(),
                    pattern: ac.pattern.clone(),
                    message: e.to_string(),
                })?;
                Some((re, ac.window))
            }
            None => None,
        };

        let default_message = def
            .message
            .clone()
            .unwrap_or_else(|| meta.description.clone());

        Ok(PhraseEngine {
            meta,
            items,
            default_message,
            allow_context,
        })
    }

    /// Adapt `replacement`'s case to `matched`: all-caps match (>= 2 letters,
    /// every letter uppercase) → uppercase the whole replacement; else a match
    /// whose first letter is uppercase → uppercase the replacement's first
    /// char; else verbatim.
    fn match_case(matched: &str, replacement: &str) -> String {
        let mut letters = 0usize;
        let mut all_upper = true;
        let mut first_upper = None;
        for c in matched.chars().filter(|c| c.is_alphabetic()) {
            letters += 1;
            let upper = c.is_uppercase();
            if !upper {
                all_upper = false;
            }
            if first_upper.is_none() {
                first_upper = Some(upper);
            }
        }
        if letters >= 2 && all_upper {
            return replacement.chars().flat_map(char::to_uppercase).collect();
        }
        if first_upper == Some(true) {
            let mut chars = replacement.chars();
            if let Some(first) = chars.next() {
                return first.to_uppercase().chain(chars).collect();
            }
        }
        replacement.to_string()
    }

    /// Expand `[start, end)` by `window` bytes each side, clamped to
    /// `source` char boundaries (and to `[0, source.len()]`).
    fn expand_window(source: &str, start: usize, end: usize, window: usize) -> (usize, usize) {
        let mut lo = start.saturating_sub(window);
        while lo > 0 && !source.is_char_boundary(lo) {
            lo -= 1;
        }
        let mut hi = end.saturating_add(window).min(source.len());
        while hi < source.len() && !source.is_char_boundary(hi) {
            hi += 1;
        }
        (lo, hi)
    }
}

impl Rule for PhraseEngine {
    fn meta(&self) -> &RuleMeta {
        &self.meta
    }

    fn interests(&self) -> Interests {
        Interests {
            blocks: true,
            ..Interests::default()
        }
    }

    fn check_block(&mut self, b: &Block, ctx: &mut Ctx) {
        let source = ctx.source;
        let text = b.range.slice(source);
        for item in &self.items {
            for m in item.regex.find_iter(text) {
                if m.start() == m.end() {
                    continue; // never report empty matches
                }
                let start = b.range.start + m.start();
                let end = b.range.start + m.end();

                if let Some((allow_re, window)) = &self.allow_context {
                    let (lo, hi) = Self::expand_window(source, start, end, *window);
                    if allow_re.is_match(&source[lo..hi]) {
                        continue;
                    }
                }

                let span = TextRange { start, end };
                let fix = item.fix.as_ref().map(|replacement| Fix {
                    edits: vec![Edit {
                        range: span,
                        replacement: Self::match_case(span.slice(source), replacement),
                    }],
                    applicability: Applicability::MachineApplicable,
                });
                ctx.report(Report {
                    span,
                    message: item
                        .message
                        .clone()
                        .unwrap_or_else(|| self.default_message.clone()),
                    suggestion: item.suggestion.clone(),
                    weight: None,
                    fix,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::BlockKind;
    use crate::types::{RuleId, Scope, Severity, Tier};

    fn meta() -> RuleMeta {
        RuleMeta {
            id: RuleId("core/test-phrase".into()),
            tier: Tier::Static,
            scope: Scope::Text,
            severity: Severity::Error,
            description: "default description".into(),
            docs_url: "https://lawlint.com/rules/test-phrase".into(),
            rationale: None,
            examples: vec![],
        }
    }

    fn def(yaml: &str) -> RuleDef {
        serde_yaml::from_str(yaml).expect("test def parses")
    }

    fn block(start: usize, end: usize) -> Block {
        Block {
            kind: BlockKind::Paragraph,
            range: TextRange { start, end },
            sentences: vec![],
        }
    }

    fn run(engine: &mut PhraseEngine, source: &str, b: &Block) -> Vec<Report> {
        let mut ctx = Ctx::new(source, 0);
        engine.check_block(b, &mut ctx);
        ctx.take_reports()
    }

    #[test]
    fn matches_at_absolute_offsets_in_later_block() {
        let source = "Intro paragraph.\n\nWe must delve deeper here.";
        let d = def(r#"
id: test-phrase
engine: phrase
message: "Avoid delve"
patterns:
  - "(?i)\\bdelve\\b"
"#);
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        // Second block only.
        let b = block(18, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(reports.len(), 1);
        let span = reports[0].span;
        assert_eq!(span.slice(source), "delve");
        assert_eq!(span.start, source.find("delve").unwrap());
        assert_eq!(reports[0].message, "Avoid delve");
        assert!(reports[0].suggestion.is_none());
        assert!(reports[0].fix.is_none());
    }

    #[test]
    fn does_not_match_outside_block_range() {
        let source = "delve here.\n\nClean paragraph.";
        let d = def("id: t\nengine: phrase\npatterns: [\"(?i)delve\"]");
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        // Only the second (clean) block is checked.
        let b = block(13, source.len());
        assert!(run(&mut e, source, &b).is_empty());
    }

    #[test]
    fn per_item_message_suggestion_override_and_default_fallback() {
        let source = "We leverage tapestry.";
        let d = def(r#"
id: t
engine: phrase
message: "default msg"
patterns:
  - { pattern: "(?i)\\bleverage\\b", message: "no leverage", suggestion: "use" }
  - "(?i)\\btapestry\\b"
"#);
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(reports.len(), 2);
        let lev = reports
            .iter()
            .find(|r| r.span.slice(source) == "leverage")
            .unwrap();
        assert_eq!(lev.message, "no leverage");
        assert_eq!(lev.suggestion.as_deref(), Some("use"));
        let tap = reports
            .iter()
            .find(|r| r.span.slice(source) == "tapestry")
            .unwrap();
        assert_eq!(tap.message, "default msg");
        assert!(tap.suggestion.is_none());
    }

    #[test]
    fn default_message_falls_back_to_description_when_no_message() {
        let source = ";";
        let d = def("id: t\nengine: phrase\npatterns: [\";\"]");
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(reports[0].message, "default description");
    }

    #[test]
    fn multiple_matches_in_one_block() {
        let source = "one; two; three";
        let d = def("id: t\nengine: phrase\npatterns: [\";\"]");
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].span, TextRange { start: 3, end: 4 });
        assert_eq!(reports[1].span, TextRange { start: 8, end: 9 });
    }

    #[test]
    fn unicode_prefix_offsets_are_byte_accurate() {
        // "café" is 5 bytes; em dash is 3 bytes.
        let source = "café—bar";
        let d = def("id: t\nengine: phrase\npatterns: [\"—\"]");
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].span, TextRange { start: 5, end: 8 });
        assert_eq!(reports[0].span.slice(source), "—");
    }

    #[test]
    fn allow_context_skips_en_dash_in_numeric_range() {
        let source = "The years 1994–2001 were formative. A dash – here is stray.";
        let d = def(r#"
id: no-en-dash
engine: phrase
message: "En dashes belong only in numeric ranges."
patterns: ["–"]
allow_context: { pattern: '\d\s?–\s?\d', window: 8 }
"#);
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        // Numeric-range en dash skipped; stray one reported.
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].span.slice(source), "–");
        assert!(reports[0].span.start > source.find("2001").unwrap());
    }

    #[test]
    fn allow_context_window_clamps_to_char_boundaries() {
        // '–' match starts at byte 9 ("a"=1, "é"=2, "é"=2, "1994"=4).
        // window 7 → lo = 2, mid-'é' (boundaries 0,1,3,5,...) → clamp down to 1.
        // hi side runs into multibyte "éé" as well. Must not panic, and the
        // numeric-range skip must still apply.
        let source = "aéé1994–2001éé tail – x";
        let d = def(
            "id: t\nengine: phrase\npatterns: [\"–\"]\nallow_context: { pattern: '\\d\\s?–\\s?\\d', window: 7 }",
        );
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(reports.len(), 1); // only the trailing stray dash
        assert_eq!(reports[0].span.slice(source), "–");
        assert_eq!(reports[0].span.start, source.rfind('–').unwrap());
    }

    #[test]
    fn allow_context_window_clamped_at_source_edges() {
        // Match at the very start/end of source: window must clamp to [0, len].
        let source = "–1 and 9–";
        let d = def(
            "id: t\nengine: phrase\npatterns: [\"–\"]\nallow_context: { pattern: '\\d–\\d', window: 100 }",
        );
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        // No digit on both sides of either dash → both reported; no panic.
        let reports = run(&mut e, source, &b);
        assert_eq!(reports.len(), 2);
    }

    #[test]
    fn fix_string_emits_machine_applicable_single_edit() {
        let source = "Please delve into it.";
        let d = def(r#"
id: t
engine: phrase
patterns:
  - { pattern: "(?i)\\bdelve\\b", message: "no delve", suggestion: "examine", fix: "examine" }
"#);
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(reports.len(), 1);
        let fix = reports[0].fix.as_ref().expect("fix present");
        assert_eq!(fix.applicability, Applicability::MachineApplicable);
        assert_eq!(fix.edits.len(), 1);
        assert_eq!(fix.edits[0].range, reports[0].span);
        assert_eq!(fix.edits[0].range.slice(source), "delve");
        assert_eq!(fix.edits[0].replacement, "examine");
    }

    #[test]
    fn fix_matches_leading_capital_of_matched_text() {
        let source = "Delve into it.";
        let d = def(r#"
id: t
engine: phrase
patterns:
  - { pattern: "(?i)\\bdelve\\b", fix: "examine" }
"#);
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(
            reports[0].fix.as_ref().unwrap().edits[0].replacement,
            "Examine"
        );
    }

    #[test]
    fn fix_matches_all_caps_of_matched_text() {
        let source = "DELVE into it.";
        let d = def(r#"
id: t
engine: phrase
patterns:
  - { pattern: "(?i)\\bdelve\\b", fix: "examine" }
"#);
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(
            reports[0].fix.as_ref().unwrap().edits[0].replacement,
            "EXAMINE"
        );
    }

    #[test]
    fn fix_lowercase_match_is_verbatim() {
        let source = "we delve into it.";
        let d = def(r#"
id: t
engine: phrase
patterns:
  - { pattern: "(?i)\\bdelve\\b", fix: "examine" }
"#);
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(
            reports[0].fix.as_ref().unwrap().edits[0].replacement,
            "examine"
        );
    }

    #[test]
    fn bad_pattern_regex_is_a_load_error_with_context() {
        let d = def("id: t\nengine: phrase\npatterns: [\"ok\", \"(\"]");
        let err = PhraseEngine::from_def(meta(), &d, "rules/t.yaml").unwrap_err();
        match err {
            LoadError::InvalidRegex {
                file,
                field,
                pattern,
                ..
            } => {
                assert_eq!(file, "rules/t.yaml");
                assert_eq!(field, "patterns[1]");
                assert_eq!(pattern, "(");
            }
            other => panic!("expected InvalidRegex, got {other:?}"),
        }
    }

    #[test]
    fn bad_allow_context_regex_is_a_load_error_with_context() {
        let d = def(
            "id: t\nengine: phrase\npatterns: [\"–\"]\nallow_context: { pattern: \"[\", window: 8 }",
        );
        let err = PhraseEngine::from_def(meta(), &d, "rules/t.yaml").unwrap_err();
        match err {
            LoadError::InvalidRegex { field, pattern, .. } => {
                assert_eq!(field, "allow_context.pattern");
                assert_eq!(pattern, "[");
            }
            other => panic!("expected InvalidRegex, got {other:?}"),
        }
    }

    #[test]
    fn empty_regex_matches_are_never_reported() {
        let source = "abc";
        let d = def("id: t\nengine: phrase\npatterns: [\"x*\"]");
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        assert!(run(&mut e, source, &b).is_empty());
    }

    #[test]
    fn interests_are_blocks_only() {
        let d = def("id: t\nengine: phrase\npatterns: [\"x\"]");
        let e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let i = e.interests();
        assert!(i.blocks && !i.tokens && !i.sentences && !i.document_exit);
        assert_eq!(e.meta().id, RuleId("core/test-phrase".into()));
    }
}
