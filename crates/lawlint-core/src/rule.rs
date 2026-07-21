//! Rule trait, RuleMeta, Ctx. Verbatim from docs/engine-design.md §4.
//! [skeleton — complete]
//!
//! Rules are **stateful, instantiated fresh per lint run** — `RuleSet` stores
//! parsed `RuleDef`s and `instantiate()`s `Vec<Box<dyn Rule>>` each run.

use serde::{Deserialize, Serialize};

use crate::document::{Block, Document, Sentence, Token};
use crate::judge::RubricFragment;
use crate::types::{Fix, Intent, RuleId, Scope, Severity, TextRange, Tier};

/// Which callbacks a rule wants. The dispatcher only calls subscribed hooks.
#[derive(Debug, Clone, Default)]
pub struct Interests {
    pub tokens: bool,
    pub sentences: bool,
    pub blocks: bool,
    pub document_exit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleExample {
    pub bad: String,
    pub good: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleMeta {
    pub id: RuleId,
    pub tier: Tier,
    pub scope: Scope,
    pub severity: Severity,
    /// style findings lint but never move the score; detection is the default.
    pub intent: Intent,
    pub description: String,
    pub docs_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    pub examples: Vec<RuleExample>,
}

/// What engines emit; the dispatcher stamps id/severity/tier, finalize adds
/// line/column/excerpt.
#[derive(Debug, Clone)]
pub struct Report {
    pub span: TextRange,
    pub message: String,
    pub suggestion: Option<String>,
    pub weight: Option<u32>,
    pub fix: Option<Fix>,
}

/// Per-run context handed to rule callbacks. Owns the report sink and the
/// per-rule threshold override (from `LintOptions::thresholds`, resolved by
/// the dispatcher for the rule currently being driven).
pub struct Ctx<'a> {
    pub source: &'a str,
    /// Scope-aware prose word count (see design doc §8).
    pub word_count: usize,
    threshold_override: Option<f64>,
    reports: Vec<Report>,
}

impl<'a> Ctx<'a> {
    pub fn new(source: &'a str, word_count: usize) -> Self {
        Ctx {
            source,
            word_count,
            threshold_override: None,
            reports: Vec::new(),
        }
    }

    /// Dispatcher: set (or clear) the threshold override before driving a
    /// rule whose id (or alias) appears in `options.thresholds`.
    pub fn set_threshold_override(&mut self, value: Option<f64>) {
        self.threshold_override = value;
    }

    /// Emit a finding.
    pub fn report(&mut self, r: Report) {
        self.reports.push(r);
    }

    /// The effective threshold for the current rule: the option override if
    /// one was configured, otherwise `default`.
    pub fn threshold(&self, default: f64) -> f64 {
        self.threshold_override.unwrap_or(default)
    }

    /// Drain accumulated reports (dispatcher calls this after driving a rule).
    pub fn take_reports(&mut self) -> Vec<Report> {
        std::mem::take(&mut self.reports)
    }
}

pub trait Rule: Send + Sync {
    fn meta(&self) -> &RuleMeta;
    fn interests(&self) -> Interests;
    fn check_token(&mut self, _t: &Token, _ctx: &mut Ctx) {}
    fn check_sentence(&mut self, _s: &Sentence, _ctx: &mut Ctx) {}
    fn check_block(&mut self, _b: &Block, _ctx: &mut Ctx) {}
    fn on_document_exit(&mut self, _doc: &Document, _ctx: &mut Ctx) {}
    /// Tier-3 rules only.
    fn rubric(&self) -> Option<&RubricFragment> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report_at(start: usize, end: usize) -> Report {
        Report {
            span: TextRange { start, end },
            message: "m".into(),
            suggestion: None,
            weight: None,
            fix: None,
        }
    }

    #[test]
    fn ctx_collects_and_drains_reports() {
        let mut ctx = Ctx::new("some source", 2);
        assert_eq!(ctx.source, "some source");
        assert_eq!(ctx.word_count, 2);
        ctx.report(report_at(0, 4));
        ctx.report(report_at(5, 11));
        let reports = ctx.take_reports();
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].span, TextRange { start: 0, end: 4 });
        // Drained: sink is empty again.
        assert!(ctx.take_reports().is_empty());
        // And still usable afterwards.
        ctx.report(report_at(1, 2));
        assert_eq!(ctx.take_reports().len(), 1);
    }

    #[test]
    fn ctx_threshold_defaults_and_overrides() {
        let mut ctx = Ctx::new("", 0);
        assert_eq!(ctx.threshold(8.0), 8.0);
        ctx.set_threshold_override(Some(3.5));
        assert_eq!(ctx.threshold(8.0), 3.5);
        ctx.set_threshold_override(None);
        assert_eq!(ctx.threshold(8.0), 8.0);
    }

    #[test]
    fn interests_default_is_all_false() {
        let i = Interests::default();
        assert!(!i.tokens && !i.sentences && !i.blocks && !i.document_exit);
    }

    #[test]
    fn rule_default_hooks_are_no_ops_and_rubric_none() {
        struct Nop {
            meta: RuleMeta,
        }
        impl Rule for Nop {
            fn meta(&self) -> &RuleMeta {
                &self.meta
            }
            fn interests(&self) -> Interests {
                Interests::default()
            }
        }
        let mut r = Nop {
            meta: RuleMeta {
                id: RuleId("core/x".into()),
                tier: Tier::Static,
                scope: Scope::Text,
                severity: Severity::Error,
                intent: Intent::Detection,
                description: "d".into(),
                docs_url: "https://lawlint.com/rules/x".into(),
                rationale: None,
                explanation: None,
                examples: vec![],
            },
        };
        let mut ctx = Ctx::new("", 0);
        let doc = Document { blocks: vec![] };
        r.on_document_exit(&doc, &mut ctx);
        assert!(ctx.take_reports().is_empty());
        assert!(r.rubric().is_none());
        assert_eq!(r.meta().id, RuleId("core/x".into()));
    }

    #[test]
    fn rule_meta_serializes_examples_and_skips_none_rationale() {
        let meta = RuleMeta {
            id: RuleId("core/x".into()),
            tier: Tier::Static,
            scope: Scope::Text,
            severity: Severity::Warning,
            intent: Intent::Style,
            description: "d".into(),
            docs_url: "u".into(),
            rationale: None,
            explanation: None,
            examples: vec![RuleExample {
                bad: "b".into(),
                good: "g".into(),
            }],
        };
        let v = serde_json::to_value(&meta).unwrap();
        assert!(v.get("rationale").is_none());
        assert_eq!(v["examples"][0]["bad"], "b");
        assert_eq!(v["severity"], "warning");
        assert_eq!(v["tier"], "static");
        assert_eq!(v["scope"], "text");
        assert_eq!(v["intent"], "style");
    }
}
