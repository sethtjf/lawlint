//! Evaluation corpus loading, metrics, and regression checks for lawlint.

use lawlint_core::{lint, LintOptions, RuleSet, Tier};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

#[cfg(feature = "sourcing")]
pub mod sourcing;

#[cfg(feature = "sourcing")]
pub mod foundry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Label {
    Human,
    Ai,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Split {
    Train,
    Test,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub id: String,
    pub label: Label,
    pub text: String,
    pub word_count: Option<usize>,
    pub source: String,
    pub register: String,
    pub era: Option<String>,
    pub date: Option<String>,
    pub court: Option<String>,
    pub model: Option<String>,
    pub prompt_style: Option<String>,
    pub pair_id: Option<String>,
    pub split: Option<Split>,
}

impl Sample {
    pub fn resolved_split(&self) -> Split {
        resolved_split(self)
    }
}

pub fn load_jsonl(path: impl AsRef<Path>) -> Result<Vec<Sample>, String> {
    let path = path.as_ref();
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    contents
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line)
                .map_err(|error| format!("{}:{}: invalid JSON: {error}", path.display(), index + 1))
        })
        .collect()
}

pub fn resolved_split(sample: &Sample) -> Split {
    if let Some(split) = sample.split {
        return split;
    }
    let key = sample.pair_id.as_deref().unwrap_or(&sample.id);
    let digest = Sha256::digest(key.as_bytes());
    let bucket = u16::from_be_bytes([digest[0], digest[1]]) % 100;
    if bucket < 70 {
        Split::Train
    } else {
        Split::Test
    }
}

#[derive(Debug, Clone)]
pub struct EvaluatedSample {
    pub sample: Sample,
    pub score: i32,
    pub fired_rules: BTreeSet<String>,
}

pub fn evaluate(samples: &[Sample]) -> Vec<EvaluatedSample> {
    let options = LintOptions::default();
    samples
        .iter()
        .cloned()
        .map(|sample| {
            let result = lint(&sample.text, &options);
            let fired_rules = result
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.rule_id.0.clone())
                .collect();
            EvaluatedSample {
                sample,
                score: result.stats.score,
                fired_rules,
            }
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuleMetrics {
    pub precision: f64,
    pub recall: f64,
    pub f1: f64,
    pub true_positive: usize,
    pub false_positive: usize,
    pub false_negative: usize,
    pub ai_support: usize,
    pub human_support: usize,
}

pub fn per_rule_metrics(
    samples: &[EvaluatedSample],
    rule_ids: impl IntoIterator<Item = String>,
) -> BTreeMap<String, RuleMetrics> {
    rule_ids
        .into_iter()
        .map(|rule_id| {
            let mut tp = 0;
            let mut fp = 0;
            let mut fn_count = 0;
            let mut ai_support = 0;
            let mut human_support = 0;
            for sample in samples {
                let fired = sample.fired_rules.contains(&rule_id);
                match sample.sample.label {
                    Label::Ai => {
                        ai_support += 1;
                        if fired {
                            tp += 1;
                        } else {
                            fn_count += 1;
                        }
                    }
                    Label::Human => {
                        human_support += 1;
                        if fired {
                            fp += 1;
                        }
                    }
                }
            }
            let precision = ratio(tp, tp + fp);
            let recall = ratio(tp, tp + fn_count);
            let f1 = if precision + recall == 0.0 {
                0.0
            } else {
                2.0 * precision * recall / (precision + recall)
            };
            (
                rule_id,
                RuleMetrics {
                    precision,
                    recall,
                    f1,
                    true_positive: tp,
                    false_positive: fp,
                    false_negative: fn_count,
                    ai_support,
                    human_support,
                },
            )
        })
        .collect()
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

pub fn rule_ids() -> Vec<String> {
    RuleSet::built_in()
        .metas()
        .iter()
        .filter(|meta| meta.tier != Tier::Inferential)
        .map(|meta| meta.id.0.clone())
        .collect()
}

pub fn inferential_rule_ids() -> Vec<String> {
    RuleSet::built_in()
        .metas()
        .iter()
        .filter(|meta| meta.tier == Tier::Inferential)
        .map(|meta| meta.id.0.clone())
        .collect()
}

pub fn auc(samples: &[EvaluatedSample]) -> f64 {
    let ai: Vec<i32> = samples
        .iter()
        .filter(|sample| sample.sample.label == Label::Ai)
        .map(|sample| 100 - sample.score)
        .collect();
    let human: Vec<i32> = samples
        .iter()
        .filter(|sample| sample.sample.label == Label::Human)
        .map(|sample| 100 - sample.score)
        .collect();
    auc_scores(&ai, &human)
}

pub fn auc_for_ai_slice(samples: &[EvaluatedSample], prompt_style: &str) -> f64 {
    let filtered: Vec<EvaluatedSample> = samples
        .iter()
        .filter(|sample| {
            sample.sample.label == Label::Human
                || (sample.sample.label == Label::Ai
                    && sample.sample.prompt_style.as_deref() == Some(prompt_style))
        })
        .cloned()
        .collect();
    auc(&filtered)
}

pub fn auc_scores(ai: &[i32], human: &[i32]) -> f64 {
    if ai.is_empty() || human.is_empty() {
        return 0.0;
    }
    let mut favorable = 0.0;
    for ai_score in ai {
        for human_score in human {
            favorable += match ai_score.cmp(human_score) {
                std::cmp::Ordering::Greater => 1.0,
                std::cmp::Ordering::Equal => 0.5,
                std::cmp::Ordering::Less => 0.0,
            };
        }
    }
    favorable / (ai.len() * human.len()) as f64
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Histogram {
    pub bucket: String,
    pub count: usize,
}

pub fn score_histogram(samples: &[EvaluatedSample], label: Label) -> Vec<Histogram> {
    let mut counts = [0usize; 11];
    for sample in samples.iter().filter(|sample| sample.sample.label == label) {
        let score = sample.score.clamp(0, 100) as usize;
        let index = if score == 100 { 10 } else { score / 10 };
        counts[index] += 1;
    }
    counts
        .iter()
        .enumerate()
        .map(|(index, count)| Histogram {
            bucket: if index == 10 {
                "100".to_string()
            } else {
                format!("{}-{}", index * 10, index * 10 + 9)
            },
            count: *count,
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Baseline {
    pub overall_auc: f64,
    pub slice_auc: BTreeMap<String, f64>,
    pub per_rule_precision: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Regression {
    pub metric: String,
    pub baseline: f64,
    pub current: f64,
}

pub fn check_regression(
    current: &Baseline,
    baseline: &Baseline,
    tolerance: f64,
) -> Vec<Regression> {
    let mut regressions = Vec::new();
    check_metric(
        &mut regressions,
        "overall_auc",
        current.overall_auc,
        baseline.overall_auc,
        tolerance,
    );
    for (name, baseline_value) in &baseline.slice_auc {
        if let Some(current_value) = current.slice_auc.get(name) {
            check_metric(
                &mut regressions,
                &format!("slice_auc.{name}"),
                *current_value,
                *baseline_value,
                tolerance,
            );
        }
    }
    for (name, baseline_value) in &baseline.per_rule_precision {
        if let Some(current_value) = current.per_rule_precision.get(name) {
            check_metric(
                &mut regressions,
                &format!("per_rule_precision.{name}"),
                *current_value,
                *baseline_value,
                tolerance,
            );
        }
    }
    regressions
}

fn check_metric(
    regressions: &mut Vec<Regression>,
    metric: &str,
    current: f64,
    baseline: f64,
    tolerance: f64,
) {
    if current < baseline - tolerance {
        regressions.push(Regression {
            metric: metric.to_string(),
            baseline,
            current,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str, label: Label, pair_id: Option<&str>) -> Sample {
        Sample {
            id: id.to_string(),
            label,
            text: "text".to_string(),
            word_count: None,
            source: "manual".to_string(),
            register: "memo".to_string(),
            era: None,
            date: None,
            court: None,
            model: None,
            prompt_style: None,
            pair_id: pair_id.map(str::to_string),
            split: None,
        }
    }

    #[test]
    fn auc_counts_ties_as_half() {
        assert!((auc_scores(&[3, 2], &[1, 2]) - 0.875).abs() < f64::EPSILON);
    }

    #[test]
    fn split_is_deterministic_and_pairs_stay_together() {
        let first = sample("one", Label::Ai, Some("pair"));
        let second = sample("two", Label::Human, Some("pair"));
        assert_eq!(resolved_split(&first), resolved_split(&first));
        assert_eq!(resolved_split(&first), resolved_split(&second));
        let forced = Sample {
            split: Some(Split::Test),
            ..first
        };
        assert_eq!(resolved_split(&forced), Split::Test);
    }

    #[test]
    fn per_rule_math_and_zero_division() {
        let mut ai = EvaluatedSample {
            sample: sample("ai", Label::Ai, None),
            score: 50,
            fired_rules: BTreeSet::from(["r".to_string()]),
        };
        let human = EvaluatedSample {
            sample: sample("human", Label::Human, None),
            score: 50,
            fired_rules: BTreeSet::from(["r".to_string()]),
        };
        let metrics = per_rule_metrics(
            &[ai.clone(), human.clone()],
            ["r".to_string(), "never".to_string()],
        );
        assert_eq!(metrics["r"].true_positive, 1);
        assert_eq!(metrics["r"].false_positive, 1);
        assert_eq!(metrics["r"].recall, 1.0);
        assert_eq!(metrics["never"].recall, 0.0);
        ai.fired_rules.clear();
        let metrics = per_rule_metrics(&[ai, human], ["r".to_string()]);
        assert_eq!(metrics["r"].false_negative, 1);
    }

    #[test]
    fn committed_seed_corpus_matches_baseline() {
        let corpus = load_jsonl(concat!(env!("CARGO_MANIFEST_DIR"), "/corpus/corpus.jsonl"))
            .expect("seed corpus should load");
        let baseline: Baseline = serde_json::from_str(include_str!("../corpus/baseline.json"))
            .expect("committed baseline should load");
        let evaluated = evaluate(&corpus);
        let train: Vec<_> = evaluated
            .iter()
            .filter(|sample| sample.sample.resolved_split() == Split::Train)
            .cloned()
            .collect();
        let rules = per_rule_metrics(&train, rule_ids());
        let current = Baseline {
            overall_auc: auc(&train),
            slice_auc: ["naive", "rule-evading", "self-edit"]
                .into_iter()
                .map(|style| (style.to_string(), auc_for_ai_slice(&train, style)))
                .collect(),
            per_rule_precision: rules
                .into_iter()
                .map(|(rule, metrics)| (rule, metrics.precision))
                .collect(),
        };
        assert!(check_regression(&current, &baseline, 0.03).is_empty());
    }
}
