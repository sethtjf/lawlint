//! Phrase engine. [agent B]
//!
//! List of `{ regex, message?, suggestion?, fix? }`. Interest: blocks. Run
//! each regex on `block.range.slice(source)`; report at absolute offsets.
//! Optional `allow_context: { pattern, window }`: expand match by `window`
//! bytes each side (clamped to char boundaries); if the pattern matches the
//! expanded slice, skip. A `fix` string on an item makes a MachineApplicable
//! single-edit Fix.
//!
//! `fix` is either a literal replacement, or — if it contains a `$` — an
//! expansion string in `regex`'s own `Captures::expand` grammar: `$1`/`${1}`
//! (numbered, `0` = whole match), `$name`/`${name}` (named groups), `$$` for
//! a literal `$`. Groups the pattern can never define are a load error (see
//! `loader::validate_fix_capture_refs`), checked both at load time and again
//! defensively in `from_def`, so the two validators can never disagree.
//! `match_case` is applied to a plain literal fix (including one that's
//! ref-free after `$$`-unescaping — that text is still author-literal), but
//! *not* to an expansion that actually pulled in a capture group: the
//! captured text already carries the source's own casing per group, and
//! re-casing the assembled result (e.g. uppercasing its first character
//! because the *whole match* happened to start uppercase) would corrupt
//! text that a reordering fix legitimately moved around.

use regex::Regex;

use crate::document::Block;
use crate::error::LoadError;
use crate::loader::{self, PatternDef, RuleDef};
use crate::rule::{Ctx, Interests, Report, Rule, RuleMeta};
use crate::types::{Applicability, Edit, Fix, TextRange};

/// A compiled `fix:` replacement. `Literal` never touches `$`-parsing and
/// stays on the plain `find_iter` path (byte-identical to pre-capture-group
/// behavior); `Expand` is only used for a fix string that actually contains
/// `$`, and switches that item to `captures_iter`.
#[derive(Debug)]
enum FixSpec {
    Literal(String),
    Expand {
        replacement: String,
        /// True when the replacement contains a real capture reference (not
        /// just a `$$` escape) — determines whether `match_case` applies.
        has_refs: bool,
    },
}

/// One compiled `patterns:` item.
#[derive(Debug)]
struct PhraseItem {
    regex: Regex,
    /// Per-item message override; falls back to the rule's default message.
    message: Option<String>,
    suggestion: Option<String>,
    /// `fix:` compiled into a literal or capture-expansion spec.
    fix: Option<FixSpec>,
}

#[derive(Debug)]
pub struct PhraseEngine {
    meta: RuleMeta,
    items: Vec<PhraseItem>,
    /// Rule-level default message (`message:` in the def, else description).
    default_message: String,
    /// Compiled `allow_context` (regex, window in bytes).
    allow_context: Option<(Regex, usize)>,
}

impl PhraseEngine {
    /// Build from a validated def. Regex compile errors become `LoadError`
    /// with `file` context.
    pub fn from_def(meta: RuleMeta, def: &RuleDef, file: &str) -> Result<Self, LoadError> {
        let mut items = Vec::with_capacity(def.patterns.len());
        for (i, p) in def.patterns.iter().enumerate() {
            let (pattern, message, suggestion, fix, fix_template) = match p {
                PatternDef::Bare(s) => (s.as_str(), None, None, None, None),
                PatternDef::Detailed {
                    pattern,
                    message,
                    suggestion,
                    fix,
                    fix_template,
                } => (
                    pattern.as_str(),
                    message.clone(),
                    suggestion.clone(),
                    fix.clone(),
                    fix_template.clone(),
                ),
            };
            let field = format!("patterns[{i}]");
            let regex = Regex::new(pattern).map_err(|e| LoadError::InvalidRegex {
                file: file.to_string(),
                field: field.clone(),
                pattern: pattern.to_string(),
                message: e.to_string(),
            })?;
            // `fix` is verbatim; `fixTemplate` interpolates capture groups.
            // Which one the author wrote decides how it is treated — never the
            // content of the string, so a literal `$1` can never be silently
            // reinterpreted as a reference.
            let fix = match (fix, fix_template) {
                (Some(_), Some(_)) => {
                    return Err(LoadError::invalid_field(
                        file,
                        format!("{field}.fixTemplate"),
                        "a pattern may set `fix` (literal) or `fixTemplate` \
                         (capture-group interpolation), not both",
                    ))
                }
                (Some(replacement), None) => Some(FixSpec::Literal(replacement)),
                (None, Some(template)) => {
                    // Defensive re-validation: the loader already checks this
                    // at load time, but `from_def` is also reachable directly
                    // (tests, a future non-Markdown rule source), so it must
                    // reject bad refs rather than let expansion silently
                    // produce empty text.
                    loader::validate_fix_capture_refs(
                        file,
                        &format!("{field}.fixTemplate"),
                        &template,
                        &regex,
                    )?;
                    let has_refs = !loader::scan_capture_refs(&template).is_empty();
                    Some(FixSpec::Expand {
                        replacement: template,
                        has_refs,
                    })
                }
                (None, None) => None,
            };
            items.push(PhraseItem {
                regex,
                message,
                suggestion,
                fix,
            });
        }

        let allow_context = match &def.allow_context {
            Some(ac) => {
                let re = Regex::new(&ac.pattern).map_err(|e| LoadError::InvalidRegex {
                    file: file.to_string(),
                    field: "allow_context.pattern".to_string(),
                    pattern: ac.pattern.clone(),
                    message: e.to_string(),
                })?;
                Some((re, ac.window))
            }
            None => None,
        };

        let default_message = def
            .message
            .clone()
            .unwrap_or_else(|| meta.description.clone());

        Ok(PhraseEngine {
            meta,
            items,
            default_message,
            allow_context,
        })
    }

    /// Adapt `replacement`'s case to `matched`: all-caps match (>= 2 letters,
    /// every letter uppercase) → uppercase the whole replacement; else a match
    /// whose first letter is uppercase → uppercase the replacement's first
    /// char; else verbatim.
    fn match_case(matched: &str, replacement: &str) -> String {
        let mut letters = 0usize;
        let mut all_upper = true;
        let mut first_upper = None;
        for c in matched.chars().filter(|c| c.is_alphabetic()) {
            letters += 1;
            let upper = c.is_uppercase();
            if !upper {
                all_upper = false;
            }
            if first_upper.is_none() {
                first_upper = Some(upper);
            }
        }
        if letters >= 2 && all_upper {
            return replacement.chars().flat_map(char::to_uppercase).collect();
        }
        if first_upper == Some(true) {
            let mut chars = replacement.chars();
            if let Some(first) = chars.next() {
                return first.to_uppercase().chain(chars).collect();
            }
        }
        replacement.to_string()
    }

    /// Expand `[start, end)` by `window` bytes each side, clamped to
    /// `source` char boundaries (and to `[0, source.len()]`).
    fn expand_window(source: &str, start: usize, end: usize, window: usize) -> (usize, usize) {
        let mut lo = start.saturating_sub(window);
        while lo > 0 && !source.is_char_boundary(lo) {
            lo -= 1;
        }
        let mut hi = end.saturating_add(window).min(source.len());
        while hi < source.len() && !source.is_char_boundary(hi) {
            hi += 1;
        }
        (lo, hi)
    }

    /// True if `allow_context` is set and matches the window around
    /// `[start, end)` — i.e. this match should be skipped.
    fn allow_context_skips(&self, source: &str, start: usize, end: usize) -> bool {
        match &self.allow_context {
            Some((allow_re, window)) => {
                let (lo, hi) = Self::expand_window(source, start, end, *window);
                allow_re.is_match(&source[lo..hi])
            }
            None => false,
        }
    }
}

impl Rule for PhraseEngine {
    fn meta(&self) -> &RuleMeta {
        &self.meta
    }

    fn interests(&self) -> Interests {
        Interests {
            blocks: true,
            ..Interests::default()
        }
    }

    fn check_block(&mut self, b: &Block, ctx: &mut Ctx) {
        let source = ctx.source;
        let text = b.range.slice(source);
        for item in &self.items {
            if let Some(FixSpec::Expand {
                replacement,
                has_refs,
            }) = &item.fix
            {
                // Capture-bearing fix: needs the capture groups per match,
                // so this item runs on `captures_iter` instead of the
                // cheaper `find_iter` the other two branches use.
                let mut buf = String::new();
                for caps in item.regex.captures_iter(text) {
                    let m = caps.get(0).expect("group 0 always matches");
                    if m.start() == m.end() {
                        continue; // never report empty matches
                    }
                    let start = b.range.start + m.start();
                    let end = b.range.start + m.end();
                    if self.allow_context_skips(source, start, end) {
                        continue;
                    }
                    let span = TextRange { start, end };
                    buf.clear();
                    caps.expand(replacement, &mut buf);
                    // Only re-case a ref-free (pure `$$`-escape) expansion:
                    // see the module doc comment for why a real capture ref
                    // must not be re-cased.
                    let replacement_text = if *has_refs {
                        buf.clone()
                    } else {
                        Self::match_case(span.slice(source), &buf)
                    };
                    ctx.report(Report {
                        span,
                        message: item
                            .message
                            .clone()
                            .unwrap_or_else(|| self.default_message.clone()),
                        suggestion: item.suggestion.clone(),
                        weight: None,
                        fix: Some(Fix {
                            edits: vec![Edit {
                                range: span,
                                replacement: replacement_text,
                            }],
                            applicability: Applicability::MachineApplicable,
                        }),
                    });
                }
                continue;
            }

            let literal = match &item.fix {
                Some(FixSpec::Literal(s)) => Some(s.as_str()),
                Some(FixSpec::Expand { .. }) => unreachable!("handled above"),
                None => None,
            };
            for m in item.regex.find_iter(text) {
                if m.start() == m.end() {
                    continue; // never report empty matches
                }
                let start = b.range.start + m.start();
                let end = b.range.start + m.end();
                if self.allow_context_skips(source, start, end) {
                    continue;
                }

                let span = TextRange { start, end };
                let fix = literal.map(|replacement| Fix {
                    edits: vec![Edit {
                        range: span,
                        replacement: Self::match_case(span.slice(source), replacement),
                    }],
                    applicability: Applicability::MachineApplicable,
                });
                ctx.report(Report {
                    span,
                    message: item
                        .message
                        .clone()
                        .unwrap_or_else(|| self.default_message.clone()),
                    suggestion: item.suggestion.clone(),
                    weight: None,
                    fix,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::BlockKind;
    use crate::types::{Intent, RuleId, Scope, Severity, Tier};

    fn meta() -> RuleMeta {
        RuleMeta {
            id: RuleId("core/test-phrase".into()),
            tier: Tier::Static,
            scope: Scope::Text,
            severity: Severity::Error,
            intent: Intent::Detection,
            description: "default description".into(),
            docs_url: "https://lawlint.com/rules/test-phrase".into(),
            rationale: None,
            explanation: None,
            examples: vec![],
        }
    }

    fn def(yaml: &str) -> RuleDef {
        serde_yaml::from_str(yaml).expect("test def parses")
    }

    fn block(start: usize, end: usize) -> Block {
        Block {
            kind: BlockKind::Paragraph,
            range: TextRange { start, end },
            sentences: vec![],
        }
    }

    fn run(engine: &mut PhraseEngine, source: &str, b: &Block) -> Vec<Report> {
        let mut ctx = Ctx::new(source, 0);
        engine.check_block(b, &mut ctx);
        ctx.take_reports()
    }

    #[test]
    fn matches_at_absolute_offsets_in_later_block() {
        let source = "Intro paragraph.\n\nWe must delve deeper here.";
        let d = def(r#"
id: test-phrase
engine: phrase
message: "Avoid delve"
patterns:
  - "(?i)\\bdelve\\b"
"#);
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        // Second block only.
        let b = block(18, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(reports.len(), 1);
        let span = reports[0].span;
        assert_eq!(span.slice(source), "delve");
        assert_eq!(span.start, source.find("delve").unwrap());
        assert_eq!(reports[0].message, "Avoid delve");
        assert!(reports[0].suggestion.is_none());
        assert!(reports[0].fix.is_none());
    }

    #[test]
    fn does_not_match_outside_block_range() {
        let source = "delve here.\n\nClean paragraph.";
        let d = def("id: t\nengine: phrase\npatterns: [\"(?i)delve\"]");
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        // Only the second (clean) block is checked.
        let b = block(13, source.len());
        assert!(run(&mut e, source, &b).is_empty());
    }

    #[test]
    fn per_item_message_suggestion_override_and_default_fallback() {
        let source = "We leverage tapestry.";
        let d = def(r#"
id: t
engine: phrase
message: "default msg"
patterns:
  - { pattern: "(?i)\\bleverage\\b", message: "no leverage", suggestion: "use" }
  - "(?i)\\btapestry\\b"
"#);
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(reports.len(), 2);
        let lev = reports
            .iter()
            .find(|r| r.span.slice(source) == "leverage")
            .unwrap();
        assert_eq!(lev.message, "no leverage");
        assert_eq!(lev.suggestion.as_deref(), Some("use"));
        let tap = reports
            .iter()
            .find(|r| r.span.slice(source) == "tapestry")
            .unwrap();
        assert_eq!(tap.message, "default msg");
        assert!(tap.suggestion.is_none());
    }

    #[test]
    fn default_message_falls_back_to_description_when_no_message() {
        let source = ";";
        let d = def("id: t\nengine: phrase\npatterns: [\";\"]");
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(reports[0].message, "default description");
    }

    #[test]
    fn multiple_matches_in_one_block() {
        let source = "one; two; three";
        let d = def("id: t\nengine: phrase\npatterns: [\";\"]");
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].span, TextRange { start: 3, end: 4 });
        assert_eq!(reports[1].span, TextRange { start: 8, end: 9 });
    }

    #[test]
    fn unicode_prefix_offsets_are_byte_accurate() {
        // "café" is 5 bytes; em dash is 3 bytes.
        let source = "café—bar";
        let d = def("id: t\nengine: phrase\npatterns: [\"—\"]");
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].span, TextRange { start: 5, end: 8 });
        assert_eq!(reports[0].span.slice(source), "—");
    }

    #[test]
    fn allow_context_skips_en_dash_in_numeric_range() {
        let source = "The years 1994–2001 were formative. A dash – here is stray.";
        let d = def(r#"
id: no-en-dash
engine: phrase
message: "En dashes belong only in numeric ranges."
patterns: ["–"]
allow_context: { pattern: '\d\s?–\s?\d', window: 8 }
"#);
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        // Numeric-range en dash skipped; stray one reported.
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].span.slice(source), "–");
        assert!(reports[0].span.start > source.find("2001").unwrap());
    }

    #[test]
    fn allow_context_window_clamps_to_char_boundaries() {
        // '–' match starts at byte 9 ("a"=1, "é"=2, "é"=2, "1994"=4).
        // window 7 → lo = 2, mid-'é' (boundaries 0,1,3,5,...) → clamp down to 1.
        // hi side runs into multibyte "éé" as well. Must not panic, and the
        // numeric-range skip must still apply.
        let source = "aéé1994–2001éé tail – x";
        let d = def(
            "id: t\nengine: phrase\npatterns: [\"–\"]\nallow_context: { pattern: '\\d\\s?–\\s?\\d', window: 7 }",
        );
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(reports.len(), 1); // only the trailing stray dash
        assert_eq!(reports[0].span.slice(source), "–");
        assert_eq!(reports[0].span.start, source.rfind('–').unwrap());
    }

    #[test]
    fn allow_context_window_clamped_at_source_edges() {
        // Match at the very start/end of source: window must clamp to [0, len].
        let source = "–1 and 9–";
        let d = def(
            "id: t\nengine: phrase\npatterns: [\"–\"]\nallow_context: { pattern: '\\d–\\d', window: 100 }",
        );
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        // No digit on both sides of either dash → both reported; no panic.
        let reports = run(&mut e, source, &b);
        assert_eq!(reports.len(), 2);
    }

    #[test]
    fn fix_string_emits_machine_applicable_single_edit() {
        let source = "Please delve into it.";
        let d = def(r#"
id: t
engine: phrase
patterns:
  - { pattern: "(?i)\\bdelve\\b", message: "no delve", suggestion: "examine", fix: "examine" }
"#);
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(reports.len(), 1);
        let fix = reports[0].fix.as_ref().expect("fix present");
        assert_eq!(fix.applicability, Applicability::MachineApplicable);
        assert_eq!(fix.edits.len(), 1);
        assert_eq!(fix.edits[0].range, reports[0].span);
        assert_eq!(fix.edits[0].range.slice(source), "delve");
        assert_eq!(fix.edits[0].replacement, "examine");
    }

    #[test]
    fn fix_matches_leading_capital_of_matched_text() {
        let source = "Delve into it.";
        let d = def(r#"
id: t
engine: phrase
patterns:
  - { pattern: "(?i)\\bdelve\\b", fix: "examine" }
"#);
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(
            reports[0].fix.as_ref().unwrap().edits[0].replacement,
            "Examine"
        );
    }

    #[test]
    fn fix_matches_all_caps_of_matched_text() {
        let source = "DELVE into it.";
        let d = def(r#"
id: t
engine: phrase
patterns:
  - { pattern: "(?i)\\bdelve\\b", fix: "examine" }
"#);
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(
            reports[0].fix.as_ref().unwrap().edits[0].replacement,
            "EXAMINE"
        );
    }

    #[test]
    fn fix_lowercase_match_is_verbatim() {
        let source = "we delve into it.";
        let d = def(r#"
id: t
engine: phrase
patterns:
  - { pattern: "(?i)\\bdelve\\b", fix: "examine" }
"#);
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(
            reports[0].fix.as_ref().unwrap().edits[0].replacement,
            "examine"
        );
    }

    #[test]
    fn capture_expansion_reorders_captured_groups() {
        let source = "cats and dogs.";
        let d = def(r#"
id: t
engine: phrase
patterns:
  - { pattern: "(?i)(\\w+) and (\\w+)", fixTemplate: "${2} and ${1}" }
"#);
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(reports.len(), 1);
        assert_eq!(
            reports[0].fix.as_ref().unwrap().edits[0].replacement,
            "dogs and cats"
        );
    }

    #[test]
    fn match_case_is_skipped_when_fix_has_capture_refs() {
        // The whole match starts uppercase ("Cats"), which would normally
        // make match_case capitalize the replacement's first character. But
        // this fix reorders a lowercase-captured word ("dogs") to the
        // front; blindly re-casing the assembled text would wrongly
        // capitalize "dogs" into "Dogs". The correct output preserves each
        // captured group's own source casing untouched.
        let source = "Cats and dogs.";
        let d = def(r#"
id: t
engine: phrase
patterns:
  - { pattern: "(?i)(\\w+) and (\\w+)", fixTemplate: "${2} and ${1}" }
"#);
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(
            reports[0].fix.as_ref().unwrap().edits[0].replacement,
            "dogs and Cats"
        );
    }

    #[test]
    fn literal_dollar_escape_still_gets_match_case() {
        // No real capture group reference here — "$$" is purely an escape
        // for a literal '$' — so the expanded text is still author-literal
        // and match_case must keep applying to it, exactly like a plain
        // Literal fix.
        let source = "Late fee applies.";
        let d = def(r#"
id: t
engine: phrase
patterns:
  - { pattern: "(?i)\\blate fee\\b", fixTemplate: "$$100 fee" }
"#);
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        let reports = run(&mut e, source, &b);
        assert_eq!(
            reports[0].fix.as_ref().unwrap().edits[0].replacement,
            "$100 fee"
        );
    }

    #[test]
    fn unknown_capture_group_in_fix_is_a_load_error() {
        let d = def(r#"
id: t
engine: phrase
patterns:
  - { pattern: "(\\w+) fee", fixTemplate: "${2}" }
"#);
        let err = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap_err();
        match err {
            LoadError::InvalidField { file, field, .. } => {
                assert_eq!(file, "t.yaml");
                assert_eq!(field, "patterns[0].fixTemplate");
            }
            other => panic!("expected InvalidField, got {other:?}"),
        }
    }

    #[test]
    fn capture_template_can_insert_into_the_middle_of_a_match() {
        // The capability oxford-comma will eventually want: an Oxford comma is
        // an *insertion*, which a literal whole-match replacement cannot
        // express. Proven here on a scoped rule rather than on oxford-comma
        // itself — see `oxford_comma_stays_advice_only_until_it_can_tell_a
        // _list_from_a_doublet`.
        let d = def(r#"
id: t
engine: phrase
patterns:
  - { pattern: "(\\w+, \\w+) (and|or) (\\w+)", fixTemplate: "${1}, ${2} ${3}" }
"#);
        let mut engine = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let source = "The parties are Alice, Bob and Carol.";
        let b = block(0, source.len());
        let reports = run(&mut engine, source, &b);
        assert_eq!(reports.len(), 1);
        let fix = reports[0].fix.as_ref().expect("template yields a fix");
        assert_eq!(fix.edits[0].replacement, "Alice, Bob, and Carol");
    }

    #[test]
    fn oxford_comma_stays_advice_only_until_it_can_tell_a_list_from_a_doublet() {
        // Regression guard, not a limitation to paper over. The rule's pattern
        // walks up to three words past a comma to find `and`/`or`, so in
        // "Pursuant to the aforementioned terms, the parties shall cease and
        // desist" it matches the legal doublet "cease and desist" and would
        // rewrite it to "cease, and desist" — corrupting the text of a legal
        // document. Detection is still useful; automatic repair is not safe
        // until the pattern can distinguish a real list. Give this rule a
        // `fixTemplate` only together with a pattern that passes this test.
        let source = "Pursuant to the aforementioned terms, the parties shall \
                      cease and desist from any and all action.";
        let opts = crate::LintOptions {
            enable: Some(vec!["oxford-comma".into()]),
            ..Default::default()
        };
        let result = crate::lint(source, &opts);
        assert!(
            !result.diagnostics.is_empty(),
            "the false positive itself is pre-existing and still detected"
        );
        assert!(
            result.diagnostics.iter().all(|d| d.fix.is_none()),
            "oxford-comma must not carry an auto-fix while it matches doublets"
        );
        assert_eq!(crate::apply_fixes(source, &result.diagnostics), source);
    }

    #[test]
    fn bad_pattern_regex_is_a_load_error_with_context() {
        let d = def("id: t\nengine: phrase\npatterns: [\"ok\", \"(\"]");
        let err = PhraseEngine::from_def(meta(), &d, "rules/t.yaml").unwrap_err();
        match err {
            LoadError::InvalidRegex {
                file,
                field,
                pattern,
                ..
            } => {
                assert_eq!(file, "rules/t.yaml");
                assert_eq!(field, "patterns[1]");
                assert_eq!(pattern, "(");
            }
            other => panic!("expected InvalidRegex, got {other:?}"),
        }
    }

    #[test]
    fn bad_allow_context_regex_is_a_load_error_with_context() {
        let d = def(
            "id: t\nengine: phrase\npatterns: [\"–\"]\nallow_context: { pattern: \"[\", window: 8 }",
        );
        let err = PhraseEngine::from_def(meta(), &d, "rules/t.yaml").unwrap_err();
        match err {
            LoadError::InvalidRegex { field, pattern, .. } => {
                assert_eq!(field, "allow_context.pattern");
                assert_eq!(pattern, "[");
            }
            other => panic!("expected InvalidRegex, got {other:?}"),
        }
    }

    #[test]
    fn empty_regex_matches_are_never_reported() {
        let source = "abc";
        let d = def("id: t\nengine: phrase\npatterns: [\"x*\"]");
        let mut e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let b = block(0, source.len());
        assert!(run(&mut e, source, &b).is_empty());
    }

    #[test]
    fn interests_are_blocks_only() {
        let d = def("id: t\nengine: phrase\npatterns: [\"x\"]");
        let e = PhraseEngine::from_def(meta(), &d, "t.yaml").unwrap();
        let i = e.interests();
        assert!(i.blocks && !i.tokens && !i.sentences && !i.document_exit);
        assert_eq!(e.meta().id, RuleId("core/test-phrase".into()));
    }
}
