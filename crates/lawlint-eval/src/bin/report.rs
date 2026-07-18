use clap::Parser;
use lawlint_eval::{
    auc, auc_for_ai_slice, evaluate, load_jsonl, per_rule_metrics, rule_ids, score_histogram,
    Baseline, Label, Split,
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

    let test: Vec<_> = evaluated
        .iter()
        .filter(|sample| sample.sample.resolved_split() == Split::Test)
        .cloned()
        .collect();
    let rules = per_rule_metrics(&test, rule_ids());
    println!("\nPer-rule metrics (test split)");
    println!("rule\tprecision\trecall\tf1\tTP\tFP\tFN");
    for (rule, metric) in &rules {
        println!(
            "{rule}\t{:.3}\t{:.3}\t{:.3}\t{}\t{}\t{}",
            metric.precision,
            metric.recall,
            metric.f1,
            metric.true_positive,
            metric.false_positive,
            metric.false_negative
        );
    }

    let overall_auc = auc(&test);
    let mut slice_auc = BTreeMap::new();
    for style in ["naive", "rule-evading", "self-edit"] {
        slice_auc.insert(style.to_string(), auc_for_ai_slice(&test, style));
    }
    println!("\nAUC (test split)");
    println!("overall: {:.3}", overall_auc);
    for (style, value) in &slice_auc {
        println!("{style}: {value:.3}");
    }
    println!("\nScore histograms (test split)");
    for (label, name) in [(Label::Human, "human"), (Label::Ai, "ai")] {
        let histogram = score_histogram(&test, label);
        println!("{name}: {}", format_histogram(&histogram));
    }

    if let Some(path) = args.emit_baseline {
        let baseline = Baseline {
            overall_auc,
            slice_auc,
            per_rule_recall: rules
                .into_iter()
                .map(|(name, metrics)| (name, metrics.recall))
                .collect(),
        };
        let json = serde_json::to_string_pretty(&baseline)
            .map_err(|error| format!("failed to serialize baseline: {error}"))?;
        fs::write(&path, format!("{json}\n"))
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
        println!("\nWrote baseline to {}", path.display());
    }
    Ok(())
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
