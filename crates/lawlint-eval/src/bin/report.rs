use clap::Parser;
use lawlint_eval::{
    auc, auc_for_ai_slice, evaluate, inferential_rule_ids, load_jsonl, per_rule_metrics,
    precision_map, rule_ids, rule_intents, score_histogram, Baseline, Label, Split,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "report", about = "Report lawlint evaluation corpus metrics")]
struct Args {
    #[arg(default_value = "crates/lawlint-eval/corpus/corpus.jsonl")]
    corpus: PathBuf,
    #[arg(long)]
    emit_baseline: Option<PathBuf>,
}

fn main() {
    if let Err(error) = run(Args::parse()) {
        eprintln!("report: {error}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<(), String> {
    let samples = load_jsonl(&args.corpus)?;
    let evaluated = evaluate(&samples);
    let mut labels = BTreeMap::new();
    let mut sources = BTreeMap::new();
    let mut registers = BTreeMap::new();
    let mut splits = BTreeMap::new();
    for sample in &samples {
        *labels
            .entry(format!("{:?}", sample.label).to_lowercase())
            .or_insert(0) += 1;
        *sources.entry(sample.source.clone()).or_insert(0) += 1;
        *registers.entry(sample.register.clone()).or_insert(0) += 1;
        *splits
            .entry(format!("{:?}", sample.resolved_split()).to_lowercase())
            .or_insert(0) += 1;
    }
    println!("Dataset: {} samples", samples.len());
    print_counts("labels", &labels);
    print_counts("sources", &sources);
    print_counts("registers", &registers);
    print_counts("splits", &splits);

    let train: Vec<_> = evaluated
        .iter()
        .filter(|sample| sample.sample.resolved_split() == Split::Train)
        .cloned()
        .collect();
    let test: Vec<_> = evaluated
        .iter()
        .filter(|sample| sample.sample.resolved_split() == Split::Test)
        .cloned()
        .collect();
    let train_metrics = print_metrics("train", &train);
    print_metrics("test (held-out)", &test);
    println!(
        "\nInferential rules not evaluated (requires judge): {}",
        inferential_rule_ids().join(", ")
    );

    if let Some(path) = args.emit_baseline {
        let baseline = Baseline {
            overall_auc: train_metrics.0,
            slice_auc: train_metrics.1,
            per_rule_precision: precision_map(&train_metrics.2),
        };
        let json = serde_json::to_string_pretty(&baseline)
            .map_err(|error| format!("failed to serialize baseline: {error}"))?;
        fs::write(&path, format!("{json}\n"))
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
        println!("\nWrote baseline to {}", path.display());
    }
    Ok(())
}

fn print_metrics(
    name: &str,
    samples: &[lawlint_eval::EvaluatedSample],
) -> (
    f64,
    BTreeMap<String, f64>,
    BTreeMap<String, lawlint_eval::RuleMetrics>,
) {
    let rules = per_rule_metrics(samples, rule_ids());
    let intents = rule_intents();
    println!("\nPer-rule metrics ({name} split; style rules lint but do not score)");
    println!("rule\tintent\tprecision\trecall\tf1\tTP\tFP\tFN");
    for (rule, metric) in &rules {
        let intent = match intents.get(rule) {
            Some(lawlint_core::Intent::Style) => "style",
            _ => "detection",
        };
        println!(
            "{rule}\t{intent}\t{:.3}\t{:.3}\t{:.3}\t{}\t{}\t{}",
            metric.precision,
            metric.recall,
            metric.f1,
            metric.true_positive,
            metric.false_positive,
            metric.false_negative
        );
    }
    let overall_auc = auc(samples);
    let slice_auc = ["naive", "rule-evading", "self-edit"]
        .into_iter()
        .map(|style| (style.to_string(), auc_for_ai_slice(samples, style)))
        .collect::<BTreeMap<_, _>>();
    println!("\nAUC ({name} split; score = 100 - lint score; P(AI is more AI-like than human))");
    println!(
        "Interpretation: 0.500 = chance; >0.500 = discriminative; <0.500 = anti-discriminative"
    );
    println!("overall: {overall_auc:.3}");
    for (style, value) in &slice_auc {
        println!("{style}: {value:.3}");
    }
    println!("\nMean human-likeness scores ({name} split; higher means fewer rule findings)");
    println!(
        "overall: human={:.3} ai={:.3}",
        mean_lint_score(samples, Label::Human),
        mean_lint_score(samples, Label::Ai)
    );
    for style in ["naive", "rule-evading", "self-edit"] {
        let ai_scores = samples
            .iter()
            .filter(|sample| {
                sample.sample.label == Label::Ai
                    && sample.sample.prompt_style.as_deref() == Some(style)
            })
            .map(|sample| sample.score)
            .collect::<Vec<_>>();
        println!(
            "{style}: human={:.3} ai={:.3}",
            mean_lint_score(samples, Label::Human),
            mean(&ai_scores)
        );
    }
    println!("\nScore histograms ({name} split)");
    for (label, label_name) in [(Label::Human, "human"), (Label::Ai, "ai")] {
        let histogram = score_histogram(samples, label);
        println!("{label_name}: {}", format_histogram(&histogram));
    }
    (overall_auc, slice_auc, rules)
}

fn mean_lint_score(samples: &[lawlint_eval::EvaluatedSample], label: Label) -> f64 {
    mean(
        &samples
            .iter()
            .filter(|sample| sample.sample.label == label)
            .map(|sample| sample.score)
            .collect::<Vec<_>>(),
    )
}

fn mean(values: &[i32]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<i32>() as f64 / values.len() as f64
    }
}

fn print_counts(name: &str, counts: &BTreeMap<String, usize>) {
    let details = counts
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(", ");
    println!("{name}: {details}");
}

fn format_histogram(histogram: &[lawlint_eval::Histogram]) -> String {
    histogram
        .iter()
        .map(|bucket| format!("{}={}", bucket.bucket, bucket.count))
        .collect::<Vec<_>>()
        .join(" ")
}
