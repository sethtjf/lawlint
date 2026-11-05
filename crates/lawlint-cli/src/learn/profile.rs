//! Style profile: a deterministic, whole-corpus fingerprint of how the
//! source texts are written, plus the hard rules it calibrates.
//!
//! The profile measures the dimensions a reader notices before any single
//! phrase: sentence and paragraph length distributions, punctuation and voice
//! rates, which connectives open sentences, and the per-passage spread of
//! every document-level metric the statistical engine knows. It feeds three
//! consumers: the mining prompts (so the agents argue from numbers, and from
//! the corpus-versus-AI-draft comparison when a contrast set exists),
//! `profile.json` in the output package, and `STYLE.md`.
//!
//! Calibrated rules reuse existing engine metrics only, with thresholds set
//! outside the corpus's own per-passage range plus a margin. They cannot fire
//! on the corpus by construction; the self-consistency gate still checks.

use std::collections::BTreeMap;

use lawlint_core::engines::statistical::{metric_value, Metric};
use lawlint_core::{BlockKind, Sentence, TokenKind};
use regex::Regex;
use serde::Serialize;

use super::{
    candidate, chunk_paragraphs, provenance, Candidate, CorpusFile, CorpusStats, PatternYaml,
    RuleYaml, GATE_CHUNK_CHARS, MIN_CORPUS_WORDS,
};

/// A metric's per-passage range is only a range with at least this many
/// measurable passages; fewer and the calibrated threshold is a guess.
const MIN_CALIBRATION_PASSAGES: usize = 4;
/// "Never opens a sentence with X" needs this many sentences of evidence.
const MIN_TRANSITION_SENTENCES: usize = 30;

/// Sentence-initial connectives the profile counts, multi-word phrases first
/// so the longest match wins. `ai_favored` marks the ones machine drafts
/// lean on; only those become "never opens with" rule patterns when the
/// corpus never uses them, because absence of a plain connective ("But",
/// "So") in a small corpus is weak evidence.
struct Connective {
    label: &'static str,
    ai_favored: bool,
}

const CONNECTIVES: &[Connective] = &[
    Connective {
        label: "it is important to note",
        ai_favored: true,
    },
    Connective {
        label: "it is worth noting",
        ai_favored: true,
    },
    Connective {
        label: "in conclusion",
        ai_favored: true,
    },
    Connective {
        label: "in summary",
        ai_favored: true,
    },
    Connective {
        label: "to summarize",
        ai_favored: true,
    },
    Connective {
        label: "in essence",
        ai_favored: true,
    },
    Connective {
        label: "in addition",
        ai_favored: true,
    },
    Connective {
        label: "as such",
        ai_favored: true,
    },
    Connective {
        label: "that said",
        ai_favored: true,
    },
    Connective {
        label: "for example",
        ai_favored: false,
    },
    Connective {
        label: "in short",
        ai_favored: false,
    },
    Connective {
        label: "even so",
        ai_favored: false,
    },
    Connective {
        label: "moreover",
        ai_favored: true,
    },
    Connective {
        label: "furthermore",
        ai_favored: true,
    },
    Connective {
        label: "additionally",
        ai_favored: true,
    },
    Connective {
        label: "notably",
        ai_favored: true,
    },
    Connective {
        label: "importantly",
        ai_favored: true,
    },
    Connective {
        label: "ultimately",
        ai_favored: true,
    },
    Connective {
        label: "consequently",
        ai_favored: true,
    },
    Connective {
        label: "indeed",
        ai_favored: true,
    },
    Connective {
        label: "overall",
        ai_favored: true,
    },
    Connective {
        label: "crucially",
        ai_favored: true,
    },
    Connective {
        label: "essentially",
        ai_favored: true,
    },
    Connective {
        label: "interestingly",
        ai_favored: true,
    },
    Connective {
        label: "significantly",
        ai_favored: true,
    },
    Connective {
        label: "nevertheless",
        ai_favored: true,
    },
    Connective {
        label: "nonetheless",
        ai_favored: true,
    },
    Connective {
        label: "however",
        ai_favored: false,
    },
    Connective {
        label: "therefore",
        ai_favored: false,
    },
    Connective {
        label: "thus",
        ai_favored: false,
    },
    Connective {
        label: "accordingly",
        ai_favored: false,
    },
    Connective {
        label: "because",
        ai_favored: false,
    },
    Connective {
        label: "instead",
        ai_favored: false,
    },
    Connective {
        label: "first",
        ai_favored: false,
    },
    Connective {
        label: "second",
        ai_favored: false,
    },
    Connective {
        label: "finally",
        ai_favored: false,
    },
    Connective {
        label: "but",
        ai_favored: false,
    },
    Connective {
        label: "and",
        ai_favored: false,
    },
    Connective {
        label: "so",
        ai_favored: false,
    },
    Connective {
        label: "yet",
        ai_favored: false,
    },
    Connective {
        label: "still",
        ai_favored: false,
    },
    Connective {
        label: "then",
        ai_favored: false,
    },
];

/// Per-1,000-word rate detectors: (profile key, regex). Passive voice and
/// nominalization are surface heuristics, labeled approximate in output.
const RATES: &[(&str, &str)] = &[
    ("emDash", "—"),
    ("enDash", "–"),
    ("semicolon", ";"),
    ("colon", r":\s"),
    ("parenthesis", r"\("),
    ("question", r"\?"),
    ("exclamation", "!"),
    ("firstPersonSingular", r"\b(?:I|me|my|mine)\b"),
    ("firstPersonPlural", r"(?i)\b(?:we|us|our|ours)\b"),
    ("secondPerson", r"(?i)\b(?:you|your|yours)\b"),
    ("contraction", r"(?i)\b\w+(?:n['’]t|['’](?:re|ve|ll|m|d))\b"),
    (
        "passiveApprox",
        r"(?i)\b(?:am|is|are|was|were|be|been|being)\s+(?:\w+ly\s+)?\w+(?:ed|en)\b",
    ),
    (
        "nominalizationApprox",
        r"(?i)\b\w{3,}(?:tion|tions|ment|ments|ance|ances|ence|ences|ancy|ency)\b",
    ),
];

/// Document-level metrics measured per passage, with the profile key.
const PASSAGE_METRICS: &[(&str, Metric)] = &[
    ("sentenceLengthVariance", Metric::SentenceLengthVariance),
    ("cadenceAutocorrelation", Metric::CadenceAutocorrelation),
    ("repeatedOpenerDensity", Metric::RepeatedOpenerDensity),
    ("triadDensity", Metric::TriadDensity),
    ("pairedConjunctionRate", Metric::PairedAdjectiveRate),
];

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Distribution {
    pub n: usize,
    pub mean: f64,
    pub min: f64,
    pub p10: f64,
    pub median: f64,
    pub p90: f64,
    pub max: f64,
}

impl Distribution {
    fn of(values: &[f64]) -> Distribution {
        if values.is_empty() {
            return Distribution::default();
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let at = |p: f64| sorted[((sorted.len() - 1) as f64 * p).round() as usize];
        Distribution {
            n: sorted.len(),
            mean: round2(sorted.iter().sum::<f64>() / sorted.len() as f64),
            min: round2(sorted[0]),
            p10: round2(at(0.1)),
            median: round2(at(0.5)),
            p90: round2(at(0.9)),
            max: round2(sorted[sorted.len() - 1]),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct TransitionUse {
    pub connective: &'static str,
    pub count: usize,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct StyleProfile {
    pub texts: usize,
    pub words: usize,
    pub sentences: usize,
    pub paragraphs: usize,
    /// Words per sentence (citation sentences excluded).
    pub sentence_words: Distribution,
    /// Sentences per paragraph (prose blocks only).
    pub paragraph_sentences: Distribution,
    /// Occurrences per 1,000 words, keyed as in `RATES`.
    pub rates: BTreeMap<&'static str, f64>,
    /// Sentence-initial connectives in use, most frequent first.
    pub transitions: Vec<TransitionUse>,
    /// AI-favored connectives that never open a sentence here.
    pub never_opens_with: Vec<&'static str>,
    /// Per-passage spread of each document-level metric (passages are the
    /// same ~2,000-character chunks the self-consistency gate lints).
    pub passages: BTreeMap<&'static str, Distribution>,
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn connective_regexes() -> Vec<Regex> {
    CONNECTIVES
        .iter()
        .map(|c| Regex::new(&format!(r"(?i)^{}\b", regex::escape(c.label))).expect("static"))
        .collect()
}

/// The sentence text with leading quotes/brackets stripped, for opener tests.
fn opening<'a>(sentence: &Sentence, source: &'a str) -> &'a str {
    sentence
        .range
        .slice(source)
        .trim_start_matches(|c: char| c.is_whitespace() || "\"'“‘([".contains(c))
}

fn sentence_word_count(sentence: &Sentence) -> usize {
    sentence
        .tokens
        .iter()
        .filter(|t| matches!(t.kind, TokenKind::Word | TokenKind::Number))
        .count()
}

/// Measure a set of texts as one body of writing. `markdown` follows each
/// text's register, except passage metrics, which parse plain like the gate.
pub(super) fn measure(texts: &[(&str, bool)]) -> StyleProfile {
    let connectives = connective_regexes();
    let rates: Vec<(&'static str, Regex)> = RATES
        .iter()
        .map(|(key, pattern)| (*key, Regex::new(pattern).expect("static rate regex")))
        .collect();

    let mut profile = StyleProfile {
        texts: texts.len(),
        ..StyleProfile::default()
    };
    let mut sentence_lengths = Vec::new();
    let mut paragraph_lengths = Vec::new();
    let mut transition_counts = vec![0usize; CONNECTIVES.len()];
    let mut rate_counts = vec![0usize; rates.len()];
    let mut passage_values: Vec<Vec<f64>> = vec![Vec::new(); PASSAGE_METRICS.len()];

    for (text, markdown) in texts {
        let document = lawlint_core::parse(text, *markdown);
        for block in &document.blocks {
            if block.kind == BlockKind::CodeBlock {
                continue;
            }
            let prose: Vec<&Sentence> = block.sentences.iter().filter(|s| !s.is_citation).collect();
            if matches!(block.kind, BlockKind::Paragraph | BlockKind::BlockQuote)
                && !prose.is_empty()
            {
                paragraph_lengths.push(prose.len() as f64);
                profile.paragraphs += 1;
            }
            for sentence in &block.sentences {
                // Openers count over every sentence (citations included):
                // the transition rule's self-fire evidence must cover
                // everything the leading engine will see.
                let opener = opening(sentence, text);
                if let Some(index) = connectives.iter().position(|re| re.is_match(opener)) {
                    transition_counts[index] += 1;
                }
                if sentence.is_citation {
                    continue;
                }
                let words = sentence_word_count(sentence);
                if words == 0 {
                    continue;
                }
                profile.sentences += 1;
                profile.words += words;
                sentence_lengths.push(words as f64);
                for (index, (_, re)) in rates.iter().enumerate() {
                    rate_counts[index] += re.find_iter(sentence.range.slice(text)).count();
                }
            }
        }
        for chunk in chunk_paragraphs(text, GATE_CHUNK_CHARS) {
            let document = lawlint_core::parse(&chunk, false);
            for (index, (_, metric)) in PASSAGE_METRICS.iter().enumerate() {
                if let Some(value) = metric_value(*metric, &chunk, &document) {
                    passage_values[index].push(value);
                }
            }
        }
    }

    profile.sentence_words = Distribution::of(&sentence_lengths);
    profile.paragraph_sentences = Distribution::of(&paragraph_lengths);
    let per_thousand = |count: usize| {
        if profile.words == 0 {
            0.0
        } else {
            round2(count as f64 * 1000.0 / profile.words as f64)
        }
    };
    profile.rates = rates
        .iter()
        .zip(&rate_counts)
        .map(|((key, _), count)| (*key, per_thousand(*count)))
        .collect();
    let mut used: Vec<TransitionUse> = CONNECTIVES
        .iter()
        .zip(&transition_counts)
        .filter(|(_, count)| **count > 0)
        .map(|(c, count)| TransitionUse {
            connective: c.label,
            count: *count,
        })
        .collect();
    used.sort_by(|a, b| b.count.cmp(&a.count).then(a.connective.cmp(b.connective)));
    profile.transitions = used;
    profile.never_opens_with = CONNECTIVES
        .iter()
        .zip(&transition_counts)
        .filter(|(c, count)| c.ai_favored && **count == 0)
        .map(|(c, _)| c.label)
        .collect();
    profile.passages = PASSAGE_METRICS
        .iter()
        .zip(&passage_values)
        .map(|((key, _), values)| (*key, Distribution::of(values)))
        .collect();
    profile
}

/// Profile of a corpus on disk.
pub(super) fn measure_files(files: &[CorpusFile]) -> StyleProfile {
    let texts: Vec<(&str, bool)> = files
        .iter()
        .map(|f| (f.text.as_str(), f.register == "markdown"))
        .collect();
    measure(&texts)
}

/// Readable labels for profile rate keys.
pub(super) fn rate_label(key: &str) -> &'static str {
    match key {
        "emDash" => "em dashes",
        "enDash" => "en dashes",
        "semicolon" => "semicolons",
        "colon" => "colons",
        "parenthesis" => "parentheticals",
        "question" => "question marks",
        "exclamation" => "exclamation points",
        "firstPersonSingular" => "first person singular",
        "firstPersonPlural" => "first person plural",
        "secondPerson" => "second person",
        "contraction" => "contractions",
        "passiveApprox" => "passive constructions (approx.)",
        "nominalizationApprox" => "nominalizations (approx.)",
        _ => "other",
    }
}

pub(super) fn passage_label(key: &str) -> &'static str {
    match key {
        "sentenceLengthVariance" => "sentence-length variance",
        "cadenceAutocorrelation" => "cadence autocorrelation",
        "repeatedOpenerDensity" => "repeated-opener rate",
        "triadDensity" => "triads per 1,000 words",
        "pairedConjunctionRate" => "\"x and y\" pairs per 1,000 words",
        _ => "other",
    }
}

/// Prompt-ready profile lines. With a contrast profile, each measure reads
/// "corpus X vs AI drafts Y" so the mining agents see where machine drafts
/// of the same content diverge.
pub(super) fn profile_block(profile: &StyleProfile, contrast: Option<&StyleProfile>) -> String {
    let mut lines = Vec::new();
    let versus = |ours: f64, theirs: Option<f64>| match theirs {
        Some(theirs) => format!("{ours} (AI drafts: {theirs})"),
        None => format!("{ours}"),
    };
    lines.push(format!(
        "- words per sentence: median {}, middle 80% {}-{}, mean {}{}",
        profile.sentence_words.median,
        profile.sentence_words.p10,
        profile.sentence_words.p90,
        profile.sentence_words.mean,
        contrast
            .map(|c| format!(
                " (AI drafts: median {}, mean {})",
                c.sentence_words.median, c.sentence_words.mean
            ))
            .unwrap_or_default()
    ));
    lines.push(format!(
        "- sentences per paragraph: mean {}",
        versus(
            profile.paragraph_sentences.mean,
            contrast.map(|c| c.paragraph_sentences.mean)
        )
    ));
    for (key, value) in &profile.rates {
        lines.push(format!(
            "- {} per 1,000 words: {}",
            rate_label(key),
            versus(*value, contrast.and_then(|c| c.rates.get(key).copied()))
        ));
    }
    for (key, dist) in &profile.passages {
        if dist.n == 0 {
            continue;
        }
        lines.push(format!(
            "- {} per passage: median {}, range {} to {}{}",
            passage_label(key),
            dist.median,
            dist.min,
            dist.max,
            contrast
                .and_then(|c| c.passages.get(key))
                .filter(|d| d.n > 0)
                .map(|d| format!(" (AI drafts: median {})", d.median))
                .unwrap_or_default()
        ));
    }
    let used = profile
        .transitions
        .iter()
        .take(8)
        .map(|t| format!("\"{}\" ({})", t.connective, t.count))
        .collect::<Vec<_>>()
        .join(", ");
    lines.push(format!(
        "- sentence-initial connectives used: {}",
        if used.is_empty() { "none" } else { &used }
    ));
    if let Some(contrast) = contrast {
        let theirs = contrast
            .transitions
            .iter()
            .take(8)
            .map(|t| format!("\"{}\" ({})", t.connective, t.count))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!(
            "- sentence-initial connectives in AI drafts: {}",
            if theirs.is_empty() { "none" } else { &theirs }
        ));
    }
    let mut block = lines.join("\n");
    block.push('\n');
    block
}

// ---- calibrated candidates ---------------------------------------------

/// How a passage metric becomes a rule: which side flags and how far past
/// the corpus's own extreme the threshold sits.
struct Calibration {
    key: &'static str,
    metric: &'static str,
    id: &'static str,
    /// Threshold from the corpus distribution; `None` = the corpus range is
    /// too wide (or too narrow) for a useful boundary.
    threshold: fn(&Distribution) -> Option<f64>,
    direction: &'static str,
    describe: &'static str,
    message: &'static str,
}

const CALIBRATIONS: &[Calibration] = &[
    Calibration {
        key: "sentenceLengthVariance",
        metric: "sentence-length-variance",
        id: "rhythm-floor",
        // Below 80% of the flattest corpus passage. Corpora of uniformly
        // short sentences have a low floor that would never bite; skip.
        threshold: |d| {
            let t = (d.min * 0.8).floor();
            (t >= 20.0).then_some(t)
        },
        direction: "below",
        describe: "Sentence lengths vary less than in any passage of the corpus",
        message:
            "Sentence lengths are flatter than anything in the source corpus; vary the rhythm.",
    },
    Calibration {
        key: "cadenceAutocorrelation",
        metric: "cadence-autocorrelation",
        id: "cadence-ceiling",
        threshold: |d| {
            let t = round2(d.max + 0.15);
            (t < 0.9).then_some(t)
        },
        direction: "above",
        describe: "Sentence lengths move in a more regular cadence than any passage of the corpus",
        message: "Sentence cadence is more regular than anything in the source corpus.",
    },
    Calibration {
        key: "repeatedOpenerDensity",
        metric: "repeated-opener-density",
        id: "opener-repetition-ceiling",
        threshold: |d| {
            let t = round2(d.max + 0.1);
            (t < 0.6).then_some(t)
        },
        direction: "above",
        describe: "Adjacent sentences share an opener more often than in any passage of the corpus",
        message: "Adjacent sentences repeat openers more than the source corpus ever does.",
    },
    Calibration {
        key: "triadDensity",
        metric: "triad-density",
        id: "triad-ceiling",
        threshold: |d| Some(((d.max * 1.25 + 1.0) * 10.0).round() / 10.0),
        direction: "above",
        describe: "Three-part \"A, B, and C\" lists are denser than in any passage of the corpus",
        message: "Three-part lists are denser than anything in the source corpus.",
    },
    Calibration {
        key: "pairedConjunctionRate",
        metric: "paired-adjective-rate",
        id: "paired-phrase-ceiling",
        threshold: |d| Some(((d.max * 1.25 + 2.0) * 10.0).round() / 10.0),
        direction: "above",
        describe: "Balanced \"x and y\" pairs are denser than in any passage of the corpus",
        message: "Balanced \"x and y\" pairs are denser than anything in the source corpus.",
    },
];

/// Origin label for calibrated rules (gate + transcript).
pub(super) const CALIBRATED_ORIGIN: &str = "profile (calibrated)";

/// Document-level rules whose thresholds sit just outside the corpus's own
/// per-passage range. Intent `style`: they measure distance from this
/// corpus, not corpus-validated AI detection.
pub(super) fn calibrated_candidates(profile: &StyleProfile, stats: &CorpusStats) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    if profile.words < MIN_CORPUS_WORDS {
        return candidates;
    }
    for calibration in CALIBRATIONS {
        let Some(dist) = profile.passages.get(calibration.key) else {
            continue;
        };
        if dist.n < MIN_CALIBRATION_PASSAGES {
            continue;
        }
        let Some(threshold) = (calibration.threshold)(dist) else {
            continue;
        };
        let note = format!(
            "{} (corpus range {} to {} across {} passages; flags {} {threshold})",
            calibration.describe, dist.min, dist.max, dist.n, calibration.direction
        );
        let rule = RuleYaml {
            id: calibration.id.to_string(),
            engine: "statistical".to_string(),
            severity: "suggestion".to_string(),
            intent: Some("style".to_string()),
            description: provenance(stats, &note),
            rationale: None,
            message: Some(calibration.message.to_string()),
            examples: Vec::new(),
            patterns: Vec::new(),
            metric: Some(calibration.metric.to_string()),
            params: None,
            threshold: Some(threshold),
            direction: Some(calibration.direction.to_string()),
        };
        match candidate(rule, CALIBRATED_ORIGIN) {
            Ok(c) => candidates.push(c),
            Err(error) => debug_assert!(false, "calibrated candidate failed to build: {error}"),
        }
    }
    candidates
}

/// Capitalize the first letter of `connective` for display and examples.
fn capitalized(connective: &str) -> String {
    let mut chars = connective.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Lowercase a sentence's first letter so a connective can precede it,
/// unless the first word looks like a proper noun or acronym ("I", "FDA",
/// "Smith"). Proper nouns are not detectable here; a lowercased name only
/// affects the readability of a synthetic example, never correctness.
fn decapitalize(sentence: &str) -> String {
    let first_word = sentence.split_whitespace().next().unwrap_or("");
    let acronym = first_word.len() > 1
        && first_word
            .chars()
            .filter(|c| c.is_alphabetic())
            .all(char::is_uppercase);
    if first_word == "I" || acronym {
        return sentence.to_string();
    }
    let mut chars = sentence.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// "Never opens a sentence with X" for AI-favored connectives the corpus
/// never uses, as one leading rule. Its example pair quotes a real corpus
/// sentence (good) and the same sentence with a banned opener (bad).
pub(super) fn transition_candidate(
    files: &[CorpusFile],
    profile: &StyleProfile,
    stats: &CorpusStats,
) -> Option<Candidate> {
    if profile.words < MIN_CORPUS_WORDS
        || profile.sentences < MIN_TRANSITION_SENTENCES
        || profile.never_opens_with.is_empty()
    {
        return None;
    }
    let favorites = profile
        .transitions
        .iter()
        .take(3)
        .map(|t| capitalized(t.connective))
        .collect::<Vec<_>>();
    let suggestion = if favorites.is_empty() {
        "Open with the point itself.".to_string()
    } else {
        format!(
            "Open with the point itself, or with a connective the corpus uses ({}).",
            favorites.join(", ")
        )
    };
    let patterns: Vec<PatternYaml> = profile
        .never_opens_with
        .iter()
        .map(|label| PatternYaml {
            pattern: format!(r"{}\b", regex::escape(label)),
            message: Some(format!(
                "The source corpus never opens a sentence with \"{}\".",
                capitalized(label)
            )),
            suggestion: Some(suggestion.clone()),
            fix: None,
        })
        .collect();

    let opener = capitalized(profile.never_opens_with[0]);
    let good = files
        .iter()
        .flat_map(|file| {
            let document = lawlint_core::parse(&file.text, file.register == "markdown");
            document
                .blocks
                .iter()
                .flat_map(|b| b.sentences.iter())
                .filter(|s| !s.is_citation)
                .map(|s| s.range.slice(&file.text).trim().to_string())
                .collect::<Vec<_>>()
        })
        .find(|text| (20..=160).contains(&text.len()) && !text.contains('\n'))
        .unwrap_or_else(|| "The court denied the motion.".to_string());
    let bad = format!("{opener}, {}", decapitalize(&good));
    let listed = profile
        .never_opens_with
        .iter()
        .take(6)
        .map(|label| capitalized(label))
        .collect::<Vec<_>>()
        .join(", ");
    let more = profile.never_opens_with.len().saturating_sub(6);
    let note = if more > 0 {
        format!("The corpus never opens a sentence with {listed}, or {more} similar connectives")
    } else {
        format!("The corpus never opens a sentence with {listed}")
    };
    candidate(
        RuleYaml {
            id: "transitions-never-used".to_string(),
            engine: "leading".to_string(),
            severity: "warning".to_string(),
            intent: None,
            description: provenance(stats, &note),
            rationale: Some(
                "Machine drafts announce logical moves with stock connectives; this corpus \
                 lets the content carry the connection."
                    .to_string(),
            ),
            message: None,
            examples: vec![lawlint_core::RuleExample { bad, good }],
            patterns,
            metric: None,
            params: None,
            threshold: None,
            direction: None,
        },
        "pass 1 (statistics)",
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lawlint_core::RuleSet;

    fn varied_corpus() -> String {
        let mut text = String::new();
        for index in 0..30 {
            text.push_str(&format!(
                "The court denied motion {index}. But the record, read as a whole and with \
                 the deposition transcripts, the expert reports, and the emails, shows a \
                 genuine dispute about who knew what and when. So the case goes to trial. \
                 We filed the reply on time, and the clerk docketed it the same day.\n\n"
            ));
        }
        text
    }

    #[test]
    fn measure_counts_sentences_rates_and_transitions() {
        let text = varied_corpus();
        let profile = measure(&[(&text, false)]);
        assert_eq!(profile.texts, 1);
        assert_eq!(profile.sentences, 120);
        assert_eq!(profile.paragraphs, 30);
        assert!(profile.sentence_words.max > profile.sentence_words.min);
        assert_eq!(profile.rates["emDash"], 0.0);
        assert!(profile.rates["firstPersonPlural"] > 0.0);
        let used: Vec<&str> = profile.transitions.iter().map(|t| t.connective).collect();
        assert!(used.contains(&"but") && used.contains(&"so"), "{used:?}");
        assert!(profile.never_opens_with.contains(&"moreover"));
        assert!(
            !profile.never_opens_with.contains(&"but"),
            "plain connectives never listed"
        );
        let variance = profile.passages["sentenceLengthVariance"];
        assert!(variance.n >= MIN_CALIBRATION_PASSAGES, "{variance:?}");
    }

    #[test]
    fn distribution_percentiles_are_ordered() {
        let d = Distribution::of(&[5.0, 1.0, 3.0, 2.0, 4.0]);
        assert_eq!(
            (d.n, d.min, d.median, d.max, d.mean),
            (5, 1.0, 3.0, 5.0, 3.0)
        );
        assert!(d.p10 <= d.median && d.median <= d.p90);
        assert_eq!(Distribution::of(&[]).n, 0);
    }

    #[test]
    fn profile_block_compares_against_contrast() {
        let text = varied_corpus();
        let ours = measure(&[(&text, false)]);
        let theirs = measure(&[(
            "Moreover, the analysis is robust. Furthermore, it is comprehensive. \
             Additionally, it is thorough.",
            false,
        )]);
        let block = profile_block(&ours, Some(&theirs));
        assert!(block.contains("AI drafts"), "{block}");
        assert!(block.contains("\"moreover\" (1)"), "{block}");
        assert!(!profile_block(&ours, None).contains("AI drafts"));
    }

    #[test]
    fn calibrated_and_transition_rules_never_fire_on_their_corpus() {
        let files = vec![CorpusFile {
            name: "brief.txt".into(),
            register: "plain-text",
            modified: std::time::SystemTime::UNIX_EPOCH,
            text: varied_corpus(),
        }];
        let stats = super::super::corpus_stats(&files);
        let profile = measure_files(&files);
        let calibrated = calibrated_candidates(&profile, &stats);
        let ids: Vec<&str> = calibrated.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"triad-ceiling"), "{ids:?}");
        assert!(calibrated
            .iter()
            .all(|c| c.def.intent.as_deref() == Some("style")));
        let transition = transition_candidate(&files, &profile, &stats).expect("transition rule");
        assert!(transition.def.examples[0]
            .bad
            .starts_with("It is important to note, "));

        let mut all = calibrated;
        all.push(transition);
        let report = super::super::self_consistency_gate("learned", all, &files).unwrap();
        assert!(report.dropped.is_empty(), "{:?}", report.dropped);

        // And they do fire on machine-shaped text.
        let set = RuleSet::from_sources(
            "learned",
            &report
                .kept
                .iter()
                .map(|c| (c.file_name.clone(), c.yaml.clone()))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let machine = (0..12)
            .map(|i| format!("Moreover, item {i} is clear, concise, and compelling."))
            .collect::<Vec<_>>()
            .join(" ");
        let fired: Vec<String> =
            lawlint_core::lint_with(&machine, &lawlint_core::LintOptions::default(), &set)
                .diagnostics
                .into_iter()
                .map(|d| d.rule_id.0)
                .collect();
        assert!(
            fired.contains(&"learned/transitions-never-used".to_string()),
            "{fired:?}"
        );
        assert!(
            fired.contains(&"learned/triad-ceiling".to_string()),
            "{fired:?}"
        );
    }

    #[test]
    fn decapitalize_keeps_acronyms_and_i() {
        assert_eq!(decapitalize("The court ruled."), "the court ruled.");
        assert_eq!(decapitalize("FDA rules apply."), "FDA rules apply.");
        assert_eq!(decapitalize("I agree."), "I agree.");
    }
}
