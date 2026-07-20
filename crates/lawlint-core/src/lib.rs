//! lawlint-core — rules engine v2.
//!
//! Contract: docs/engine-design.md. Three tiers (static, statistical,
//! inferential); declarative-first YAML rules with a programmatic `Rule`
//! trait escape hatch; byte-offset spans into the original source; judge
//! findings must ground to a span or they do not exist; wasm-safe and
//! inference-agnostic (judge backends live in `crates/lawlint-judge`).

pub mod config;
pub mod dispatch;
pub mod document;
pub mod engines;
pub mod error;
pub mod judge;
pub mod loader;
pub mod markdown;
pub mod prompt;
pub mod registry;
pub mod rule;
pub mod scoring;
pub mod segment;
pub mod types;

use std::sync::OnceLock;

// ---- Public re-exports -------------------------------------------------

pub use config::{JudgeOptions, LintOptions};
pub use document::{parse, Block, BlockKind, Document, Sentence, Token, TokenKind};
pub use error::{JudgeError, LoadError};
// Host-driven tier-3 (wasm): plan/run/ground are public.
pub use judge::{
    default_quote_ground, plan_judge, run_judge, Granularity, Judge, JudgeCache, JudgeFinding,
    JudgeRequest, JudgeStats, MemoryCache, MockJudge, RubricFragment, PROMPT_VERSION,
};
pub use loader::{AllowContextDef, PackageManifest, PatternDef, RuleDef};
pub use prompt::{remediation_prompt, PromptSource};
pub use registry::{InferentialRule, RuleSet};
pub use rule::{Ctx, Interests, Report, Rule, RuleExample, RuleMeta};
pub use scoring::finalize;
pub use types::{
    Applicability, Diagnostic, Edit, Fix, LintResult, RuleId, Scope, Severity, Stats, TextRange,
    Tier,
};

/// Default tier-3 confidence floor when `options.judge.floor` is unset.
const DEFAULT_CONFIDENCE_FLOOR: f32 = 0.6;

/// The built-in rule set, validated once per process.
fn built_in_set() -> &'static RuleSet {
    static SET: OnceLock<RuleSet> = OnceLock::new();
    SET.get_or_init(RuleSet::built_in)
}

// ---- Public API (§8) ---------------------------------------------------

/// Lint with the built-in rule set, tiers 1–2 only.
pub fn lint(text: &str, options: &LintOptions) -> LintResult {
    lint_with(text, options, built_in_set())
}

/// Lint with an explicit rule set, tiers 1–2 only.
pub fn lint_with(text: &str, options: &LintOptions, rules: &RuleSet) -> LintResult {
    let doc = document::parse(text, options.markdown.unwrap_or(false));
    let instances = rules.instantiate(options);
    let diagnostics = dispatch::run(text, &doc, instances, options, rules);
    scoring::finalize(text, diagnostics, &doc)
}

/// Lint including tier 3: plan_judge → run_judge (with cache) → ground →
/// merge diagnostics (confidence floor + severity cap applied in scoring).
pub fn lint_full(
    text: &str,
    options: &LintOptions,
    rules: &RuleSet,
    judge: &dyn Judge,
    cache: Option<&dyn JudgeCache>,
) -> LintResult {
    let doc = document::parse(text, options.markdown.unwrap_or(false));
    let instances = rules.instantiate(options);
    // Rubrics (plus each rule's scope, for masking grounded findings) from
    // the (enable/disable/severity-resolved) instances, captured before the
    // dispatcher consumes them.
    let fragments: Vec<(Scope, RubricFragment)> = instances
        .iter()
        .filter_map(|r| r.rubric().cloned().map(|f| (r.meta().scope, f)))
        .collect();
    let mut diagnostics = dispatch::run(text, &doc, instances, options, rules);

    let refs: Vec<&RubricFragment> = fragments.iter().map(|(_, f)| f).collect();
    let reqs = plan_judge(&doc, text, &refs);
    let (grounded, stats) = run_judge(judge, cache, &reqs, text);

    let suppressions = dispatch::Suppressions::new(text, rules);
    let mut tier3 = Vec::new();
    for (_req, finding, span) in grounded {
        let Some((scope, fragment)) = fragments.iter().find(|(_, f)| f.rule.0 == finding.rule)
        else {
            continue; // run_judge already discards foreign rules; belt and suspenders
        };
        // Tier-3 diagnostics obey the same scope mask as tiers 1–2 (§8):
        // grounding can land a quote in a citation sentence, a code block, or
        // non-block source the rule may never report into.
        let mask = dispatch::scope_mask(*scope, text, &doc);
        if !dispatch::mask_contains(&mask, &span) {
            continue;
        }
        if suppressions.suppressed(&fragment.rule, span.start) {
            continue;
        }
        let message = if finding.explanation.trim().is_empty() {
            format!("Flagged by {}.", fragment.rule.0)
        } else {
            finding.explanation.clone()
        };
        let fix = finding.suggested_rewrite.as_ref().map(|replacement| Fix {
            edits: vec![Edit {
                range: span,
                replacement: replacement.clone(),
            }],
            applicability: Applicability::MaybeIncorrect, // tier-3 fixes always
        });
        tier3.push(Diagnostic {
            rule_id: fragment.rule.clone(),
            severity: fragment.severity,
            tier: Tier::Inferential,
            span,
            message,
            line: 0,
            column: 0,
            end_line: None,
            end_column: None,
            excerpt: String::new(),
            suggestion: finding.suggested_rewrite.clone(),
            weight: None,
            confidence: Some(finding.confidence),
            fix,
        });
    }
    let floor = options
        .judge
        .as_ref()
        .and_then(|j| j.floor)
        .unwrap_or(DEFAULT_CONFIDENCE_FLOOR);
    diagnostics.extend(scoring::gate_tier3(tier3, floor));

    let mut result = scoring::finalize(text, diagnostics, &doc);
    result.judge = Some(stats);
    result
}

/// Apply MachineApplicable fixes: non-overlapping, span order, single pass.
pub fn apply_fixes(text: &str, diagnostics: &[Diagnostic]) -> String {
    let mut edits: Vec<&Edit> = diagnostics
        .iter()
        .filter_map(|d| d.fix.as_ref())
        .filter(|f| f.applicability == Applicability::MachineApplicable)
        .flat_map(|f| f.edits.iter())
        .filter(|e| e.range.start <= e.range.end && e.range.end <= text.len())
        .collect();
    edits.sort_by_key(|e| (e.range.start, e.range.end));

    let mut out = String::with_capacity(text.len());
    let mut pos = 0usize;
    for edit in edits {
        if edit.range.start < pos {
            continue; // overlaps an already-applied edit: skip
        }
        out.push_str(&text[pos..edit.range.start]);
        out.push_str(&edit.replacement);
        pos = edit.range.end;
    }
    out.push_str(&text[pos..]);
    out
}

// ------------------------------------------------------------------------
// Ported old test suite (§12) + integration tests.
// ------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn has(text: &str, id: &str) -> bool {
        lint(text, &LintOptions::default())
            .diagnostics
            .iter()
            .any(|d| d.rule_id.0 == id)
    }

    #[test]
    fn parenthetical_asides_exempt_statutory_subdivisions() {
        // Subdivision refs attach to the preceding token; asides follow a space.
        assert!(!has(
            "Under Section 4(b), the agreement is binding.",
            "core/no-parenthetical-asides"
        ));
        assert!(!has(
            "Section 12(a)(1) controls the filing deadline.",
            "core/no-parenthetical-asides"
        ));
        assert!(has(
            "The court (again) delayed the ruling (twice).",
            "core/no-parenthetical-asides"
        ));

        // The consumed leading whitespace must not enter the reported span:
        // an aside opening line 2 reports line 2, column 1 — not the end of
        // line 1.
        let result = lint(
            "Intro line here.\n(again) the court delayed (twice) more.",
            &LintOptions::default(),
        );
        let d = result
            .diagnostics
            .iter()
            .find(|d| d.rule_id.0 == "core/no-parenthetical-asides")
            .expect("aside flagged");
        assert_eq!((d.line, d.column), (2, 1));
        assert!(d.excerpt.starts_with("(again)"));
    }

    // ---- ported: registry ----------------------------------------------

    #[test]
    fn registry() {
        // Old suite: 20 bespoke rules. Now 27 (two inferential rules plus five
        // Orwell/AI-voice writing rules).
        let rs = RuleSet::built_in();
        assert_eq!(rs.metas().len(), 27);
        assert!(rs.metas().iter().all(|m| m.id.0.starts_with("core/")));
        assert!(rs.metas().iter().all(|m| !m.description.is_empty()));
    }

    // ---- ported: basics ------------------------------------------------

    #[test]
    fn basics() {
        assert!(has(
            "We should delve into this issue.",
            "core/no-ai-cliches"
        ));
        assert!(has(
            "The parties are Alice, Bob and Carol.",
            "core/oxford-comma"
        ));
        assert!(!has("The range spans 2020–2024.", "core/no-en-dash"));
    }

    #[test]
    fn en_dash_outside_numeric_range_flagged() {
        assert!(has("The court–ordered remedy failed.", "core/no-en-dash"));
        // Old EnDashRule required a digit IMMEDIATELY adjacent on each side:
        // spaced ranges are flagged.
        assert!(has("The years 2020 – 2024 mattered.", "core/no-en-dash"));
    }

    // ---- ported: options -----------------------------------------------

    #[test]
    fn options() {
        // Legacy flat id resolves through the alias map.
        let o = LintOptions {
            disable: Some(vec!["no-ai-cliches".into()]),
            ..Default::default()
        };
        assert!(lint("delve", &o)
            .diagnostics
            .iter()
            .all(|d| d.rule_id.0 != "core/no-ai-cliches"));
    }

    #[test]
    fn accepts_explicit_rule_lists() {
        // Old suite sliced `built_in_rules()`; the equivalent is an `enable`
        // allowlist over the same rule set.
        let o = LintOptions {
            enable: Some(vec!["no-ai-cliches".into()]),
            ..Default::default()
        };
        let result = lint("We delve.", &o);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].rule_id.0, "core/no-ai-cliches");
    }

    #[test]
    fn severity_override_applies_via_flat_id() {
        let o = LintOptions {
            severity: Some(
                [("no-ai-cliches".to_string(), Severity::Suggestion)]
                    .into_iter()
                    .collect(),
            ),
            enable: Some(vec!["no-ai-cliches".into()]),
            ..Default::default()
        };
        let result = lint("We delve.", &o);
        assert_eq!(result.diagnostics[0].severity, Severity::Suggestion);
    }

    // ---- ported: fixture parity ----------------------------------------

    #[test]
    fn fixture_parity() {
        let bad = include_str!("../tests/fixtures/bad.md");
        let bad_result = lint(
            bad,
            &LintOptions {
                markdown: Some(true),
                ..Default::default()
            },
        );
        // word_count and score match the old engine exactly.
        assert_eq!(bad_result.stats.word_count, 70);
        assert_eq!(bad_result.stats.score, 2);
        // sentence_count was 3 under the old `.!?` splitter; legal-aware
        // segmentation counts the heading "Agreement" as its own sentence → 4.
        assert_eq!(bad_result.stats.sentence_count, 4);
        // Same diagnostic multiset as the old engine, now in span order.
        assert_eq!(
            bad_result
                .diagnostics
                .iter()
                .map(|d| d.rule_id.0.as_str())
                .collect::<Vec<_>>(),
            vec![
                "core/no-ai-cliches",         // "It is important to note"
                "core/no-ai-cliches",         // "delve"
                "core/no-marketing-language", // "delve"
                "core/no-ai-cliches",         // "landscape of"
                "core/no-legalese",           // "Pursuant to"
                "core/no-legalese",           // "aforementioned"
                "core/oxford-comma",          // "terms, the parties … and …"
                "core/no-doublets",           // "cease and desist"
                "core/no-doublets",           // "any and all"
                "core/prefer-short-words",    // "demonstrate"
                "core/no-passive-overuse",    // "be flagged" (density span)
            ]
        );

        let clean = include_str!("../tests/fixtures/clean.txt");
        let clean_result = lint(clean, &LintOptions::default());
        assert!(
            clean_result.diagnostics.is_empty(),
            "{:?}",
            clean_result.diagnostics
        );
        assert_eq!(clean_result.stats.word_count, 20);
        assert_eq!(clean_result.stats.sentence_count, 2);
        assert_eq!(clean_result.stats.score, 100);
    }

    #[test]
    fn old_info_severity_is_now_suggestion() {
        // no-doublets was Severity::Info; it must surface as "suggestion".
        let result = lint("The order is null and void.", &LintOptions::default());
        let d = result
            .diagnostics
            .iter()
            .find(|d| d.rule_id.0 == "core/no-doublets")
            .expect("doublet flagged");
        assert_eq!(d.severity, Severity::Suggestion);
        let v = serde_json::to_value(d).unwrap();
        assert_eq!(v["severity"], "suggestion");
    }

    // ---- ported: JSON field-name contract ------------------------------

    #[test]
    fn json_shape_uses_typescript_names() {
        let result = lint("We delve.", &LintOptions::default());
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["diagnostics"][0]["ruleId"], "core/no-ai-cliches");
        assert_eq!(json["diagnostics"][0]["endLine"], 1);
        assert_eq!(json["diagnostics"][0]["endColumn"], 9);
        assert_eq!(json["diagnostics"][0]["severity"], "warning");
        assert_eq!(json["stats"]["wordCount"], 2);
        assert_eq!(json["stats"]["sentenceCount"], 1);
        assert!(json["diagnostics"][0].get("weight").is_none());
        assert!(json["diagnostics"][0].get("confidence").is_none());
        // Tiers 1-2 only: no judge stats key at all.
        assert!(json.get("judge").is_none());
    }

    // ---- ported: scoring golden parity ---------------------------------

    fn scoring_sentences(count: usize) -> String {
        let openers = [
            "Alpha", "Bravo", "Cedar", "Delta", "Ember", "Fjord", "Grove", "Harbor", "Inlet",
            "Juniper",
        ];
        (0..count)
            .map(|sentence| {
                (0..10)
                    .map(|word| {
                        if word == 0 {
                            openers[sentence % openers.len()].to_string()
                        } else {
                            format!("w{}", sentence * 10 + word)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
                    + "."
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn scoring_uses_penalty_density_decay() {
        let clean = lint("", &LintOptions::default());
        assert_eq!(clean.stats.score, 100);

        let text = format!(
            "{} We map the landscape of this matter briefly here today.",
            scoring_sentences(9)
        );
        let result = lint(&text, &LintOptions::default());
        assert_eq!(result.stats.word_count, 100);
        assert_eq!(
            result
                .diagnostics
                .iter()
                .map(|d| d.rule_id.0.as_str())
                .collect::<Vec<_>>(),
            vec!["core/no-ai-cliches"]
        );
        assert_eq!(result.stats.score, 74);
    }

    #[test]
    fn scoring_weights_density_diagnostics_by_overuse() {
        let text = |count: usize| {
            let openers = [
                "Alpha", "Bravo", "Cedar", "Delta", "Ember", "Fjord", "Grove", "Harbor", "Inlet",
                "Juniper",
            ];
            let mut remaining = count;
            (0..100)
                .map(|index| {
                    let word = if index % 10 == 0 {
                        openers[index / 10].to_string()
                    } else if remaining > 0 {
                        remaining -= 1;
                        "perhaps".to_string()
                    } else {
                        format!("w{index}")
                    };
                    let punctuation = if index == 99 {
                        "."
                    } else if (index + 1) % 10 == 0 {
                        ". "
                    } else {
                        " "
                    };
                    format!("{word}{punctuation}")
                })
                .collect::<String>()
        };
        let mild = lint(&text(3), &LintOptions::default());
        let heavy = lint(&text(12), &LintOptions::default());

        // GOLDEN PARITY (non-negotiable): weights 2/11, scores 55/4.
        assert_eq!(mild.diagnostics[0].rule_id.0, "core/no-hedging");
        assert_eq!(mild.diagnostics[0].weight, Some(2));
        assert_eq!(heavy.diagnostics[0].weight, Some(11));
        assert_eq!(mild.stats.score, 55);
        assert_eq!(heavy.stats.score, 4);
    }

    // ---- ported: bespoke rule behaviors --------------------------------

    #[test]
    fn leading_rules_fire_only_at_sentence_start() {
        assert!(has(
            "Great question! The answer is no.",
            "core/no-sycophantic-openers"
        ));
        assert!(has(
            "Fine. Here's my take on the motion.",
            "core/no-throat-clearing"
        ));
        assert!(!has(
            "That was a great question to raise.",
            "core/no-sycophantic-openers"
        ));
    }

    #[test]
    fn sentence_length_uses_threshold_override() {
        let text = "one two three four five six seven eight nine ten.";
        assert!(!has(text, "core/sentence-length"));
        let o = LintOptions {
            thresholds: Some([("sentence-length".to_string(), 5.0)].into_iter().collect()),
            ..Default::default()
        };
        let result = lint(text, &o);
        let d = result
            .diagnostics
            .iter()
            .find(|d| d.rule_id.0 == "core/sentence-length")
            .expect("long sentence flagged");
        assert_eq!(d.message, "Sentence is 10 words; consider shortening it.");
    }

    #[test]
    fn repetitive_openers_fire_and_report_last_sentence() {
        let text = "The court erred. The record shows this plainly. The remedy is reversal.";
        let result = lint(text, &LintOptions::default());
        let d = result
            .diagnostics
            .iter()
            .find(|d| d.rule_id.0 == "core/no-repetitive-openers")
            .expect("repetitive openers flagged");
        assert_eq!(d.message, "Three consecutive sentences begin with “the”.");
    }

    #[test]
    fn legal_citations_do_not_split_sentences() {
        // Old splitter counted "See Roe v. Wade, 410 U.S. 113 (1973)." as
        // several sentences; legal segmentation keeps it as one (citation)
        // sentence — sentence_count expectations shift per §12.
        let text = "See Roe v. Wade, 410 U.S. 113 (1973). The court held that this applies.";
        let result = lint(text, &LintOptions::default());
        assert_eq!(result.stats.sentence_count, 2);
    }

    #[test]
    fn markdown_code_blocks_are_not_linted_and_not_counted() {
        let text = "Clean prose here.\n\n```\nwe delve; pursuant to — herein\n```\n";
        let o = LintOptions {
            markdown: Some(true),
            ..Default::default()
        };
        let result = lint(text, &o);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.stats.word_count, 3);
    }

    // ---- suppression e2e ------------------------------------------------

    #[test]
    fn suppression_comments_silence_rules() {
        let text =
            "<!-- lawlint-disable-next-line no-ai-cliches -->\nWe delve into it.\nWe delve again.";
        let o = LintOptions {
            markdown: Some(true),
            enable: Some(vec!["no-ai-cliches".into()]),
            ..Default::default()
        };
        let result = lint(text, &o);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].line, 3);
    }

    // ---- apply_fixes ----------------------------------------------------

    #[test]
    fn apply_fixes_machine_applicable_in_span_order() {
        let text = "We leverage and delve daily.";
        let result = lint(text, &LintOptions::default());
        // Built-ins carry no `fix:` strings yet; simulate two machine fixes
        // over real diagnostics plus one MaybeIncorrect that must be ignored.
        let mk = |start: usize, end: usize, replacement: &str, applicability| Diagnostic {
            rule_id: RuleId("core/x".into()),
            severity: Severity::Error,
            tier: Tier::Static,
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
            fix: Some(Fix {
                edits: vec![Edit {
                    range: TextRange { start, end },
                    replacement: replacement.into(),
                }],
                applicability,
            }),
        };
        let lev = text.find("leverage").unwrap();
        let del = text.find("delve").unwrap();
        let diags = vec![
            // Out of span order on purpose.
            mk(del, del + 5, "dig", Applicability::MachineApplicable),
            mk(lev, lev + 8, "use", Applicability::MachineApplicable),
            mk(0, 2, "They", Applicability::MaybeIncorrect),
        ];
        assert_eq!(apply_fixes(text, &diags), "We use and dig daily.");
        // No fixes → identity.
        assert_eq!(apply_fixes(text, &result.diagnostics), text);
    }

    #[test]
    fn apply_fixes_skips_overlapping_edits() {
        let mk = |start: usize, end: usize, replacement: &str| Diagnostic {
            rule_id: RuleId("core/x".into()),
            severity: Severity::Error,
            tier: Tier::Static,
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
            fix: Some(Fix {
                edits: vec![Edit {
                    range: TextRange { start, end },
                    replacement: replacement.into(),
                }],
                applicability: Applicability::MachineApplicable,
            }),
        };
        let text = "abcdef";
        // Second edit overlaps the first → skipped.
        let diags = vec![mk(0, 4, "X"), mk(2, 6, "Y")];
        assert_eq!(apply_fixes(text, &diags), "Xef");
        // Out-of-bounds edit ignored entirely.
        let diags = vec![mk(0, 99, "X")];
        assert_eq!(apply_fixes(text, &diags), "abcdef");
    }

    #[test]
    fn apply_fixes_preserves_leading_capital_end_to_end() {
        // Engine emits the fix, case logic capitalizes it, apply_fixes composes.
        let text = "Pursuant to Section 4(b), the fee is due.";
        let result = lint(text, &LintOptions::default());
        assert!(apply_fixes(text, &result.diagnostics).starts_with("Under Section 4(b)"));
    }

    // ---- tier-3 end-to-end ----------------------------------------------

    fn finding(rule: &str, quote: &str, confidence: f32, rewrite: Option<&str>) -> JudgeFinding {
        JudgeFinding {
            rule: rule.to_string(),
            quote: quote.to_string(),
            explanation: "hedge with no stated uncertainty".into(),
            confidence,
            suggested_rewrite: rewrite.map(str::to_string),
        }
    }

    #[test]
    fn tier3_end_to_end_grounded_findings_become_diagnostics() {
        let text = "It could perhaps be argued that the claim fails. Damages total $12,000.";
        let judge = MockJudge::new().respond(
            "could perhaps",
            vec![
                finding(
                    "core/empty-hedge",
                    "It could perhaps be argued that",
                    0.9,
                    Some("The claim fails because"),
                ),
                // Below the 0.6 floor: dropped by scoring.
                finding("core/empty-hedge", "the claim fails", 0.3, None),
            ],
        );
        let result = lint_full(text, &LintOptions::default(), built_in_set(), &judge, None);
        let tier3: Vec<&Diagnostic> = result
            .diagnostics
            .iter()
            .filter(|d| d.tier == Tier::Inferential)
            .collect();
        assert_eq!(tier3.len(), 1);
        let d = tier3[0];
        assert_eq!(d.rule_id.0, "core/empty-hedge");
        assert_eq!(d.span.slice(text), "It could perhaps be argued that");
        // Indistinguishable downstream: positions/excerpt filled like any
        // static diagnostic.
        assert_eq!(d.line, 1);
        assert_eq!(d.column, 1);
        assert!(!d.excerpt.is_empty());
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(d.confidence, Some(0.9));
        // Tier-3 fixes are always MaybeIncorrect.
        assert_eq!(
            d.fix.as_ref().unwrap().applicability,
            Applicability::MaybeIncorrect
        );
        assert_eq!(d.suggestion.as_deref(), Some("The claim fails because"));
        // Judge stats surfaced.
        let stats = result.judge.as_ref().unwrap();
        assert_eq!(stats.grounded, 2);
        assert!(stats.chunks >= 1);
    }

    #[test]
    fn tier3_severity_capped_at_warning_even_with_error_override() {
        let text = "It could perhaps be argued that the claim fails.";
        let judge = MockJudge::new().respond(
            "",
            vec![finding(
                "core/empty-hedge",
                "could perhaps be argued",
                0.95,
                None,
            )],
        );
        let o = LintOptions {
            severity: Some(
                [("empty-hedge".to_string(), Severity::Error)]
                    .into_iter()
                    .collect(),
            ),
            enable: Some(vec!["empty-hedge".into()]),
            ..Default::default()
        };
        let result = lint_full(text, &o, built_in_set(), &judge, None);
        assert_eq!(result.diagnostics.len(), 1);
        // Rule severity overridden to Error, but tier-3 caps at Warning.
        assert_eq!(result.diagnostics[0].severity, Severity::Warning);
    }

    #[test]
    fn tier3_confidence_floor_from_judge_options() {
        let text = "It could perhaps be argued that the claim fails.";
        let mk_judge = || {
            MockJudge::new().respond(
                "",
                vec![finding(
                    "core/empty-hedge",
                    "could perhaps be argued",
                    0.7,
                    None,
                )],
            )
        };
        let base = LintOptions {
            enable: Some(vec!["empty-hedge".into()]),
            ..Default::default()
        };
        // Default floor 0.6: kept.
        let result = lint_full(text, &base, built_in_set(), &mk_judge(), None);
        assert_eq!(result.diagnostics.len(), 1);
        // Raised floor 0.8: dropped.
        let strict = LintOptions {
            judge: Some(JudgeOptions {
                floor: Some(0.8),
                ..Default::default()
            }),
            ..base.clone()
        };
        let result = lint_full(text, &strict, built_in_set(), &mk_judge(), None);
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn tier3_diagnostics_score_confidence_weighted() {
        // 100 words, only tier-3: one Warning finding at confidence 1.0 →
        // penalty 3 → density 30 → score 74; at 0.5 floor and confidence
        // 0.6 → penalty 1.8 → density 18 → round(100·e^-0.18) = 84.
        let filler: String = (0..93).map(|i| format!("w{i} ")).collect();
        let text = format!("{filler}It could perhaps be argued that fine.");
        let judge = MockJudge::new().respond(
            "",
            vec![finding(
                "core/empty-hedge",
                "could perhaps be argued",
                1.0,
                None,
            )],
        );
        let o = LintOptions {
            enable: Some(vec!["empty-hedge".into()]),
            ..Default::default()
        };
        let result = lint_full(&text, &o, built_in_set(), &judge, None);
        assert_eq!(result.stats.word_count, 100);
        assert_eq!(result.stats.score, 74);

        let judge = MockJudge::new().respond(
            "",
            vec![finding(
                "core/empty-hedge",
                "could perhaps be argued",
                0.6,
                None,
            )],
        );
        let result = lint_full(&text, &o, built_in_set(), &judge, None);
        assert_eq!(result.stats.score, 84);
    }

    fn inferential_set(scope: &str, granularity: &str) -> RuleSet {
        let manifest =
            loader::parse_manifest("style.yaml", "name: core\nversion: 0.1.0\n").unwrap();
        let yaml = format!(
            "id: judge-me\nengine: inferential\nscope: {scope}\nseverity: warning\n\
             granularity: {granularity}\nrubric: Flag it.\n\
             flag_examples: [a, b, c]\npass_examples: [x, y, z]\n"
        );
        let rule = loader::parse_rule("r.yaml", &yaml).unwrap();
        RuleSet::from_parts(&manifest, vec![("r.yaml".to_string(), rule)]).unwrap()
    }

    #[test]
    fn tier3_prose_scope_excludes_citation_sentences() {
        // Regression: grounded judge findings used to bypass the scope mask,
        // so a prose-scope inferential rule could report inside a citation
        // sentence that tiers 1–2 may never touch.
        let rs = inferential_set("prose", "sentence");
        let text = "See Roe v. Wade, 410 U.S. 113 (1973). The claim fails badly.";
        let judge =
            MockJudge::new().respond("", vec![finding("core/judge-me", "Roe v. Wade", 0.9, None)]);
        let result = lint_full(text, &LintOptions::default(), &rs, &judge, None);
        assert!(
            result
                .diagnostics
                .iter()
                .all(|d| d.tier != Tier::Inferential),
            "{:?}",
            result.diagnostics
        );
        // Control: the same rule may report in the non-citation sentence.
        let judge = MockJudge::new().respond(
            "",
            vec![finding("core/judge-me", "claim fails badly", 0.9, None)],
        );
        let result = lint_full(text, &LintOptions::default(), &rs, &judge, None);
        assert_eq!(
            result
                .diagnostics
                .iter()
                .filter(|d| d.tier == Tier::Inferential)
                .count(),
            1
        );
    }

    #[test]
    fn tier3_text_scope_excludes_code_blocks() {
        // A document-granularity rubric sends the WHOLE source (code blocks
        // included) to the judge; a grounded quote landing inside a code
        // block must still be masked out for the default text scope.
        let rs = inferential_set("text", "document");
        let text = "Fine prose here.\n\n```\nsecret code target\n```\n";
        let judge = MockJudge::new().respond(
            "",
            vec![finding("core/judge-me", "secret code target", 0.9, None)],
        );
        let o = LintOptions {
            markdown: Some(true),
            ..Default::default()
        };
        let result = lint_full(text, &o, &rs, &judge, None);
        assert!(
            result
                .diagnostics
                .iter()
                .all(|d| d.tier != Tier::Inferential),
            "{:?}",
            result.diagnostics
        );
    }

    #[test]
    fn tier3_findings_never_land_in_invisible_html_blocks() {
        // HTML blocks emit no Block: they must not be merged into judge
        // chunks, and even a grounded span there would fail the scope mask.
        let text =
            "Para one is fine.\n\n<div>hidden could perhaps be argued html</div>\n\nPara two is fine.";
        let judge = MockJudge::new().respond(
            "",
            vec![finding(
                "core/empty-hedge",
                "could perhaps be argued",
                0.9,
                None,
            )],
        );
        let o = LintOptions {
            markdown: Some(true),
            ..Default::default()
        };
        let result = lint_full(text, &o, built_in_set(), &judge, None);
        assert!(
            result
                .diagnostics
                .iter()
                .all(|d| d.tier != Tier::Inferential),
            "{:?}",
            result.diagnostics
        );
    }

    // ---- markdown tables -------------------------------------------------

    #[test]
    fn markdown_tables_are_not_linted_as_prose() {
        // Regression: a pipe table used to parse as one giant Paragraph with
        // no sentence terminators — core/sentence-length fired ("Sentence is
        // 56 words") and the score sank for a document that is only a table.
        let mut src = String::from("| column one | column two |\n|---|---|\n");
        for i in 0..5 {
            src.push_str(&format!("| row {i} cell alpha | row {i} cell beta |\n"));
        }
        let o = LintOptions {
            markdown: Some(true),
            ..Default::default()
        };
        let result = lint(&src, &o);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.stats.score, 100);
    }

    #[test]
    fn tier3_respects_suppression_comments() {
        let text =
            "<!-- lawlint-disable-next-line empty-hedge -->\nIt could perhaps be argued that the claim fails.";
        let judge = MockJudge::new().respond(
            "",
            vec![finding(
                "core/empty-hedge",
                "could perhaps be argued",
                0.9,
                None,
            )],
        );
        let o = LintOptions {
            markdown: Some(true),
            enable: Some(vec!["empty-hedge".into()]),
            ..Default::default()
        };
        let result = lint_full(text, &o, built_in_set(), &judge, None);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn tier3_disabled_rule_produces_no_requests() {
        let text = "It could perhaps be argued that the claim fails.";
        let judge = MockJudge::new().respond(
            "",
            vec![finding(
                "core/empty-hedge",
                "could perhaps be argued",
                0.9,
                None,
            )],
        );
        let o = LintOptions {
            disable: Some(vec!["empty-hedge".into(), "padded-elaboration".into()]),
            ..Default::default()
        };
        let result = lint_full(text, &o, built_in_set(), &judge, None);
        assert_eq!(judge.calls(), 0); // no rubrics → no chunks → no calls
        assert!(result
            .diagnostics
            .iter()
            .all(|d| d.tier != Tier::Inferential));
        assert_eq!(result.judge.as_ref().unwrap().chunks, 0);
    }

    #[test]
    fn lint_result_round_trips_through_json() {
        let result = lint("We delve — again; furthermore.", &LintOptions::default());
        let json = serde_json::to_string(&result).unwrap();
        let back: LintResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.diagnostics.len(), result.diagnostics.len());
        assert_eq!(back.stats.word_count, result.stats.word_count);
    }
}
