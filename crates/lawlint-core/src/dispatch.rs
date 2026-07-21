//! Single-pass dispatcher: traversal, scope mask, suppression. [integration]
//!
//! One traversal. Walk blocks → sentences → tokens; call subscribed rules
//! whose `scope` admits the node (Prose/Text/All + citation exclusion); then
//! `on_document_exit` for all. Collect `Report`s per rule → stamp
//! rule_id/severity/tier. Scope masking is enforced HERE, not in engines:
//! any report whose span falls outside the rule's scope mask is dropped.
//!
//! Suppression (also here): case-insensitive scan of source lines;
//! `lawlint-disable-next-line [ids…]` (bare or inside `<!-- -->` / `//`)
//! suppresses on the next non-blank line; `lawlint-disable [ids…]` …
//! `lawlint-enable [ids…]` block-scoped; `lawlint-disable-file` at top.
//! No ids = all rules. Ids resolve through aliases.

use std::collections::HashSet;

use crate::config::LintOptions;
use crate::document::{BlockKind, Document, Sentence};
use crate::registry::RuleSet;
use crate::rule::{Ctx, Rule};
use crate::scoring;
use crate::types::{Diagnostic, RuleId, Scope, TextRange};

// ---- Scope admission ---------------------------------------------------

fn block_admitted(scope: Scope, kind: BlockKind) -> bool {
    match scope {
        Scope::All => true,
        Scope::Text => kind != BlockKind::CodeBlock,
        Scope::Prose => matches!(kind, BlockKind::Paragraph | BlockKind::ListItem),
    }
}

fn sentence_admitted(scope: Scope, block_kind: BlockKind, s: &Sentence) -> bool {
    match scope {
        Scope::All => true,
        // Text includes citation sentences (and headings/quotes).
        Scope::Text => block_kind != BlockKind::CodeBlock,
        Scope::Prose => {
            matches!(block_kind, BlockKind::Paragraph | BlockKind::ListItem) && !s.is_citation
        }
    }
}

/// Sorted, non-overlapping ranges a rule of the given scope may report into.
/// Shared with `lint_full` so tier-3 diagnostics obey the same mask.
pub(crate) fn scope_mask(scope: Scope, source: &str, doc: &Document) -> Vec<TextRange> {
    match scope {
        Scope::All => vec![TextRange {
            start: 0,
            end: source.len(),
        }],
        Scope::Text => doc
            .blocks
            .iter()
            .filter(|b| b.kind != BlockKind::CodeBlock)
            .map(|b| b.range)
            .collect(),
        Scope::Prose => doc
            .blocks
            .iter()
            .filter(|b| matches!(b.kind, BlockKind::Paragraph | BlockKind::ListItem))
            .flat_map(|b| b.sentences.iter())
            .filter(|s| !s.is_citation)
            .map(|s| s.range)
            .collect(),
    }
}

/// True when `span` sits entirely inside one mask range. Ranges are sorted by
/// construction (document order).
pub(crate) fn mask_contains(mask: &[TextRange], span: &TextRange) -> bool {
    let idx = mask.partition_point(|r| r.start <= span.start);
    idx > 0 && mask[idx - 1].contains(span)
}

// ---- Suppression comments ----------------------------------------------

#[derive(Debug, Default, Clone)]
struct LineSuppression {
    all: bool,
    ids: HashSet<String>,
}

impl LineSuppression {
    fn suppresses(&self, id: &RuleId) -> bool {
        self.all || self.ids.contains(&id.0)
    }
    fn merge(&mut self, other: &LineSuppression) {
        self.all |= other.all;
        self.ids.extend(other.ids.iter().cloned());
    }
}

/// Parsed suppression state for one source. Shared with `lint_full` so tier-3
/// diagnostics honor the same comments as tiers 1–2.
#[derive(Debug)]
pub(crate) struct Suppressions {
    line_starts: Vec<usize>,
    /// Per line (0-based), what is suppressed on it.
    per_line: Vec<LineSuppression>,
    file: LineSuppression,
}

/// Directive ids: tokens after the directive keyword, stripped of comment
/// closers and separators, resolved through the rule set's aliases.
fn parse_ids(rest: &str, rule_set: &RuleSet) -> (bool, HashSet<String>) {
    let rest = rest
        .replace("-->", " ")
        .replace("*/", " ")
        .replace(',', " ");
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    if tokens.is_empty() {
        return (true, HashSet::new()); // no ids = all rules
    }
    let ids = tokens
        .iter()
        .filter_map(|t| {
            rule_set
                .resolve(t)
                .or_else(|| rule_set.resolve(&t.to_lowercase()))
        })
        .map(|id| id.0.clone())
        .collect();
    (false, ids)
}

/// Byte offset (into the ORIGINAL line) of the first ASCII-case-insensitive
/// occurrence of `needle`. `line.to_lowercase()` cannot be used to find
/// directive positions: Unicode lowercasing is not byte-length-preserving
/// (e.g. 'İ' U+0130 is 2 bytes but its lowercase form "i\u{307}" is 3), so an
/// offset found in the lowered copy may over/under-shoot the original —
/// panicking or mis-slicing the id list. Directive keywords are pure ASCII,
/// so a byte-wise ASCII-case-insensitive scan is exact and every match starts
/// and ends on a char boundary.
fn find_directive(line: &str, needle: &str) -> Option<usize> {
    line.as_bytes()
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

impl Suppressions {
    pub(crate) fn new(source: &str, rule_set: &RuleSet) -> Self {
        let line_starts = scoring::line_starts(source);
        let lines: Vec<&str> = source.split('\n').collect();
        let mut per_line: Vec<LineSuppression> = vec![LineSuppression::default(); lines.len()];
        let mut file = LineSuppression::default();

        // Block-scoped running state.
        let mut active = LineSuppression::default();
        // (target line, suppression) from disable-next-line directives.
        let mut next_line: Vec<(usize, LineSuppression)> = Vec::new();

        for (i, line) in lines.iter().enumerate() {
            // A line carrying any directive is a comment: findings on it are
            // suppressed for everything (avoids flagging the directive text
            // itself, e.g. "delve" inside "no-delve").
            let mut directive_line = false;
            if let Some(pos) = find_directive(line, "lawlint-disable-file") {
                directive_line = true;
                let rest = &line[pos + "lawlint-disable-file".len()..];
                let (all, ids) = parse_ids(rest, rule_set);
                file.merge(&LineSuppression { all, ids });
            } else if let Some(pos) = find_directive(line, "lawlint-disable-next-line") {
                directive_line = true;
                let rest = &line[pos + "lawlint-disable-next-line".len()..];
                let (all, ids) = parse_ids(rest, rule_set);
                // Applies to the next non-blank line.
                if let Some(target) = lines
                    .iter()
                    .enumerate()
                    .skip(i + 1)
                    .find(|(_, l)| !l.trim().is_empty())
                    .map(|(j, _)| j)
                {
                    next_line.push((target, LineSuppression { all, ids }));
                }
            } else if let Some(pos) = find_directive(line, "lawlint-disable") {
                directive_line = true;
                let rest = &line[pos + "lawlint-disable".len()..];
                let (all, ids) = parse_ids(rest, rule_set);
                if all {
                    active.all = true;
                } else {
                    active.ids.extend(ids);
                }
            } else if let Some(pos) = find_directive(line, "lawlint-enable") {
                directive_line = true;
                let rest = &line[pos + "lawlint-enable".len()..];
                let (all, ids) = parse_ids(rest, rule_set);
                if all {
                    active = LineSuppression::default();
                } else {
                    active.all = false;
                    for id in &ids {
                        active.ids.remove(id);
                    }
                }
            }
            per_line[i] = active.clone();
            if directive_line {
                per_line[i].all = true;
            }
        }
        for (target, supp) in next_line {
            per_line[target].merge(&supp);
        }

        Suppressions {
            line_starts,
            per_line,
            file,
        }
    }

    /// Is a diagnostic for `id` starting at byte `offset` suppressed?
    pub(crate) fn suppressed(&self, id: &RuleId, offset: usize) -> bool {
        if self.file.suppresses(id) {
            return true;
        }
        let line = self.line_starts.partition_point(|&s| s <= offset).max(1) - 1;
        self.per_line.get(line).is_some_and(|l| l.suppresses(id))
    }
}

// ---- Threshold resolution ----------------------------------------------

/// The `options.thresholds` override for a rule, if any key (full id or
/// alias) resolves to it. An exact full-id key wins over an alias.
fn threshold_for(options: &LintOptions, rule_set: &RuleSet, id: &RuleId) -> Option<f64> {
    let thresholds = options.thresholds.as_ref()?;
    if let Some(v) = thresholds.get(&id.0) {
        return Some(*v);
    }
    thresholds
        .iter()
        .find(|(k, _)| rule_set.resolve(k) == Some(id))
        .map(|(_, v)| *v)
}

// ---- Dispatch ----------------------------------------------------------

struct Slot {
    rule: Box<dyn Rule>,
    threshold: Option<f64>,
    diagnostics: Vec<Diagnostic>,
}

impl Slot {
    /// Drain `ctx`'s reports, stamping this rule's id/severity/tier.
    fn collect(&mut self, ctx: &mut Ctx) {
        let meta = self.rule.meta();
        let (id, severity, tier, intent) = (meta.id.clone(), meta.severity, meta.tier, meta.intent);
        for r in ctx.take_reports() {
            self.diagnostics.push(Diagnostic {
                rule_id: id.clone(),
                severity,
                tier,
                intent,
                span: r.span,
                message: r.message,
                line: 0,
                column: 0,
                end_line: None,
                end_column: None,
                excerpt: String::new(),
                suggestion: r.suggestion,
                weight: r.weight,
                confidence: None,
                fix: r.fix,
            });
        }
    }
}

/// Run all rules over the document and return raw (pre-finalize)
/// diagnostics: spans stamped with rule id/severity/tier, but no
/// line/column/excerpt yet. `rules` come from `rule_set.instantiate()`;
/// `rule_set` is also needed for suppression-comment alias resolution.
/// [integration]
pub fn run(
    source: &str,
    doc: &Document,
    rules: Vec<Box<dyn Rule>>,
    options: &LintOptions,
    rule_set: &RuleSet,
) -> Vec<Diagnostic> {
    let word_count = scoring::word_count(source, doc);
    let mut ctx = Ctx::new(source, word_count);

    let mut slots: Vec<Slot> = rules
        .into_iter()
        .map(|rule| {
            let threshold = threshold_for(options, rule_set, &rule.meta().id);
            Slot {
                rule,
                threshold,
                diagnostics: Vec::new(),
            }
        })
        .collect();

    // Single pass: blocks → sentences → tokens.
    for block in &doc.blocks {
        for slot in slots.iter_mut() {
            let (interests, scope) = (slot.rule.interests(), slot.rule.meta().scope);
            if interests.blocks && block_admitted(scope, block.kind) {
                ctx.set_threshold_override(slot.threshold);
                slot.rule.check_block(block, &mut ctx);
                slot.collect(&mut ctx);
            }
        }
        for sentence in &block.sentences {
            for slot in slots.iter_mut() {
                let (interests, scope) = (slot.rule.interests(), slot.rule.meta().scope);
                if interests.sentences && sentence_admitted(scope, block.kind, sentence) {
                    ctx.set_threshold_override(slot.threshold);
                    slot.rule.check_sentence(sentence, &mut ctx);
                    slot.collect(&mut ctx);
                }
            }
            for token in &sentence.tokens {
                for slot in slots.iter_mut() {
                    let (interests, scope) = (slot.rule.interests(), slot.rule.meta().scope);
                    if interests.tokens && sentence_admitted(scope, block.kind, sentence) {
                        ctx.set_threshold_override(slot.threshold);
                        slot.rule.check_token(token, &mut ctx);
                        slot.collect(&mut ctx);
                    }
                }
            }
        }
    }
    for slot in slots.iter_mut() {
        if slot.rule.interests().document_exit {
            ctx.set_threshold_override(slot.threshold);
            slot.rule.on_document_exit(doc, &mut ctx);
            slot.collect(&mut ctx);
        }
    }

    // Scope-mask filter (belt and suspenders) + suppression comments.
    let suppressions = Suppressions::new(source, rule_set);
    let mut out = Vec::new();
    for slot in slots {
        if slot.diagnostics.is_empty() {
            continue;
        }
        let mask = scope_mask(slot.rule.meta().scope, source, doc);
        for d in slot.diagnostics {
            if !mask_contains(&mask, &d.span) {
                continue;
            }
            if suppressions.suppressed(&d.rule_id, d.span.start) {
                continue;
            }
            out.push(d);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::parse;
    use crate::loader::{parse_manifest, parse_rule};
    use crate::registry::RuleSet;

    fn set(rules: &[(&str, &str)]) -> RuleSet {
        let manifest = parse_manifest("style.yaml", "name: core\nversion: 0.1.0\n").unwrap();
        let parsed = rules
            .iter()
            .map(|(file, yaml)| {
                let markdown = format!("---\n{yaml}\n---\n");
                ((*file).to_string(), parse_rule(file, &markdown).unwrap())
            })
            .collect();
        RuleSet::from_parts(&manifest, parsed).unwrap()
    }

    fn phrase_set(scope: &str) -> RuleSet {
        set(&[(
            "r.yaml",
            &format!(
                "id: no-delve\nengine: phrase\nscope: {scope}\nseverity: warning\n\
                 message: \"no delve\"\npatterns: ['(?i)\\bdelve\\b']\n"
            ),
        )])
    }

    fn run_on(
        source: &str,
        markdown: bool,
        rs: &RuleSet,
        options: &LintOptions,
    ) -> Vec<Diagnostic> {
        let doc = parse(source, markdown);
        run(source, &doc, rs.instantiate(options), options, rs)
    }

    #[test]
    fn stamps_id_severity_tier_and_leaves_positions_for_finalize() {
        let rs = phrase_set("text");
        let diags = run_on("We delve here.", false, &rs, &LintOptions::default());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule_id.0, "core/no-delve");
        assert_eq!(diags[0].severity, crate::types::Severity::Warning);
        assert_eq!(diags[0].tier, crate::types::Tier::Static);
        assert_eq!(diags[0].line, 0); // finalize fills positions
        assert_eq!(diags[0].message, "no delve");
    }

    #[test]
    fn text_scope_skips_code_blocks_but_hits_headings() {
        let src = "# We delve\n\n```\ndelve\n```\n\nClean paragraph.";
        let rs = phrase_set("text");
        let diags = run_on(src, true, &rs, &LintOptions::default());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].span.slice(src), "delve");
        assert!(diags[0].span.start < src.find("```").unwrap());
    }

    #[test]
    fn all_scope_includes_code_blocks() {
        let src = "Clean.\n\n```\nwe delve\n```\n";
        let rs = phrase_set("all");
        let diags = run_on(src, true, &rs, &LintOptions::default());
        assert_eq!(diags.len(), 1);
        assert!(diags[0].span.start > src.find("```").unwrap());
    }

    #[test]
    fn prose_scope_skips_headings_and_citation_sentences() {
        let src = "# delve heading\n\nSee Roe v. Wade, 410 U.S. 113 (1973). We delve after.\n";
        let rs = phrase_set("prose");
        let diags = run_on(src, true, &rs, &LintOptions::default());
        // Heading skipped (block not admitted); paragraph match kept only in
        // the non-citation sentence.
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].span.slice(src), "delve");
        assert!(diags[0].span.start > src.find("(1973)").unwrap());
    }

    #[test]
    fn prose_scope_mask_drops_citation_span_reports() {
        // A phrase rule runs per block, so a Prose rule sees the citation
        // sentence's text; the scope mask must drop reports landing there.
        let src = "See Roe v. Wade, 410 U.S. 113 (1973). Nothing else here.";
        let rs = phrase_set("prose");
        let doc = parse(src, false);
        assert!(doc.blocks[0].sentences[0].is_citation);
        // "Wade" only appears inside the citation sentence.
        let rs2 = set(&[(
            "r.yaml",
            "id: no-wade\nengine: phrase\nscope: prose\nmessage: m\npatterns: ['Wade']\n",
        )]);
        let _ = rs;
        let diags = run(
            src,
            &doc,
            rs2.instantiate(&LintOptions::default()),
            &LintOptions::default(),
            &rs2,
        );
        assert!(diags.is_empty());
    }

    #[test]
    fn threshold_override_resolves_via_alias_and_full_id() {
        let rs = set(&[(
            "d.yaml",
            "id: dens\nengine: density\nmessage: m\nthreshold: 1000\npatterns: ['x']\n",
        )]);
        let src = "x x x x";
        // Default threshold 1000 per 1000 words: 4 matches in 4 words = 1000,
        // not strictly above → silent.
        assert!(run_on(src, false, &rs, &LintOptions::default()).is_empty());
        // Alias key lowers it.
        let options = LintOptions {
            thresholds: Some([("dens".to_string(), 1.0)].into_iter().collect()),
            ..Default::default()
        };
        let diags = run_on(src, false, &rs, &options);
        assert_eq!(diags.len(), 1);
        // Full-id key too.
        let options = LintOptions {
            thresholds: Some([("core/dens".to_string(), 1.0)].into_iter().collect()),
            ..Default::default()
        };
        assert_eq!(run_on(src, false, &rs, &options).len(), 1);
    }

    #[test]
    fn statistical_block_entry_precedes_sentences() {
        // Two blocks with runs that only fire if the run resets between
        // blocks via check_block-on-entry ordering.
        let rs = set(&[(
            "s.yaml",
            "id: rep\nengine: statistical\nmetric: repetitive-openers\n",
        )]);
        let src = "The a. The b.\n\nThe c. The d. The e.";
        let diags = run_on(src, false, &rs, &LintOptions::default());
        // Without the reset, sentence 3 ("The c.") would complete a run of 3.
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].span.slice(src), "The e.");
    }

    #[test]
    fn density_word_count_is_scope_aware() {
        // Density M comes from ctx.word_count (code blanked).
        let rs = set(&[(
            "d.yaml",
            "id: dens\nengine: density\nmessage: m\nthreshold: 10\npatterns: ['perhaps']\n",
        )]);
        let src = "perhaps one two.\n\n```\nfiller filler filler filler filler filler\n```\n";
        let diags = run_on(src, true, &rs, &LintOptions::default());
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.contains("(1 occurrences in 3 words)"),
            "{}",
            diags[0].message
        );
    }

    // ---- suppression ---------------------------------------------------

    #[test]
    fn disable_next_line_bare_and_html_comment() {
        let rs = phrase_set("text");
        let src = "lawlint-disable-next-line\nWe delve here.\nWe delve again.";
        let diags = run_on(src, false, &rs, &LintOptions::default());
        assert_eq!(diags.len(), 1);
        assert!(diags[0].span.start >= src.rfind("delve").unwrap());

        let src = "<!-- lawlint-disable-next-line no-delve -->\nWe delve here.";
        assert!(run_on(src, true, &rs, &LintOptions::default()).is_empty());
    }

    #[test]
    fn disable_next_line_skips_blank_lines() {
        let rs = phrase_set("text");
        let src = "// lawlint-disable-next-line no-delve\n\nWe delve here.";
        assert!(run_on(src, false, &rs, &LintOptions::default()).is_empty());
    }

    #[test]
    fn disable_next_line_with_other_id_does_not_suppress() {
        let rs = set(&[
            (
                "a.yaml",
                "id: no-delve\nengine: phrase\nmessage: m\npatterns: ['(?i)\\bdelve\\b']\n",
            ),
            (
                "b.yaml",
                "id: no-tapestry\nengine: phrase\nmessage: m\npatterns: ['(?i)\\btapestry\\b']\n",
            ),
        ]);
        let src = "<!-- lawlint-disable-next-line no-tapestry -->\nWe delve into tapestry.";
        let diags = run_on(src, false, &rs, &LintOptions::default());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule_id.0, "core/no-delve");
    }

    #[test]
    fn disable_enable_block_scoped() {
        let rs = phrase_set("text");
        let src = "We delve one.\n<!-- lawlint-disable -->\nWe delve two.\nWe delve three.\n<!-- lawlint-enable -->\nWe delve four.";
        let diags = run_on(src, false, &rs, &LintOptions::default());
        let excerpts: Vec<&str> = diags.iter().map(|d| d.span.slice(src)).collect();
        assert_eq!(excerpts.len(), 2, "{excerpts:?}");
        // Lines "two"/"three" suppressed; "one" and "four" kept.
        let lines: Vec<usize> = diags
            .iter()
            .map(|d| src[..d.span.start].matches('\n').count())
            .collect();
        assert_eq!(lines, vec![0, 5]);
    }

    #[test]
    fn disable_enable_with_ids_resolve_through_aliases() {
        let rs = phrase_set("text");
        let src = "<!-- lawlint-disable core/no-delve -->\nWe delve.\n<!-- lawlint-enable no-delve -->\nWe delve again.";
        let diags = run_on(src, false, &rs, &LintOptions::default());
        assert_eq!(diags.len(), 1);
        assert!(diags[0].span.start > src.find("enable").unwrap());
    }

    #[test]
    fn disable_file_suppresses_everything() {
        let rs = phrase_set("text");
        let src = "<!-- lawlint-disable-file -->\nWe delve. We delve more.";
        assert!(run_on(src, false, &rs, &LintOptions::default()).is_empty());
    }

    #[test]
    fn unknown_ids_in_directives_are_ignored() {
        let rs = phrase_set("text");
        // Unknown id resolves to nothing → suppresses nothing.
        let src = "<!-- lawlint-disable-next-line no-such -->\nWe delve.";
        assert_eq!(run_on(src, false, &rs, &LintOptions::default()).len(), 1);
    }

    #[test]
    fn directives_on_lines_with_multibyte_case_shifts_do_not_panic_and_apply() {
        // 'İ' (U+0130) is 2 bytes but lowercases to the 3-byte "i\u{307}":
        // offsets computed in a lowered copy of the line would over-shoot the
        // original (panic) or mis-slice the id list (silently dropping the
        // suppression). Regression for both failure modes.
        let rs = phrase_set("text");

        // Block directive: used to panic slicing past the end of the line.
        let src = "İİ lawlint-disable\nWe delve here.";
        assert!(run_on(src, false, &rs, &LintOptions::default()).is_empty());

        // disable-file: same panic.
        let src = "İ lawlint-disable-file\nWe delve here.";
        assert!(run_on(src, false, &rs, &LintOptions::default()).is_empty());

        // next-line with ids: used to slice the id list 2 bytes off, so the
        // suppression silently failed.
        let src = "İstanbul İzmir lawlint-disable-next-line no-delve\nWe delve here.";
        assert!(run_on(src, false, &rs, &LintOptions::default()).is_empty());

        // Bare next-line after a multibyte prefix suppresses only line 2.
        let src = "TİTLE lawlint-disable-next-line\nWe delve here.\nWe delve again.";
        let diags = run_on(src, false, &rs, &LintOptions::default());
        assert_eq!(diags.len(), 1);
        assert!(diags[0].span.start >= src.rfind("delve").unwrap());
    }

    #[test]
    fn directives_are_case_insensitive() {
        let rs = phrase_set("text");
        let src = "<!-- LAWLINT-DISABLE-NEXT-LINE -->\nWe delve.";
        assert!(run_on(src, false, &rs, &LintOptions::default()).is_empty());
    }
}
