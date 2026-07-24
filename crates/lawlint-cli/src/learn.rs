//! `lawlint learn <path>` — mine a personal rule package from the user's own
//! writing corpus.
//!
//! Two passes. Pass 1 is a local statistical pre-pass in plain code over the
//! FULL corpus — no AI: punctuation habits, AI-tell term frequencies,
//! sentence-length distribution, opener repetition, Oxford-comma consistency.
//! Mechanically measurable "never does X" habits become deterministic rule
//! candidates here, with a `fix:` wherever the replacement is a drop-in.
//! Pass 2 sends a stratified, token-capped sample (~20-30 passages by
//! register/length/recency) plus the pass-1 stats to a mining agent through
//! the ax boundary (`lawlint_judge::create_client`) for judgment-required
//! patterns. A career of briefs never needs to fit in a context window.
//!
//! The key quality bar is the self-consistency gate: a generated rule that
//! fires on the user's own corpus is wrong by construction. Candidates run
//! back over the whole corpus (lawlint-eval's evaluate/per-rule machinery,
//! the corpus as the all-human class); any rule with a nonzero self-fire
//! count is dropped, and every kept rule's own examples must flag/pass
//! (`lawlint rules test` semantics). A weak model costs recall, not
//! correctness.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use lawlint_core::{lint_with, loader, LintOptions, RuleDef, RuleExample, RuleSet, TokenKind};
use lawlint_eval::{evaluate_with, per_rule_metrics, Label, Sample};
use lawlint_judge::AxAIClient;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::progress::MultiSpinner;

/// Below this many corpus words, "the corpus never does X" is noise, not
/// signal — pass 1 emits no candidates (pass 2 still runs).
const MIN_CORPUS_WORDS: usize = 300;
/// Pass-2 sample caps: passages and total characters (~chars/4 tokens), so
/// hosted-backend users send samples, not archives.
const MAX_PASSAGES: usize = 28;
const MAX_SAMPLE_CHARS: usize = 16_000;
/// Passage sizes: prompt passages are readable excerpts; gate chunks must
/// cover the corpus completely, so they are never truncated.
const PROMPT_PASSAGE_CHARS: usize = 900;
const GATE_CHUNK_CHARS: usize = 2_000;
/// At most this many candidates one lens's agent call may propose. Each lens
/// covers a narrower slice of style than the old single-call prompt did, so
/// this is smaller than the old MAX_MINED_RULES (10) was for the whole run —
/// the merged pool across lenses is what the self-consistency gate filters.
const MAX_MINED_RULES_PER_LENS: usize = 5;

// ---- corpus ingestion --------------------------------------------------

#[derive(Debug)]
struct CorpusFile {
    /// Path relative to the corpus root (provenance notes).
    name: String,
    /// Coarse register proxy from the file type.
    register: &'static str,
    modified: SystemTime,
    text: String,
}

fn register_for(extension: &str) -> Option<&'static str> {
    match extension.to_ascii_lowercase().as_str() {
        "docx" => Some("document"),
        "md" | "markdown" => Some("markdown"),
        "txt" | "text" => Some("plain-text"),
        _ => None,
    }
}

fn read_corpus_file(path: &Path, register: &'static str) -> Result<String, String> {
    if register == "document" {
        let bytes = fs::read(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        lawlint_docx::extract(&bytes)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))
    } else {
        fs::read_to_string(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))
    }
}

/// Collect the corpus: a single file, or a directory walked recursively.
/// `.docx` goes through lawlint-docx extraction; `.md`/`.txt` are read as
/// text; anything else (and dotfiles) is skipped. Deterministic order:
/// sorted paths.
fn ingest(root: &Path) -> Result<Vec<CorpusFile>, String> {
    let mut paths: Vec<PathBuf> = Vec::new();
    if root.is_file() {
        paths.push(root.to_path_buf());
    } else if root.is_dir() {
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = fs::read_dir(&dir)
                .map_err(|error| format!("failed to read {}: {error}", dir.display()))?;
            for entry in entries.filter_map(|entry| entry.ok()) {
                let path = entry.path();
                let hidden = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with('.'));
                if hidden {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                } else {
                    paths.push(path);
                }
            }
        }
    } else {
        return Err(format!(
            "failed to read {}: no such file or directory",
            root.display()
        ));
    }
    paths.sort();

    let mut files = Vec::new();
    for path in paths {
        let Some(register) = path
            .extension()
            .and_then(|ext| ext.to_str())
            .and_then(register_for)
        else {
            // A named single file with an unsupported extension is an error;
            // unsupported directory entries are just not corpus.
            if root.is_file() {
                return Err(format!(
                    "{}: unsupported corpus file type (use .docx, .md, or .txt)",
                    path.display()
                ));
            }
            continue;
        };
        let text = read_corpus_file(&path, register)?;
        if text.trim().is_empty() {
            continue;
        }
        let name = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let name = if name.is_empty() {
            path.display().to_string()
        } else {
            name
        };
        let modified = fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        files.push(CorpusFile {
            name,
            register,
            modified,
            text,
        });
    }
    Ok(files)
}

// ---- pass 1: local statistics ------------------------------------------

/// One AI-tell term family the corpus may never use: patterns are
/// case-sensitive lowercase (so the `fix` replacement stays mechanical) and
/// each variant carries its drop-in replacement.
struct TermFamily {
    id: &'static str,
    variants: &'static [(&'static str, &'static str)],
    description: &'static str,
    /// `examples.good` seek: a corpus sentence matching this regex anchors
    /// the counterfactual; replacing its first match with `inject` yields
    /// `examples.bad`.
    seek: &'static str,
    inject: &'static str,
    canonical_bad: &'static str,
    canonical_good: &'static str,
}

const TERM_FAMILIES: &[TermFamily] = &[
    TermFamily {
        id: "no-utilize",
        variants: &[
            (r"\butilize\b", "use"),
            (r"\butilizes\b", "uses"),
            (r"\butilized\b", "used"),
            (r"\butilizing\b", "using"),
        ],
        description: "Flags \"utilize\" where you write \"use\".",
        seek: r"\buse\b",
        inject: "utilize",
        canonical_bad: "We utilize the standard form.",
        canonical_good: "We use the standard form.",
    },
    TermFamily {
        id: "no-in-order-to",
        variants: &[(r"\bin order to\b", "to")],
        description: "Flags \"in order to\" where you write \"to\".",
        seek: r"\bto\b",
        inject: "in order to",
        canonical_bad: "He filed early in order to preserve the claim.",
        canonical_good: "He filed early to preserve the claim.",
    },
    TermFamily {
        id: "no-prior-to",
        variants: &[(r"\bprior to\b", "before")],
        description: "Flags \"prior to\" where you write \"before\".",
        seek: r"\bbefore\b",
        inject: "prior to",
        canonical_bad: "Serve the notice prior to the hearing.",
        canonical_good: "Serve the notice before the hearing.",
    },
    TermFamily {
        id: "no-subsequent-to",
        variants: &[(r"\bsubsequent to\b", "after")],
        description: "Flags \"subsequent to\" where you write \"after\".",
        seek: r"\bafter\b",
        inject: "subsequent to",
        canonical_bad: "The amendment came subsequent to the filing.",
        canonical_good: "The amendment came after the filing.",
    },
    TermFamily {
        id: "no-commence",
        variants: &[
            (r"\bcommence\b", "begin"),
            (r"\bcommences\b", "begins"),
            (r"\bcommencing\b", "beginning"),
        ],
        description: "Flags \"commence\" where you write \"begin\".",
        seek: r"\bbegin\b",
        inject: "commence",
        canonical_bad: "The trial will commence on Monday.",
        canonical_good: "The trial will begin on Monday.",
    },
    TermFamily {
        id: "no-endeavor",
        variants: &[
            (r"\bendeavor\b", "try"),
            (r"\bendeavors\b", "tries"),
            (r"\bendeavoring\b", "trying"),
        ],
        description: "Flags \"endeavor\" where you write \"try\".",
        seek: r"\btry\b",
        inject: "endeavor",
        canonical_bad: "We will endeavor to respond by Friday.",
        canonical_good: "We will try to respond by Friday.",
    },
];

struct CorpusStats {
    files: usize,
    words: usize,
    sentences: usize,
    em_dashes: usize,
    en_dashes: usize,
    semicolons: usize,
    /// Lists written with / without the serial (Oxford) comma.
    oxford_with: usize,
    oxford_without: usize,
    sentence_words_max: usize,
    sentence_words_mean: f64,
    /// Sentence-opening words (lowercased) by frequency, most common first.
    opener_top: Vec<(String, usize)>,
    /// Corpus hit count per term family (index-aligned with TERM_FAMILIES).
    term_counts: Vec<usize>,
}

fn corpus_stats(files: &[CorpusFile]) -> CorpusStats {
    // Both detectors count only list-shaped text so the two counts are
    // comparable: "A, B, and C" = serial-comma usage; "A, B and C" = its
    // absence. Clause commas ("..., and the case proceeded") match neither —
    // they are not serial-comma evidence either way.
    let oxford_with = Regex::new(
        r"[A-Za-z][\w'’-]*,\s+(?:[A-Za-z][\w'’-]*\s+){0,3}[A-Za-z][\w'’-]*,\s+(?:and|or)\s+[A-Za-z]",
    )
    .expect("static regex");
    let oxford_without =
        Regex::new(r"[A-Za-z][\w'’-]*,\s+(?:[A-Za-z][\w'’-]*\s+){1,3}(?:and|or)\s+[A-Za-z]")
            .expect("static regex");
    let term_regexes: Vec<Regex> = TERM_FAMILIES
        .iter()
        .map(|family| {
            let alternation = family
                .variants
                .iter()
                .map(|(pattern, _)| format!("(?:{pattern})"))
                .collect::<Vec<_>>()
                .join("|");
            Regex::new(&alternation).expect("static term regex")
        })
        .collect();

    let mut stats = CorpusStats {
        files: files.len(),
        words: 0,
        sentences: 0,
        em_dashes: 0,
        en_dashes: 0,
        semicolons: 0,
        oxford_with: 0,
        oxford_without: 0,
        sentence_words_max: 0,
        sentence_words_mean: 0.0,
        opener_top: Vec::new(),
        term_counts: vec![0; TERM_FAMILIES.len()],
    };
    let mut sentence_words_total = 0usize;
    let mut openers: BTreeMap<String, usize> = BTreeMap::new();

    for file in files {
        stats.words += file.text.split_whitespace().count();
        stats.em_dashes += file.text.matches('—').count();
        stats.en_dashes += file.text.matches('–').count();
        stats.semicolons += file.text.matches(';').count();
        stats.oxford_with += oxford_with.find_iter(&file.text).count();
        stats.oxford_without += oxford_without.find_iter(&file.text).count();
        for (index, regex) in term_regexes.iter().enumerate() {
            stats.term_counts[index] += regex.find_iter(&file.text).count();
        }
        let document = lawlint_core::parse(&file.text, file.register == "markdown");
        for block in &document.blocks {
            for sentence in &block.sentences {
                if sentence.is_citation {
                    continue;
                }
                let words = sentence
                    .tokens
                    .iter()
                    .filter(|token| matches!(token.kind, TokenKind::Word | TokenKind::Number))
                    .count();
                if words == 0 {
                    continue;
                }
                stats.sentences += 1;
                sentence_words_total += words;
                stats.sentence_words_max = stats.sentence_words_max.max(words);
                if let Some(token) = sentence
                    .tokens
                    .iter()
                    .find(|token| token.kind == TokenKind::Word)
                {
                    *openers
                        .entry(token.range.slice(&file.text).to_lowercase())
                        .or_default() += 1;
                }
            }
        }
    }
    if stats.sentences > 0 {
        stats.sentence_words_mean = sentence_words_total as f64 / stats.sentences as f64;
    }
    let mut opener_top: Vec<(String, usize)> = openers.into_iter().collect();
    opener_top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    opener_top.truncate(5);
    stats.opener_top = opener_top;
    stats
}

// ---- Markdown rules -----------------------------------------------------

#[derive(Serialize)]
struct PatternYaml {
    pattern: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    suggestion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fix: Option<String>,
}

#[derive(Serialize)]
struct RuleYaml {
    id: String,
    engine: String,
    severity: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    rationale: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    examples: Vec<RuleExample>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    patterns: Vec<PatternYaml>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metric: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<BTreeMap<String, f64>>,
}

/// A generated rule that parsed and validated as a real `RuleDef`.
#[derive(Debug, Clone)]
struct Candidate {
    id: String,
    /// `rules/<id>.md`.
    file_name: String,
    yaml: String,
    origin: &'static str,
    def: RuleDef,
}

const GENERATED_HEADER: &str =
    "# Generated by `lawlint learn` from your own writing. Edit or delete freely.\n";

/// Serialize + round-trip through the loader so every emitted candidate is a
/// valid rule file by construction. Errors surface as drop reasons.
fn candidate(rule: RuleYaml, origin: &'static str) -> Result<Candidate, String> {
    let file_name = format!("rules/{}.md", rule.id);
    let body = serde_yaml::to_string(&rule).map_err(|error| error.to_string())?;
    let markdown = format!("---\n{body}---\n\n{GENERATED_HEADER}");
    let def = loader::parse_rule(&file_name, &markdown).map_err(|error| error.to_string())?;
    Ok(Candidate {
        id: rule.id,
        file_name,
        yaml: markdown,
        origin,
        def,
    })
}

// ---- pass 1: deterministic candidates ----------------------------------

/// Pick `examples` for a pass-1 rule: `good` is a real corpus sentence
/// (matching `seek`, not matching the rule), `bad` is the counterfactual
/// (first `seek` match replaced with `inject`, verified to flag). Falls back
/// to a canonical pair when no corpus sentence anchors the transform.
fn synthesize_example(
    files: &[CorpusFile],
    seek: &str,
    inject: &str,
    flags: &Regex,
    canonical_bad: &str,
    canonical_good: &str,
) -> RuleExample {
    let seek = Regex::new(seek).expect("static seek regex");
    for file in files {
        let document = lawlint_core::parse(&file.text, file.register == "markdown");
        for block in &document.blocks {
            for sentence in &block.sentences {
                if sentence.is_citation {
                    continue;
                }
                let text = sentence.range.slice(&file.text).trim();
                if text.len() < 20 || text.len() > 160 || text.contains('\n') {
                    continue;
                }
                if flags.is_match(text) || !seek.is_match(text) {
                    continue;
                }
                let bad = seek.replace(text, inject).into_owned();
                if flags.is_match(&bad) {
                    return RuleExample {
                        bad,
                        good: text.to_string(),
                    };
                }
            }
        }
    }
    RuleExample {
        bad: canonical_bad.to_string(),
        good: canonical_good.to_string(),
    }
}

fn provenance(stats: &CorpusStats, note: &str) -> String {
    format!(
        "{note} (mined from your corpus: {} file{}, ~{} words).",
        stats.files,
        if stats.files == 1 { "" } else { "s" },
        stats.words
    )
}

/// The alternation of a rule's patterns, for example verification.
fn combined_pattern(patterns: &[PatternYaml]) -> Regex {
    let alternation = patterns
        .iter()
        .map(|item| format!("(?:{})", item.pattern))
        .collect::<Vec<_>>()
        .join("|");
    Regex::new(&alternation).expect("candidate patterns were compiled at build time")
}

/// Deterministic candidates from the pass-1 counts. Everything here is
/// "your corpus never does X"; by construction those cannot self-fire, but
/// they still go through the same gate as agent candidates.
fn statistical_candidates(files: &[CorpusFile], stats: &CorpusStats) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    if stats.words < MIN_CORPUS_WORDS {
        return candidates;
    }
    let mut push = |rule: RuleYaml| match candidate(rule, "pass 1 (statistics)") {
        Ok(candidate) => candidates.push(candidate),
        Err(error) => debug_assert!(false, "pass-1 candidate failed to build: {error}"),
    };

    if stats.em_dashes == 0 {
        let patterns = vec![PatternYaml {
            pattern: "—".to_string(),
            message: None,
            suggestion: Some(
                "Substitute a comma, colon, or parentheses — whatever you would write.".to_string(),
            ),
            fix: None,
        }];
        let example = synthesize_example(
            files,
            ", ",
            "—",
            &combined_pattern(&patterns),
            "It was—frankly—wrong.",
            "It was, frankly, wrong.",
        );
        push(RuleYaml {
            id: "no-em-dash".to_string(),
            engine: "phrase".to_string(),
            severity: "warning".to_string(),
            description: provenance(stats, "You never use em dashes"),
            rationale: None,
            message: Some("You never use em dashes.".to_string()),
            examples: vec![example],
            patterns,
            metric: None,
            params: None,
        });
    }

    if stats.semicolons == 0 {
        let patterns = vec![PatternYaml {
            pattern: ";".to_string(),
            message: None,
            suggestion: Some("Split into two sentences or use a comma.".to_string()),
            fix: None,
        }];
        let example = synthesize_example(
            files,
            ", ",
            "; ",
            &combined_pattern(&patterns),
            "The motion failed; the case proceeded.",
            "The motion failed, and the case proceeded.",
        );
        push(RuleYaml {
            id: "no-semicolons".to_string(),
            engine: "phrase".to_string(),
            severity: "suggestion".to_string(),
            description: provenance(stats, "You never use semicolons"),
            rationale: None,
            message: Some("You never use semicolons.".to_string()),
            examples: vec![example],
            patterns,
            metric: None,
            params: None,
        });
    }

    // Oxford-comma consistency: only when the corpus is consistent one way
    // (a handful of lists minimum) does the opposite become a rule. The
    // rule patterns are the same list shapes the detectors count, so the
    // gate exercises exactly what the rule will flag.
    if stats.oxford_without == 0 && stats.oxford_with >= 3 {
        let patterns = vec![PatternYaml {
            pattern: r"[A-Za-z][\w'’-]*,\s+(?:[A-Za-z][\w'’-]*\s+){1,3}(?:and|or)\s+[A-Za-z]"
                .to_string(),
            message: None,
            suggestion: Some("Add the serial comma before the conjunction.".to_string()),
            fix: None,
        }];
        let example = synthesize_example(
            files,
            r", ([^,;]{1,40}), (and|or)\b",
            ", $1 $2",
            &combined_pattern(&patterns),
            "We reviewed the brief, the exhibits and the binder.",
            "We reviewed the brief, the exhibits, and the binder.",
        );
        push(RuleYaml {
            id: "serial-comma-required".to_string(),
            engine: "phrase".to_string(),
            severity: "suggestion".to_string(),
            description: provenance(stats, "You always use the serial (Oxford) comma"),
            rationale: None,
            message: Some("List is missing your usual serial comma.".to_string()),
            examples: vec![example],
            patterns,
            metric: None,
            params: None,
        });
    } else if stats.oxford_with == 0 && stats.oxford_without >= 3 {
        let patterns = vec![PatternYaml {
            pattern:
                r"[A-Za-z][\w'’-]*,\s+(?:[A-Za-z][\w'’-]*\s+){0,3}[A-Za-z][\w'’-]*,\s+(?:and|or)\b"
                    .to_string(),
            message: None,
            suggestion: Some("Drop the comma before the conjunction.".to_string()),
            fix: None,
        }];
        let example = synthesize_example(
            files,
            r", ([^,;]{1,40}) (and|or)\b",
            ", $1, $2",
            &combined_pattern(&patterns),
            "We reviewed the brief, the exhibits, and the binder.",
            "We reviewed the brief, the exhibits and the binder.",
        );
        push(RuleYaml {
            id: "no-serial-comma".to_string(),
            engine: "phrase".to_string(),
            severity: "suggestion".to_string(),
            description: provenance(stats, "You never use the serial (Oxford) comma"),
            rationale: None,
            message: Some("You never use the serial comma.".to_string()),
            examples: vec![example],
            patterns,
            metric: None,
            params: None,
        });
    }

    // AI-tell terms the corpus never uses, with mechanical fixes. Patterns
    // are lowercase-only so the fix string is a true drop-in.
    for (family, count) in TERM_FAMILIES.iter().zip(&stats.term_counts) {
        if *count > 0 {
            continue;
        }
        let patterns: Vec<PatternYaml> = family
            .variants
            .iter()
            .map(|(pattern, fix)| PatternYaml {
                pattern: (*pattern).to_string(),
                message: None,
                suggestion: Some(format!("Write \"{fix}\".")),
                fix: Some((*fix).to_string()),
            })
            .collect();
        let example = synthesize_example(
            files,
            family.seek,
            family.inject,
            &combined_pattern(&patterns),
            family.canonical_bad,
            family.canonical_good,
        );
        push(RuleYaml {
            id: family.id.to_string(),
            engine: "phrase".to_string(),
            severity: "warning".to_string(),
            description: provenance(stats, family.description.trim_end_matches('.')),
            rationale: None,
            message: None,
            examples: vec![example],
            patterns,
            metric: None,
            params: None,
        });
    }

    // Personal sentence-length cap from the corpus distribution (statistical
    // engine). Corpus max + headroom → never self-fires by construction.
    if stats.sentences >= 30 {
        let max_words = ((stats.sentence_words_max as f64 * 1.2).ceil() as u64).max(30) as f64;
        push(RuleYaml {
            id: "sentence-length".to_string(),
            engine: "statistical".to_string(),
            severity: "suggestion".to_string(),
            description: provenance(
                stats,
                &format!(
                    "Sentences longer than {max_words:.0} words are outside your range \
                     (your mean is {:.0}, your longest {})",
                    stats.sentence_words_mean, stats.sentence_words_max
                ),
            ),
            rationale: None,
            message: None,
            examples: Vec::new(),
            patterns: Vec::new(),
            metric: Some("sentence-length".to_string()),
            params: Some([("max_words".to_string(), max_words)].into_iter().collect()),
        });
    }

    candidates
}

// ---- pass 2: stratified sample + mining agent --------------------------

struct Passage {
    source: String,
    register: &'static str,
    text: String,
}

/// One stylistic dimension a mining worker focuses on. Every lens sees the
/// FULL corpus sample + stats — a "never does X" claim needs whole-corpus
/// evidence to be trustworthy — lenses vary the prompt's focus, not what
/// evidence it's given. `static` (not `const`) so `&LENSES[i]` is a single
/// canonical `'static` reference shareable across worker threads.
struct Lens {
    name: &'static str,
    focus: &'static str,
}

static LENSES: [Lens; 4] = [
    Lens {
        name: "punctuation & mechanics",
        focus: "dash, semicolon, and comma habits; quotation and citation punctuation style",
    },
    Lens {
        name: "phrasing & word choice",
        focus: "AI-tell terms, hedges, transitions, and filler phrases",
    },
    Lens {
        name: "sentence structure & rhythm",
        focus: "sentence length distribution, clause and parenthetical shape",
    },
    Lens {
        name: "structural & organizational habits",
        focus: "sentence openers, closers, paragraph and list shape",
    },
];

/// Split text into chunks of roughly `target` chars on paragraph
/// boundaries. Never drops text: an oversized paragraph becomes its own
/// chunk (the gate needs full coverage; the prompt truncates separately).
fn chunk_paragraphs(text: &str, target: usize) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    for paragraph in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        if !current.is_empty() && current.len() + paragraph.len() + 2 > target {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(paragraph);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(max_chars).collect();
    format!("{cut}…")
}

/// The pass-2 sample: recency-first across files (register diversity rides
/// on file diversity), passages spread evenly through each file (opening,
/// middle, closing prose), capped by count and characters.
fn stratified_sample(files: &[CorpusFile], max_passages: usize, max_chars: usize) -> Vec<Passage> {
    let mut by_recency: Vec<&CorpusFile> = files.iter().collect();
    by_recency.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.name.cmp(&b.name))
    });

    let per_file = max_passages.div_ceil(files.len().max(1)).max(1);
    let mut sample = Vec::new();
    let mut budget = max_chars;
    'files: for file in by_recency {
        let chunks = chunk_paragraphs(&file.text, PROMPT_PASSAGE_CHARS);
        if chunks.is_empty() {
            continue;
        }
        let quota = per_file.min(chunks.len());
        for slot in 0..quota {
            // Even spread: index slot*(len-1)/(quota-1) — first, middle, last.
            let index = if quota == 1 {
                0
            } else {
                slot * (chunks.len() - 1) / (quota - 1)
            };
            let text = truncate_chars(&chunks[index], PROMPT_PASSAGE_CHARS);
            if sample.len() >= max_passages || text.len() > budget {
                break 'files;
            }
            budget -= text.len();
            sample.push(Passage {
                source: file.name.clone(),
                register: file.register,
                text,
            });
        }
    }
    sample
}

fn stats_block(stats: &CorpusStats) -> String {
    let mut block = String::new();
    let _ = writeln!(
        block,
        "- {} files, ~{} words, {} sentences",
        stats.files, stats.words, stats.sentences
    );
    let _ = writeln!(
        block,
        "- punctuation: {} em dashes, {} en dashes, {} semicolons",
        stats.em_dashes, stats.en_dashes, stats.semicolons
    );
    let _ = writeln!(
        block,
        "- serial (Oxford) comma: {} lists with it, {} without",
        stats.oxford_with, stats.oxford_without
    );
    let _ = writeln!(
        block,
        "- sentence length: mean {:.0} words, longest {}",
        stats.sentence_words_mean, stats.sentence_words_max
    );
    let openers = stats
        .opener_top
        .iter()
        .map(|(word, count)| format!("\"{word}\" ({count})"))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        block,
        "- most common sentence openers: {}",
        if openers.is_empty() { "n/a" } else { &openers }
    );
    let absent = TERM_FAMILIES
        .iter()
        .zip(&stats.term_counts)
        .filter(|(_, count)| **count == 0)
        .map(|(family, _)| {
            family
                .id
                .strip_prefix("no-")
                .unwrap_or(family.id)
                .replace('-', " ")
        })
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        block,
        "- terms never used: {}",
        if absent.is_empty() { "none" } else { &absent }
    );
    block
}

fn mining_prompt(stats: &CorpusStats, sample: &[Passage], lens: &Lens) -> String {
    let mut prompt = String::new();
    let _ = write!(
        prompt,
        "You are lawlint's rule-mining agent, one of several parallel specialists \
         each mining the same corpus for a different stylistic dimension. Below are \
         statistics computed over a writer's FULL body of prior writing, then sample \
         passages from it. Propose lint rules that capture this writer's personal \
         style so AI-drafted text can be checked against their voice: patterns the \
         writer NEVER uses (flag them), and consistent habits (flag deviations).\n\n\
         Your focus this pass is {}: {}. Only propose rules in this lane; other \
         specialists are covering the rest.\n\nCorpus statistics:\n",
        lens.name, lens.focus
    );
    prompt.push_str(&stats_block(stats));
    prompt.push_str("\nSample passages:\n");
    for (index, passage) in sample.iter().enumerate() {
        let _ = writeln!(
            prompt,
            "--- passage {} ({}, {}) ---\n{}",
            index + 1,
            passage.source,
            passage.register,
            passage.text
        );
    }
    let _ = write!(
        prompt,
        "\nRespond with ONLY a JSON array of at most {MAX_MINED_RULES_PER_LENS} rules. Each rule:\n\
         {{\"id\": \"kebab-case-name\", \"engine\": \"phrase\" or \"leading\", \
         \"severity\": \"warning\" or \"suggestion\", \"description\": \"...\", \
         \"message\": \"...\", \
         \"patterns\": [{{\"pattern\": \"<Rust-flavored regex>\", \"suggestion\": \"...\"}}], \
         \"examples\": [{{\"bad\": \"<short counterfactual text the rule flags>\", \
         \"good\": \"<the writer's actual phrasing, quoted from a passage>\"}}], \
         \"mined_from\": \"<source passage and approximate frequency>\"}}\n\
         Rules must describe this writer specifically, not generic style advice. \
         A rule's patterns must NOT match any passage above — rules that fire on the \
         writer's own text will be rejected. \"leading\" rules match sentence openers. \
         Return [] if nothing is reliable."
    );
    prompt
}

// ---- mined-candidate parsing -------------------------------------------

#[derive(Deserialize)]
#[serde(untagged)]
enum MinedPattern {
    Bare(String),
    // No `fix` field: a `fix:` ships as a MachineApplicable edit that
    // `--fix` (and the .docx revision path) applies verbatim, and the gate
    // never exercises replacement text — examples only check flag/not-flag.
    // Mechanical fixes stay hand-curated in pass 1; an agent-supplied "fix"
    // key is ignored by parsing.
    Detailed {
        pattern: String,
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
        suggestion: Option<String>,
    },
}

#[derive(Deserialize)]
struct MinedExample {
    bad: String,
    good: String,
}

#[derive(Deserialize)]
struct MinedRule {
    id: String,
    #[serde(default)]
    engine: Option<String>,
    #[serde(default)]
    severity: Option<String>,
    description: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    rationale: Option<String>,
    #[serde(default)]
    patterns: Vec<MinedPattern>,
    #[serde(default)]
    examples: Vec<MinedExample>,
    #[serde(default)]
    mined_from: Option<String>,
}

/// Strip a single wrapping markdown code fence (``` or ```json).
fn strip_code_fences(s: &str) -> &str {
    let s = s.trim();
    let Some(rest) = s.strip_prefix("```") else {
        return s;
    };
    let rest = match rest.find('\n') {
        Some(index) => &rest[index + 1..],
        None => rest,
    };
    rest.trim_end().strip_suffix("```").unwrap_or(rest).trim()
}

/// Parse the agent's response into mined rules. Tolerates code fences and
/// prose around the JSON array (same posture as the judge's finding parse).
fn parse_mined(content: &str) -> Result<Vec<MinedRule>, String> {
    let stripped = strip_code_fences(content.trim());
    let mut candidates: Vec<&str> = vec![stripped];
    if let (Some(start), Some(end)) = (stripped.find('['), stripped.rfind(']')) {
        if start < end {
            candidates.push(&stripped[start..=end]);
        }
    }
    for candidate in candidates {
        if let Ok(rules) = serde_json::from_str::<Vec<MinedRule>>(candidate) {
            return Ok(rules);
        }
    }
    Err(format!(
        "mining agent returned no parseable rule array: {}",
        truncate_chars(content, 200)
    ))
}

/// kebab-case a mined id; empty results get a positional fallback.
fn sanitize_id(raw: &str, index: usize) -> String {
    let mut id = String::new();
    for c in raw.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            id.push(c);
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
    }
    let id = id.trim_matches('-').to_string();
    if id.is_empty() {
        format!("mined-{}", index + 1)
    } else {
        id
    }
}

/// Convert one mined rule into a validated candidate. Engine is restricted
/// to phrase/leading (the agent gets no judgment-free engines), severity is
/// clamped below error, and provenance lands in the description.
fn mined_candidate(
    rule: MinedRule,
    index: usize,
    model_id: &str,
    lens_name: &str,
    used_ids: &BTreeSet<String>,
) -> Result<Candidate, (String, String)> {
    let id = sanitize_id(&rule.id, index);
    let fail = |reason: String| Err((id.clone(), reason));
    if used_ids.contains(&id) {
        return fail("duplicate rule id".to_string());
    }
    let engine = rule.engine.as_deref().unwrap_or("phrase");
    if !matches!(engine, "phrase" | "leading") {
        return fail(format!(
            "engine {engine:?} not allowed for mined rules (use phrase or leading)"
        ));
    }
    let severity = match rule.severity.as_deref() {
        Some("warning") | Some("error") => "warning",
        _ => "suggestion",
    };
    if rule.examples.is_empty() {
        return fail("no examples (mined rules must quote your text)".to_string());
    }
    let mut description = rule.description.trim().trim_end_matches('.').to_string();
    if description.is_empty() {
        return fail("empty description".to_string());
    }
    match &rule.mined_from {
        Some(note) if !note.trim().is_empty() => {
            let _ = write!(
                description,
                " (mined by {model_id}, lens: {lens_name}, from {}).",
                note.trim()
            );
        }
        _ => {
            let _ = write!(
                description,
                " (mined by {model_id}, lens: {lens_name}, from your corpus sample)."
            );
        }
    }
    let patterns = rule
        .patterns
        .into_iter()
        .map(|pattern| match pattern {
            MinedPattern::Bare(pattern) => PatternYaml {
                pattern,
                message: None,
                suggestion: None,
                fix: None,
            },
            MinedPattern::Detailed {
                pattern,
                message,
                suggestion,
            } => PatternYaml {
                pattern,
                message,
                suggestion,
                fix: None,
            },
        })
        .collect();
    let examples = rule
        .examples
        .into_iter()
        .map(|example| RuleExample {
            bad: example.bad,
            good: example.good,
        })
        .collect();
    candidate(
        RuleYaml {
            id,
            engine: engine.to_string(),
            severity: severity.to_string(),
            description,
            rationale: rule.rationale,
            message: rule.message,
            examples,
            patterns,
            metric: None,
            params: None,
        },
        "pass 2 (mining agent)",
    )
    .map_err(|error| (sanitize_id(&rule.id, index), error))
}

// ---- self-consistency gate ---------------------------------------------

struct GateReport {
    kept: Vec<Candidate>,
    /// (rule id, origin, reason).
    dropped: Vec<(String, &'static str, String)>,
    gate_samples: usize,
}

/// Run the candidate package back over the full corpus (the all-human
/// class) and keep only rules that never self-fire AND whose own examples
/// flag/pass correctly.
fn self_consistency_gate(
    package: &str,
    candidates: Vec<Candidate>,
    files: &[CorpusFile],
) -> Result<GateReport, String> {
    let samples: Vec<Sample> = files
        .iter()
        .flat_map(|file| {
            chunk_paragraphs(&file.text, GATE_CHUNK_CHARS)
                .into_iter()
                .enumerate()
                .map(|(index, text)| Sample {
                    id: format!("{}#{index}", file.name),
                    label: Label::Human,
                    text,
                    word_count: None,
                    source: file.name.clone(),
                    register: file.register.to_string(),
                    era: None,
                    date: None,
                    court: None,
                    model: None,
                    prompt_style: None,
                    pair_id: None,
                    split: None,
                })
                .collect::<Vec<_>>()
        })
        .collect();

    let sources: Vec<(String, String)> = candidates
        .iter()
        .map(|candidate| (candidate.file_name.clone(), candidate.yaml.clone()))
        .collect();
    let set = RuleSet::from_sources(package, &sources).map_err(|error| error.to_string())?;
    let options = LintOptions::default();
    let evaluated = evaluate_with(&samples, &options, &set);
    let metrics = per_rule_metrics(
        &evaluated,
        candidates
            .iter()
            .map(|candidate| format!("{package}/{}", candidate.id)),
    );

    let mut report = GateReport {
        kept: Vec::new(),
        dropped: Vec::new(),
        gate_samples: samples.len(),
    };
    for candidate in candidates {
        let full_id = format!("{package}/{}", candidate.id);
        let self_fires = metrics
            .get(&full_id)
            .map(|metric| metric.false_positive)
            .unwrap_or(0);
        if self_fires > 0 {
            report.dropped.push((
                candidate.id,
                candidate.origin,
                format!(
                    "fired on {self_fires} of {} corpus passages (your writing does this)",
                    samples.len()
                ),
            ));
            continue;
        }
        // `lawlint rules test` semantics on the rule's own examples: bad
        // must flag, good must not (single-rule set, defaults).
        match example_failure(package, &candidate) {
            None => report.kept.push(candidate),
            Some(reason) => report
                .dropped
                .push((candidate.id, candidate.origin, reason)),
        }
    }
    Ok(report)
}

/// First failing example of a candidate, if any (None = all pass or the
/// rule declares no examples, which `rules test` skips).
fn example_failure(package: &str, candidate: &Candidate) -> Option<String> {
    let set = RuleSet::from_sources(
        package,
        &[(candidate.file_name.clone(), candidate.yaml.clone())],
    )
    .ok()?;
    let full_id = format!("{package}/{}", candidate.id);
    let options = LintOptions::default();
    let fires = |text: &str| {
        lint_with(text, &options, &set)
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.rule_id.0 == full_id)
    };
    for (index, example) in candidate.def.examples.iter().enumerate() {
        if !fires(&example.bad) {
            return Some(format!("examples.bad[{index}] is not flagged by the rule"));
        }
        if fires(&example.good) {
            return Some(format!("examples.good[{index}] is flagged by the rule"));
        }
    }
    None
}

// ---- output package ----------------------------------------------------

/// Package name from the output directory (init-style sanitization);
/// "core" is reserved for the built-ins.
fn package_name(out: &Path) -> String {
    let raw = out
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut name = String::new();
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c);
        } else if !name.is_empty() && !name.ends_with('-') {
            name.push('-');
        }
    }
    let name = name.trim_matches('-').to_string();
    if name.is_empty() || name == "core" {
        "personal".to_string()
    } else {
        name
    }
}

/// Delete generated rule files from earlier runs that this run did not
/// keep. Without this, a rule the current gate would drop (or no longer
/// proposes) stays on disk and active in the package — flagging the user's
/// own current writing, which is exactly what the gate exists to prevent.
/// Only files starting with GENERATED_HEADER are touched; user-authored
/// rules survive. Returns how many files were removed.
fn prune_stale_rules(out: &Path, kept: &[Candidate]) -> Result<usize, String> {
    let Ok(entries) = fs::read_dir(out.join("rules")) else {
        return Ok(0); // no package on disk yet
    };
    let kept_names: BTreeSet<String> = kept
        .iter()
        .map(|candidate| format!("{}.md", candidate.id))
        .collect();
    let mut removed = 0;
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        let markdown = path.extension().and_then(|ext| ext.to_str()) == Some("md");
        let kept = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| kept_names.contains(name));
        let generated = markdown
            && !kept
            && fs::read_to_string(&path).is_ok_and(|text| text.contains(GENERATED_HEADER));
        if generated {
            fs::remove_file(&path)
                .map_err(|error| format!("failed to remove stale {}: {error}", path.display()))?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn write_package(
    out: &Path,
    package: &str,
    corpus_label: &str,
    kept: &[Candidate],
) -> Result<(), String> {
    prune_stale_rules(out, kept)?;
    let rules_dir = out.join("rules");
    fs::create_dir_all(&rules_dir)
        .map_err(|error| format!("failed to create {}: {error}", rules_dir.display()))?;
    let manifest = format!(
        "name: {package}\nversion: 0.1.0\ndescription: Personal style rules mined by \
         `lawlint learn` from {corpus_label}.\n"
    );
    let manifest_path = out.join("style.yaml");
    fs::write(&manifest_path, manifest)
        .map_err(|error| format!("failed to write {}: {error}", manifest_path.display()))?;
    for candidate in kept {
        let path = out.join(&candidate.file_name);
        fs::write(&path, &candidate.yaml)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    }
    Ok(())
}

// ---- command -----------------------------------------------------------

/// The mining chat round-trip: one retry on malformed output (same posture
/// as the judge), errors degrade to "no mined candidates" upstream.
fn mine(client: &mut dyn AxAIClient, prompt: &str) -> Result<Vec<MinedRule>, String> {
    let request = json!({
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0,
    });
    let mut last_error = String::new();
    for _ in 0..2 {
        let response = match client.chat(request.clone()) {
            Ok(response) => response,
            Err(error) => return Err(error.to_string()),
        };
        let Some(content) = lawlint_judge::chat_content(&response) else {
            last_error = format!(
                "no textual content in chat response: {}",
                truncate_chars(&response.to_string(), 200)
            );
            continue;
        };
        match parse_mined(&content) {
            Ok(rules) => return Ok(rules),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

/// One pass-2 mining round-trip: which client runs it, which lens it's
/// mining for, and the prompt already built for that lens.
struct MiningJob {
    client: Box<dyn AxAIClient + Send>,
    lens: &'static Lens,
    prompt: String,
}

/// What one job produced. The client comes back too — clients are cheap and
/// stateless (`lawlint_judge::create_client`'s own doc comment), but a
/// worker's client is still perfectly usable for a follow-up round (e.g.
/// self-repair) and reusing it beats building another.
struct MiningOutcome {
    client: Box<dyn AxAIClient + Send>,
    lens: &'static Lens,
    result: Result<Vec<MinedRule>, String>,
}

/// Run every job on its own thread concurrently, one client per lens (no
/// mutex/checkout needed — unlike `AxJudge`'s pool, here #jobs == #clients,
/// so each thread just owns its client outright for the call's duration).
/// `progress`'s line index is each job's position in `jobs`.
fn mine_concurrent(jobs: Vec<MiningJob>, progress: &MultiSpinner) -> Vec<MiningOutcome> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .into_iter()
            .enumerate()
            .map(|(index, job)| {
                scope.spawn(move || {
                    let MiningJob {
                        mut client,
                        lens,
                        prompt,
                    } = job;
                    progress.set_running(index, "mining…");
                    // AxAIClient is a public trait with third-party impls (same
                    // posture as AxJudge's fetch_parallel, judge.rs) — a
                    // panicking backend must fail this one lens, not tear down
                    // every other concurrent worker.
                    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        mine(client.as_mut(), &prompt)
                    }))
                    .unwrap_or_else(|_| {
                        Err("mining worker panicked while evaluating this lens".to_string())
                    });
                    match &result {
                        Ok(rules) => progress.set_done(
                            index,
                            format!(
                                "{} candidate{}",
                                rules.len(),
                                if rules.len() == 1 { "" } else { "s" }
                            ),
                        ),
                        Err(error) => progress.set_failed(index, truncate_chars(error, 60)),
                    }
                    MiningOutcome {
                        client,
                        lens,
                        result,
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .expect("mining worker panic was already caught inside catch_unwind")
            })
            .collect()
    })
}

/// Whether a gate-rejection reason is a content problem the agent could
/// plausibly fix by rewriting the rule. "duplicate rule id" is a naming
/// collision with an unrelated candidate, not a rule-quality problem — a
/// rewrite wouldn't touch that and would just burn a round-trip.
fn is_repairable_reason(reason: &str) -> bool {
    !reason.contains("duplicate rule id")
}

/// The first corpus passage a candidate self-fired on, so a repair prompt
/// can show the model exactly what its pattern over-matched instead of just
/// a count. Chunked the same way the gate itself chunks (`GATE_CHUNK_CHARS`)
/// so this is the same evidence the gate used, truncated to prompt size.
fn self_fire_example(package: &str, candidate: &Candidate, files: &[CorpusFile]) -> Option<String> {
    let set = RuleSet::from_sources(
        package,
        &[(candidate.file_name.clone(), candidate.yaml.clone())],
    )
    .ok()?;
    let full_id = format!("{package}/{}", candidate.id);
    let options = LintOptions::default();
    for file in files {
        for chunk in chunk_paragraphs(&file.text, GATE_CHUNK_CHARS) {
            let fires = lint_with(&chunk, &options, &set)
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.rule_id.0 == full_id);
            if fires {
                return Some(truncate_chars(&chunk, PROMPT_PASSAGE_CHARS));
            }
        }
    }
    None
}

/// One lens's batched follow-up: every candidate from that lens the gate
/// rejected, why, and (when found) the corpus passage it incorrectly fired
/// on. Asks for corrected versions keyed by the same `id` so responses can
/// be matched back to the originals.
fn repair_prompt(items: &[(&Candidate, &str)], examples: &BTreeMap<String, String>) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "The following rules you proposed failed lawlint's self-consistency check: \
         each one fires on the same writer's own corpus it was supposed to describe, \
         or fails its own bad/good example. For each, the original rule, why it \
         failed, and (when available) the corpus passage it incorrectly fired on.\n\n",
    );
    for (candidate, reason) in items {
        let _ = write!(
            prompt,
            "--- {} ---\n{}\nFailure: {reason}\n",
            candidate.id, candidate.yaml
        );
        if let Some(example) = examples.get(&candidate.id) {
            let _ = write!(
                prompt,
                "Fired on this passage from the writer's own corpus (it must not):\n{example}\n"
            );
        }
        prompt.push('\n');
    }
    prompt.push_str(
        "Fix each rule so it no longer fires on the writer's own text (narrow the \
         pattern, or fix the failing example) while still describing a real habit. \
         Respond with ONLY a JSON array of the corrected rules, same schema as \
         before, one entry per rule above using the SAME \"id\" so it can be matched \
         back. Omit a rule from the array if it genuinely cannot be fixed.",
    );
    prompt
}

fn run_learn(
    files: &[CorpusFile],
    corpus_label: &str,
    out: &Path,
    clients: Vec<Box<dyn AxAIClient + Send>>,
    model_id: &str,
    quiet: bool,
    output: &mut dyn Write,
) -> Result<i32, String> {
    let say = |output: &mut dyn Write, line: &str| -> Result<(), String> {
        writeln!(output, "{line}").map_err(|error| error.to_string())
    };

    // Pass 1: full-corpus statistics + deterministic candidates.
    let stats = corpus_stats(files);
    say(
        output,
        &format!(
            "Pass 1 (local statistics): {} file{}, ~{} words, {} sentences",
            stats.files,
            if stats.files == 1 { "" } else { "s" },
            stats.words,
            stats.sentences
        ),
    )?;
    if stats.words < MIN_CORPUS_WORDS {
        say(
            output,
            &format!(
                "  corpus is under {MIN_CORPUS_WORDS} words — too small for \
                 \"never does X\" statistics; skipping pass-1 candidates"
            ),
        )?;
    }
    let mut candidates = statistical_candidates(files, &stats);
    let pass1_count = candidates.len();

    // Pass 2: stratified sample → mining agent over the ax boundary, fanned
    // out across up to `clients.len()` lenses concurrently (`zip` clamps to
    // whichever of `clients`/`LENSES` is shorter).
    let sample = stratified_sample(files, MAX_PASSAGES, MAX_SAMPLE_CHARS);
    let sample_chars: usize = sample.iter().map(|passage| passage.text.len()).sum();
    let active_lenses: Vec<&'static Lens> = LENSES.iter().take(clients.len()).collect();
    say(
        output,
        &format!(
            "Pass 2 (mining agent): {} lens{} over {} passage{} (~{} tokens each) -> {model_id}",
            active_lenses.len(),
            if active_lenses.len() == 1 { "" } else { "es" },
            sample.len(),
            if sample.len() == 1 { "" } else { "s" },
            sample_chars / 4
        ),
    )?;

    let spinner_header = format!("Pass 2 (mining agent) -> {model_id}");
    let spinner = MultiSpinner::new(
        &spinner_header,
        active_lenses
            .iter()
            .map(|lens| lens.name.to_string())
            .collect(),
        quiet,
    );
    let jobs: Vec<MiningJob> = clients
        .into_iter()
        .zip(active_lenses.iter().copied())
        .map(|(client, lens)| MiningJob {
            client,
            lens,
            prompt: mining_prompt(&stats, &sample, lens),
        })
        .collect();
    let outcomes = mine_concurrent(jobs, &spinner);
    spinner.finish();

    let mut used_ids: BTreeSet<String> = candidates.iter().map(|c| c.id.clone()).collect();
    let mut seen_patterns: BTreeSet<String> = BTreeSet::new();
    let mut invalid: Vec<(String, &'static str, String)> = Vec::new();
    let mut duplicate_count = 0usize;
    // Snapshot every merged mined candidate (kept + not-yet-gated), keyed by
    // id, so a candidate the gate later drops can still be routed to a
    // repair prompt through the lens that proposed it.
    let mut mined_snapshot: BTreeMap<String, (Candidate, &'static Lens)> = BTreeMap::new();
    // Reusable lens clients for a possible repair round — cheap and
    // stateless, so reusing beats rebuilding (create_client's own posture).
    let mut lens_clients: BTreeMap<&'static str, Box<dyn AxAIClient + Send>> = BTreeMap::new();
    let mut lens_errors: Vec<(&'static str, String)> = Vec::new();
    let mut agent_count = 0usize;

    for outcome in outcomes {
        lens_clients.insert(outcome.lens.name, outcome.client);
        match outcome.result {
            Ok(mined) => {
                for (index, rule) in mined.into_iter().take(MAX_MINED_RULES_PER_LENS).enumerate() {
                    agent_count += 1;
                    match mined_candidate(rule, index, model_id, outcome.lens.name, &used_ids) {
                        Ok(candidate) => {
                            let pattern_key = candidate
                                .def
                                .patterns
                                .first()
                                .map(|pattern| pattern.pattern().trim().to_ascii_lowercase());
                            if let Some(key) = &pattern_key {
                                if !seen_patterns.insert(key.clone()) {
                                    // Another lens already proposed this same
                                    // habit; first-seen wins, this is not a
                                    // failure worth reporting as one.
                                    duplicate_count += 1;
                                    continue;
                                }
                            }
                            used_ids.insert(candidate.id.clone());
                            mined_snapshot
                                .insert(candidate.id.clone(), (candidate.clone(), outcome.lens));
                            candidates.push(candidate);
                        }
                        Err((id, reason)) => invalid.push((id, "pass 2 (mining agent)", reason)),
                    }
                }
            }
            Err(error) => lens_errors.push((outcome.lens.name, error)),
        }
    }
    for (lens_name, error) in &lens_errors {
        say(
            output,
            &format!(
                "lawlint: warning: mining lens {lens_name:?} unavailable ({error}); \
                 other lenses still ran"
            ),
        )?;
    }
    if lens_errors.len() == active_lenses.len() && !active_lenses.is_empty() {
        say(
            output,
            "lawlint: warning: every mining lens was unavailable; keeping pass-1 candidates only",
        )?;
    }
    let dedup_note = if duplicate_count > 0 {
        format!(", {duplicate_count} duplicate merged")
    } else {
        String::new()
    };
    say(
        output,
        &format!(
            "Candidates: {} ({pass1_count} statistical, {agent_count} agent{dedup_note})",
            candidates.len() + invalid.len()
        ),
    )?;

    // Self-consistency gate over the full corpus.
    let package = package_name(out);
    let mut report = self_consistency_gate(&package, candidates, files)?;

    // Bounded self-repair: gate-rejected mined candidates get exactly one
    // chance to fix themselves, routed back through the lens that proposed
    // them. Duplicate-id rejections (`invalid`) are not content problems, so
    // they never enter this round.
    let repairable: Vec<(Candidate, &'static Lens, String)> = report
        .dropped
        .iter()
        .filter(|(_, origin, reason)| {
            *origin == "pass 2 (mining agent)" && is_repairable_reason(reason)
        })
        .filter_map(|(id, _, reason)| {
            mined_snapshot
                .get(id)
                .map(|(candidate, lens)| (candidate.clone(), *lens, reason.clone()))
        })
        .collect();
    let mut rescued = 0usize;
    if !repairable.is_empty() {
        say(
            output,
            &format!(
                "Refining {} candidate{} that failed the self-consistency gate…",
                repairable.len(),
                if repairable.len() == 1 { "" } else { "s" }
            ),
        )?;
        let repairing_ids: BTreeSet<String> =
            repairable.iter().map(|(c, _, _)| c.id.clone()).collect();
        let repair_used_ids: BTreeSet<String> =
            used_ids.difference(&repairing_ids).cloned().collect();

        let mut by_lens: BTreeMap<&'static str, Vec<(Candidate, String)>> = BTreeMap::new();
        for (candidate, lens, reason) in &repairable {
            by_lens
                .entry(lens.name)
                .or_default()
                .push((candidate.clone(), reason.clone()));
        }
        let repair_jobs: Vec<MiningJob> = by_lens
            .into_iter()
            .filter_map(|(lens_name, items)| {
                let client = lens_clients.remove(lens_name)?;
                let lens = active_lenses
                    .iter()
                    .copied()
                    .find(|lens| lens.name == lens_name)?;
                let examples: BTreeMap<String, String> = items
                    .iter()
                    .filter_map(|(candidate, _)| {
                        self_fire_example(&package, candidate, files)
                            .map(|example| (candidate.id.clone(), example))
                    })
                    .collect();
                let refs: Vec<(&Candidate, &str)> = items
                    .iter()
                    .map(|(candidate, reason)| (candidate, reason.as_str()))
                    .collect();
                Some(MiningJob {
                    client,
                    lens,
                    prompt: repair_prompt(&refs, &examples),
                })
            })
            .collect();

        let repair_header = format!("Refining candidates -> {model_id}");
        let repair_spinner = MultiSpinner::new(
            &repair_header,
            repair_jobs
                .iter()
                .map(|job| job.lens.name.to_string())
                .collect(),
            quiet,
        );
        let repair_outcomes = mine_concurrent(repair_jobs, &repair_spinner);
        repair_spinner.finish();

        let mut repaired_candidates: Vec<Candidate> = Vec::new();
        for outcome in repair_outcomes {
            let Ok(mined) = outcome.result else { continue };
            for (index, rule) in mined.into_iter().enumerate() {
                let lens_label = format!("{}, refined", outcome.lens.name);
                if let Ok(candidate) =
                    mined_candidate(rule, index, model_id, &lens_label, &repair_used_ids)
                {
                    if repairing_ids.contains(&candidate.id) {
                        repaired_candidates.push(candidate);
                    }
                }
            }
        }

        if !repaired_candidates.is_empty() {
            let repair_report = self_consistency_gate(&package, repaired_candidates, files)?;
            rescued = repair_report.kept.len();
            // Every candidate that went through the repair round must leave
            // `report.dropped` exactly once: rescued ones move to `kept`,
            // re-failed ones are re-added below with a note. Retaining a
            // re-failed candidate's original entry would list (and count) it
            // twice. Candidates the agent never returned a repair for are
            // absent from `repair_report` and correctly keep their one entry.
            let repaired_ids: BTreeSet<String> = repair_report
                .kept
                .iter()
                .map(|c| c.id.clone())
                .chain(repair_report.dropped.iter().map(|(id, _, _)| id.clone()))
                .collect();
            report
                .dropped
                .retain(|(id, _, _)| !repaired_ids.contains(id));
            for (id, origin, reason) in repair_report.dropped {
                report
                    .dropped
                    .push((id, origin, format!("{reason} (after one repair attempt)")));
            }
            report.kept.extend(repair_report.kept);
        }
        say(
            output,
            &format!(
                "Refined: {rescued} rescued, {} still dropped",
                repairable.len() - rescued
            ),
        )?;
    }

    say(
        output,
        &format!(
            "Self-consistency gate over {} corpus passage{}:",
            report.gate_samples,
            if report.gate_samples == 1 { "" } else { "s" }
        ),
    )?;
    for candidate in &report.kept {
        say(
            output,
            &format!(
                "  kept    {package}/{:<28} self-fire 0/{} [{}]",
                candidate.id, report.gate_samples, candidate.origin
            ),
        )?;
    }
    for (id, origin, reason) in report.dropped.iter().chain(invalid.iter()) {
        say(
            output,
            &format!("  dropped {package}/{id:<28} {reason} [{origin}]"),
        )?;
    }
    let total = report.kept.len() + report.dropped.len() + invalid.len();
    say(
        output,
        &format!("Kept {} of {} candidates.", report.kept.len(), total),
    )?;

    if report.kept.is_empty() {
        // Stale generated rules from a previous run must still go: leaving
        // them would keep dropped rules active in the package.
        let removed = prune_stale_rules(out, &report.kept)?;
        if removed > 0 {
            say(
                output,
                &format!(
                    "Removed {removed} stale generated rule{} from {}.",
                    if removed == 1 { "" } else { "s" },
                    out.display()
                ),
            )?;
        }
        say(
            output,
            "No rules survived; nothing written. A larger corpus (or a stronger \
             model via `lawlint init`) mines more.",
        )?;
        return Ok(0);
    }
    write_package(out, &package, corpus_label, &report.kept)?;
    say(
        output,
        &format!(
            "Wrote {} (style.yaml + {} rule{})",
            out.display(),
            report.kept.len(),
            if report.kept.len() == 1 { "" } else { "s" }
        ),
    )?;
    say(
        output,
        &format!(
            "Next:\n  lawlint rules test {}   check the generated rules\n  \
             add {:?} to ruleDirs in .lawlint/config.json to lint with them",
            out.display(),
            out.display().to_string()
        ),
    )?;
    Ok(0)
}

/// The mining model spec: `--model` overrides; otherwise the AI preferences
/// from `lawlint init` (`ai.features.learn`, then `ai.model`). No silent
/// local fallback (#50): unconfigured errors with init guidance instead of
/// downloading a model.
fn resolve_model_spec(model_flag: Option<&str>, config: &LintOptions) -> Result<String, String> {
    model_flag
        .map(str::to_string)
        .or_else(|| config.ai_model("learn"))
        .ok_or_else(|| {
            "lawlint learn needs an AI model but none is configured — run `lawlint init` \
             to choose one (hosted providers recommended), or pass an explicit \
             --model <spec> (e.g. --model anthropic:<model>)"
                .to_string()
        })
}

/// How many of `LENSES` to mine concurrently: `--workers` overrides
/// `learn.concurrency`, which overrides the default (every lens). Clamped to
/// `[1, LENSES.len()]` — more workers than lenses buys nothing today, same
/// posture as `lawlint_judge::concurrency()`.
fn resolve_concurrency(workers_flag: Option<usize>, config: &LintOptions) -> usize {
    let configured =
        workers_flag.or_else(|| config.learn.as_ref().and_then(|learn| learn.concurrency));
    configured.unwrap_or(LENSES.len()).clamp(1, LENSES.len())
}

pub(crate) fn learn_command(
    path: &Path,
    out: &Path,
    model_flag: Option<&str>,
    workers_flag: Option<usize>,
    quiet: bool,
) -> Result<i32, String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let (config, _) = crate::find_config(cwd)?;
    // Ingest before any model work: a bad path must not trigger downloads.
    let files = ingest(path)?;
    if files.is_empty() {
        return Err(format!(
            "no corpus files found under {} (looked for .docx, .md, .txt)",
            path.display()
        ));
    }
    let spec = resolve_model_spec(model_flag, &config)?;
    if let Some(notice) = crate::local_notice(&spec) {
        eprintln!("{notice}");
    }
    let workers = resolve_concurrency(workers_flag, &config);
    let mut clients: Vec<Box<dyn AxAIClient + Send>> = Vec::with_capacity(workers);
    let mut model_id = String::new();
    // Clients are cheap and stateless (lawlint_judge::create_client's own
    // doc comment: "clients are independent, so this costs a little setup
    // and no network traffic") — building `workers` of them up front is the
    // same pattern `create_judge` already uses for the lint judge's pool.
    for _ in 0..workers {
        let (client, id) =
            lawlint_judge::create_client(&spec).map_err(|error| error.to_string())?;
        model_id = id;
        clients.push(client);
    }
    let stdout = io::stdout();
    run_learn(
        &files,
        &path.display().to_string(),
        out,
        clients,
        &model_id,
        quiet,
        &mut stdout.lock(),
    )
}

// ------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use lawlint_judge::{AxError, AxResult};
    use serde_json::Value;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    // A scripted mining backend — the mock AxAIClient the whole flow is
    // tested against. No network, no real model. `requests` is shared via
    // `Arc<Mutex<..>>` (not owned by the client) so a test can still read it
    // after the client is moved into `run_learn`'s `Vec<Box<dyn AxAIClient>>`.
    struct FakeClient {
        responses: VecDeque<AxResult<Value>>,
        requests: Arc<Mutex<Vec<Value>>>,
    }

    impl FakeClient {
        /// A raw client plus a handle to its request log.
        fn new(responses: Vec<AxResult<Value>>) -> (Self, Arc<Mutex<Vec<Value>>>) {
            let requests = Arc::new(Mutex::new(Vec::new()));
            (
                FakeClient {
                    responses: responses.into(),
                    requests: requests.clone(),
                },
                requests,
            )
        }

        /// A boxed client ready to go into `run_learn`'s client pool, plus a
        /// handle to its request log.
        fn boxed(
            responses: Vec<AxResult<Value>>,
        ) -> (Box<dyn AxAIClient + Send>, Arc<Mutex<Vec<Value>>>) {
            let (client, requests) = Self::new(responses);
            (Box::new(client), requests)
        }
    }

    impl AxAIClient for FakeClient {
        fn chat(&mut self, request: Value) -> AxResult<Value> {
            self.requests.lock().unwrap().push(request);
            self.responses
                .pop_front()
                .unwrap_or_else(|| Err(AxError::runtime("fake client exhausted")))
        }
    }

    fn choices_response(content: &str) -> AxResult<Value> {
        Ok(json!({
            "choices": [{"index": 0, "message": {"role": "assistant", "content": content}}]
        }))
    }

    fn corpus_file(name: &str, text: &str) -> CorpusFile {
        CorpusFile {
            name: name.to_string(),
            register: "plain-text",
            modified: SystemTime::UNIX_EPOCH,
            text: text.to_string(),
        }
    }

    /// A corpus comfortably over MIN_CORPUS_WORDS with no em dashes,
    /// semicolons, or AI-tell terms; consistent Oxford commas; short
    /// sentences. Varied sentences so phrase example synthesis has anchors.
    fn sample_corpus() -> String {
        let mut text = String::new();
        text.push_str(
            "We use the standard form for every filing. The court denied the motion, \
             and the case proceeded to trial. Counsel reviewed the brief, the exhibits, \
             and the binder before the hearing.\n\n",
        );
        for index in 0..40 {
            let _ = writeln!(
                text,
                "The witness answered question {index} directly, and the record shows it. \
                 We keep sentences short, plain, and precise in every draft.\n"
            );
        }
        text
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lawlint-learn-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ---- ingestion ------------------------------------------------------

    #[test]
    fn ingest_recurses_and_routes_extensions() {
        let dir = temp_dir("ingest");
        fs::write(dir.join("memo.txt"), "A memo body.").unwrap();
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/post.md"), "# A post\n\nBody.").unwrap();
        fs::write(dir.join("notes.log"), "ignored").unwrap();
        fs::write(dir.join(".hidden.txt"), "ignored").unwrap();

        let files = ingest(&dir).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].name, "memo.txt");
        assert_eq!(files[0].register, "plain-text");
        assert_eq!(files[1].name, "sub/post.md");
        assert_eq!(files[1].register, "markdown");

        // Single file works; a single unsupported file is an error.
        assert_eq!(ingest(&dir.join("memo.txt")).unwrap().len(), 1);
        assert!(ingest(&dir.join("notes.log"))
            .unwrap_err()
            .contains("unsupported"));
        assert!(ingest(&dir.join("missing"))
            .unwrap_err()
            .contains("no such file"));

        fs::remove_dir_all(&dir).unwrap();
    }

    // ---- pass-1 statistics ----------------------------------------------

    #[test]
    fn corpus_stats_counts_habits() {
        let files = vec![corpus_file(
            "a.txt",
            "It was—frankly—wrong; truly. We utilize the form. The court heard the \
             brief, the exhibits, and the binder. The court took the motion, the reply \
             and the exhibits.",
        )];
        let stats = corpus_stats(&files);
        assert_eq!(stats.em_dashes, 2);
        assert_eq!(stats.semicolons, 1);
        assert_eq!(stats.oxford_with, 1);
        assert_eq!(stats.oxford_without, 1);
        assert_eq!(stats.term_counts[0], 1); // utilize
        assert_eq!(stats.sentences, 4);
        assert!(stats.sentence_words_max >= 8);
        // "the" opens two sentences.
        assert_eq!(stats.opener_top[0], ("the".to_string(), 2));

        // Clause commas are not serial-comma evidence either way.
        let files = vec![corpus_file(
            "b.txt",
            "The court denied the motion, and the case proceeded to trial. \
             He objected, and the judge overruled it.",
        )];
        let stats = corpus_stats(&files);
        assert_eq!(stats.oxford_with, 0);
        assert_eq!(stats.oxford_without, 0);
    }

    #[test]
    fn statistical_candidates_mirror_absence() {
        let files = vec![corpus_file("corpus.txt", &sample_corpus())];
        let stats = corpus_stats(&files);
        assert!(stats.words >= MIN_CORPUS_WORDS);
        let candidates = statistical_candidates(&files, &stats);
        let ids: Vec<&str> = candidates.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"no-em-dash"), "{ids:?}");
        assert!(ids.contains(&"no-semicolons"), "{ids:?}");
        assert!(ids.contains(&"no-utilize"), "{ids:?}");
        assert!(ids.contains(&"serial-comma-required"), "{ids:?}");
        assert!(ids.contains(&"sentence-length"), "{ids:?}");

        // The utilize rule carries mechanical fixes and corpus provenance.
        let utilize = candidates.iter().find(|c| c.id == "no-utilize").unwrap();
        assert!(utilize.yaml.contains("fix: use"), "{}", utilize.yaml);
        assert!(
            utilize.yaml.contains("mined from your corpus"),
            "{}",
            utilize.yaml
        );

        // A corpus that uses em dashes and "utilize" emits neither rule.
        let files = vec![corpus_file(
            "corpus.txt",
            &format!("{} We utilize em dashes—often.", sample_corpus()),
        )];
        let stats = corpus_stats(&files);
        let ids: Vec<String> = statistical_candidates(&files, &stats)
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert!(!ids.contains(&"no-em-dash".to_string()), "{ids:?}");
        assert!(!ids.contains(&"no-utilize".to_string()), "{ids:?}");

        // A tiny corpus emits nothing.
        let files = vec![corpus_file("tiny.txt", "Four words of text.")];
        let stats = corpus_stats(&files);
        assert!(statistical_candidates(&files, &stats).is_empty());
    }

    #[test]
    fn clause_commas_do_not_fabricate_serial_comma_rules() {
        // Compound sentences only — plenty of ", and" clause commas, zero
        // three-item lists. Neither Oxford rule may be inferred from them.
        let text = "The court denied the motion, and the case proceeded to trial. ".repeat(40);
        let files = vec![corpus_file("clauses.txt", &text)];
        let stats = corpus_stats(&files);
        assert!(stats.words >= MIN_CORPUS_WORDS);
        assert_eq!(stats.oxford_with, 0);
        assert_eq!(stats.oxford_without, 0);
        let ids: Vec<String> = statistical_candidates(&files, &stats)
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert!(
            !ids.contains(&"serial-comma-required".to_string()),
            "{ids:?}"
        );
        assert!(!ids.contains(&"no-serial-comma".to_string()), "{ids:?}");
    }

    #[test]
    fn synthesized_examples_quote_user_text() {
        let files = vec![corpus_file("corpus.txt", &sample_corpus())];
        let stats = corpus_stats(&files);
        let candidates = statistical_candidates(&files, &stats);
        let utilize = candidates.iter().find(|c| c.id == "no-utilize").unwrap();
        // examples.good is a real corpus sentence; examples.bad is the
        // counterfactual with the flagged term injected.
        assert_eq!(
            utilize.def.examples[0].good,
            "We use the standard form for every filing."
        );
        assert_eq!(
            utilize.def.examples[0].bad,
            "We utilize the standard form for every filing."
        );
    }

    // ---- sampling --------------------------------------------------------

    #[test]
    fn chunk_paragraphs_covers_everything() {
        let text = "one\n\ntwo\n\nthree";
        assert_eq!(chunk_paragraphs(text, 8), vec!["one\n\ntwo", "three"]);
        assert_eq!(chunk_paragraphs(text, 3), vec!["one", "two", "three"]);
        // Oversized paragraphs are kept whole, never dropped.
        let long = "x".repeat(50);
        let chunks = chunk_paragraphs(&format!("{long}\n\nshort"), 10);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], long);
    }

    #[test]
    fn stratified_sample_caps_and_spreads() {
        let mut files = Vec::new();
        for index in 0..10 {
            let mut text = String::new();
            for paragraph in 0..20 {
                let _ = write!(text, "File {index} paragraph {paragraph}. ");
                text.push_str(&"Words fill the paragraph here. ".repeat(10));
                text.push_str("\n\n");
            }
            files.push(corpus_file(&format!("f{index}.txt"), &text));
        }
        let sample = stratified_sample(&files, MAX_PASSAGES, MAX_SAMPLE_CHARS);
        assert!(sample.len() <= MAX_PASSAGES);
        let total: usize = sample.iter().map(|p| p.text.len()).sum();
        assert!(total <= MAX_SAMPLE_CHARS);
        // Multiple files are represented.
        let sources: BTreeSet<&str> = sample.iter().map(|p| p.source.as_str()).collect();
        assert!(sources.len() >= 5, "{sources:?}");
    }

    // ---- mined-candidate parsing ----------------------------------------

    #[test]
    fn parse_mined_tolerates_fences_and_prose() {
        let valid = r#"[{"id": "no-moreover", "description": "d",
            "patterns": ["(?i)\\bmoreover\\b"],
            "examples": [{"bad": "Moreover, it rains.", "good": "It rains."}]}]"#;
        for content in [
            valid.to_string(),
            format!("```json\n{valid}\n```"),
            format!("Here are the rules:\n{valid}\nDone."),
        ] {
            let rules = parse_mined(&content).unwrap();
            assert_eq!(rules.len(), 1, "{content}");
            assert_eq!(rules[0].id, "no-moreover");
        }
        assert!(parse_mined("no json here").is_err());
        assert!(parse_mined("[]").unwrap().is_empty());
    }

    #[test]
    fn mined_candidate_sanitizes_and_restricts() {
        let rule = |json_text: &str| serde_json::from_str::<MinedRule>(json_text).unwrap();
        let none = BTreeSet::new();
        let lens = "punctuation & mechanics";

        // Engine restriction: density is rejected.
        let dense = rule(
            r#"{"id": "x", "engine": "density", "description": "d",
                "examples": [{"bad": "b", "good": "g"}]}"#,
        );
        let (_, reason) = mined_candidate(dense, 0, "m", lens, &none).unwrap_err();
        assert!(reason.contains("not allowed"), "{reason}");

        // Severity clamps below error; provenance (model + lens) lands in
        // the description.
        let ok = rule(
            r#"{"id": "No Moreover!", "severity": "error", "description": "Never moreover",
                "patterns": ["(?i)\\bmoreover\\b"],
                "examples": [{"bad": "Moreover, yes.", "good": "Yes."}],
                "mined_from": "passage 3, 0 occurrences"}"#,
        );
        let candidate = mined_candidate(ok, 0, "local:test", lens, &none).unwrap();
        assert_eq!(candidate.id, "no-moreover");
        assert_eq!(candidate.def.severity.as_deref(), Some("warning"));
        assert!(
            candidate
                .yaml
                .contains("mined by local:test, lens: punctuation & mechanics, from passage 3"),
            "{}",
            candidate.yaml
        );

        // Duplicate ids and example-less rules are rejected.
        let mut used = BTreeSet::new();
        used.insert("no-moreover".to_string());
        let dup = rule(
            r#"{"id": "no-moreover", "description": "d",
                "patterns": ["x"], "examples": [{"bad": "b", "good": "g"}]}"#,
        );
        assert!(mined_candidate(dup, 0, "m", lens, &used)
            .unwrap_err()
            .1
            .contains("duplicate"));
        let bare = rule(r#"{"id": "x", "description": "d", "patterns": ["x"]}"#);
        assert!(mined_candidate(bare, 0, "m", lens, &none)
            .unwrap_err()
            .1
            .contains("no examples"));

        // Agent-supplied fixes never survive: a `fix:` would ship as a
        // MachineApplicable edit no gate ever exercised.
        let fixed = rule(
            r#"{"id": "no-utilize-agent", "description": "d",
                "patterns": [{"pattern": "\\butilize[sd]?\\b", "suggestion": "use",
                              "fix": "use"}],
                "examples": [{"bad": "We utilize it.", "good": "We use it."}]}"#,
        );
        let candidate = mined_candidate(fixed, 0, "m", lens, &none).unwrap();
        assert!(!candidate.yaml.contains("fix:"), "{}", candidate.yaml);
        assert!(
            candidate.yaml.contains("suggestion: use"),
            "{}",
            candidate.yaml
        );

        // A broken regex is caught by the loader round-trip.
        let broken = rule(
            r#"{"id": "bad-re", "description": "d", "patterns": ["("],
                "examples": [{"bad": "b", "good": "g"}]}"#,
        );
        assert!(mined_candidate(broken, 0, "m", lens, &none)
            .unwrap_err()
            .1
            .contains("invalid regex"));
    }

    // ---- self-consistency gate ------------------------------------------

    fn phrase_candidate(id: &str, pattern: &str, bad: &str, good: &str) -> Candidate {
        candidate(
            RuleYaml {
                id: id.to_string(),
                engine: "phrase".to_string(),
                severity: "warning".to_string(),
                description: "d".to_string(),
                rationale: None,
                message: None,
                examples: vec![RuleExample {
                    bad: bad.to_string(),
                    good: good.to_string(),
                }],
                patterns: vec![PatternYaml {
                    pattern: pattern.to_string(),
                    message: None,
                    suggestion: None,
                    fix: None,
                }],
                metric: None,
                params: None,
            },
            "pass 2 (mining agent)",
        )
        .unwrap()
    }

    #[test]
    fn gate_drops_self_firing_rules_and_broken_examples() {
        let files = vec![corpus_file("corpus.txt", &sample_corpus())];
        let candidates = vec![
            // Fires on the corpus ("the" is everywhere) → dropped.
            phrase_candidate("no-the", r"(?i)\bthe\b", "The end.", "An end."),
            // Never fires on the corpus, examples correct → kept.
            phrase_candidate(
                "no-zebra",
                r"\bzebra\b",
                "A zebra appears.",
                "A horse appears.",
            ),
            // Never fires, but the bad example does not flag → dropped.
            phrase_candidate(
                "no-quokka",
                r"\bquokka\b",
                "A wombat naps.",
                "A horse naps.",
            ),
        ];
        let report = self_consistency_gate("personal", candidates, &files).unwrap();
        assert_eq!(report.kept.len(), 1);
        assert_eq!(report.kept[0].id, "no-zebra");
        assert_eq!(report.dropped.len(), 2);
        let dropped: BTreeMap<&str, &str> = report
            .dropped
            .iter()
            .map(|(id, _, reason)| (id.as_str(), reason.as_str()))
            .collect();
        assert!(dropped["no-the"].contains("fired on"), "{dropped:?}");
        assert!(dropped["no-quokka"].contains("examples.bad"), "{dropped:?}");
        assert!(report.gate_samples >= 1);
    }

    // ---- end-to-end with the mock ax client ------------------------------

    const MINED_JSON: &str = r#"[
        {"id": "no-moreover", "engine": "leading", "severity": "warning",
         "description": "You never open sentences with Moreover",
         "message": "You never open with \"Moreover\".",
         "patterns": ["Moreover"],
         "examples": [{"bad": "Moreover, the court agreed.", "good": "The court agreed."}],
         "mined_from": "all passages, 0 occurrences"},
        {"id": "no-short", "engine": "phrase", "severity": "warning",
         "description": "Self-firing candidate",
         "patterns": ["(?i)\\bshort\\b"],
         "examples": [{"bad": "A short one.", "good": "A brief one."}]}
    ]"#;

    #[test]
    fn run_learn_end_to_end_gates_and_writes_package() {
        let dir = temp_dir("e2e");
        let corpus = dir.join("corpus");
        fs::create_dir_all(&corpus).unwrap();
        fs::write(corpus.join("memo.txt"), sample_corpus()).unwrap();
        let out = dir.join("out").join("personal");

        let files = ingest(&corpus).unwrap();
        // A single client == `--workers 1`: exactly one lens runs, same
        // one-request shape the mining round-trip has always had.
        let (client, requests) = FakeClient::boxed(vec![choices_response(MINED_JSON)]);
        let mut output = Vec::new();
        let code = run_learn(
            &files,
            "corpus",
            &out,
            vec![client],
            "local:test-model",
            true,
            &mut output,
        )
        .unwrap();
        assert_eq!(code, 0);
        let transcript = String::from_utf8(output).unwrap();

        // The request went through the ax boundary with the stats + passages.
        // A second request follows: "no-short" self-fires, so the repair
        // round sends one follow-up attempt through the same (now
        // exhausted) fake client, which fails — the candidate stays dropped.
        let sent = requests.lock().unwrap();
        assert_eq!(sent.len(), 2);
        let prompt = sent[0]["messages"][0]["content"].as_str().unwrap();
        assert!(prompt.contains("Corpus statistics"));
        assert!(prompt.contains("memo.txt"));
        drop(sent);

        // "no-short" self-fires (the corpus says "short") → dropped, and the
        // repair attempt above fails too (client exhausted) → still dropped;
        // "no-moreover" survives; pass-1 rules survive.
        assert!(
            transcript.contains("kept    personal/no-moreover"),
            "{transcript}"
        );
        assert!(
            transcript.contains("dropped personal/no-short"),
            "{transcript}"
        );
        assert!(
            transcript.contains("kept    personal/no-em-dash"),
            "{transcript}"
        );

        // The written package is a loadable rule package whose kept rules
        // carry provenance; the dropped rule is not on disk.
        let set = RuleSet::load_dir(&out).unwrap();
        let ids: Vec<String> = set.metas().iter().map(|meta| meta.id.0.clone()).collect();
        assert!(ids.contains(&"personal/no-moreover".to_string()), "{ids:?}");
        assert!(!ids.contains(&"personal/no-short".to_string()), "{ids:?}");
        let moreover = fs::read_to_string(out.join("rules/no-moreover.md")).unwrap();
        assert!(moreover.contains("mined by local:test-model"), "{moreover}");
        assert!(moreover.contains("# Generated by `lawlint learn`"));

        // Kept rules actually lint: the flagged phrase fires, corpus-style
        // text does not.
        let result = lint_with("Moreover, the court agreed.", &LintOptions::default(), &set);
        assert!(result
            .diagnostics
            .iter()
            .any(|d| d.rule_id.0 == "personal/no-moreover"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_learn_survives_agent_failure_with_pass1_rules() {
        let dir = temp_dir("agent-fail");
        fs::write(dir.join("memo.txt"), sample_corpus()).unwrap();
        let out = dir.join("personal");

        let files = ingest(&dir).unwrap();
        let (client, _) = FakeClient::boxed(vec![Err(AxError::runtime("connection refused"))]);
        let mut output = Vec::new();
        let code = run_learn(&files, "corpus", &out, vec![client], "m", true, &mut output).unwrap();
        assert_eq!(code, 0);
        let transcript = String::from_utf8(output).unwrap();
        assert!(
            transcript.contains("every mining lens was unavailable"),
            "{transcript}"
        );
        assert!(transcript.contains("connection refused"), "{transcript}");
        // Pass-1 rules still shipped.
        assert!(RuleSet::load_dir(&out).unwrap().metas().len() >= 3);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_learn_retries_once_on_malformed_output() {
        let dir = temp_dir("retry");
        fs::write(dir.join("memo.txt"), sample_corpus()).unwrap();
        let out = dir.join("personal");

        let files = ingest(&dir).unwrap();
        let (client, requests) = FakeClient::boxed(vec![
            choices_response("I could not find any rules, sorry."),
            choices_response("[]"),
        ]);
        let mut output = Vec::new();
        run_learn(&files, "corpus", &out, vec![client], "m", true, &mut output).unwrap();
        assert_eq!(requests.lock().unwrap().len(), 2); // exactly one retry
        let transcript = String::from_utf8(output).unwrap();
        assert!(!transcript.contains("unavailable"), "{transcript}");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_learn_fans_out_across_lenses_concurrently() {
        let dir = temp_dir("concurrent");
        fs::write(dir.join("memo.txt"), sample_corpus()).unwrap();
        let out = dir.join("personal");

        let files = ingest(&dir).unwrap();
        // 4 clients == 4 lenses: every lens must fire exactly once, and one
        // lens erroring must not sink the other three.
        let (c0, r0) = FakeClient::boxed(vec![choices_response(
            r#"[{"id": "no-alpha", "description": "d", "patterns": ["alpha"],
                "examples": [{"bad": "alpha text", "good": "clean text"}]}]"#,
        )]);
        let (c1, r1) = FakeClient::boxed(vec![choices_response(
            r#"[{"id": "no-beta", "description": "d", "patterns": ["beta"],
                "examples": [{"bad": "beta text", "good": "clean text"}]}]"#,
        )]);
        let (c2, _r2) = FakeClient::boxed(vec![Err(AxError::runtime("timeout"))]);
        let (c3, r3) = FakeClient::boxed(vec![choices_response("[]")]);

        let mut output = Vec::new();
        let code = run_learn(
            &files,
            "corpus",
            &out,
            vec![c0, c1, c2, c3],
            "m",
            true,
            &mut output,
        )
        .unwrap();
        assert_eq!(code, 0);
        let transcript = String::from_utf8(output).unwrap();

        // Every lens got exactly one request — no work-stealing, no
        // duplication, no lens skipped because a sibling failed.
        assert_eq!(r0.lock().unwrap().len(), 1);
        assert_eq!(r1.lock().unwrap().len(), 1);
        assert_eq!(r3.lock().unwrap().len(), 1);

        // The failing lens is reported but does not sink the run.
        assert!(transcript.contains("timeout"), "{transcript}");
        assert!(
            !transcript.contains("every mining lens was unavailable"),
            "{transcript}"
        );
        assert!(
            transcript.contains("kept    personal/no-alpha"),
            "{transcript}"
        );
        assert!(
            transcript.contains("kept    personal/no-beta"),
            "{transcript}"
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_learn_merges_duplicate_patterns_across_lenses() {
        let dir = temp_dir("dedupe");
        fs::write(dir.join("memo.txt"), sample_corpus()).unwrap();
        let out = dir.join("personal");

        let files = ingest(&dir).unwrap();
        // Two lenses independently notice the same habit under different
        // ids/wording — same pattern text, so only the first-seen survives.
        let (c0, _) = FakeClient::boxed(vec![choices_response(
            r#"[{"id": "no-gamma-a", "description": "d", "patterns": ["gamma"],
                "examples": [{"bad": "gamma text", "good": "clean text"}]}]"#,
        )]);
        let (c1, _) = FakeClient::boxed(vec![choices_response(
            r#"[{"id": "no-gamma-b", "description": "d", "patterns": ["gamma"],
                "examples": [{"bad": "gamma text", "good": "clean text"}]}]"#,
        )]);

        let mut output = Vec::new();
        let code = run_learn(&files, "corpus", &out, vec![c0, c1], "m", true, &mut output).unwrap();
        assert_eq!(code, 0);
        let transcript = String::from_utf8(output).unwrap();

        assert!(transcript.contains("1 duplicate merged"), "{transcript}");
        assert!(
            transcript.contains("kept    personal/no-gamma-a"),
            "{transcript}"
        );
        assert!(!transcript.contains("no-gamma-b"), "{transcript}");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_learn_repairs_a_self_firing_candidate() {
        let dir = temp_dir("repair");
        fs::write(dir.join("memo.txt"), sample_corpus()).unwrap();
        let out = dir.join("personal");

        let files = ingest(&dir).unwrap();
        // First response self-fires (the corpus says "short"); the repair
        // round gets a second response for the same lens with a narrower
        // pattern that does not.
        let (client, requests) = FakeClient::boxed(vec![
            choices_response(
                r#"[{"id": "no-short", "description": "d", "patterns": ["(?i)\\bshort\\b"],
                    "examples": [{"bad": "A short one.", "good": "A brief one."}]}]"#,
            ),
            choices_response(
                r#"[{"id": "no-short", "description": "d", "patterns": ["\\bshortish\\b"],
                    "examples": [{"bad": "A shortish one.", "good": "A brief one."}]}]"#,
            ),
        ]);
        let mut output = Vec::new();
        let code = run_learn(&files, "corpus", &out, vec![client], "m", true, &mut output).unwrap();
        assert_eq!(code, 0);
        let transcript = String::from_utf8(output).unwrap();

        // One repair round-trip, on top of the original mining call.
        assert_eq!(requests.lock().unwrap().len(), 2);
        assert!(
            transcript.contains("Refining 1 candidate that failed the self-consistency gate"),
            "{transcript}"
        );
        assert!(
            transcript.contains("Refined: 1 rescued, 0 still dropped"),
            "{transcript}"
        );
        assert!(
            transcript.contains("kept    personal/no-short"),
            "{transcript}"
        );
        let rescued = fs::read_to_string(out.join("rules/no-short.md")).unwrap();
        assert!(rescued.contains("refined"), "{rescued}");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_learn_gives_up_after_one_failed_repair() {
        let dir = temp_dir("repair-fail");
        fs::write(dir.join("memo.txt"), sample_corpus()).unwrap();
        let out = dir.join("personal");

        let files = ingest(&dir).unwrap();
        // Both the original and the repaired version self-fire.
        let (client, requests) = FakeClient::boxed(vec![
            choices_response(
                r#"[{"id": "no-short", "description": "d", "patterns": ["(?i)\\bshort\\b"],
                    "examples": [{"bad": "A short one.", "good": "A brief one."}]}]"#,
            ),
            choices_response(
                r#"[{"id": "no-short", "description": "d", "patterns": ["(?i)\\bshort\\b"],
                    "examples": [{"bad": "A short one.", "good": "A brief one."}]}]"#,
            ),
        ]);
        let mut output = Vec::new();
        run_learn(&files, "corpus", &out, vec![client], "m", true, &mut output).unwrap();
        assert_eq!(requests.lock().unwrap().len(), 2); // exactly one repair attempt, no loop
        let transcript = String::from_utf8(output).unwrap();
        assert!(
            transcript.contains("Refined: 0 rescued, 1 still dropped"),
            "{transcript}"
        );
        assert!(
            transcript.contains("dropped personal/no-short"),
            "{transcript}"
        );
        // A re-failed candidate must be listed exactly once, not once for the
        // original gate failure and again after the repair round. The stray
        // second entry would also inflate the "Kept X of Y" total.
        assert_eq!(
            transcript.matches("dropped personal/no-short").count(),
            1,
            "{transcript}"
        );
        assert!(!out.join("rules/no-short.md").exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resolve_concurrency_clamps_to_lens_count() {
        let bare = LintOptions::default();
        assert_eq!(resolve_concurrency(None, &bare), LENSES.len());
        assert_eq!(resolve_concurrency(Some(0), &bare), 1);
        assert_eq!(resolve_concurrency(Some(2), &bare), 2);
        assert_eq!(resolve_concurrency(Some(99), &bare), LENSES.len());

        let config = LintOptions {
            learn: Some(lawlint_core::LearnOptions {
                concurrency: Some(2),
            }),
            ..Default::default()
        };
        assert_eq!(resolve_concurrency(None, &config), 2);
        // The flag still overrides the config.
        assert_eq!(resolve_concurrency(Some(1), &config), 1);
    }

    #[test]
    fn write_package_prunes_stale_generated_rules() {
        let dir = temp_dir("prune");
        let rules = dir.join("rules");
        fs::create_dir_all(&rules).unwrap();
        // A generated rule from an earlier run (not kept this run) and a
        // user-authored rule (no header).
        fs::write(
            rules.join("stale.md"),
            format!("{GENERATED_HEADER}id: stale\n"),
        )
        .unwrap();
        fs::write(rules.join("mine.md"), "id: mine\n").unwrap();

        let kept = vec![phrase_candidate(
            "no-zebra",
            r"\bzebra\b",
            "A zebra appears.",
            "A horse appears.",
        )];
        write_package(&dir, "personal", "corpus", &kept).unwrap();
        assert!(!rules.join("stale.md").exists());
        assert!(rules.join("mine.md").exists());
        assert!(rules.join("no-zebra.md").exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stale_rules_are_pruned_even_when_nothing_survives() {
        let dir = temp_dir("prune-empty");
        let corpus = dir.join("corpus");
        fs::create_dir_all(&corpus).unwrap();
        // Under MIN_CORPUS_WORDS: no pass-1 candidates; the agent mines
        // nothing → kept is empty, yet the stale generated rule must go.
        fs::write(corpus.join("memo.txt"), "A corpus far too small to mine.").unwrap();
        let out = dir.join("personal");
        fs::create_dir_all(out.join("rules")).unwrap();
        fs::write(
            out.join("rules/stale.md"),
            format!("{GENERATED_HEADER}id: stale\n"),
        )
        .unwrap();

        let files = ingest(&corpus).unwrap();
        let (client, _) = FakeClient::boxed(vec![choices_response("[]")]);
        let mut output = Vec::new();
        let code = run_learn(&files, "corpus", &out, vec![client], "m", true, &mut output).unwrap();
        assert_eq!(code, 0);
        let transcript = String::from_utf8(output).unwrap();
        assert!(
            transcript.contains("Removed 1 stale generated rule"),
            "{transcript}"
        );
        assert!(!out.join("rules/stale.md").exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resolve_model_spec_errors_when_unconfigured() {
        use lawlint_core::AiOptions;
        // Nothing configured, no flag: error with init guidance (#50) — the
        // command must never fall back to downloading a local model.
        let bare = LintOptions::default();
        let err = resolve_model_spec(None, &bare).unwrap_err();
        assert!(err.contains("lawlint init"), "{err}");
        assert!(err.contains("none is configured"), "{err}");

        // The flag always wins; the ai preferences fill in otherwise
        // (per-feature override first).
        assert_eq!(
            resolve_model_spec(Some("anthropic:cli"), &bare).unwrap(),
            "anthropic:cli"
        );
        let mut config = LintOptions {
            ai: Some(AiOptions {
                model: Some("foundry:d".into()),
                features: Some(
                    [("learn".to_string(), "anthropic:m".to_string())]
                        .into_iter()
                        .collect(),
                ),
            }),
            ..Default::default()
        };
        assert_eq!(resolve_model_spec(None, &config).unwrap(), "anthropic:m");
        config.ai.as_mut().unwrap().features = None;
        assert_eq!(resolve_model_spec(None, &config).unwrap(), "foundry:d");
    }

    #[test]
    fn package_name_from_out_dir() {
        assert_eq!(
            package_name(Path::new(".lawlint/rules/personal")),
            "personal"
        );
        assert_eq!(package_name(Path::new("My Rules")), "my-rules");
        assert_eq!(package_name(Path::new("core")), "personal");
    }
}
