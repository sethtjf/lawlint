//! Soft-rule mining: judgment-level style the regex engines cannot express.
//!
//! Tone, how paragraphs connect, whether each step follows from the last,
//! padding, and how conclusions are supported are the dimensions where agent
//! drafts most visibly part from careful legal prose, and none of them reduce
//! to a pattern. These become inferential rules: a rubric the AI judge
//! applies, with flag and pass examples as few-shot anchors.
//!
//! Two checks keep them honest, mirroring the hard-rule gate:
//!
//! 1. Example grounding, before any judge call. Every pass example must quote
//!    the corpus verbatim (so the rule describes this writing, not generic
//!    advice), and no flag example may appear in it.
//! 2. A judge gate. Each rule runs through the real lint path (`lint_full`)
//!    over a spread of corpus passages and, when a contrast set exists, over
//!    the AI drafts. A rule the judge applies to the corpus is dropped, as is
//!    one that never flags a draft. A judge failure drops the rule too: an
//!    unverified soft rule does not ship.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use lawlint_core::{lint_full, loader, Judge, JudgeOptions, LintOptions, RuleSet};
use serde::{Deserialize, Serialize};

use super::contrast::ContrastPair;
use super::{
    chunk_paragraphs, sanitize_id, truncate_chars, Candidate, CorpusFile, Passage,
    GATE_CHUNK_CHARS, GENERATED_HEADER, PROMPT_PASSAGE_CHARS,
};

pub(super) const SOFT_ORIGIN: &str = "soft (mining agent)";
/// Rules one soft lens may propose. Each costs two judge calls in the gate.
pub(super) const MAX_SOFT_RULES_PER_LENS: usize = 3;
/// Corpus passages the judge gate reads (each ~2,000 characters).
const SOFT_GATE_PASSAGES: usize = 6;
/// Matched pairs shown in a mining prompt.
pub(super) const MAX_PROMPT_PAIRS: usize = 4;
/// Examples kept per side; the loader needs three.
const MAX_EXAMPLES: usize = 6;
const MIN_EXAMPLES: usize = 3;
/// A pass example shorter than this is too generic to prove anything.
const MIN_PASS_EXAMPLE_WORDS: usize = 5;

pub(super) struct SoftLens {
    pub name: &'static str,
    pub focus: &'static str,
}

pub(super) static SOFT_LENSES: [SoftLens; 4] = [
    SoftLens {
        name: "tone & register",
        focus: "stance and formality, how confidently claims are made, how concessions and \
                uncertainty are handled, how the writer treats the reader and the opposing side",
    },
    SoftLens {
        name: "transitions & cohesion",
        focus: "how each sentence follows from the one before, how paragraphs hand off to \
                each other, whether logical links are carried by content or announced by \
                stock connectives, topic sentences",
    },
    SoftLens {
        name: "clarity & concision",
        focus: "padding and restatement, abstraction versus concrete facts, sentence economy, \
                defined terms used consistently, one point per sentence",
    },
    SoftLens {
        name: "reasoning & structure",
        focus: "how conclusions are stated and where, how rules and authority are applied to \
                facts, whether every claim is supported, paragraph architecture",
    },
];

#[derive(Deserialize)]
pub(super) struct MinedSoftRule {
    id: String,
    #[serde(default)]
    granularity: Option<String>,
    #[serde(default)]
    severity: Option<String>,
    description: String,
    #[serde(default)]
    message: Option<String>,
    rubric: String,
    #[serde(default)]
    flag_examples: Vec<String>,
    #[serde(default)]
    pass_examples: Vec<String>,
    #[serde(default)]
    mined_from: Option<String>,
}

#[derive(Serialize)]
struct SoftRuleYaml {
    id: String,
    engine: &'static str,
    severity: &'static str,
    intent: &'static str,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    granularity: &'static str,
    flag_examples: Vec<String>,
    pass_examples: Vec<String>,
}

pub(super) fn soft_prompt(
    profile_block: &str,
    sample: &[Passage],
    pairs: &[ContrastPair],
    lens: &SoftLens,
) -> String {
    let mut prompt = String::new();
    let _ = write!(
        prompt,
        "You are lawlint's style-mining agent, one of several parallel specialists. \
         Below are measurements of a body of source writing, sample passages from it, \
         and (when present) matched pairs: a source passage next to an AI-written \
         version of the same content. Derive rubric rules an AI judge can apply to new \
         drafts so they read like the source writing and not like a machine draft.\n\n\
         Your dimension is {}: {}. Only propose rules in this dimension.\n\n\
         Source measurements:\n{profile_block}\nSample passages:\n",
        lens.name, lens.focus
    );
    for (index, passage) in sample.iter().enumerate() {
        let _ = writeln!(
            prompt,
            "--- passage {} ({}) ---\n{}",
            index + 1,
            passage.source,
            passage.text
        );
    }
    if !pairs.is_empty() {
        prompt.push_str("\nMatched pairs (same content; the difference is style):\n");
        for (index, pair) in pairs.iter().take(MAX_PROMPT_PAIRS).enumerate() {
            let _ = writeln!(
                prompt,
                "--- pair {n}: source ({}) ---\n{}\n--- pair {n}: AI draft ---\n{}",
                pair.source,
                truncate_chars(&pair.original, PROMPT_PASSAGE_CHARS),
                truncate_chars(&pair.draft, PROMPT_PASSAGE_CHARS),
                n = index + 1
            );
        }
    }
    let _ = write!(
        prompt,
        "\nRespond with ONLY a JSON array of at most {MAX_SOFT_RULES_PER_LENS} rules. Each rule:\n\
         {{\"id\": \"kebab-case-name\", \"granularity\": \"sentence\" or \"paragraph\", \
         \"severity\": \"warning\" or \"suggestion\", \"description\": \"one line\", \
         \"message\": \"what to tell the drafter\", \
         \"rubric\": \"instructions to a judge: what to flag, and what not to flag\", \
         \"flag_examples\": [\"3-6 short texts the rule must flag\"], \
         \"pass_examples\": [\"3-6 sentences copied EXACTLY from the source passages\"], \
         \"mined_from\": \"which passages or pairs show the pattern\"}}\n\
         Requirements: each rule names one specific, observable difference between the \
         source writing and the AI drafts, not generic advice. pass_examples must be \
         verbatim, character-for-character quotes from the source passages above; \
         paraphrases are rejected. flag_examples should quote the AI drafts where they \
         show the problem. The rubric must say what NOT to flag, so that none of the \
         source passages would be flagged. Return [] if nothing is reliable."
    );
    prompt
}

/// Whitespace-collapsed, quote-straightened, lowercased form for verbatim
/// checks: a model that re-flows a line or swaps curly quotes still quoted it.
pub(super) fn normalize(text: &str) -> String {
    text.trim()
        .trim_matches(|c| matches!(c, '"' | '“' | '”'))
        .replace(['“', '”'], "\"")
        .replace(['‘', '’'], "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The corpus in `normalize` form, paragraph by paragraph joined with a
/// separator no quote can span, for substring checks.
pub(super) fn normalized_corpus(files: &[CorpusFile]) -> String {
    files
        .iter()
        .flat_map(|f| f.text.split("\n\n"))
        .map(normalize)
        .collect::<Vec<_>>()
        .join("\n\u{1}\n")
}

/// Validate one mined soft rule into a candidate. Errors carry (id, reason).
pub(super) fn soft_candidate(
    rule: MinedSoftRule,
    index: usize,
    model_id: &str,
    lens_name: &str,
    used_ids: &BTreeSet<String>,
    corpus: &str,
) -> Result<Candidate, (String, String)> {
    let id = sanitize_id(&rule.id, index);
    let fail = |reason: String| Err((id.clone(), reason));
    if used_ids.contains(&id) {
        return fail("duplicate rule id".to_string());
    }
    let granularity = match rule.granularity.as_deref() {
        Some("paragraph") => "paragraph",
        _ => "sentence",
    };
    let severity = match rule.severity.as_deref() {
        Some("warning") | Some("error") => "warning",
        _ => "suggestion",
    };
    let rubric = rule.rubric.trim().to_string();
    if rubric.len() < 40 {
        return fail("rubric is too short to apply".to_string());
    }
    let mut description = rule.description.trim().trim_end_matches('.').to_string();
    if description.is_empty() {
        return fail("empty description".to_string());
    }

    let total_pass = rule.pass_examples.len();
    let pass_examples: Vec<String> = rule
        .pass_examples
        .into_iter()
        .map(|e| e.trim().to_string())
        .filter(|e| {
            let n = normalize(e);
            n.split(' ').count() >= MIN_PASS_EXAMPLE_WORDS && corpus.contains(&n)
        })
        .take(MAX_EXAMPLES)
        .collect();
    if pass_examples.len() < MIN_EXAMPLES {
        return fail(format!(
            "only {} of {total_pass} pass examples quote the corpus verbatim (need {MIN_EXAMPLES})",
            pass_examples.len()
        ));
    }
    let flag_examples: Vec<String> = rule
        .flag_examples
        .into_iter()
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty() && !corpus.contains(&normalize(e)))
        .take(MAX_EXAMPLES)
        .collect();
    if flag_examples.len() < MIN_EXAMPLES {
        return fail(format!(
            "only {} usable flag examples (need {MIN_EXAMPLES}; examples found in the corpus are removed)",
            flag_examples.len()
        ));
    }

    let source = rule
        .mined_from
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("the corpus sample");
    let _ = write!(
        description,
        " (mined by {model_id}, lens: {lens_name}, from {source})."
    );
    let yaml = SoftRuleYaml {
        id: id.clone(),
        engine: "inferential",
        severity,
        intent: "style",
        description,
        message: rule
            .message
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty()),
        granularity,
        flag_examples,
        pass_examples,
    };
    let file_name = format!("rules/{id}.md");
    let frontmatter = match serde_yaml::to_string(&yaml) {
        Ok(body) => body,
        Err(error) => return fail(error.to_string()),
    };
    // The header rides in the frontmatter as a YAML comment: an inferential
    // rule's body is its rubric, and the header must not reach the judge.
    let markdown = format!("---\n{GENERATED_HEADER}{frontmatter}---\n\n{rubric}\n");
    let def = match loader::parse_rule(&file_name, &markdown) {
        Ok(def) => def,
        Err(error) => return fail(error.to_string()),
    };
    Ok(Candidate {
        id,
        file_name,
        yaml: markdown,
        origin: SOFT_ORIGIN,
        def,
    })
}

/// The corpus side of the judge gate: `SOFT_GATE_PASSAGES` gate-sized chunks
/// spread evenly across the whole corpus, as one document.
pub(super) fn gate_corpus_document(files: &[CorpusFile]) -> String {
    let chunks: Vec<String> = files
        .iter()
        .flat_map(|f| chunk_paragraphs(&f.text, GATE_CHUNK_CHARS))
        .collect();
    let picked: Vec<&String> = if chunks.len() <= SOFT_GATE_PASSAGES {
        chunks.iter().collect()
    } else {
        (0..SOFT_GATE_PASSAGES)
            .map(|slot| &chunks[slot * (chunks.len() - 1) / (SOFT_GATE_PASSAGES - 1)])
            .collect()
    };
    picked
        .into_iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(super) struct SoftVerdict {
    pub candidate: Candidate,
    /// `Ok(contrast hits)` = keep; `Err(reason)` = drop.
    pub outcome: Result<Option<usize>, String>,
}

fn judge_options() -> LintOptions {
    LintOptions {
        judge: Some(JudgeOptions {
            // One request per rule per document: each gate document fits in
            // a single unit, so a rule costs exactly one call per side.
            per_rule: Some(true),
            context_chars: Some(32_000),
            ..JudgeOptions::default()
        }),
        ..LintOptions::default()
    }
}

/// Judge findings for `candidate` over `text`, or why the judge could not
/// say. A failed chunk is a failure, not a pass.
fn findings(
    judge: &dyn Judge,
    package: &str,
    candidate: &Candidate,
    text: &str,
) -> Result<Vec<String>, String> {
    let set = RuleSet::from_sources(
        package,
        &[(candidate.file_name.clone(), candidate.yaml.clone())],
    )
    .map_err(|error| error.to_string())?;
    let result = lint_full(text, &judge_options(), &set, judge, None);
    if let Some(stats) = &result.judge {
        if stats.chunks_failed > 0 {
            return Err(format!(
                "judge failed ({})",
                stats.first_failure.as_deref().unwrap_or("no detail")
            ));
        }
    }
    let full_id = format!("{package}/{}", candidate.id);
    Ok(result
        .diagnostics
        .iter()
        .filter(|d| d.rule_id.0 == full_id)
        .map(|d| truncate_chars(d.span.slice(text).trim(), 80))
        .collect())
}

fn verdict(
    judge: &dyn Judge,
    package: &str,
    candidate: &Candidate,
    corpus_doc: &str,
    contrast_doc: Option<&str>,
) -> Result<Option<usize>, String> {
    let own = findings(judge, package, candidate, corpus_doc)
        .map_err(|error| format!("{error} on the corpus check"))?;
    if let Some(first) = own.first() {
        return Err(format!(
            "judge flagged {} corpus sentence{} (e.g. \"{first}\")",
            own.len(),
            if own.len() == 1 { "" } else { "s" }
        ));
    }
    let Some(contrast_doc) = contrast_doc else {
        return Ok(None);
    };
    let hits = findings(judge, package, candidate, contrast_doc)
        .map_err(|error| format!("{error} on the AI-draft check"))?
        .len();
    if hits == 0 {
        return Err("never flagged the AI contrast drafts".to_string());
    }
    Ok(Some(hits))
}

/// Judge-gate every candidate concurrently (the judge's own client pool
/// bounds real parallelism).
pub(super) fn gate(
    judge: &dyn Judge,
    package: &str,
    candidates: Vec<Candidate>,
    corpus_doc: &str,
    contrast_doc: Option<&str>,
) -> Vec<SoftVerdict> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = candidates
            .into_iter()
            .map(|candidate| {
                scope.spawn(move || {
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        verdict(judge, package, &candidate, corpus_doc, contrast_doc)
                    }))
                    .unwrap_or_else(|_| Err("judge panicked".to_string()));
                    SoftVerdict { candidate, outcome }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("panic caught inside the gate worker"))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lawlint_core::{JudgeFinding, MockJudge};

    const CORPUS: &str = "The court denied the motion because the record shows a dispute.\n\n\
        We filed the reply on Monday and served it the same day.\n\n\
        The contract says the buyer pays on delivery, and the buyer did not pay.\n\n\
        Nothing in the statute supports the defendant's reading of section 4.";

    fn corpus() -> String {
        normalized_corpus(&[CorpusFile {
            name: "c.txt".into(),
            register: "plain-text",
            modified: std::time::SystemTime::UNIX_EPOCH,
            text: CORPUS.into(),
        }])
    }

    fn mined(pass: &[&str], flag: &[&str]) -> MinedSoftRule {
        MinedSoftRule {
            id: "Concrete Reasons".into(),
            granularity: Some("sentence".into()),
            severity: Some("error".into()),
            description: "States the reason with the conclusion.".into(),
            message: Some("Give the concrete reason.".into()),
            rubric: "Flag a sentence that states a conclusion with an abstract justification \
                     instead of the concrete fact. Do not flag a sentence that names the fact."
                .into(),
            flag_examples: flag.iter().map(|s| s.to_string()).collect(),
            pass_examples: pass.iter().map(|s| s.to_string()).collect(),
            mined_from: Some("pairs 1-2".into()),
        }
    }

    const PASS: [&str; 3] = [
        "The court denied the motion because the record shows a dispute.",
        "“We filed the reply on Monday and   served it the same day.”",
        "the buyer pays on delivery, and the buyer did not pay",
    ];
    const FLAG: [&str; 3] = [
        "The motion was denied due to various considerations.",
        "Several factors weigh against the defendant's position.",
        "The analysis demonstrates a comprehensive basis for relief.",
    ];

    #[test]
    fn soft_candidate_grounds_examples_and_writes_a_loadable_rule() {
        let c = soft_candidate(
            mined(&PASS, &FLAG),
            0,
            "m",
            "tone",
            &BTreeSet::new(),
            &corpus(),
        )
        .unwrap();
        assert_eq!(c.id, "concrete-reasons");
        assert_eq!(c.def.engine, "inferential");
        assert_eq!(c.def.severity.as_deref(), Some("warning"));
        assert_eq!(c.def.intent.as_deref(), Some("style"));
        assert_eq!(c.def.pass_examples.len(), 3);
        assert!(c
            .def
            .rubric
            .as_deref()
            .unwrap()
            .starts_with("Flag a sentence"));
        assert!(!c.def.rubric.as_deref().unwrap().contains("Generated by"));
        assert!(c.yaml.contains(GENERATED_HEADER), "prune must recognize it");
        assert!(c
            .def
            .description
            .as_deref()
            .unwrap()
            .contains("mined by m, lens: tone"));
    }

    #[test]
    fn soft_candidate_rejects_paraphrased_pass_examples() {
        let paraphrased = [
            "The judge denied the motion since the record had a dispute.",
            PASS[1],
            PASS[2],
        ];
        let (_, reason) = soft_candidate(
            mined(&paraphrased, &FLAG),
            0,
            "m",
            "tone",
            &BTreeSet::new(),
            &corpus(),
        )
        .unwrap_err();
        assert!(reason.contains("only 2 of 3 pass examples"), "{reason}");
    }

    #[test]
    fn soft_candidate_drops_flag_examples_found_in_corpus() {
        let flags = [FLAG[0], FLAG[1], PASS[0]];
        let (_, reason) = soft_candidate(
            mined(&PASS, &flags),
            0,
            "m",
            "tone",
            &BTreeSet::new(),
            &corpus(),
        )
        .unwrap_err();
        assert!(reason.contains("only 2 usable flag examples"), "{reason}");
    }

    fn finding(rule: &str, quote: &str) -> JudgeFinding {
        JudgeFinding {
            rule: rule.into(),
            quote: quote.into(),
            explanation: "abstract".into(),
            confidence: 0.95,
            suggested_rewrite: None,
        }
    }

    #[test]
    fn gate_keeps_rules_that_flag_drafts_but_not_the_corpus() {
        let corpus_doc = CORPUS;
        let drafts = "The motion was denied due to various considerations. It is what it is.";
        let rule = soft_candidate(
            mined(&PASS, &FLAG),
            0,
            "m",
            "tone",
            &BTreeSet::new(),
            &corpus(),
        )
        .unwrap();
        let judge = MockJudge::new().respond(
            "various considerations",
            vec![finding(
                "learned/concrete-reasons",
                "due to various considerations",
            )],
        );
        let verdicts = gate(
            &judge,
            "learned",
            vec![rule.clone()],
            corpus_doc,
            Some(drafts),
        );
        assert_eq!(verdicts[0].outcome, Ok(Some(1)));

        // The same rule self-fires when the judge flags corpus text.
        let judge = MockJudge::new().respond(
            "record shows a dispute",
            vec![finding(
                "learned/concrete-reasons",
                "the record shows a dispute",
            )],
        );
        let verdicts = gate(
            &judge,
            "learned",
            vec![rule.clone()],
            corpus_doc,
            Some(drafts),
        );
        let reason = verdicts[0].outcome.clone().unwrap_err();
        assert!(
            reason.contains("judge flagged 1 corpus sentence"),
            "{reason}"
        );

        // Silent on the drafts → no evidence it separates them.
        let verdicts = gate(
            &MockJudge::new(),
            "learned",
            vec![rule.clone()],
            corpus_doc,
            Some(drafts),
        );
        assert_eq!(
            verdicts[0].outcome,
            Err("never flagged the AI contrast drafts".to_string())
        );

        // A judge error fails closed.
        let judge = MockJudge::new()
            .respond_err("", "boom")
            .respond_err("", "boom");
        let verdicts = gate(&judge, "learned", vec![rule], corpus_doc, None);
        assert!(verdicts[0]
            .outcome
            .clone()
            .unwrap_err()
            .contains("judge failed"));
    }

    #[test]
    fn soft_prompt_includes_pairs_and_lens() {
        let pairs = vec![ContrastPair {
            source: "c.txt".into(),
            original: "Source text.".into(),
            draft: "Moreover, draft text.".into(),
        }];
        let prompt = soft_prompt("- stats\n", &[], &pairs, &SOFT_LENSES[1]);
        assert!(prompt.contains("transitions & cohesion"));
        assert!(prompt.contains("pair 1: AI draft"));
        assert!(prompt.contains("Moreover, draft text."));
        assert!(prompt.contains("verbatim"));
    }
}
