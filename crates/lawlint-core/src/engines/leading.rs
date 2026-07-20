//! Sentence-opener engine. [agent B]
//!
//! Needle list. Interest: sentences. Case-insensitive match of any needle at
//! sentence start → report the needle span. Always the rule's configured
//! severity (built-ins: error).
//!
//! Needles are regex fragments (matching the old implementation's built-in
//! lists, e.g. `(?:great|good) question`), compiled case-insensitively and
//! anchored to the sentence start. First matching needle wins — one report
//! per sentence.

use regex::Regex;

use crate::document::Sentence;
use crate::error::LoadError;
use crate::loader::{PatternDef, RuleDef};
use crate::rule::{Ctx, Interests, Report, Rule, RuleMeta};
use crate::types::TextRange;

/// One compiled needle (+ per-needle message/suggestion overrides).
#[derive(Debug)]
struct LeadingItem {
    /// Compiled as `(?i)^(?:<needle>)`.
    regex: Regex,
    message: Option<String>,
    suggestion: Option<String>,
}

#[derive(Debug)]
pub struct LeadingEngine {
    meta: RuleMeta,
    items: Vec<LeadingItem>,
    /// Rule-level default message (`message:` in the def, else description).
    default_message: String,
}

impl LeadingEngine {
    /// Build from a validated def. Regex compile errors become `LoadError`
    /// with `file` context.
    pub fn from_def(meta: RuleMeta, def: &RuleDef, file: &str) -> Result<Self, LoadError> {
        let mut items = Vec::with_capacity(def.patterns.len());
        for (i, p) in def.patterns.iter().enumerate() {
            let (needle, message, suggestion) = match p {
                PatternDef::Bare(s) => (s.as_str(), None, None),
                PatternDef::Detailed {
                    pattern,
                    message,
                    suggestion,
                    ..
                } => (pattern.as_str(), message.clone(), suggestion.clone()),
            };
            let anchored = format!("(?i)^(?:{needle})");
            let regex = Regex::new(&anchored).map_err(|e| LoadError::InvalidRegex {
                file: file.to_string(),
                field: format!("patterns[{i}]"),
                pattern: needle.to_string(),
                message: e.to_string(),
            })?;
            items.push(LeadingItem {
                regex,
                message,
                suggestion,
            });
        }

        let default_message = def
            .message
            .clone()
            .unwrap_or_else(|| meta.description.clone());

        Ok(LeadingEngine {
            meta,
            items,
            default_message,
        })
    }
}

impl Rule for LeadingEngine {
    fn meta(&self) -> &RuleMeta {
        &self.meta
    }

    fn interests(&self) -> Interests {
        Interests {
            sentences: true,
            ..Interests::default()
        }
    }

    fn check_sentence(&mut self, s: &Sentence, ctx: &mut Ctx) {
        let source = ctx.source;
        let slice = s.range.slice(source);
        // Tolerate sentence ranges that include leading whitespace.
        let trimmed = slice.trim_start();
        let base = s.range.start + (slice.len() - trimmed.len());
        for item in &self.items {
            if let Some(m) = item.regex.find(trimmed) {
                if m.end() == 0 {
                    continue; // never report an empty needle match
                }
                debug_assert_eq!(m.start(), 0);
                ctx.report(Report {
                    span: TextRange {
                        start: base,
                        end: base + m.end(),
                    },
                    message: item
                        .message
                        .clone()
                        .unwrap_or_else(|| self.default_message.clone()),
                    suggestion: item.suggestion.clone(),
                    weight: None,
                    fix: None,
                });
                return; // one report per sentence: first matching needle wins
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Intent, RuleId, Scope, Severity, Tier};

    fn meta() -> RuleMeta {
        RuleMeta {
            id: RuleId("core/test-leading".into()),
            tier: Tier::Static,
            scope: Scope::Text,
            severity: Severity::Error,
            intent: Intent::Detection,
            description: "default description".into(),
            docs_url: "https://lawlint.com/rules/test-leading".into(),
            rationale: None,
            examples: vec![],
        }
    }

    fn def(yaml: &str) -> RuleDef {
        serde_yaml::from_str(yaml).expect("test def parses")
    }

    fn sentence(start: usize, end: usize) -> Sentence {
        Sentence {
            range: TextRange { start, end },
            tokens: vec![],
            is_citation: false,
        }
    }

    fn run(engine: &mut LeadingEngine, source: &str, s: &Sentence) -> Vec<Report> {
        let mut ctx = Ctx::new(source, 0);
        engine.check_sentence(s, &mut ctx);
        ctx.take_reports()
    }

    fn sycophantic() -> LeadingEngine {
        let d = def(r#"
id: no-sycophantic-openers
engine: leading
message: "Skip the sycophantic opener and start with the substance."
patterns:
  - "(?:great|good|excellent|fantastic|wonderful) question"
  - "what a (?:great|fascinating|wonderful|excellent|interesting) (?:question|problem|point)"
"#);
        LeadingEngine::from_def(meta(), &d, "t.yaml").unwrap()
    }

    #[test]
    fn matches_needle_case_insensitively_at_sentence_start() {
        let source = "Great question! The answer follows.";
        let mut e = sycophantic();
        let s = sentence(0, 15);
        let reports = run(&mut e, source, &s);
        assert_eq!(reports.len(), 1);
        // Span is the needle, not the whole sentence.
        assert_eq!(reports[0].span, TextRange { start: 0, end: 14 });
        assert_eq!(reports[0].span.slice(source), "Great question");
        assert_eq!(
            reports[0].message,
            "Skip the sycophantic opener and start with the substance."
        );
    }

    #[test]
    fn does_not_match_mid_sentence() {
        let source = "That was a great question to raise.";
        let mut e = sycophantic();
        let s = sentence(0, source.len());
        assert!(run(&mut e, source, &s).is_empty());
    }

    #[test]
    fn reports_absolute_offsets_for_later_sentences() {
        let source = "Fine. Good question, indeed.";
        let mut e = sycophantic();
        let s = sentence(6, source.len());
        let reports = run(&mut e, source, &s);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].span, TextRange { start: 6, end: 19 });
        assert_eq!(reports[0].span.slice(source), "Good question");
    }

    #[test]
    fn tolerates_leading_whitespace_in_sentence_range() {
        let source = "First.   excellent question follows.";
        let mut e = sycophantic();
        // Range starts at the whitespace after "First."
        let s = sentence(6, source.len());
        let reports = run(&mut e, source, &s);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].span.slice(source), "excellent question");
        assert_eq!(reports[0].span.start, source.find("excellent").unwrap());
    }

    #[test]
    fn unicode_needle_with_curly_apostrophe() {
        let source = "Here’s my take on damages.";
        let d = def(r#"
id: no-throat-clearing
engine: leading
message: "Cut the throat-clearing and lead with the point."
patterns:
  - "here['’]s my take"
"#);
        let mut e = LeadingEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let s = sentence(0, source.len());
        let reports = run(&mut e, source, &s);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].span.slice(source), "Here’s my take");
        // Byte-accurate end: "Here" (4) + ’ (3) + "s my take" (9).
        assert_eq!(reports[0].span, TextRange { start: 0, end: 16 });
    }

    #[test]
    fn per_item_message_suggestion_override_and_default_fallback() {
        let source = "Wonderful question.";
        let d = def(r#"
id: t
engine: leading
message: "default msg"
patterns:
  - { pattern: "wonderful question", message: "custom msg", suggestion: "cut it" }
  - "great question"
"#);
        let mut e = LeadingEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let s = sentence(0, source.len());
        let reports = run(&mut e, source, &s);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].message, "custom msg");
        assert_eq!(reports[0].suggestion.as_deref(), Some("cut it"));

        // Bare needle falls back to the rule-level message, no suggestion.
        let source2 = "Great question.";
        let reports2 = run(&mut e, source2, &sentence(0, source2.len()));
        assert_eq!(reports2.len(), 1);
        assert_eq!(reports2[0].message, "default msg");
        assert!(reports2[0].suggestion.is_none());
    }

    #[test]
    fn default_message_falls_back_to_description() {
        let source = "Great question.";
        let d = def("id: t\nengine: leading\npatterns: [\"great question\"]");
        let mut e = LeadingEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let reports = run(&mut e, source, &sentence(0, source.len()));
        assert_eq!(reports[0].message, "default description");
    }

    #[test]
    fn first_matching_needle_wins_single_report() {
        let source = "Great question about torts.";
        let d =
            def("id: t\nengine: leading\npatterns: [\"great question\", \"great question about\"]");
        let mut e = LeadingEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let reports = run(&mut e, source, &sentence(0, source.len()));
        assert_eq!(reports.len(), 1);
        // First listed needle wins, even though the second also matches.
        assert_eq!(reports[0].span.slice(source), "Great question");
    }

    #[test]
    fn optional_needle_group_matching_empty_is_not_reported() {
        let source = "Anything at all.";
        let d = def("id: t\nengine: leading\npatterns: [\"(?:never)?\"]");
        let mut e = LeadingEngine::from_def(meta(), &d, "t.yaml").unwrap();
        assert!(run(&mut e, source, &sentence(0, source.len())).is_empty());
    }

    #[test]
    fn bad_needle_regex_is_a_load_error_with_context() {
        let d = def("id: t\nengine: leading\npatterns: [\"fine\", \"(\"]");
        let err = LeadingEngine::from_def(meta(), &d, "rules/t.yaml").unwrap_err();
        match err {
            LoadError::InvalidRegex {
                file,
                field,
                pattern,
                ..
            } => {
                assert_eq!(file, "rules/t.yaml");
                assert_eq!(field, "patterns[1]");
                // Error carries the author's needle, not the anchored wrapper.
                assert_eq!(pattern, "(");
            }
            other => panic!("expected InvalidRegex, got {other:?}"),
        }
    }

    #[test]
    fn interests_are_sentences_only() {
        let d = def("id: t\nengine: leading\npatterns: [\"x\"]");
        let e = LeadingEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let i = e.interests();
        assert!(i.sentences && !i.tokens && !i.blocks && !i.document_exit);
        assert_eq!(e.meta().id, RuleId("core/test-leading".into()));
    }
}
