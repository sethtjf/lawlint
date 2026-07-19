#[cfg(feature = "sourcing")]
mod app {
    use lawlint_core::{lint, LintOptions, RuleSet};
    use lawlint_eval::{foundry::FoundryClient, sourcing::normalize_whitespace};
    use lawlint_eval::{load_jsonl, Label, Sample};
    use serde_json::to_string;
    use std::collections::BTreeMap;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::thread;
    use std::time::Duration;

    const CORPUS: &str = "crates/lawlint-eval/corpus/corpus.jsonl";
    const MODEL_NAMES: [&str; 3] = ["gpt-5.5", "claude-opus-4-8", "FW-GLM-5.1"];
    const STYLES: [&str; 3] = ["naive", "rule-evading", "self-edit"];

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let client = FoundryClient::from_env()?;
        let samples = load_jsonl(CORPUS)?;
        let human_samples: Vec<Sample> = samples
            .iter()
            .filter(|sample| sample.label == Label::Human)
            .cloned()
            .collect();
        let existing_ai = samples.iter().filter(|sample| sample.label == Label::Ai);
        let existing_pairs = existing_ai
            .clone()
            .filter_map(|sample| sample.pair_id.as_deref())
            .collect::<std::collections::BTreeSet<_>>();
        let mut output = OpenOptions::new().append(true).open(CORPUS)?;
        let mut counts = existing_ai
            .filter_map(|sample| sample.prompt_style.as_deref())
            .map(|style| (style.replace('-', "_"), 1_usize))
            .fold(
                BTreeMap::<String, usize>::new(),
                |mut counts, (style, _)| {
                    *counts.entry(style).or_default() += 1;
                    counts
                },
            );
        for (index, human) in human_samples.iter().enumerate() {
            let pair_id = human
                .pair_id
                .as_deref()
                .ok_or_else(|| format!("human sample {} has no pair_id", human.id))?;
            if existing_pairs.contains(pair_id) {
                eprintln!("skip {}: pair already has an AI sample", human.id);
                continue;
            }
            let style_index = index % STYLES.len();
            let style = STYLES[style_index];
            let mut model = MODEL_NAMES[(index / STYLES.len() + style_index) % MODEL_NAMES.len()];
            let target_words = human
                .word_count
                .unwrap_or_else(|| human.text.split_whitespace().count());
            let topic = topic_descriptor(human);
            let avoidance = avoidance_instructions();
            let base_prompt = prompt(style, &human.register, &topic, target_words, &avoidance);
            let mut text =
                match generate_with_validation(&client, model, style, &base_prompt, target_words) {
                    Ok(text) => text,
                    Err(error) => {
                        let fallback = if model == "claude-opus-4-8" {
                            "gpt-5.5"
                        } else {
                            "claude-opus-4-8"
                        };
                        eprintln!(
                            "{}: {model} failed ({error}); falling back to {fallback}",
                            human.id
                        );
                        model = fallback;
                        match generate_with_validation(
                            &client,
                            model,
                            style,
                            &base_prompt,
                            target_words,
                        ) {
                            Ok(text) => text,
                            Err(error) => {
                                eprintln!("skip {}: {error}", human.id);
                                continue;
                            }
                        }
                    }
                };
            if style == "self-edit" {
                let original = text.clone();
                text = match self_edit(&client, model, text, target_words) {
                    Ok(text) if (100..=500).contains(&text.split_whitespace().count()) => text,
                    Ok(_) | Err(_) => original,
                };
            }
            let id_style = style.replace('-', "_");
            let count = counts.entry(id_style.clone()).or_default();
            *count += 1;
            let actual_words = text.split_whitespace().count();
            let ai = Sample {
                id: format!("ai-{id_style}-{count:06}"),
                label: Label::Ai,
                text,
                word_count: Some(actual_words),
                source: "foundry".to_string(),
                register: human.register.clone(),
                era: human.era.clone(),
                date: human.date.clone(),
                court: human.court.clone(),
                model: Some(model.to_string()),
                prompt_style: Some(style.to_string()),
                pair_id: Some(pair_id.to_string()),
                split: None,
            };
            writeln!(output, "{}", to_string(&ai)?)?;
            eprintln!("generated {} ({style}, {model})", human.id);
            thread::sleep(Duration::from_millis(250));
        }
        Ok(())
    }

    fn topic_descriptor(sample: &Sample) -> String {
        let opening = sample
            .text
            .split_once('.')
            .map_or(sample.text.as_str(), |(opening, _)| opening);
        opening
            .split_whitespace()
            .take(24)
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn avoidance_instructions() -> String {
        RuleSet::built_in()
            .metas()
            .iter()
            .map(|meta| {
                let examples = meta
                    .examples
                    .iter()
                    .take(1)
                    .map(|example| format!("bad `{}`; good `{}`", example.bad, example.good))
                    .collect::<Vec<_>>()
                    .join("; ");
                format!("- {}: {} {}", meta.id.0, meta.description, examples)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn prompt(
        style: &str,
        register: &str,
        topic: &str,
        target_words: usize,
        avoidance: &str,
    ) -> String {
        let mut prompt = format!(
        "Write a {register} passage about this subject: {topic}. Produce approximately {target_words} words. \
         Return only the passage, with no markdown, preamble, title, commentary, or explanation. \
         Make it read like a substantive legal document, not a generic essay."
    );
        if style != "naive" {
            prompt.push_str(&format!(
            "\nAvoid the following lawlint tells while preserving clear legal prose:\n{avoidance}"
        ));
        }
        prompt
    }

    fn generate_with_validation(
        client: &FoundryClient,
        model: &str,
        style: &str,
        prompt: &str,
        target_words: usize,
    ) -> Result<String, String> {
        let system = "You are a legal-text generation model. Follow the requested register and return only clean prose.";
        let mut last = String::new();
        for attempt in 0..3 {
            let request = if attempt == 0 {
                prompt.to_string()
            } else {
                format!(
                "{prompt}\nYour previous output was {} words. Rewrite it to approximately {target_words} words, keeping it between 100 and 500 words.",
                last.split_whitespace().count()
            )
            };
            let output = fit_output(
                &clean_model_text(&client.complete(model, system, &request, 2000)?),
                target_words,
            );
            let words = output.split_whitespace().count();
            if (100..=500).contains(&words) && words.abs_diff(target_words) <= target_words / 2 + 20
            {
                return Ok(output);
            }
            last = output;
            thread::sleep(Duration::from_millis(250));
        }
        Err(format!(
            "{style} output stayed outside target range after retries ({} words)",
            last.split_whitespace().count()
        ))
    }

    fn fit_output(text: &str, target_words: usize) -> String {
        let words = text.split_whitespace().collect::<Vec<_>>();
        if words.len() <= target_words {
            return text.to_string();
        }
        let mut end = target_words.min(500);
        while end >= 100 && !words[end - 1].ends_with(['.', '!', '?']) {
            end -= 1;
        }
        if end >= 100 {
            words[..end].join(" ")
        } else {
            words[..target_words.min(500)].join(" ")
        }
    }

    fn self_edit(
        client: &FoundryClient,
        model: &str,
        mut text: String,
        target_words: usize,
    ) -> Result<String, String> {
        let options = LintOptions::default();
        for _ in 0..3 {
            let result = lint(&text, &options);
            if result.diagnostics.is_empty() || result.stats.score >= 100 {
                return Ok(text);
            }
            let findings = result
                .diagnostics
                .iter()
                .map(|diagnostic| {
                    format!(
                        "- {}: {} (excerpt: {:?})",
                        diagnostic.rule_id.0, diagnostic.message, diagnostic.excerpt
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let prompt = format!(
            "Revise the passage below to eliminate these lawlint findings while preserving its \
             legal content, register, and approximately {target_words} words. Return only the revised passage.\n\
             Findings:\n{findings}\n\nPassage:\n{text}"
        );
            let revised = clean_model_text(&client.complete(
                model,
                "You are revising legal prose carefully. Preserve substance and return only prose.",
                &prompt,
                2000,
            )?);
            if (100..=500).contains(&revised.split_whitespace().count()) {
                text = revised;
            }
        }
        Ok(text)
    }

    fn clean_model_text(text: &str) -> String {
        let text = text
            .replace("```text", "")
            .replace("```markdown", "")
            .replace("```", "");
        let text = text.trim();
        let text = text
            .strip_prefix("Passage:")
            .or_else(|| text.strip_prefix("Here is the passage:"))
            .unwrap_or(text)
            .trim();
        normalize_whitespace(text)
    }
}

#[cfg(feature = "sourcing")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run()
}

#[cfg(not(feature = "sourcing"))]
fn main() {}
