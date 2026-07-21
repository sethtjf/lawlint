//! Statistical (metric) engine. [agent C]
//!
//! Per-sentence metrics (`metric` + `params`):
//! - `sentence-length`: params `max_words` (default 45, overridable via
//!   thresholds). Per sentence: count Word+Number tokens; over max → report
//!   sentence span.
//! - `repetitive-openers`: params `run_length` (default 3). Track consecutive
//!   sentences (within a block) sharing the same lowercased first word token;
//!   on reaching run_length → report the run's last sentence span; reset
//!   after firing.
//!
//! Document-level metrics (#37; `metric` + `threshold` + `direction`): one
//! measured value per document, computed by `metric_value` over non-code
//! blocks; the rule flags when the value falls on the `direction` side of
//! `threshold` and emits ONE diagnostic anchored on the first sentence, with
//! the measured value in the message. Thresholds are data (tuned on the eval
//! corpus train split); the computations are code.
//!
//! Metric set is non-exhaustive; an unknown metric is a load error.

use std::sync::OnceLock;

use regex::Regex;

use crate::document::{Block, BlockKind, Document, Sentence, TokenKind};
use crate::error::LoadError;
use crate::loader::RuleDef;
use crate::rule::{Ctx, Interests, Report, Rule, RuleMeta};

/// Known statistical metrics. Non-exhaustive extension point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Metric {
    SentenceLength,
    RepetitiveOpeners,
    /// Burstiness: population variance of per-sentence word counts.
    SentenceLengthVariance,
    /// Lag-1 autocorrelation of the sentence-length sequence.
    CadenceAutocorrelation,
    /// Fraction of adjacent same-block sentence pairs sharing an opener word.
    RepeatedOpenerDensity,
    /// "A, B, and C" three-part constructions per 1000 words.
    TriadDensity,
    /// Lowercase "x and y" pairs per 1000 words.
    PairedAdjectiveRate,
}

impl Metric {
    /// Parse a YAML `metric:` value. `None` for unknown metrics (load error).
    pub fn parse(s: &str) -> Option<Metric> {
        match s {
            "sentence-length" => Some(Metric::SentenceLength),
            "repetitive-openers" => Some(Metric::RepetitiveOpeners),
            "sentence-length-variance" => Some(Metric::SentenceLengthVariance),
            "cadence-autocorrelation" => Some(Metric::CadenceAutocorrelation),
            "repeated-opener-density" => Some(Metric::RepeatedOpenerDensity),
            "triad-density" => Some(Metric::TriadDensity),
            "paired-adjective-rate" => Some(Metric::PairedAdjectiveRate),
            _ => None,
        }
    }

    /// Valid `metric:` values for load-error messages (loader + engine).
    pub fn alternatives() -> &'static str {
        "sentence-length, repetitive-openers, sentence-length-variance, \
         cadence-autocorrelation, repeated-opener-density, triad-density, or \
         paired-adjective-rate"
    }

    /// Document-level metrics measure once per document (threshold +
    /// direction in YAML); the rest report per sentence.
    pub fn is_document_level(self) -> bool {
        !matches!(self, Metric::SentenceLength | Metric::RepetitiveOpeners)
    }
}

/// Which side of `threshold` flags for a document-level metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Above,
    Below,
}

impl Direction {
    pub fn parse(s: &str) -> Option<Direction> {
        match s {
            "above" => Some(Direction::Above),
            "below" => Some(Direction::Below),
            _ => None,
        }
    }
}

// ---- Document-level metric computation (#37) ---------------------------
//
// Shared verbatim by the engine and the eval-side threshold tuner
// (`lawlint-eval/src/bin/tune_statistical.rs`): the values tuned on the
// corpus must be the values the rules act on.

/// Rhythm metrics (variance, autocorrelation, opener density) need a real
/// sequence; below this many sentences the value is noise, not cadence.
const MIN_RHYTHM_SENTENCES: usize = 8;

/// Rate metrics (per 1000 words) need a denominator; below this many words a
/// single match dominates the rate.
const MIN_RATE_WORDS: usize = 50;

fn sentence_words(s: &Sentence) -> usize {
    s.tokens
        .iter()
        .filter(|t| matches!(t.kind, TokenKind::Word | TokenKind::Number))
        .count()
}

fn opener_word<'a>(s: &Sentence, source: &'a str) -> Option<&'a str> {
    s.tokens
        .iter()
        .find(|t| t.kind == TokenKind::Word)
        .map(|t| t.range.slice(source))
}

/// Non-code blocks in document order. Citation sentences are included: they
/// are part of the document's rhythm (and a large part of human legal prose).
fn text_blocks(doc: &Document) -> impl Iterator<Item = &Block> {
    doc.blocks.iter().filter(|b| b.kind != BlockKind::CodeBlock)
}

fn mean(xs: &[f64]) -> f64 {
    xs.iter().sum::<f64>() / xs.len() as f64
}

/// "A, B, and C" three-part list, list items up to four words each (the same
/// shape `core/no-rule-of-three` matches lexically).
fn triad_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b\w+(?:\s+\w+){0,3},\s+\w+(?:\s+\w+){0,3},\s+and\s+\w+")
            .expect("triad regex compiles")
    })
}

/// Balanced lowercase "x and y" pair ("clear and convincing" shape). Case
/// sensitive on purpose: capitalized operands are usually party names or
/// defined terms, not stylistic balance.
fn pair_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\b[a-z][a-z'’-]{2,}\s+and\s+[a-z][a-z'’-]{2,}\b").expect("pair regex compiles")
    })
}

/// Measure a document-level metric. `None` when the document is too short for
/// the metric to mean anything (the rule then stays silent) or the metric is
/// per-sentence.
pub fn metric_value(metric: Metric, source: &str, doc: &Document) -> Option<f64> {
    let lengths: Vec<f64> = text_blocks(doc)
        .flat_map(|b| b.sentences.iter())
        .map(|s| sentence_words(s) as f64)
        .collect();
    let words: f64 = lengths.iter().sum();
    match metric {
        Metric::SentenceLength | Metric::RepetitiveOpeners => None,
        Metric::SentenceLengthVariance => {
            if lengths.len() < MIN_RHYTHM_SENTENCES {
                return None;
            }
            let m = mean(&lengths);
            Some(lengths.iter().map(|x| (x - m).powi(2)).sum::<f64>() / lengths.len() as f64)
        }
        Metric::CadenceAutocorrelation => {
            if lengths.len() < MIN_RHYTHM_SENTENCES {
                return None;
            }
            let m = mean(&lengths);
            let denom: f64 = lengths.iter().map(|x| (x - m).powi(2)).sum();
            if denom == 0.0 {
                return None; // constant rhythm: correlation undefined
            }
            let num: f64 = lengths.windows(2).map(|w| (w[0] - m) * (w[1] - m)).sum();
            Some(num / denom)
        }
        Metric::RepeatedOpenerDensity => {
            if lengths.len() < MIN_RHYTHM_SENTENCES {
                return None;
            }
            let mut repeats = 0usize;
            let mut transitions = 0usize;
            for block in text_blocks(doc) {
                let openers: Vec<String> = block
                    .sentences
                    .iter()
                    .filter_map(|s| opener_word(s, source).map(str::to_lowercase))
                    .collect();
                for w in openers.windows(2) {
                    transitions += 1;
                    if w[0] == w[1] {
                        repeats += 1;
                    }
                }
            }
            if transitions == 0 {
                return None;
            }
            Some(repeats as f64 / transitions as f64)
        }
        Metric::TriadDensity | Metric::PairedAdjectiveRate => {
            if words < MIN_RATE_WORDS as f64 {
                return None;
            }
            let re = if metric == Metric::TriadDensity {
                triad_regex()
            } else {
                pair_regex()
            };
            let matches: usize = text_blocks(doc)
                .flat_map(|b| b.sentences.iter())
                .map(|s| re.find_iter(s.range.slice(source)).count())
                .sum();
            Some(matches as f64 / words * 1000.0)
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
    /// document-level metrics: flag boundary (thresholds-overridable).
    threshold: f64,
    /// document-level metrics: side of `threshold` that flags.
    direction: Direction,
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
                    "{metric_str:?} is not a known metric — use {}",
                    Metric::alternatives()
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
        // Document-level metrics: threshold + direction are required and
        // validated in the loader; mirror here for direct constructions.
        let (threshold, direction) = if metric.is_document_level() {
            let threshold = def.threshold.ok_or_else(|| LoadError::MissingField {
                file: file.to_string(),
                field: "threshold".to_string(),
            })?;
            if !threshold.is_finite() {
                return Err(LoadError::invalid_field(
                    file,
                    "threshold",
                    format!("{threshold} is not a valid threshold — use a finite number"),
                ));
            }
            let direction_str =
                def.direction
                    .as_deref()
                    .ok_or_else(|| LoadError::MissingField {
                        file: file.to_string(),
                        field: "direction".to_string(),
                    })?;
            let direction = Direction::parse(direction_str).ok_or_else(|| {
                LoadError::invalid_field(
                    file,
                    "direction",
                    format!("{direction_str:?} is not a direction — use above or below"),
                )
            })?;
            (threshold, direction)
        } else {
            (0.0, Direction::Above) // unused for per-sentence metrics
        };
        Ok(StatisticalEngine {
            meta,
            metric,
            max_words,
            run_length: run_length_raw as usize,
            opener: None,
            run: 0,
            threshold,
            direction,
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
        if self.metric.is_document_level() {
            // The whole measurement happens once, from the document tree.
            return Interests {
                document_exit: true,
                ..Interests::default()
            };
        }
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
            // Document-level metrics never subscribe to sentences.
            _ => {}
        }
    }

    fn on_document_exit(&mut self, doc: &Document, ctx: &mut Ctx) {
        if !self.metric.is_document_level() {
            return;
        }
        let Some(value) = metric_value(self.metric, ctx.source, doc) else {
            return; // document too short to measure
        };
        let threshold = ctx.threshold(self.threshold);
        let fires = match self.direction {
            Direction::Above => value > threshold,
            Direction::Below => value < threshold,
        };
        if !fires {
            return;
        }
        // One document-level diagnostic, anchored on the first sentence (a
        // whole-document span would fail the Text/Prose scope mask).
        let Some(span) = text_blocks(doc)
            .flat_map(|b| b.sentences.iter())
            .map(|s| s.range)
            .next()
        else {
            return;
        };
        // "typically sits above/below" states the human side of the boundary
        // — the opposite side from the flag direction.
        let human_side = match self.direction {
            Direction::Above => "below",
            Direction::Below => "above",
        };
        let label = match self.metric {
            Metric::SentenceLengthVariance => "Sentence-length variance",
            Metric::CadenceAutocorrelation => "Sentence-cadence autocorrelation",
            Metric::RepeatedOpenerDensity => "Repeated-opener rate",
            Metric::TriadDensity => "Triad density (per 1000 words)",
            Metric::PairedAdjectiveRate => "Paired-conjunction rate (per 1000 words)",
            _ => unreachable!("document-level metrics only"),
        };
        ctx.report(Report {
            span,
            message: format!(
                "{label} is {value:.2} across the document; human legal prose typically \
                 sits {human_side} {threshold:.2}."
            ),
            suggestion: None,
            weight: None,
            fix: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{BlockKind, Token};
    use crate::types::{Intent, RuleId, Scope, Severity, TextRange, Tier};
    use std::collections::HashMap;

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
            explanation: None,
            examples: vec![],
        }
    }

    fn base_def(metric: Option<&str>, params: Option<HashMap<String, f64>>) -> RuleDef {
        RuleDef {
            id: "x".into(),
            engine: "statistical".into(),
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
            metric: metric.map(|m| m.to_string()),
            params,
            direction: None,
            granularity: None,
            rubric: None,
            flag_examples: vec![],
            pass_examples: vec![],
            explanation: None,
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
        assert!(s.contains("sentence-length, repetitive-openers,"));
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

    // --- document-level metrics (#37) -----------------------------------

    use crate::document::parse;

    fn doc_def(metric: &str, threshold: f64, direction: &str) -> RuleDef {
        let mut def = base_def(Some(metric), None);
        def.threshold = Some(threshold);
        def.direction = Some(direction.to_string());
        def
    }

    fn doc_engine(metric: &str, threshold: f64, direction: &str) -> StatisticalEngine {
        StatisticalEngine::from_def(meta_for("x"), &doc_def(metric, threshold, direction), "f")
            .unwrap()
    }

    /// Distinct capitalized openers: the segmenter only splits before an
    /// uppercase letter, and distinct openers keep opener metrics quiet.
    const OPENERS: [&str; 10] = [
        "Alpha", "Bravo", "Cedar", "Delta", "Ember", "Fjord", "Grove", "Harbor", "Inlet", "Juniper",
    ];

    /// `count` sentences of `words` words each, distinct openers.
    fn uniform_text(count: usize, words: usize) -> String {
        (0..count)
            .map(|i| {
                let mut s = OPENERS[i % OPENERS.len()].to_string();
                for _ in 1..words {
                    s.push_str(" mid");
                }
                s.push('.');
                s
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn value(metric: Metric, source: &str) -> Option<f64> {
        metric_value(metric, source, &parse(source, false))
    }

    #[test]
    fn metric_parse_document_level_names() {
        for (name, m) in [
            ("sentence-length-variance", Metric::SentenceLengthVariance),
            ("cadence-autocorrelation", Metric::CadenceAutocorrelation),
            ("repeated-opener-density", Metric::RepeatedOpenerDensity),
            ("triad-density", Metric::TriadDensity),
            ("paired-adjective-rate", Metric::PairedAdjectiveRate),
        ] {
            assert_eq!(Metric::parse(name), Some(m));
            assert!(m.is_document_level());
        }
        assert!(!Metric::SentenceLength.is_document_level());
        assert!(!Metric::RepetitiveOpeners.is_document_level());
        assert_eq!(Direction::parse("above"), Some(Direction::Above));
        assert_eq!(Direction::parse("below"), Some(Direction::Below));
        assert_eq!(Direction::parse("sideways"), None);
    }

    #[test]
    fn doc_metric_from_def_requires_threshold_and_direction() {
        let err = StatisticalEngine::from_def(
            meta_for("x"),
            &base_def(Some("sentence-length-variance"), None),
            "f",
        )
        .unwrap_err();
        assert!(matches!(err, LoadError::MissingField { ref field, .. } if field == "threshold"));

        let mut def = base_def(Some("sentence-length-variance"), None);
        def.threshold = Some(10.0);
        let err = StatisticalEngine::from_def(meta_for("x"), &def, "f").unwrap_err();
        assert!(matches!(err, LoadError::MissingField { ref field, .. } if field == "direction"));

        let err =
            StatisticalEngine::from_def(meta_for("x"), &doc_def("triad-density", 2.0, "up"), "f")
                .unwrap_err();
        assert!(err.to_string().contains("use above or below"), "{err}");

        let err = StatisticalEngine::from_def(
            meta_for("x"),
            &doc_def("triad-density", f64::NAN, "above"),
            "f",
        )
        .unwrap_err();
        assert!(err.to_string().contains("threshold"), "{err}");

        // Negative thresholds are legal (autocorrelation lives in [-1, 1]).
        doc_engine("cadence-autocorrelation", -0.5, "below");
    }

    #[test]
    fn variance_of_known_sequence_and_min_sentence_gate() {
        // Word counts 1..=8: mean 4.5, population variance 5.25.
        let text = (1..=8)
            .map(|n| uniform_text(1, n).replace("Alpha", OPENERS[n - 1]))
            .collect::<Vec<_>>()
            .join(" ");
        let v = value(Metric::SentenceLengthVariance, &text).unwrap();
        assert!((v - 5.25).abs() < 1e-9, "{v}");
        // 7 sentences: below the rhythm floor, unmeasurable.
        assert_eq!(
            value(Metric::SentenceLengthVariance, &uniform_text(7, 4)),
            None
        );
        // Uniform 8 sentences: variance 0.
        assert_eq!(
            value(Metric::SentenceLengthVariance, &uniform_text(8, 4)),
            Some(0.0)
        );
    }

    #[test]
    fn autocorrelation_of_alternating_lengths_is_negative() {
        // Lengths 2,8 alternating ×4: mean 5, deviations ±3;
        // r = 7·(−9) / 8·9 = −0.875.
        let text = (0..8)
            .map(|i| uniform_text(1, if i % 2 == 0 { 2 } else { 8 }).replace("Alpha", OPENERS[i]))
            .collect::<Vec<_>>()
            .join(" ");
        let v = value(Metric::CadenceAutocorrelation, &text).unwrap();
        assert!((v + 0.875).abs() < 1e-9, "{v}");
        // Constant rhythm: correlation undefined.
        assert_eq!(
            value(Metric::CadenceAutocorrelation, &uniform_text(8, 4)),
            None
        );
    }

    #[test]
    fn repeated_opener_density_counts_adjacent_pairs_per_block() {
        let text = "The a x. The b x. Apple c x. Bat d x. The e x. The f x. Cat g x. Dog h x.";
        // Openers: The The Apple Bat The The Cat Dog → 7 transitions, 2 repeats.
        let v = value(Metric::RepeatedOpenerDensity, text).unwrap();
        assert!((v - 2.0 / 7.0).abs() < 1e-9, "{v}");
        // A block boundary breaks the pair: same openers, split into two
        // paragraphs between the repeats → 0 repeats.
        let text = "The a x. Apple b x. Bat c x. The d x.\n\nThe e x. Cat f x. Dog g x. Emu h x.";
        assert_eq!(value(Metric::RepeatedOpenerDensity, text), Some(0.0));
    }

    #[test]
    fn triad_density_rate_and_min_word_gate() {
        // One triad in exactly 50 words → 20 per 1000.
        let filler = uniform_text(1, 43); // 43 words
        let text = format!("{filler} The rule is clear, simple, and fair."); // +7 = 50
        let v = value(Metric::TriadDensity, &text).unwrap();
        assert!((v - 20.0).abs() < 1e-9, "{v}");
        // 49 words: unmeasurable.
        let text = format!(
            "{} The rule is clear, simple, and fair.",
            uniform_text(1, 42)
        );
        assert_eq!(value(Metric::TriadDensity, &text), None);
    }

    #[test]
    fn paired_adjective_rate_skips_capitalized_operands() {
        // "null and void" counts; "Buyer and Seller" (proper-noun shape) does
        // not; "up and to" (short operands) does not.
        let filler = uniform_text(1, 36); // 36 words
        let text =
            format!("{filler} The deal is null and void. Buyer and Seller act up and to it."); // +6 +8 = 50
        let v = value(Metric::PairedAdjectiveRate, &text).unwrap();
        assert!((v - 20.0).abs() < 1e-9, "{v}");
    }

    #[test]
    fn per_sentence_metrics_have_no_document_value() {
        let text = uniform_text(10, 5);
        assert_eq!(value(Metric::SentenceLength, &text), None);
        assert_eq!(value(Metric::RepetitiveOpeners, &text), None);
    }

    fn run_doc(e: &mut StatisticalEngine, source: &str, over: Option<f64>) -> Vec<Report> {
        let doc = parse(source, false);
        let mut ctx = Ctx::new(source, 0);
        ctx.set_threshold_override(over);
        e.on_document_exit(&doc, &mut ctx);
        ctx.take_reports()
    }

    #[test]
    fn doc_engine_fires_below_and_reports_value_on_first_sentence() {
        let mut e = doc_engine("sentence-length-variance", 105.0, "below");
        // Doc metrics subscribe to document exit only.
        assert!(e.interests().document_exit);
        assert!(!e.interests().sentences);
        let src = uniform_text(10, 6);
        let reports = run_doc(&mut e, &src, None);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].span.slice(&src), "Alpha mid mid mid mid mid.");
        assert_eq!(
            reports[0].message,
            "Sentence-length variance is 0.00 across the document; human legal prose \
             typically sits above 105.00."
        );
    }

    #[test]
    fn doc_engine_silent_when_on_the_human_side_or_unmeasurable() {
        // High variance: 5 short + 5 long sentences.
        let src = format!("{} {}", uniform_text(5, 2), uniform_text(5, 30));
        let mut e = doc_engine("sentence-length-variance", 105.0, "below");
        assert!(run_doc(&mut e, &src, None).is_empty());
        // Unmeasurable (too few sentences): silent, not a zero-variance flag.
        let mut e = doc_engine("sentence-length-variance", 105.0, "below");
        assert!(run_doc(&mut e, &uniform_text(4, 6), None).is_empty());
        // Empty document.
        let mut e = doc_engine("sentence-length-variance", 105.0, "below");
        assert!(run_doc(&mut e, "", None).is_empty());
    }

    #[test]
    fn doc_engine_direction_above_and_threshold_override() {
        let filler = uniform_text(8, 6); // 48 words, 8 sentences
        let src = format!("{filler} The rule is clear, simple, and fair."); // 55 words, 1 triad
                                                                            // 1/55·1000 ≈ 18.2 per 1000 > 2 → fires.
        let mut e = doc_engine("triad-density", 2.0, "above");
        let reports = run_doc(&mut e, &src, None);
        assert_eq!(reports.len(), 1);
        assert!(
            reports[0].message.starts_with("Triad density"),
            "{}",
            reports[0].message
        );
        // Threshold override raises the bar above the measured value.
        let mut e = doc_engine("triad-density", 2.0, "above");
        assert!(run_doc(&mut e, &src, Some(50.0)).is_empty());
    }
}
