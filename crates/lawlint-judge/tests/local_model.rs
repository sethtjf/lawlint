//! Real tiny-model inference (downloads ~1 GB on first run, then cached).
//!
//! Run with: `cargo test -p lawlint-judge -- --ignored`

use lawlint_core::{
    parse, plan_judge, Granularity, JudgeOptions, RubricFragment, RuleId, Severity,
};

#[test]
#[ignore = "downloads and runs a real local model"]
fn local_candle_judge_evaluates_a_real_chunk() {
    let judge = lawlint_judge::create_judge(&JudgeOptions::default()).expect("create local judge");
    assert!(judge.model_id().starts_with("local:"));

    let source = "It could perhaps be argued that the agreement might possibly be \
                  unenforceable. The deposit was paid on March 3, 2024.";
    let doc = parse(source, false);
    let rubric = RubricFragment {
        rule: RuleId("core/empty-hedge".to_string()),
        severity: Severity::Warning,
        granularity: Granularity::Sentence,
        rubric: "Flag hedges that carry no information about actual uncertainty.".to_string(),
        flag_examples: vec![
            "It could perhaps be argued that the clause fails.".to_string(),
            "One might possibly conclude the point.".to_string(),
            "It may arguably be the case that liability attaches.".to_string(),
        ],
        pass_examples: vec![
            "Damages are uncertain because treatment is ongoing.".to_string(),
            "The deposit was paid on March 3, 2024.".to_string(),
            "The court granted the motion.".to_string(),
        ],
    };
    let refs = [&rubric];
    let reqs = plan_judge(&doc, source, &refs);
    assert!(!reqs.is_empty());

    // The real assertion is the plumbing: model downloads/loads, chat template
    // applies, generation completes, and the output parses as the strict
    // JudgeFinding[] contract (possibly after core's single retry semantics —
    // here one shot must already be Ok or a clean JudgeError, never a panic).
    match judge.evaluate(&reqs[0]) {
        Ok(findings) => {
            eprintln!("local judge findings: {findings:#?}");
            for f in &findings {
                assert!(!f.quote.is_empty());
                assert!((0.0..=1.0).contains(&f.confidence) || f.confidence.is_nan());
            }
        }
        Err(e) => panic!("local judge failed: {e}"),
    }
}
