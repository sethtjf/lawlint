//! Remediation prompt: turn a lint result into instructions for an AI model.
//!
//! Most rules cannot be fixed mechanically — the correct rewrite depends on
//! context only a model (or human) can judge. Since the flagged text was
//! usually AI-generated in the first place, the natural remediation loop is:
//! lint, hand the model a precise revision brief, re-lint the result. This
//! module builds that brief: every violated rule with its rationale and
//! before/after examples, then every concrete violation to revise.
//!
//! The output is fed verbatim to a model, so it is a work order, not a chat
//! message. The structure is deterministic — preamble, one `## <rule-id>`
//! section per violated rule in first-violation order, a closing line — so
//! callers and tests can rely on it.

use std::fmt::Write as _;

use crate::rule::RuleMeta;
use crate::types::LintResult;
use crate::RuleSet;

/// Build a revision brief for `result`'s diagnostics. Returns `None` when
/// there are no diagnostics — an empty brief would instruct a model to do
/// nothing. Rules appear in first-violation order; instances in document
/// order under each rule.
pub fn remediation_prompt(result: &LintResult, rules: &RuleSet) -> Option<String> {
    if result.diagnostics.is_empty() {
        return None;
    }

    let metas = rules.metas();
    let meta_for = |id: &str| -> Option<&RuleMeta> { metas.iter().copied().find(|m| m.id.0 == id) };

    // Violated rules in order of first appearance in the document.
    let mut order: Vec<&str> = Vec::new();
    for d in &result.diagnostics {
        if !order.contains(&d.rule_id.0.as_str()) {
            order.push(&d.rule_id.0);
        }
    }

    let mut out = String::new();
    out.push_str(PREAMBLE);

    for rule_id in order {
        let _ = write!(out, "\n## {rule_id}\n");

        if let Some(meta) = meta_for(rule_id) {
            let _ = writeln!(out, "{}", meta.description);
            if let Some(rationale) = &meta.rationale {
                let _ = writeln!(out, "{rationale}");
            }
            for ex in &meta.examples {
                let _ = writeln!(out, "Before: {}", ex.bad);
                let _ = writeln!(out, "After: {}", ex.good);
            }
        }

        out.push_str("Findings:\n");
        for d in result.diagnostics.iter().filter(|d| d.rule_id.0 == rule_id) {
            let _ = write!(
                out,
                "- line {}, column {}: \"{}\"",
                d.line, d.column, d.excerpt
            );
            if let Some(suggestion) = &d.suggestion {
                let _ = write!(out, " (suggestion: {suggestion})");
            }
            out.push('\n');
        }
    }

    out.push('\n');
    out.push_str(CLOSING);
    Some(out)
}

const PREAMBLE: &str = "\
Revise the document that follows. It was flagged by lawlint, a linter for \
legal and general prose. Apply the fixes described below and return the \
corrected document.

Hard constraints:
- Preserve the document's meaning and legal precision exactly.
- Change only the text a finding covers. Leave every other character \
byte-for-byte identical, including whitespace, capitalization, and \
punctuation.
- Do not introduce new stylistic problems while fixing the listed ones.
- Return only the revised document text. Do not add commentary, \
explanations, or code fences.

Each section names a violated rule, explains it, gives before/after \
examples, then lists the findings to fix.
";

const CLOSING: &str =
    "Return only the revised document text, with every listed finding fixed and nothing else changed.\n";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LintOptions;
    use crate::lint_with;
    use crate::types::{LintResult, Stats};

    fn built_in() -> RuleSet {
        RuleSet::built_in()
    }

    #[test]
    fn none_when_no_diagnostics() {
        let result = LintResult {
            diagnostics: vec![],
            stats: Stats {
                word_count: 0,
                sentence_count: 0,
                score: 100,
            },
            judge: None,
        };
        assert!(remediation_prompt(&result, &built_in()).is_none());
    }

    #[test]
    fn sections_ordered_by_first_violation_with_examples_and_excerpt() {
        let rules = built_in();
        // "delve" (core/no-ai-cliches) appears before the em dash
        // (core/no-em-dash), so the cliché section must come first.
        let text = "We should delve into this matter—now.";
        let result = lint_with(text, &LintOptions::default(), &rules);
        let prompt = remediation_prompt(&result, &rules).expect("has diagnostics");

        let cliches = prompt
            .find("## core/no-ai-cliches")
            .expect("cliché section present");
        let em_dash = prompt
            .find("## core/no-em-dash")
            .expect("em-dash section present");
        assert!(cliches < em_dash, "sections ordered by first violation");

        // Before:/After: lines come straight from each rule's YAML examples.
        assert!(prompt.contains("Before: We should delve into this issue."));
        assert!(prompt.contains("After: We should examine this issue."));
        assert!(prompt.contains("Before: It was—frankly—wrong."));

        // The offending source line is surfaced as the finding excerpt.
        assert!(prompt.contains("We should delve into this matter"));

        // Framing is present and stable.
        assert!(prompt.starts_with("Revise the document that follows."));
        assert!(prompt.trim_end().ends_with("nothing else changed."));
    }

    #[test]
    fn finding_includes_suggestion_when_present() {
        let rules = built_in();
        let text = "It was—wrong.";
        let result = lint_with(text, &LintOptions::default(), &rules);
        // The em-dash rule carries a suggestion on every finding.
        assert!(result
            .diagnostics
            .iter()
            .any(|d| d.rule_id.0 == "core/no-em-dash" && d.suggestion.is_some()));

        let prompt = remediation_prompt(&result, &rules).expect("has diagnostics");
        assert!(prompt.contains("(suggestion: "));
    }
}
