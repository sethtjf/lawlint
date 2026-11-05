//! Contrast set: AI drafts of the same content as sampled corpus passages.
//!
//! The self-consistency gate proves a rule never fires on the corpus. It
//! cannot prove the rule ever fires on anything else: a pattern that matches
//! nothing passes the gate and adds nothing. Matched pairs fix both halves.
//! Each pair holds a corpus passage and the configured model's own version of
//! it (same facts and conclusions, written fresh), so content is held
//! constant and the difference between the two texts is style.
//!
//! Pairs serve three consumers: the mining prompts (contrastive evidence:
//! "flag what the drafts do and the corpus never does"), the AI-draft side of
//! the profile comparison, and the discrimination check, which drops mined and
//! calibrated rules that fire on none of the drafts.

use std::collections::BTreeMap;

use lawlint_core::{LintOptions, RuleSet};
use lawlint_eval::{evaluate_with, per_rule_metrics, Label, Sample};
use lawlint_judge::AxAIClient;
use serde_json::json;

use super::{
    chunk_paragraphs, fan_out, strip_code_fences, truncate_chars, Candidate, CorpusFile,
    GATE_CHUNK_CHARS,
};
use crate::progress::MultiSpinner;

/// Default number of matched pairs. Each costs one model call and sends one
/// ~2,000-character passage, so six pairs upload roughly 12,000 characters
/// on top of the mining sample.
pub(super) const DEFAULT_CONTRAST_PASSAGES: usize = 6;
/// Passages shorter than this carry too little style to rewrite usefully.
const MIN_SOURCE_WORDS: usize = 80;
/// Hard cap on one source passage. A paragraph longer than the gate chunk
/// size stays whole in the gate, but the contrast prompt truncates it.
const MAX_SOURCE_CHARS: usize = GATE_CHUNK_CHARS + GATE_CHUNK_CHARS / 2;

#[derive(Debug, Clone)]
pub(super) struct ContrastSource {
    pub source: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub(super) struct ContrastPair {
    pub source: String,
    pub original: String,
    pub draft: String,
}

fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

/// `count` corpus passages spread evenly across every gate-sized chunk of
/// the corpus, in corpus order. Deterministic for a given corpus.
pub(super) fn select_sources(files: &[CorpusFile], count: usize) -> Vec<ContrastSource> {
    if count == 0 {
        return Vec::new();
    }
    let chunks: Vec<ContrastSource> = files
        .iter()
        .flat_map(|file| {
            chunk_paragraphs(&file.text, GATE_CHUNK_CHARS)
                .into_iter()
                .filter(|chunk| word_count(chunk) >= MIN_SOURCE_WORDS)
                .map(|chunk| ContrastSource {
                    source: file.name.clone(),
                    text: truncate_chars(&chunk, MAX_SOURCE_CHARS),
                })
                .collect::<Vec<_>>()
        })
        .collect();
    if chunks.len() <= count {
        return chunks;
    }
    (0..count)
        .map(|slot| {
            let index = if count == 1 {
                0
            } else {
                slot * (chunks.len() - 1) / (count - 1)
            };
            chunks[index].clone()
        })
        .collect()
}

pub(super) fn contrast_prompt(source: &ContrastSource) -> String {
    format!(
        "You are drafting legal work product. Below is a passage from an existing \
         document. Write your own version of it from scratch for the same reader and \
         purpose. Keep the same facts, parties, authorities, obligations, and \
         conclusions, at roughly {} words. Write it the way you naturally would. Do \
         not reuse its sentences or follow its sentence-by-sentence structure. \
         Respond with only the passage text: no title, preamble, or commentary.\n\n\
         --- passage ---\n{}\n--- end passage ---",
        word_count(&source.text),
        source.text
    )
}

/// Normalized sentences of `text` for the copy check.
fn sentences(text: &str) -> Vec<String> {
    text.split(['.', '?', '!'])
        .map(|s| {
            s.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        })
        .filter(|s| s.split(' ').count() >= 5)
        .collect()
}

/// Accept a draft only if it is a plausible rewrite: non-trivial length and
/// not a copy. A copied draft would teach the miner that the corpus's own
/// sentences are AI style.
pub(super) fn validate_draft(original: &str, draft: &str) -> Result<String, String> {
    let draft = strip_code_fences(draft).trim().to_string();
    if draft.is_empty() {
        return Err("empty draft".to_string());
    }
    if word_count(&draft) * 5 < word_count(original) * 2 {
        return Err(format!(
            "draft is {} words for a {}-word passage",
            word_count(&draft),
            word_count(original)
        ));
    }
    let original_sentences = sentences(original);
    let draft_sentences = sentences(&draft);
    let copied = draft_sentences
        .iter()
        .filter(|s| original_sentences.contains(s))
        .count();
    if !draft_sentences.is_empty() && copied * 2 > draft_sentences.len() {
        return Err(format!(
            "draft copies {copied} of {} sentences from the passage",
            draft_sentences.len()
        ));
    }
    Ok(draft)
}

fn draft_one(client: &mut dyn AxAIClient, source: &ContrastSource) -> Result<String, String> {
    let request = json!({
        "messages": [{"role": "user", "content": contrast_prompt(source)}],
        // Default sampling on purpose: the point is the model's natural
        // drafting voice, and temperature 0 is not how agents usually draft.
    });
    let response = client.chat(request).map_err(|error| error.to_string())?;
    let content = lawlint_judge::chat_content(&response).ok_or_else(|| {
        format!(
            "no textual content in chat response: {}",
            truncate_chars(&response.to_string(), 200)
        )
    })?;
    validate_draft(&source.text, &content)
}

/// Draft every source on the client pool. Failed drafts are reported, not
/// fatal: fewer pairs weakens the discrimination check but changes nothing
/// else.
pub(super) fn generate(
    clients: &mut [Box<dyn AxAIClient + Send>],
    sources: &[ContrastSource],
    model_id: &str,
    quiet: bool,
) -> (Vec<ContrastPair>, Vec<(String, String)>) {
    let spinner = MultiSpinner::new(
        &format!("Contrast drafts -> {model_id}"),
        sources
            .iter()
            .enumerate()
            .map(|(index, s)| format!("passage {} ({})", index + 1, s.source))
            .collect(),
        quiet,
    );
    let results = fan_out(
        clients,
        sources,
        &spinner,
        &|client, source| draft_one(client, source),
        &|draft: &String| format!("{} words", word_count(draft)),
    );
    spinner.finish();
    let mut pairs = Vec::new();
    let mut failures = Vec::new();
    for (index, (source, result)) in sources.iter().zip(results).enumerate() {
        match result {
            Ok(draft) => pairs.push(ContrastPair {
                source: source.source.clone(),
                original: source.text.clone(),
                draft,
            }),
            Err(error) => failures.push((
                format!("passage {} ({})", index + 1, source.source),
                truncate_chars(&error, 120),
            )),
        }
    }
    (pairs, failures)
}

/// How many contrast drafts each candidate fires on (full rule id → count).
/// Hard rules only; soft rules are measured by the judge in `soft::gate`.
pub(super) fn contrast_hits(
    package: &str,
    candidates: &[Candidate],
    pairs: &[ContrastPair],
) -> Result<BTreeMap<String, usize>, String> {
    if candidates.is_empty() || pairs.is_empty() {
        return Ok(BTreeMap::new());
    }
    let samples: Vec<Sample> = pairs
        .iter()
        .enumerate()
        .map(|(index, pair)| Sample {
            id: format!("contrast#{index}"),
            label: Label::Ai,
            text: pair.draft.clone(),
            word_count: None,
            source: pair.source.clone(),
            register: "contrast".to_string(),
            era: None,
            date: None,
            court: None,
            model: None,
            prompt_style: None,
            pair_id: None,
            split: None,
        })
        .collect();
    let sources: Vec<(String, String)> = candidates
        .iter()
        .map(|c| (c.file_name.clone(), c.yaml.clone()))
        .collect();
    let set = RuleSet::from_sources(package, &sources).map_err(|error| error.to_string())?;
    let evaluated = evaluate_with(&samples, &LintOptions::default(), &set);
    let metrics = per_rule_metrics(
        &evaluated,
        candidates.iter().map(|c| format!("{package}/{}", c.id)),
    );
    Ok(candidates
        .iter()
        .map(|c| {
            let hits = metrics
                .get(&format!("{package}/{}", c.id))
                .map(|m| m.true_positive)
                .unwrap_or(0);
            (c.id.clone(), hits)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, text: &str) -> CorpusFile {
        CorpusFile {
            name: name.to_string(),
            register: "plain-text",
            modified: std::time::SystemTime::UNIX_EPOCH,
            text: text.to_string(),
        }
    }

    fn long_paragraph(tag: usize) -> String {
        (0..12)
            .map(|i| {
                format!("Paragraph {tag} sentence {i} states one concrete fact for the record.")
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn select_sources_spreads_and_skips_short_chunks() {
        let text = (0..10)
            .map(long_paragraph)
            .collect::<Vec<_>>()
            .join("\n\nShort one.\n\n");
        let files = vec![file("a.txt", &text)];
        let picked = select_sources(&files, 3);
        assert_eq!(picked.len(), 3);
        assert!(picked
            .iter()
            .all(|s| word_count(&s.text) >= MIN_SOURCE_WORDS));
        assert!(picked[0].text.contains("Paragraph 0"));
        assert!(picked[2].text.contains("Paragraph 9"));
        assert!(select_sources(&files, 0).is_empty());
    }

    #[test]
    fn validate_draft_rejects_copies_and_stubs() {
        let original = long_paragraph(1);
        assert!(validate_draft(&original, &original)
            .unwrap_err()
            .contains("copies"));
        assert!(validate_draft(&original, "Too short.")
            .unwrap_err()
            .contains("words"));
        assert!(validate_draft(&original, "```\n```").is_err());
        let rewrite = (0..12)
            .map(|i| format!("Moreover, item {i} is comprehensively documented herein."))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            validate_draft(&original, &format!("```\n{rewrite}\n```")).unwrap(),
            rewrite
        );
    }

    #[test]
    fn contrast_prompt_carries_passage_and_length() {
        let source = ContrastSource {
            source: "a.txt".to_string(),
            text: long_paragraph(2),
        };
        let prompt = contrast_prompt(&source);
        assert!(prompt.contains("Paragraph 2 sentence 0"));
        assert!(prompt.contains(&format!("roughly {} words", word_count(&source.text))));
    }
}
