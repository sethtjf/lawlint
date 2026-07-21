//! Density engine. [agent C]
//!
//! One regex + `threshold` (matches per 1000 words). Interest: blocks +
//! document_exit. Accumulate matches; at exit, fire only if
//! `count/words*1000 > threshold` (threshold overridable via
//! `Ctx::threshold`). Emit ONE report at the first match span with
//! `weight = ceil(count - threshold*words/1000).max(1) as u32` and message
//! suffixed `" (N occurrences in M words)"`. This formula is parity-critical
//! (semantics copied from the old `DensityRule`).

use regex::Regex;

use crate::document::{Block, Document};
use crate::error::LoadError;
use crate::loader::RuleDef;
use crate::rule::{Ctx, Interests, Report, Rule, RuleMeta};
use crate::types::TextRange;

#[derive(Debug)]
pub struct DensityEngine {
    meta: RuleMeta,
    re: Regex,
    default_threshold: f64,
    message: String,
    /// Matches accumulated across `check_block` calls this run.
    count: usize,
    /// Absolute span of the first match seen this run.
    first_span: Option<TextRange>,
}

impl DensityEngine {
    /// Build from a validated def (exactly one pattern + threshold).
    pub fn from_def(meta: RuleMeta, def: &RuleDef, file: &str) -> Result<Self, LoadError> {
        let threshold = def.threshold.ok_or_else(|| LoadError::MissingField {
            file: file.to_string(),
            field: "threshold".to_string(),
        })?;
        if def.patterns.len() != 1 {
            return Err(LoadError::invalid_field(
                file,
                "patterns",
                format!(
                    "a density rule needs exactly one pattern, but {} were given",
                    def.patterns.len()
                ),
            ));
        }
        let pattern = def.patterns[0].pattern();
        let re = Regex::new(pattern).map_err(|e| LoadError::InvalidRegex {
            file: file.to_string(),
            field: "patterns[0]".to_string(),
            pattern: pattern.to_string(),
            message: e.to_string(),
        })?;
        let message = def
            .message
            .clone()
            .unwrap_or_else(|| meta.description.clone());
        Ok(DensityEngine {
            meta,
            re,
            default_threshold: threshold,
            message,
            count: 0,
            first_span: None,
        })
    }
}

impl Rule for DensityEngine {
    fn meta(&self) -> &RuleMeta {
        &self.meta
    }

    fn interests(&self) -> Interests {
        Interests {
            blocks: true,
            document_exit: true,
            ..Interests::default()
        }
    }

    fn check_block(&mut self, b: &Block, ctx: &mut Ctx) {
        let text = b.range.slice(ctx.source);
        for caps in self.re.captures_iter(text) {
            self.count += 1;
            if self.first_span.is_none() {
                // A capture group 1 marks the reportable part of the match —
                // patterns without lookbehind consume leading context (e.g.
                // the whitespace before a parenthetical aside) that must not
                // land in the diagnostic span.
                let m = caps.get(1).or_else(|| caps.get(0)).expect("group 0");
                let start = b.range.start + m.start();
                // Parity with the old engine: guard zero-width matches so the
                // span is at least one byte wide.
                let end = (b.range.start + m.end()).max(start + 1);
                self.first_span = Some(TextRange { start, end });
            }
        }
    }

    fn on_document_exit(&mut self, _doc: &Document, ctx: &mut Ctx) {
        let Some(span) = self.first_span else { return };
        let words = ctx.word_count.max(1);
        let threshold = ctx.threshold(self.default_threshold);
        // Fire only strictly above the threshold (matches per 1000 words).
        if (self.count as f64 / words as f64) * 1000.0 <= threshold {
            return;
        }
        // Parity-critical weight formula (old DensityRule, verbatim).
        let weight =
            (((self.count as f64) - (threshold * words as f64) / 1000.0).ceil() as u32).max(1);
        ctx.report(Report {
            span,
            message: format!(
                "{} ({} occurrences in {} words)",
                self.message, self.count, words
            ),
            suggestion: None,
            weight: Some(weight),
            fix: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::BlockKind;
    use crate::loader::PatternDef;
    use crate::types::{Intent, RuleId, Scope, Severity, Tier};

    fn meta_for(id: &str) -> RuleMeta {
        RuleMeta {
            id: RuleId(format!("core/{id}")),
            tier: Tier::Statistical,
            scope: Scope::Text,
            severity: Severity::Warning,
            intent: Intent::Detection,
            description: "desc".into(),
            docs_url: format!("https://lawlint.com/rules/{id}"),
            rationale: None,
            examples: vec![],
        }
    }

    fn base_def() -> RuleDef {
        RuleDef {
            id: "x".into(),
            engine: "density".into(),
            scope: None,
            severity: None,
            intent: None,
            description: None,
            rationale: None,
            docs: None,
            message: None,
            examples: vec![],
            patterns: vec![],
            allow_context: None,
            threshold: None,
            metric: None,
            params: None,
            direction: None,
            granularity: None,
            rubric: None,
            flag_examples: vec![],
            pass_examples: vec![],
            skill: None,
        }
    }

    fn engine(pattern: &str, threshold: f64, message: &str) -> DensityEngine {
        let mut def = base_def();
        def.patterns = vec![PatternDef::Bare(pattern.into())];
        def.threshold = Some(threshold);
        def.message = Some(message.into());
        DensityEngine::from_def(meta_for("x"), &def, "rules/x.yaml").unwrap()
    }

    fn whole_block(source: &str) -> Block {
        Block {
            kind: BlockKind::Paragraph,
            range: TextRange {
                start: 0,
                end: source.len(),
            },
            sentences: vec![],
        }
    }

    /// Source with `n` occurrences of "perhaps" separated by filler.
    fn hedged(n: usize) -> String {
        std::iter::repeat_n("perhaps", n)
            .collect::<Vec<_>>()
            .join(" so ")
    }

    /// Run engine over a single block covering `source`, with `word_count`
    /// injected, optionally with a threshold override. Returns reports.
    fn run(
        e: &mut DensityEngine,
        source: &str,
        word_count: usize,
        over: Option<f64>,
    ) -> Vec<Report> {
        let mut ctx = Ctx::new(source, word_count);
        ctx.set_threshold_override(over);
        e.check_block(&whole_block(source), &mut ctx);
        e.on_document_exit(&Document { blocks: vec![] }, &mut ctx);
        ctx.take_reports()
    }

    #[test]
    fn from_def_requires_threshold() {
        let mut def = base_def();
        def.patterns = vec![PatternDef::Bare("x".into())];
        let err = DensityEngine::from_def(meta_for("x"), &def, "rules/x.yaml").unwrap_err();
        assert!(matches!(err, LoadError::MissingField { ref field, .. } if field == "threshold"));
    }

    #[test]
    fn from_def_requires_exactly_one_pattern() {
        let mut def = base_def();
        def.threshold = Some(8.0);
        // Zero patterns.
        let err = DensityEngine::from_def(meta_for("x"), &def, "rules/x.yaml").unwrap_err();
        assert!(err.to_string().contains("exactly one pattern"));
        // Two patterns.
        def.patterns = vec![PatternDef::Bare("a".into()), PatternDef::Bare("b".into())];
        let err = DensityEngine::from_def(meta_for("x"), &def, "rules/x.yaml").unwrap_err();
        assert!(err.to_string().contains("2 were given"));
    }

    #[test]
    fn from_def_reports_bad_regex() {
        let mut def = base_def();
        def.threshold = Some(8.0);
        def.patterns = vec![PatternDef::Bare("(".into())];
        let err = DensityEngine::from_def(meta_for("x"), &def, "rules/x.yaml").unwrap_err();
        match err {
            LoadError::InvalidRegex {
                file,
                field,
                pattern,
                ..
            } => {
                assert_eq!(file, "rules/x.yaml");
                assert_eq!(field, "patterns[0]");
                assert_eq!(pattern, "(");
            }
            other => panic!("expected InvalidRegex, got {other:?}"),
        }
    }

    #[test]
    fn no_matches_no_report() {
        let mut e = engine(r"\bnever\b", 8.0, "m");
        assert!(run(&mut e, "plain text without the word", 100, None).is_empty());
    }

    #[test]
    fn at_threshold_boundary_does_not_fire() {
        // count=10, words=1000 → 10 per 1000 == threshold 10 → strictly-above
        // required, so no report.
        let mut e = engine(r"perhaps", 10.0, "m");
        let src = hedged(10);
        assert!(run(&mut e, &src, 1000, None).is_empty());
    }

    #[test]
    fn just_above_threshold_fires_with_weight_one() {
        // count=11, words=1000, threshold=10 → weight = ceil(11 - 10) = 1.
        let mut e = engine(r"perhaps", 10.0, "m");
        let src = hedged(11);
        let reports = run(&mut e, &src, 1000, None);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].weight, Some(1));
    }

    #[test]
    fn weight_arithmetic_exact_integer() {
        // count=10, words=500, threshold=8 → rate 20 > 8;
        // weight = ceil(10 - 8*500/1000) = ceil(6.0) = 6.
        let mut e = engine(r"perhaps", 8.0, "m");
        let src = hedged(10);
        let reports = run(&mut e, &src, 500, None);
        assert_eq!(reports[0].weight, Some(6));
    }

    #[test]
    fn weight_arithmetic_fractional_ceils_up() {
        // count=4, words=150, threshold=10 → rate 26.7 > 10;
        // weight = ceil(4 - 1.5) = ceil(2.5) = 3.
        let mut e = engine(r"perhaps", 10.0, "m");
        let src = hedged(4);
        let reports = run(&mut e, &src, 150, None);
        assert_eq!(reports[0].weight, Some(3));
    }

    #[test]
    fn weight_floors_at_one() {
        // count=1, words=1000, override threshold 0.5 → fires;
        // weight = ceil(1 - 0.5) = 1.
        let mut e = engine(r"perhaps", 10.0, "m");
        let reports = run(&mut e, "perhaps", 1000, Some(0.5));
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].weight, Some(1));
    }

    #[test]
    fn threshold_override_can_silence() {
        // Default 10 would fire (11 in 1000); override 20 silences it.
        let mut e = engine(r"perhaps", 10.0, "m");
        let src = hedged(11);
        assert!(run(&mut e, &src, 1000, Some(20.0)).is_empty());
    }

    #[test]
    fn message_suffix_and_word_display() {
        let mut e = engine(r"perhaps", 1.0, "Reduce hedging.");
        let src = hedged(3);
        let reports = run(&mut e, &src, 100, None);
        assert_eq!(
            reports[0].message,
            "Reduce hedging. (3 occurrences in 100 words)"
        );
    }

    #[test]
    fn report_spans_first_match_absolute_offsets() {
        let source = "aa perhaps bb perhaps";
        let mut e = engine(r"perhaps", 1.0, "m");
        let reports = run(&mut e, source, 4, None);
        assert_eq!(reports[0].span, TextRange { start: 3, end: 10 });
        assert_eq!(reports[0].span.slice(source), "perhaps");
    }

    #[test]
    fn accumulates_across_blocks_first_span_wins() {
        let source = "xx perhaps yy\n\nperhaps perhaps zz";
        let b1 = Block {
            kind: BlockKind::Paragraph,
            range: TextRange { start: 0, end: 13 },
            sentences: vec![],
        };
        let b2 = Block {
            kind: BlockKind::Paragraph,
            range: TextRange {
                start: 15,
                end: source.len(),
            },
            sentences: vec![],
        };
        let mut e = engine(r"perhaps", 1.0, "m");
        let mut ctx = Ctx::new(source, 7);
        e.check_block(&b1, &mut ctx);
        e.check_block(&b2, &mut ctx);
        e.on_document_exit(&Document { blocks: vec![] }, &mut ctx);
        let reports = ctx.take_reports();
        assert_eq!(reports.len(), 1);
        // 3 matches total; first span is in block 1.
        assert!(reports[0].message.contains("(3 occurrences in 7 words)"));
        assert_eq!(reports[0].span, TextRange { start: 3, end: 10 });
    }

    #[test]
    fn zero_word_count_treated_as_one() {
        // words.max(1): fires, displays "in 1 words" (old-engine parity).
        let mut e = engine(r"perhaps", 10.0, "m");
        let reports = run(&mut e, "perhaps", 0, None);
        assert_eq!(reports.len(), 1);
        assert!(reports[0].message.ends_with("(1 occurrences in 1 words)"));
        assert_eq!(reports[0].weight, Some(1));
    }

    #[test]
    fn message_falls_back_to_description_when_absent() {
        let mut def = base_def();
        def.patterns = vec![PatternDef::Bare("perhaps".into())];
        def.threshold = Some(1.0);
        let mut e = DensityEngine::from_def(meta_for("x"), &def, "rules/x.yaml").unwrap();
        let reports = run(&mut e, "perhaps perhaps", 2, None);
        assert!(reports[0].message.starts_with("desc ("));
    }
}
