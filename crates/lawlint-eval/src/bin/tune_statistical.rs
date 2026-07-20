//! Dev tool for #37: measure document-level statistical metrics on the eval
//! corpus TRAIN split and grid-search thresholds. For each metric it prints
//! the per-class distribution and the train AUC obtained by adding that
//! metric's flag ALONE on top of the current built-in ruleset — the
//! acceptance gate for shipping a metric. The test split is never read.

use lawlint_core::engines::statistical::{metric_value, Metric};
use lawlint_core::{lint, parse, Intent, LintOptions, Severity};
use lawlint_eval::{auc_scores, load_jsonl, Label, Sample, Split};

const METRICS: [(Metric, &str); 5] = [
    (Metric::SentenceLengthVariance, "sentence-length-variance"),
    (Metric::CadenceAutocorrelation, "cadence-autocorrelation"),
    (Metric::RepeatedOpenerDensity, "repeated-opener-density"),
    (Metric::TriadDensity, "triad-density"),
    (Metric::PairedAdjectiveRate, "paired-adjective-rate"),
];

/// Everything needed to re-score a sample with extra penalty points.
struct Row {
    label: Label,
    naive: bool,
    penalty: f64,
    words: usize,
    values: Vec<Option<f64>>, // parallel to METRICS
}

fn points(severity: Severity) -> f64 {
    match severity {
        Severity::Error => 5.0,
        Severity::Warning => 3.0,
        Severity::Suggestion => 1.0,
    }
}

/// scoring::finalize's formula, with `extra` penalty points added.
fn score(penalty: f64, extra: f64, words: usize) -> i32 {
    let density = (penalty + extra) / words.max(1) as f64 * 1000.0;
    (100.0 * (-density / 100.0).exp()).round().clamp(0.0, 100.0) as i32
}

fn auc_with_flag(rows: &[Row], fired: &[bool], extra: f64, naive_only: bool) -> f64 {
    let mut ai = Vec::new();
    let mut human = Vec::new();
    for (row, &f) in rows.iter().zip(fired) {
        let s = 100 - score(row.penalty, if f { extra } else { 0.0 }, row.words);
        match row.label {
            Label::Ai if !naive_only || row.naive => ai.push(s),
            Label::Human => human.push(s),
            _ => {}
        }
    }
    auc_scores(&ai, &human)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

fn dist(name: &str, values: &[f64]) {
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let mean = v.iter().sum::<f64>() / v.len().max(1) as f64;
    println!(
        "    {name:6} n={:3} mean={mean:7.2} p10={:7.2} p25={:7.2} p50={:7.2} p75={:7.2} p90={:7.2}",
        v.len(),
        percentile(&v, 0.10),
        percentile(&v, 0.25),
        percentile(&v, 0.50),
        percentile(&v, 0.75),
        percentile(&v, 0.90),
    );
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "crates/lawlint-eval/corpus/corpus.jsonl".to_string());
    let samples = load_jsonl(&path).expect("corpus loads");
    let train: Vec<Sample> = samples
        .into_iter()
        .filter(|s| s.resolved_split() == Split::Train)
        .collect();
    let options = LintOptions::default();

    let rows: Vec<Row> = train
        .iter()
        .map(|s| {
            let result = lint(&s.text, &options);
            let penalty: f64 = result
                .diagnostics
                .iter()
                .filter(|d| d.intent == Intent::Detection)
                .map(|d| points(d.severity) * f64::from(d.weight.unwrap_or(1)))
                .sum();
            let doc = parse(&s.text, false);
            let values = METRICS
                .iter()
                .map(|(m, _)| metric_value(*m, &s.text, &doc))
                .collect();
            Row {
                label: s.label,
                naive: s.prompt_style.as_deref() == Some("naive"),
                penalty,
                words: result.stats.word_count,
                values,
            }
        })
        .collect();

    let none: Vec<bool> = vec![false; rows.len()];
    let base = auc_with_flag(&rows, &none, 0.0, false);
    let base_naive = auc_with_flag(&rows, &none, 0.0, true);
    println!(
        "train rows: {} | base AUC {base:.4} | naive-slice AUC {base_naive:.4}",
        rows.len()
    );
    // Artifact control: a flag that always fires still moves AUC (the flat
    // penalty is divided by word count, so it reorders by length). Any
    // metric whose gain is near this control is NOT a real signal.
    let all: Vec<bool> = vec![true; rows.len()];
    println!(
        "always-fire control: AUC {:.4} ({:+.4})",
        auc_with_flag(&rows, &all, 3.0, false),
        auc_with_flag(&rows, &all, 3.0, false) - base
    );

    for (i, (_, name)) in METRICS.iter().enumerate() {
        println!("\n== {name} ==");
        let ai: Vec<f64> = rows
            .iter()
            .filter(|r| r.label == Label::Ai)
            .filter_map(|r| r.values[i])
            .collect();
        let human: Vec<f64> = rows
            .iter()
            .filter(|r| r.label == Label::Human)
            .filter_map(|r| r.values[i])
            .collect();
        dist("ai", &ai);
        dist("human", &human);
        // Raw metric separability (AUC of the metric value itself, above).
        let ai_i: Vec<i32> = ai.iter().map(|v| (v * 1000.0) as i32).collect();
        let hu_i: Vec<i32> = human.iter().map(|v| (v * 1000.0) as i32).collect();
        println!(
            "    raw value AUC (ai higher): {:.4}",
            auc_scores(&ai_i, &hu_i)
        );

        // Grid: pooled percentiles as candidate thresholds, both directions.
        let mut pooled: Vec<f64> = ai.iter().chain(human.iter()).copied().collect();
        pooled.sort_by(|a, b| a.total_cmp(b));
        pooled.dedup();
        let mut best: Vec<(f64, &str, f64, f64, usize, usize)> = Vec::new();
        for pct in 1..100 {
            let t = percentile(&pooled, pct as f64 / 100.0);
            for dir in ["above", "below"] {
                let fired: Vec<bool> = rows
                    .iter()
                    .map(|r| {
                        r.values[i].is_some_and(|v| match dir {
                            "above" => v > t,
                            _ => v < t,
                        })
                    })
                    .collect();
                let a = auc_with_flag(&rows, &fired, 3.0, false);
                let a_naive = auc_with_flag(&rows, &fired, 3.0, true);
                let fired_ai = rows
                    .iter()
                    .zip(&fired)
                    .filter(|(r, &f)| f && r.label == Label::Ai)
                    .count();
                let fired_human = rows
                    .iter()
                    .zip(&fired)
                    .filter(|(r, &f)| f && r.label == Label::Human)
                    .count();
                best.push((t, dir, a, a_naive, fired_ai, fired_human));
            }
        }
        best.sort_by(|a, b| b.2.total_cmp(&a.2));
        best.dedup_by(|a, b| a.2 == b.2 && a.1 == b.1);
        println!("    top thresholds by train AUC (delta vs base {base:.4}):");
        for (t, dir, a, a_naive, fa, fh) in best.iter().take(6) {
            println!(
                "      {dir:5} {t:8.3} -> AUC {a:.4} ({:+.4}) naive {a_naive:.4} fired ai/human {fa}/{fh}",
                a - base
            );
        }
    }

    // Combined check: thresholds passed as name=dir:threshold args after the
    // corpus path, e.g. sentence-length-variance=below:60.
    let combos: Vec<(usize, String, f64)> = std::env::args()
        .skip(2)
        .filter_map(|arg| {
            let (name, rest) = arg.split_once('=')?;
            let (dir, t) = rest.split_once(':')?;
            let idx = METRICS.iter().position(|(_, n)| *n == name)?;
            Some((idx, dir.to_string(), t.parse().ok()?))
        })
        .collect();
    if !combos.is_empty() {
        let extra_per: f64 = 3.0;
        let mut ai = Vec::new();
        let mut human = Vec::new();
        let mut ai_naive = Vec::new();
        let mut fire_counts = vec![(0usize, 0usize); combos.len()];
        for row in &rows {
            let mut extra = 0.0;
            for (c, (idx, dir, t)) in combos.iter().enumerate() {
                let fired = row.values[*idx].is_some_and(|v| match dir.as_str() {
                    "above" => v > *t,
                    _ => v < *t,
                });
                if fired {
                    extra += extra_per;
                    match row.label {
                        Label::Ai => fire_counts[c].0 += 1,
                        Label::Human => fire_counts[c].1 += 1,
                    }
                }
            }
            let s = 100 - score(row.penalty, extra, row.words);
            match row.label {
                Label::Ai => {
                    ai.push(s);
                    if row.naive {
                        ai_naive.push(s);
                    }
                }
                Label::Human => human.push(s),
            }
        }
        println!(
            "\ncombined: AUC {:.4} | naive-slice AUC {:.4}",
            auc_scores(&ai, &human),
            auc_scores(&ai_naive, &human)
        );
        for (c, (idx, dir, t)) in combos.iter().enumerate() {
            let (fa, fh) = fire_counts[c];
            println!(
                "  {} {dir} {t}: fired ai/human {fa}/{fh} (precision {:.3})",
                METRICS[*idx].1,
                fa as f64 / (fa + fh).max(1) as f64
            );
        }
    }
}
