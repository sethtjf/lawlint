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

/// Where the linted document lives, which decides how the brief hands it to
/// the model.
pub enum PromptSource<'a> {
    /// Inline text (stdin, a TUI buffer, the playground). The brief embeds
    /// the document so it is self-contained.
    Text(&'a str),
    /// A file on disk. The brief references the path and instructs the model
    /// to edit the file in place — embedding a large document would blow up
    /// the receiving agent's context, and it can read the file itself.
    File(&'a str),
}

/// Build a revision brief for `result`'s diagnostics over the document in
/// `source`. Returns `None` when there are no diagnostics — an empty brief
/// would instruct a model to do nothing. Rules appear in first-violation
/// order; instances in document order under each rule.
pub fn remediation_prompt(
    source: PromptSource<'_>,
    result: &LintResult,
    rules: &RuleSet,
) -> Option<String> {
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
    match &source {
        PromptSource::Text(_) => out.push_str(
            "Revise the document that follows. It was flagged by lawlint, a \
             linter for legal and general prose. Apply the fixes described \
             below and return the corrected document.\n",
        ),
        PromptSource::File(path) => {
            let _ = writeln!(
                out,
                "Revise the document at `{path}`. It was flagged by lawlint, \
                 a linter for legal and general prose. Read the file, apply \
                 the fixes described below, and write the corrected document \
                 back to the same path."
            );
        }
    }
    out.push_str(CONSTRAINTS);
    match &source {
        PromptSource::Text(_) => out.push_str(
            "- Return only the revised document text. Do not add commentary, \
             explanations, or code fences.\n",
        ),
        PromptSource::File(_) => out.push_str(
            "- Line numbers in the findings refer to the file as it exists \
             now, before any edits.\n",
        ),
    }
    out.push_str(
        "\nEach section names a violated rule, explains it, gives \
         before/after examples, then lists the findings to fix.\n",
    );

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
            // Line + excerpt locate the finding; a column offset would be
            // measured against the untrimmed source line, not the trimmed
            // excerpt quoted here, so it is omitted rather than misleading.
            let _ = write!(out, "- line {}: \"{}\"", d.line, d.excerpt);
            if let Some(suggestion) = &d.suggestion {
                let _ = write!(out, " (suggestion: {suggestion})");
            }
            out.push('\n');
        }
    }

    match &source {
        PromptSource::Text(text) => {
            out.push_str("\n---\nDocument to revise:\n\n");
            out.push_str(text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("---\n\n");
            out.push_str(
                "Return only the revised document text, with every listed \
                 finding fixed and nothing else changed.\n",
            );
        }
        PromptSource::File(path) => {
            let _ = writeln!(
                out,
                "\nEdit `{path}` in place, fixing every listed finding and \
                 changing nothing else."
            );
        }
    }
    Some(out)
}

const CONSTRAINTS: &str = "
Hard constraints:
- Preserve the document's meaning and legal precision exactly.
- Change only the text a finding covers. Leave every other character \
byte-for-byte identical, including whitespace, capitalization, and \
punctuation.
- Do not introduce new stylistic problems while fixing the listed ones.
";

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
        assert!(remediation_prompt(PromptSource::Text(""), &result, &built_in()).is_none());
    }

    #[test]
    fn sections_ordered_by_first_violation_with_examples_and_excerpt() {
        let rules = built_in();
        // "delve" (core/no-ai-cliches) appears before the em dash
        // (core/no-em-dash), so the cliché section must come first.
        let text = "We should delve into this matter—now.";
        let result = lint_with(text, &LintOptions::default(), &rules);
        let prompt =
            remediation_prompt(PromptSource::Text(text), &result, &rules).expect("has diagnostics");

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

        // The brief is self-contained: the document itself is embedded.
        assert!(prompt.contains("Document to revise:\n\nWe should delve into this matter—now.\n"));

        // Framing is present and stable.
        assert!(prompt.starts_with("Revise the document that follows."));
        assert!(prompt.trim_end().ends_with("nothing else changed."));
    }

    #[test]
    fn file_source_references_path_and_never_embeds_the_document() {
        let rules = built_in();
        let text = "We should delve into this matter—now.";
        let result = lint_with(text, &LintOptions::default(), &rules);
        let prompt = remediation_prompt(PromptSource::File("docs/brief.md"), &result, &rules)
            .expect("has diagnostics");

        assert!(prompt.contains("Revise the document at `docs/brief.md`."));
        assert!(prompt.contains("Edit `docs/brief.md` in place"));
        // The document body must NOT be embedded — that is the point of the
        // file variant. Excerpts inside findings still quote flagged lines.
        assert!(!prompt.contains("Document to revise:"));
        assert!(!prompt.contains("Return only the revised document text"));
        assert!(prompt.contains("Line numbers in the findings refer to the file"));
    }

    #[test]
    fn multiple_findings_of_one_rule_list_in_document_order() {
        let rules = built_in();
        let text = "Pursuant to the lease, rent is due.\nFees accrue pursuant to the rider.";
        let result = lint_with(text, &LintOptions::default(), &rules);
        let prompt =
            remediation_prompt(PromptSource::Text(text), &result, &rules).expect("has diagnostics");

        let section = prompt
            .find("## core/no-legalese")
            .expect("legalese section present");
        let first = prompt[section..].find("- line 1:").expect("line 1 finding");
        let second = prompt[section..].find("- line 2:").expect("line 2 finding");
        assert!(first < second, "instances listed in document order");
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

        let prompt =
            remediation_prompt(PromptSource::Text(text), &result, &rules).expect("has diagnostics");
        assert!(prompt.contains("(suggestion: "));
    }
}
