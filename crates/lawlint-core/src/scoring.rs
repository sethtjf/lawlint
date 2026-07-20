//! Finalize: line/col/excerpt, stats, score. [integration]
//!
//! Parity-critical scoring:
//! - `word_count`: regex `(?u)\b[\w'’-]+\b` over source with code-block
//!   ranges blanked.
//! - `sentence_count`: total Document sentences.
//! - Points: Error=5, Warning=3, Suggestion=1; × weight (default 1); tier-3
//!   additionally × confidence. Only detection-intent diagnostics carry
//!   points; style-intent findings report but never move the score.
//! - Tier-3 findings below the confidence floor (default 0.6,
//!   `options.judge.floor`) are dropped; surviving tier-3 severity =
//!   min(rule severity, Warning). (Applied by `gate_tier3`, called from
//!   `lint_full` before `finalize`.)
//! - `density = penalty / max(words,1) * 1000`;
//!   `score = round(100 * exp(-density/100)).clamp(0,100)`.
//! - Golden parity: mild hedging → weight 2, score 55; heavy hedging →
//!   weight 11, score 4.

use std::sync::OnceLock;

use regex::Regex;

use crate::document::{BlockKind, Document};
use crate::types::{Diagnostic, Intent, LintResult, Severity, Stats, Tier};

/// Word-count regex, identical to the old engine's (the class contains both
/// apostrophe forms and the hyphen).
fn word_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?u)\b[\w'’-]+\b").expect("word regex compiles"))
}

/// Scope-aware word count: `(?u)\b[\w'’-]+\b` over the source with code-block
/// ranges blanked (matches falling inside a `CodeBlock` block range are not
/// counted). Shared by the dispatcher (`Ctx::word_count`) and `finalize`.
pub fn word_count(source: &str, doc: &Document) -> usize {
    let code: Vec<_> = doc
        .blocks
        .iter()
        .filter(|b| b.kind == BlockKind::CodeBlock)
        .map(|b| b.range)
        .collect();
    word_regex()
        .find_iter(source)
        .filter(|m| {
            !code
                .iter()
                .any(|r| m.start() >= r.start && m.end() <= r.end)
        })
        .count()
}

/// Byte offsets of the start of every line (line 1 starts at 0).
pub(crate) fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, c) in source.char_indices() {
        if c == '\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// (line, column): 1-based, column in UTF-16 code units, via
/// `partition_point` over line starts.
pub(crate) fn location(source: &str, starts: &[usize], offset: usize) -> (usize, usize) {
    let line = starts.partition_point(|&s| s <= offset).max(1);
    let column = source[starts[line - 1]..offset].encode_utf16().count() + 1;
    (line, column)
}

fn points(severity: Severity) -> f64 {
    match severity {
        Severity::Error => 5.0,
        Severity::Warning => 3.0,
        Severity::Suggestion => 1.0,
    }
}

/// Tier-3 gate: drop inferential findings below the confidence `floor`, and
/// cap surviving inferential severity at Warning (Error → Warning; Suggestion
/// stays Suggestion). Tiers 1–2 pass through untouched.
pub fn gate_tier3(diagnostics: Vec<Diagnostic>, floor: f32) -> Vec<Diagnostic> {
    diagnostics
        .into_iter()
        .filter(|d| d.tier != Tier::Inferential || d.confidence.unwrap_or(0.0) >= floor)
        .map(|mut d| {
            if d.tier == Tier::Inferential && d.severity == Severity::Error {
                d.severity = Severity::Warning;
            }
            d
        })
        .collect()
}

/// Sort diagnostics by span start, fill line/column/end_line/end_column
/// (UTF-16 columns via `partition_point` over line starts) and `excerpt`
/// (trimmed line), compute stats + score. [integration]
pub fn finalize(source: &str, mut diagnostics: Vec<Diagnostic>, doc: &Document) -> LintResult {
    diagnostics.sort_by_key(|d| (d.span.start, d.span.end));

    let starts = line_starts(source);
    let lines: Vec<&str> = source.split('\n').collect();
    for d in &mut diagnostics {
        let (line, column) = location(source, &starts, d.span.start);
        let (end_line, end_column) = location(source, &starts, d.span.end);
        d.line = line;
        d.column = column;
        d.end_line = Some(end_line);
        d.end_column = Some(end_column);
        d.excerpt = lines.get(line - 1).unwrap_or(&"").trim().to_string();
    }

    let words = word_count(source, doc);
    let sentence_count: usize = doc.blocks.iter().map(|b| b.sentences.len()).sum();

    // The human-likeness score aggregates detection-intent diagnostics only;
    // style findings are drafting lint and must not move it.
    let penalty: f64 = diagnostics
        .iter()
        .filter(|d| d.intent == Intent::Detection)
        .map(|d| {
            let confidence = if d.tier == Tier::Inferential {
                f64::from(d.confidence.unwrap_or(1.0))
            } else {
                1.0
            };
            points(d.severity) * f64::from(d.weight.unwrap_or(1)) * confidence
        })
        .sum();
    let density = penalty / words.max(1) as f64 * 1000.0;
    let score = (100.0 * (-density / 100.0).exp()).round().clamp(0.0, 100.0) as i32;

    LintResult {
        diagnostics,
        stats: Stats {
            word_count: words,
            sentence_count,
            score,
        },
        judge: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::parse;
    use crate::types::{RuleId, TextRange};

    fn diag(start: usize, end: usize, severity: Severity, tier: Tier) -> Diagnostic {
        Diagnostic {
            rule_id: RuleId("core/x".into()),
            severity,
            tier,
            intent: Intent::Detection,
            span: TextRange { start, end },
            message: "m".into(),
            line: 0,
            column: 0,
            end_line: None,
            end_column: None,
            excerpt: String::new(),
            suggestion: None,
            weight: None,
            confidence: None,
            fix: None,
        }
    }

    #[test]
    fn word_count_matches_old_regex_semantics() {
        let doc = parse("the party’s cross-motion, filed 2024", false);
        assert_eq!(word_count("the party’s cross-motion, filed 2024", &doc), 5);
        let doc = parse("", false);
        assert_eq!(word_count("", &doc), 0);
    }

    #[test]
    fn word_count_blanks_code_blocks() {
        let src = "One two.\n\n```js\nconst x = delve;\n```\n\nThree four.";
        let doc = parse(src, true);
        assert_eq!(word_count(src, &doc), 4);
        // Plain mode: no code blocks, everything counts.
        let plain = parse(src, false);
        assert!(word_count(src, &plain) > 4);
    }

    #[test]
    fn location_is_one_based_with_utf16_columns() {
        let src = "ab\n𝒳cd\nend";
        let starts = line_starts(src);
        assert_eq!(location(src, &starts, 0), (1, 1));
        assert_eq!(location(src, &starts, 2), (1, 3));
        // Line 2 starts at byte 3; 𝒳 is 4 bytes / 2 UTF-16 units.
        assert_eq!(location(src, &starts, 3), (2, 1));
        assert_eq!(location(src, &starts, 7), (2, 3)); // after 𝒳
        assert_eq!(location(src, &starts, 10), (3, 1));
    }

    #[test]
    fn finalize_sorts_fills_positions_and_excerpt() {
        let src = "first line here\nsecond — line";
        let doc = parse(src, false);
        let d1 = diag(6, 10, Severity::Warning, Tier::Static);
        let d2 = diag(0, 5, Severity::Warning, Tier::Static);
        let result = finalize(src, vec![d1, d2], &doc);
        assert_eq!(result.diagnostics[0].span.start, 0);
        assert_eq!(result.diagnostics[0].line, 1);
        assert_eq!(result.diagnostics[0].column, 1);
        assert_eq!(result.diagnostics[0].end_line, Some(1));
        assert_eq!(result.diagnostics[0].end_column, Some(6));
        assert_eq!(result.diagnostics[0].excerpt, "first line here");
        assert_eq!(result.diagnostics[1].span.start, 6);
    }

    #[test]
    fn score_formula_matches_old_engine() {
        // 100 words, one Warning with weight 2 → penalty 6 → density 60 →
        // round(100·e^-0.6) = 55 (golden mild-hedging parity arithmetic).
        let src = (0..100)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let doc = parse(&src, false);
        let mut d = diag(0, 2, Severity::Warning, Tier::Static);
        d.weight = Some(2);
        let result = finalize(&src, vec![d], &doc);
        assert_eq!(result.stats.word_count, 100);
        assert_eq!(result.stats.score, 55);
        // Weight 11 → penalty 33 → density 330 → 4 (heavy parity arithmetic).
        let mut d = diag(0, 2, Severity::Warning, Tier::Static);
        d.weight = Some(11);
        let result = finalize(&src, vec![d], &doc);
        assert_eq!(result.stats.score, 4);
    }

    #[test]
    fn style_intent_diagnostics_do_not_move_the_score() {
        // 100 words, one Warning weight 2: detection scores 55 (golden
        // parity); the identical diagnostic marked style leaves score 100
        // while still surfacing in the diagnostics list.
        let src = (0..100)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let doc = parse(&src, false);
        let mut d = diag(0, 2, Severity::Warning, Tier::Static);
        d.weight = Some(2);
        d.intent = Intent::Style;
        let result = finalize(&src, vec![d], &doc);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.stats.score, 100);
        // Mixed: only the detection diagnostic carries points.
        let mut style = diag(0, 2, Severity::Error, Tier::Static);
        style.intent = Intent::Style;
        style.weight = Some(100);
        let mut detection = diag(3, 5, Severity::Warning, Tier::Static);
        detection.weight = Some(2);
        let result = finalize(&src, vec![style, detection], &doc);
        assert_eq!(result.stats.score, 55);
    }

    #[test]
    fn score_empty_text_is_100_and_penalty_clamps() {
        let doc = parse("", false);
        assert_eq!(finalize("", vec![], &doc).stats.score, 100);
        // Enormous penalty on a tiny text clamps at 0.
        let src = "a b";
        let doc = parse(src, false);
        let mut d = diag(0, 1, Severity::Error, Tier::Static);
        d.weight = Some(1000);
        assert_eq!(finalize(src, vec![d], &doc).stats.score, 0);
    }

    #[test]
    fn tier3_points_scale_with_confidence() {
        // 100 words. Static Warning (3) + inferential Warning conf 0.8 (2.4)
        // → penalty 5.4 → density 54 → round(100·e^-0.54) = 58.
        let src = (0..100)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let doc = parse(&src, false);
        let d1 = diag(0, 2, Severity::Warning, Tier::Static);
        let mut d2 = diag(3, 5, Severity::Warning, Tier::Inferential);
        d2.confidence = Some(0.8);
        let result = finalize(&src, vec![d1, d2], &doc);
        assert_eq!(result.stats.score, 58);
    }

    #[test]
    fn gate_tier3_floor_and_severity_cap() {
        let mut low = diag(0, 1, Severity::Warning, Tier::Inferential);
        low.confidence = Some(0.5);
        let mut high = diag(1, 2, Severity::Error, Tier::Inferential);
        high.confidence = Some(0.9);
        let static_err = diag(2, 3, Severity::Error, Tier::Static);
        let out = gate_tier3(vec![low, high, static_err], 0.6);
        assert_eq!(out.len(), 2);
        // Surviving tier-3 Error capped at Warning.
        assert_eq!(out[0].tier, Tier::Inferential);
        assert_eq!(out[0].severity, Severity::Warning);
        // Static severity untouched.
        assert_eq!(out[1].severity, Severity::Error);
        // Suggestion stays Suggestion (not raised to Warning).
        let mut sug = diag(0, 1, Severity::Suggestion, Tier::Inferential);
        sug.confidence = Some(0.9);
        let out = gate_tier3(vec![sug], 0.6);
        assert_eq!(out[0].severity, Severity::Suggestion);
    }

    #[test]
    fn gate_tier3_boundary_at_floor_keeps() {
        let mut at = diag(0, 1, Severity::Warning, Tier::Inferential);
        at.confidence = Some(0.6);
        assert_eq!(gate_tier3(vec![at], 0.6).len(), 1);
        let mut missing = diag(0, 1, Severity::Warning, Tier::Inferential);
        missing.confidence = None;
        assert!(gate_tier3(vec![missing], 0.6).is_empty());
    }

    #[test]
    fn sentence_count_totals_document_sentences() {
        let src = "One. Two. Three.\n\nFour.";
        let doc = parse(src, false);
        let result = finalize(src, vec![], &doc);
        assert_eq!(result.stats.sentence_count, 4);
    }
}
