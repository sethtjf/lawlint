//! Statistical (metric) engine. [agent C]
//!
//! `metric` + `params`:
//! - `sentence-length`: params `max_words` (default 45, overridable via
//!   thresholds). Per sentence: count Word+Number tokens; over max → report
//!   sentence span.
//! - `repetitive-openers`: params `run_length` (default 3). Track consecutive
//!   sentences (within a block) sharing the same lowercased first word token;
//!   on reaching run_length → report the run's last sentence span; reset
//!   after firing.
//!
//! Metric set is non-exhaustive; an unknown metric is a load error.

use crate::document::{Block, Sentence, TokenKind};
use crate::error::LoadError;
use crate::loader::RuleDef;
use crate::rule::{Ctx, Interests, Report, Rule, RuleMeta};

/// Known statistical metrics. Non-exhaustive extension point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Metric {
    SentenceLength,
    RepetitiveOpeners,
}

impl Metric {
    /// Parse a YAML `metric:` value. `None` for unknown metrics (load error).
    pub fn parse(s: &str) -> Option<Metric> {
        match s {
            "sentence-length" => Some(Metric::SentenceLength),
            "repetitive-openers" => Some(Metric::RepetitiveOpeners),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct StatisticalEngine {
    meta: RuleMeta,
    metric: Metric,
    /// sentence-length: default max word count (thresholds-overridable).
    max_words: f64,
    /// repetitive-openers: run length that triggers a report.
    run_length: usize,
    /// repetitive-openers state: current run's lowercased opener word.
    opener: Option<String>,
    /// repetitive-openers state: length of the current run.
    run: usize,
}

impl StatisticalEngine {
    /// Build from a validated def (known metric + params).
    pub fn from_def(meta: RuleMeta, def: &RuleDef, file: &str) -> Result<Self, LoadError> {
        let metric_str = def
            .metric
            .as_deref()
            .ok_or_else(|| LoadError::MissingField {
                file: file.to_string(),
                field: "metric".to_string(),
            })?;
        let metric = Metric::parse(metric_str).ok_or_else(|| {
            LoadError::invalid_field(
                file,
                "metric",
                format!(
                    "{metric_str:?} is not a known metric — use sentence-length or repetitive-openers"
                ),
            )
        })?;
        let param = |name: &str| def.params.as_ref().and_then(|p| p.get(name)).copied();
        let max_words = param("max_words").unwrap_or(45.0);
        if !max_words.is_finite() || max_words < 1.0 {
            return Err(LoadError::invalid_field(
                file,
                "params.max_words",
                format!(
                    "{max_words} is not a valid sentence length — use a positive number of \
                     words like 45"
                ),
            ));
        }
        let run_length_raw = param("run_length").unwrap_or(3.0);
        if !run_length_raw.is_finite() || run_length_raw < 1.0 || run_length_raw.fract() != 0.0 {
            return Err(LoadError::invalid_field(
                file,
                "params.run_length",
                format!(
                    "{run_length_raw} is not a valid run length — use a whole number of at least 1"
                ),
            ));
        }
        Ok(StatisticalEngine {
            meta,
            metric,
            max_words,
            run_length: run_length_raw as usize,
            opener: None,
            run: 0,
        })
    }

    fn reset_run(&mut self) {
        self.opener = None;
        self.run = 0;
    }
}

/// Spell out small run lengths for the report message ("Three consecutive
/// sentences begin with …" — old-engine message parity at the default 3).
fn spelled(n: usize) -> String {
    match n {
        2 => "Two".to_string(),
        3 => "Three".to_string(),
        4 => "Four".to_string(),
        5 => "Five".to_string(),
        6 => "Six".to_string(),
        7 => "Seven".to_string(),
        8 => "Eight".to_string(),
        9 => "Nine".to_string(),
        10 => "Ten".to_string(),
        _ => n.to_string(),
    }
}

impl Rule for StatisticalEngine {
    fn meta(&self) -> &RuleMeta {
        &self.meta
    }

    fn interests(&self) -> Interests {
        // blocks: repetitive-openers runs reset at block boundaries.
        Interests {
            sentences: true,
            blocks: true,
            ..Interests::default()
        }
    }

    fn check_block(&mut self, _b: &Block, _ctx: &mut Ctx) {
        // A new block never continues an opener run from the previous one.
        self.reset_run();
    }

    fn check_sentence(&mut self, s: &Sentence, ctx: &mut Ctx) {
        match self.metric {
            Metric::SentenceLength => {
                let n = s
                    .tokens
                    .iter()
                    .filter(|t| matches!(t.kind, TokenKind::Word | TokenKind::Number))
                    .count();
                let max = ctx.threshold(self.max_words);
                if n as f64 > max {
                    ctx.report(Report {
                        span: s.range,
                        message: format!("Sentence is {n} words; consider shortening it."),
                        suggestion: None,
                        weight: None,
                        fix: None,
                    });
                }
            }
            Metric::RepetitiveOpeners => {
                let first_word = s
                    .tokens
                    .iter()
                    .find(|t| t.kind == TokenKind::Word)
                    .map(|t| t.range.slice(ctx.source).to_lowercase());
                let Some(word) = first_word else {
                    // No word token (e.g. bare number/punctuation): break run.
                    self.reset_run();
                    return;
                };
                if self.opener.as_deref() == Some(word.as_str()) {
                    self.run += 1;
                } else {
                    self.opener = Some(word.clone());
                    self.run = 1;
                }
                if self.run >= self.run_length {
                    ctx.report(Report {
                        span: s.range,
                        message: format!(
                            "{} consecutive sentences begin with “{}”.",
                            spelled(self.run_length),
                            word
                        ),
                        suggestion: None,
                        weight: None,
                        fix: None,
                    });
                    // Reset after firing: the next same-opener sentence starts
                    // a fresh run of 1.
                    self.reset_run();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{BlockKind, Token};
    use crate::types::{RuleId, Scope, Severity, TextRange, Tier};
    use std::collections::HashMap;

    fn meta_for(id: &str) -> RuleMeta {
        RuleMeta {
            id: RuleId(format!("core/{id}")),
            tier: Tier::Statistical,
            scope: Scope::Text,
            severity: Severity::Warning,
            description: "desc".into(),
            docs_url: format!("https://lawlint.com/rules/{id}"),
            rationale: None,
            examples: vec![],
        }
    }

    fn base_def(metric: Option<&str>, params: Option<HashMap<String, f64>>) -> RuleDef {
        RuleDef {
            id: "x".into(),
            engine: "statistical".into(),
            scope: None,
            severity: None,
            description: None,
            rationale: None,
            docs: None,
            message: None,
            examples: vec![],
            patterns: vec![],
            allow_context: None,
            threshold: None,
            metric: metric.map(|m| m.to_string()),
            params,
            granularity: None,
            rubric: None,
            flag_examples: vec![],
            pass_examples: vec![],
        }
    }

    fn engine(metric: &str, params: Option<HashMap<String, f64>>) -> StatisticalEngine {
        StatisticalEngine::from_def(
            meta_for("x"),
            &base_def(Some(metric), params),
            "rules/x.yaml",
        )
        .unwrap()
    }

    /// Test-only tokenizer: ASCII words/numbers/punct with byte ranges.
    fn build_sentence(source: &str, start: usize, end: usize) -> Sentence {
        let b = source.as_bytes();
        let mut tokens = vec![];
        let mut i = start;
        while i < end {
            let c = b[i];
            if c.is_ascii_whitespace() {
                i += 1;
            } else if c.is_ascii_alphabetic() || c == b'\'' {
                let mut j = i;
                while j < end && (b[j].is_ascii_alphabetic() || b[j] == b'\'') {
                    j += 1;
                }
                tokens.push(Token {
                    range: TextRange { start: i, end: j },
                    kind: TokenKind::Word,
                });
                i = j;
            } else if c.is_ascii_digit() {
                let mut j = i;
                while j < end && b[j].is_ascii_digit() {
                    j += 1;
                }
                tokens.push(Token {
                    range: TextRange { start: i, end: j },
                    kind: TokenKind::Number,
                });
                i = j;
            } else {
                tokens.push(Token {
                    range: TextRange {
                        start: i,
                        end: i + 1,
                    },
                    kind: TokenKind::Punct,
                });
                i += 1;
            }
        }
        Sentence {
            range: TextRange { start, end },
            tokens,
            is_citation: false,
        }
    }

    /// Split `source` on `.` into sentences (test-only, no legal awareness).
    fn sentences(source: &str) -> Vec<Sentence> {
        let b = source.as_bytes();
        let mut out = vec![];
        let mut start = 0;
        let mut i = 0;
        while i < source.len() {
            if b[i] == b'.' {
                out.push(build_sentence(source, start, i + 1));
                i += 1;
                while i < source.len() && b[i] == b' ' {
                    i += 1;
                }
                start = i;
            } else {
                i += 1;
            }
        }
        if start < source.len() {
            out.push(build_sentence(source, start, source.len()));
        }
        out
    }

    fn run_over(e: &mut StatisticalEngine, source: &str, over: Option<f64>) -> Vec<Report> {
        let mut ctx = Ctx::new(source, 0);
        ctx.set_threshold_override(over);
        for s in sentences(source) {
            e.check_sentence(&s, &mut ctx);
        }
        ctx.take_reports()
    }

    // --- from_def ---

    #[test]
    fn from_def_requires_metric() {
        let err = StatisticalEngine::from_def(meta_for("x"), &base_def(None, None), "rules/x.yaml")
            .unwrap_err();
        assert!(matches!(err, LoadError::MissingField { ref field, .. } if field == "metric"));
    }

    #[test]
    fn from_def_rejects_unknown_metric() {
        let err = StatisticalEngine::from_def(
            meta_for("x"),
            &base_def(Some("word-salad"), None),
            "rules/x.yaml",
        )
        .unwrap_err();
        let s = err.to_string();
        assert!(s.contains("rules/x.yaml"));
        assert!(s.contains("word-salad"));
        assert!(s.contains("sentence-length or repetitive-openers"));
    }

    #[test]
    fn from_def_rejects_bad_run_length() {
        for bad in [0.0, -1.0, 2.5] {
            let params = HashMap::from([("run_length".to_string(), bad)]);
            let err = StatisticalEngine::from_def(
                meta_for("x"),
                &base_def(Some("repetitive-openers"), Some(params)),
                "rules/x.yaml",
            )
            .unwrap_err();
            assert!(
                err.to_string().contains("run_length"),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn from_def_rejects_bad_max_words() {
        for bad in [0.0, -5.0, f64::NAN, f64::INFINITY] {
            let params = HashMap::from([("max_words".to_string(), bad)]);
            let err = StatisticalEngine::from_def(
                meta_for("x"),
                &base_def(Some("sentence-length"), Some(params)),
                "rules/x.yaml",
            )
            .unwrap_err();
            assert!(
                err.to_string().contains("max_words"),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn from_def_rejects_non_finite_run_length() {
        for bad in [f64::NAN, f64::INFINITY] {
            let params = HashMap::from([("run_length".to_string(), bad)]);
            let err = StatisticalEngine::from_def(
                meta_for("x"),
                &base_def(Some("repetitive-openers"), Some(params)),
                "rules/x.yaml",
            )
            .unwrap_err();
            assert!(
                err.to_string().contains("run_length"),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn metric_parse_known_and_unknown() {
        assert_eq!(
            Metric::parse("sentence-length"),
            Some(Metric::SentenceLength)
        );
        assert_eq!(
            Metric::parse("repetitive-openers"),
            Some(Metric::RepetitiveOpeners)
        );
        assert_eq!(Metric::parse("nope"), None);
    }

    // --- sentence-length ---

    #[test]
    fn sentence_length_boundary_at_default_45() {
        let mut e = engine("sentence-length", None);
        // Exactly 45 words: not over the max, no report.
        let at = format!("{}.", vec!["w"; 45].join(" "));
        assert!(run_over(&mut e, &at, None).is_empty());
        // 46 words: fires, reporting the sentence span.
        let over = format!("{}.", vec!["w"; 46].join(" "));
        let reports = run_over(&mut e, &over, None);
        assert_eq!(reports.len(), 1);
        assert_eq!(
            reports[0].span,
            TextRange {
                start: 0,
                end: over.len()
            }
        );
        assert_eq!(
            reports[0].message,
            "Sentence is 46 words; consider shortening it."
        );
    }

    #[test]
    fn sentence_length_counts_words_and_numbers_not_punct() {
        let params = HashMap::from([("max_words".to_string(), 3.0)]);
        let mut e = engine("sentence-length", Some(params));
        // 2 words + 1 number = 3 countable tokens (punct excluded): at max, silent.
        assert!(run_over(&mut e, "pay 100 dollars.", None).is_empty());
        // 3 words + 1 number = 4 countable tokens > 3: fires; the comma and
        // period do not count.
        let mut e = engine(
            "sentence-length",
            Some(HashMap::from([("max_words".to_string(), 3.0)])),
        );
        let reports = run_over(&mut e, "pay 100 dollars, now.", None);
        assert_eq!(reports.len(), 1);
        assert_eq!(
            reports[0].message,
            "Sentence is 4 words; consider shortening it."
        );
    }

    #[test]
    fn sentence_length_threshold_override_wins() {
        let mut e = engine("sentence-length", None);
        let src = "one two three four five six.";
        // Default 45: silent. Override 5: 6 words fires.
        assert!(run_over(&mut e, src, None).is_empty());
        let mut e = engine("sentence-length", None);
        let reports = run_over(&mut e, src, Some(5.0));
        assert_eq!(reports.len(), 1);
        assert_eq!(
            reports[0].message,
            "Sentence is 6 words; consider shortening it."
        );
    }

    #[test]
    fn sentence_length_max_words_param_used_as_default() {
        let params = HashMap::from([("max_words".to_string(), 2.0)]);
        let mut e = engine("sentence-length", Some(params));
        let reports = run_over(&mut e, "one two three.", None);
        assert_eq!(reports.len(), 1);
    }

    #[test]
    fn sentence_length_reports_each_long_sentence() {
        let params = HashMap::from([("max_words".to_string(), 2.0)]);
        let mut e = engine("sentence-length", Some(params));
        let reports = run_over(&mut e, "one two three. four. five six seven.", None);
        assert_eq!(reports.len(), 2);
    }

    // --- repetitive-openers ---

    #[test]
    fn repetitive_openers_fires_on_third_reports_last_sentence() {
        let mut e = engine("repetitive-openers", None);
        let src = "The cat sat. The dog ran. The bird flew.";
        let reports = run_over(&mut e, src, None);
        assert_eq!(reports.len(), 1);
        // Span is the LAST sentence of the run.
        assert_eq!(reports[0].span.slice(src), "The bird flew.");
        assert_eq!(
            reports[0].message,
            "Three consecutive sentences begin with “the”."
        );
    }

    #[test]
    fn repetitive_openers_case_insensitive() {
        let mut e = engine("repetitive-openers", None);
        let src = "THE cat sat. the dog ran. The bird flew.";
        let reports = run_over(&mut e, src, None);
        assert_eq!(reports.len(), 1);
        assert!(reports[0].message.contains("“the”"));
    }

    #[test]
    fn repetitive_openers_two_is_not_enough() {
        let mut e = engine("repetitive-openers", None);
        assert!(run_over(&mut e, "The cat sat. The dog ran. A bird flew.", None).is_empty());
    }

    #[test]
    fn repetitive_openers_resets_after_firing() {
        let mut e = engine("repetitive-openers", None);
        // 5 consecutive "The" sentences: fires once (at #3), then run restarts
        // at #4-5 which only reaches 2.
        let src = "The a. The b. The c. The d. The e.";
        let reports = run_over(&mut e, src, None);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].span.slice(src), "The c.");
        // 6 consecutive: fires at #3 and again at #6.
        let mut e = engine("repetitive-openers", None);
        let src = "The a. The b. The c. The d. The e. The f.";
        let reports = run_over(&mut e, src, None);
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[1].span.slice(src), "The f.");
    }

    #[test]
    fn repetitive_openers_different_opener_resets_run() {
        let mut e = engine("repetitive-openers", None);
        let src = "The a. The b. A c. The d. The e.";
        assert!(run_over(&mut e, src, None).is_empty());
    }

    #[test]
    fn repetitive_openers_block_boundary_resets_run() {
        let mut e = engine("repetitive-openers", None);
        let src = "The a. The b. The c.";
        let sents = sentences(src);
        let block = Block {
            kind: BlockKind::Paragraph,
            range: TextRange {
                start: 0,
                end: src.len(),
            },
            sentences: vec![],
        };
        let mut ctx = Ctx::new(src, 0);
        // Two sentences in one block, then a new block, then the third.
        e.check_block(&block, &mut ctx);
        e.check_sentence(&sents[0], &mut ctx);
        e.check_sentence(&sents[1], &mut ctx);
        e.check_block(&block, &mut ctx);
        e.check_sentence(&sents[2], &mut ctx);
        assert!(ctx.take_reports().is_empty());
    }

    #[test]
    fn repetitive_openers_wordless_sentence_breaks_run() {
        let mut e = engine("repetitive-openers", None);
        let src = "The a. The b. 42. The c.";
        assert!(run_over(&mut e, src, None).is_empty());
    }

    #[test]
    fn repetitive_openers_custom_run_length() {
        let params = HashMap::from([("run_length".to_string(), 2.0)]);
        let mut e = engine("repetitive-openers", Some(params));
        let src = "The cat sat. The dog ran.";
        let reports = run_over(&mut e, src, None);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].span.slice(src), "The dog ran.");
        assert_eq!(
            reports[0].message,
            "Two consecutive sentences begin with “the”."
        );
    }

    #[test]
    fn spelled_numbers() {
        assert_eq!(spelled(2), "Two");
        assert_eq!(spelled(3), "Three");
        assert_eq!(spelled(10), "Ten");
        assert_eq!(spelled(11), "11");
    }
}
