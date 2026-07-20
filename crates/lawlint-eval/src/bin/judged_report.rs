//! Opt-in tier-3 judged evaluation (#39 part 2): runs the real judge
//! pipeline over the corpus train split and reports per-rule
//! precision/recall/F1 for the inferential rules plus the verdict-discipline
//! rate. Feature-gated (`judged`) and never part of the default CI gate —
//! judge runs are slow and backend-dependent.

#[cfg(feature = "judged")]
mod app {
    use clap::Parser;
    use lawlint_core::{lint_full, Judge, JudgeCache, JudgeError, JudgeFinding, JudgeRequest};
    use lawlint_core::{LintOptions, RuleSet};
    use lawlint_eval::judged::VerdictDiscipline;
    use lawlint_eval::{
        inferential_rule_ids, load_jsonl, per_rule_metrics, EvaluatedSample, Sample, Split,
    };
    use lawlint_judge::{create_client, AxJudge, DiskCache};
    use sha2::{Digest, Sha256};
    use std::collections::BTreeSet;
    use std::fmt::Write as _;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Instant;

    #[derive(Debug, Parser)]
    #[command(
        name = "judged_report",
        about = "Report tier-3 judged evaluation metrics over the corpus train split"
    )]
    struct Args {
        #[arg(default_value = "crates/lawlint-eval/corpus/corpus.jsonl")]
        corpus: PathBuf,
        /// Judge model spec(s), repeatable or comma-separated: "local",
        /// "local:<hf-repo>[#<gguf-file>]", "anthropic:<model>",
        /// "openai:<base-url>#<model>", "foundry:<deployment>".
        #[arg(long = "model", value_delimiter = ',', default_value = "local")]
        models: Vec<String>,
        /// Evaluate only the first N train samples (smoke runs).
        #[arg(long)]
        limit: Option<usize>,
    }

    pub fn run() -> Result<(), String> {
        let args = Args::parse();
        let samples = load_jsonl(&args.corpus)?;
        let mut train: Vec<Sample> = samples
            .into_iter()
            .filter(|sample| sample.resolved_split() == Split::Train)
            .collect();
        if let Some(limit) = args.limit {
            train.truncate(limit);
        }
        println!("Judged evaluation: {} train samples", train.len());

        let cache = match DiskCache::new() {
            Ok(cache) => Some(cache),
            Err(error) => {
                eprintln!(
                    "judged_report: warning: judge cache unavailable ({error}); running uncached"
                );
                None
            }
        };

        let mut ran = 0usize;
        for spec in &args.models {
            match run_model(spec, &train, cache.as_ref()) {
                Ok(()) => ran += 1,
                Err(error) => {
                    // Per-backend graceful failure: report and keep going so
                    // backends that ran still print results.
                    eprintln!("judged_report: backend {spec:?} unavailable: {error}");
                }
            }
        }
        if ran == 0 {
            return Err("no backend produced results".to_string());
        }
        Ok(())
    }

    fn run_model(spec: &str, train: &[Sample], cache: Option<&DiskCache>) -> Result<(), String> {
        let (client, model_id) = create_client(spec).map_err(|error| error.to_string())?;
        let judge = MeasuredJudge {
            inner: AxJudge::new(client, model_id),
            cache,
            tally: Mutex::new(Tally::default()),
            cache_hits: AtomicUsize::new(0),
            last_error: Mutex::new(None),
        };
        let options = LintOptions::default();
        let rules = RuleSet::built_in();

        let mut evaluated = Vec::with_capacity(train.len());
        let mut discipline = VerdictDiscipline::default();
        let mut agg = Aggregate::default();
        let start = Instant::now();
        for (index, sample) in train.iter().enumerate() {
            let result = lint_full(&sample.text, &options, &rules, &judge, None);
            let stats = result.judge.as_ref().expect("lint_full sets judge stats");
            // Backend that cannot serve a single chunk (bad endpoint, missing
            // key at request time, model that never emits parseable JSON):
            // abort this model instead of grinding through 330 failures.
            if index == 0 && stats.chunks > 0 && stats.chunks_failed == stats.chunks {
                let detail = judge
                    .last_error
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| "no parseable response".to_string());
                return Err(format!(
                    "failed on every chunk of the first sample: {detail}"
                ));
            }
            agg.add(stats);
            let tally = std::mem::take(&mut *judge.tally.lock().unwrap());
            discipline.add_sample(sample.label, &tally.kept, tally.dropped_negative);
            let fired_rules: BTreeSet<String> = result
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.rule_id.0.clone())
                .collect();
            evaluated.push(EvaluatedSample {
                sample: sample.clone(),
                score: result.stats.score,
                fired_rules,
            });
            if (index + 1) % 10 == 0 || index + 1 == train.len() {
                eprintln!(
                    "[{}] {}/{} samples, {:.0}s elapsed",
                    judge.model_id(),
                    index + 1,
                    train.len(),
                    start.elapsed().as_secs_f64()
                );
            }
        }
        agg.cache_hits = judge.cache_hits.load(Ordering::Relaxed);
        print_report(
            judge.model_id(),
            spec,
            &evaluated,
            &discipline,
            &agg,
            start.elapsed().as_secs_f64(),
        );
        Ok(())
    }

    fn print_report(
        model_id: &str,
        spec: &str,
        evaluated: &[EvaluatedSample],
        discipline: &VerdictDiscipline,
        agg: &Aggregate,
        elapsed: f64,
    ) {
        println!("\n== model {model_id} (spec {spec:?}) ==");
        println!(
            "samples: {}; elapsed: {elapsed:.0}s; judge chunks: {} (cache hits {}, failed {}, grounded {}, hallucinated {})",
            evaluated.len(),
            agg.chunks,
            agg.cache_hits,
            agg.chunks_failed,
            agg.grounded,
            agg.hallucinated
        );

        let metrics = per_rule_metrics(evaluated, inferential_rule_ids());
        println!("\nPer-rule metrics (train split, judged)");
        println!("rule\tprecision\trecall\tf1\tTP\tFP\tFN");
        for (rule, metric) in &metrics {
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

        println!("\nVerdict discipline (parse layer)");
        println!("model-emitted findings: {}", discipline.emitted);
        println!(
            "dropped by polarity guard: {} (raw negation-emission rate {})",
            discipline.dropped_negative,
            percent(discipline.negation_emission_rate())
        );
        println!(
            "guard survivors on human samples: {}; negational (broad heuristic): {} (surviving-negation rate {}; target <5%)",
            discipline.human_kept,
            discipline.human_kept_negational,
            percent(discipline.surviving_negation_rate())
        );
    }

    fn percent(rate: f64) -> String {
        format!("{:.1}%", rate * 100.0)
    }

    // ---- judge wrapper ---------------------------------------------------

    /// Parse-layer tally for the sample currently being judged; drained by
    /// the harness between samples.
    #[derive(Default)]
    struct Tally {
        kept: Vec<JudgeFinding>,
        dropped_negative: usize,
    }

    /// Wraps [`AxJudge`] with an evaluate-level disk cache holding both the
    /// kept findings and the polarity-guard drops. Core's `run_judge` cache
    /// stores kept findings only, which would zero the verdict-discipline
    /// stats on every rerun — so caching lives here and `lint_full` gets
    /// `cache: None`.
    struct MeasuredJudge<'a> {
        inner: AxJudge,
        cache: Option<&'a DiskCache>,
        tally: Mutex<Tally>,
        // Hits happen here, not in `run_judge` (which sees no cache), so
        // `JudgeStats.cache_hits` would always read zero.
        cache_hits: AtomicUsize,
        last_error: Mutex<Option<String>>,
    }

    impl MeasuredJudge<'_> {
        fn record(&self, kept: &[JudgeFinding], dropped_negative: usize) {
            let mut tally = self.tally.lock().unwrap();
            tally.kept.extend_from_slice(kept);
            tally.dropped_negative += dropped_negative;
        }
    }

    impl Judge for MeasuredJudge<'_> {
        fn evaluate(&self, req: &JudgeRequest) -> Result<Vec<JudgeFinding>, JudgeError> {
            // The kept entry mirrors core's full cache key
            // (sha256(cache_key_base + model_id)), so entries are shared with
            // ordinary `lawlint --judge` runs over the same chunks; the drops
            // live under a suffixed sibling key. A hit requires both entries —
            // kept findings without their drop count cannot feed the
            // verdict-discipline rate.
            let key = sha256_hex(&format!("{}{}", req.cache_key_base, self.inner.model_id()));
            let dropped_key = format!("{key}:dropped-negative");
            if let Some(cache) = self.cache {
                if let (Some(kept), Some(dropped)) = (cache.get(&key), cache.get(&dropped_key)) {
                    self.cache_hits.fetch_add(1, Ordering::Relaxed);
                    self.record(&kept, dropped.len());
                    return Ok(kept);
                }
            }
            let parsed = self.inner.evaluate_with_stats(req).inspect_err(|error| {
                *self.last_error.lock().unwrap() = Some(error.to_string());
            })?;
            if let Some(cache) = self.cache {
                cache.put(&key, &parsed.kept);
                cache.put(&dropped_key, &parsed.dropped_negative);
            }
            self.record(&parsed.kept, parsed.dropped_negative.len());
            Ok(parsed.kept)
        }

        fn model_id(&self) -> &str {
            self.inner.model_id()
        }
    }

    fn sha256_hex(input: &str) -> String {
        let digest = Sha256::digest(input.as_bytes());
        let mut out = String::with_capacity(64);
        for byte in digest {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }

    // ---- aggregate judge stats --------------------------------------------

    #[derive(Default)]
    struct Aggregate {
        chunks: usize,
        cache_hits: usize,
        chunks_failed: usize,
        grounded: usize,
        hallucinated: usize,
    }

    impl Aggregate {
        fn add(&mut self, stats: &lawlint_core::JudgeStats) {
            self.chunks += stats.chunks;
            self.chunks_failed += stats.chunks_failed;
            self.grounded += stats.grounded;
            self.hallucinated += stats.hallucinated.values().sum::<usize>();
        }
    }
}

#[cfg(feature = "judged")]
fn main() {
    if let Err(error) = app::run() {
        eprintln!("judged_report: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(feature = "judged"))]
fn main() {}
