//! Metric plumbing for the opt-in tier-3 judged evaluation (#39 part 2).
//! Pure aggregation over parse-layer counts — no judge backend here, so the
//! math is testable without model downloads. The backend-driving harness is
//! the feature-gated `judged_report` binary.

use crate::Label;
use lawlint_core::JudgeFinding;

/// Verdict-discipline tallies across one judged evaluation run, fed one
/// sample at a time from the parse layer's kept/dropped partition.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerdictDiscipline {
    /// All model-emitted findings (kept + guard-dropped), both labels.
    pub emitted: usize,
    /// Findings dropped by the verdict-polarity guard, both labels.
    pub dropped_negative: usize,
    /// Guard-surviving findings on human (clean) samples.
    pub human_kept: usize,
    /// Guard-surviving findings on human samples whose explanation still
    /// reads as a negation ([`looks_negational`]) — potential guard misses.
    pub human_kept_negational: usize,
}

impl VerdictDiscipline {
    pub fn add_sample(&mut self, label: Label, kept: &[JudgeFinding], dropped_negative: usize) {
        self.emitted += kept.len() + dropped_negative;
        self.dropped_negative += dropped_negative;
        if label == Label::Human {
            self.human_kept += kept.len();
            self.human_kept_negational += kept
                .iter()
                .filter(|finding| looks_negational(finding))
                .count();
        }
    }

    /// Raw negation-emission rate: how often the model emits its pass
    /// verdict as a finding (guard-dropped / all model-emitted findings).
    pub fn negation_emission_rate(&self) -> f64 {
        ratio(self.dropped_negative, self.emitted)
    }

    /// Surviving-negation rate on human samples: guard survivors whose
    /// explanation still reads as a negation, over all survivors on human
    /// samples. The #39 target is <5% post-guard.
    pub fn surviving_negation_rate(&self) -> f64 {
        ratio(self.human_kept_negational, self.human_kept)
    }
}

/// Verdict-negation heuristic for the surviving-negation measurement: does
/// the explanation negate the *violation* (a pass verdict emitted as a
/// finding)? Deliberately wider than the parse-time polarity guard — guard
/// misses must show up here — but verdict-directed, not any-negation: the
/// rubrics themselves phrase violations with negations ("hedges a claim
/// without saying what is uncertain"), so a bag-of-negation-words match
/// would count nearly every rubric-echoing explanation. Still an
/// over-approximation, so the rate is an upper bound.
pub fn looks_negational(finding: &JudgeFinding) -> bool {
    // Normalize: lowercase, punctuation to spaces, single-space, padded so
    // every phrase match is word-bounded ("doesn't" → "doesn t").
    let normalized: String = finding
        .explanation
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let padded = format!(
        " {} ",
        normalized.split_whitespace().collect::<Vec<_>>().join(" ")
    );

    const VERDICT_PHRASES: &[&str] = &[
        " does not flag ",
        " do not flag ",
        " doesn t flag ",
        " nothing to flag ",
        " does not violate ",
        " do not violate ",
        " doesn t violate ",
        " not violate ",
        " not violated ",
        " no violation ",
        " no violations ",
        " not a violation ",
        " complies ",
        " compliant ",
        " satisfies ",
        " adheres ",
        " conforms ",
        " no issue ",
        " no issues ",
        " no problem ",
        " no problems ",
        " is acceptable ",
        " is fine ",
    ];
    if VERDICT_PHRASES.iter().any(|phrase| padded.contains(phrase)) {
        return true;
    }

    // Negations whose object is the rule's own name, wider than the guard:
    // the full phrase ("empty hedge") and its head word alone ("hedge",
    // which also covers verb forms like "does not hedge").
    let rule_phrase = finding
        .rule
        .rsplit('/')
        .next()
        .unwrap_or(&finding.rule)
        .replace('-', " ")
        .to_lowercase();
    let head = rule_phrase
        .rsplit(' ')
        .next()
        .unwrap_or_default()
        .to_string();
    let mut objects = vec![rule_phrase];
    if !head.is_empty() && objects[0] != head {
        objects.push(head);
    }
    objects
        .iter()
        .filter(|object| !object.trim().is_empty())
        .any(|object| {
            [
                format!(" no {object} "),
                format!(" not a {object} "),
                format!(" not an {object} "),
                format!(" not {object} "),
                format!(" does not {object} "),
                format!(" doesn t {object} "),
            ]
            .iter()
            .any(|pattern| padded.contains(pattern.as_str()))
        })
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(explanation: &str) -> JudgeFinding {
        JudgeFinding {
            rule: "core/empty-hedge".to_string(),
            quote: "quote".to_string(),
            explanation: explanation.to_string(),
            confidence: 0.9,
            suggested_rewrite: None,
        }
    }

    #[test]
    fn rates_from_canned_negation_tallies() {
        let mut discipline = VerdictDiscipline::default();
        // Human sample: one genuine-looking survivor, one negational
        // survivor, two guard drops.
        discipline.add_sample(
            Label::Human,
            &[
                finding("Stacked qualifiers on a single claim."),
                finding("The sentence does not hedge anything."),
            ],
            2,
        );
        // AI sample: two survivors, one drop; survivors on AI samples never
        // count toward the human surviving-negation rate.
        discipline.add_sample(
            Label::Ai,
            &[
                finding("Hedge that adds no information."),
                finding("Padding."),
            ],
            1,
        );
        assert_eq!(discipline.emitted, 7);
        assert_eq!(discipline.dropped_negative, 3);
        assert_eq!(discipline.human_kept, 2);
        assert_eq!(discipline.human_kept_negational, 1);
        assert!((discipline.negation_emission_rate() - 3.0 / 7.0).abs() < f64::EPSILON);
        assert!((discipline.surviving_negation_rate() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn empty_run_rates_are_zero_not_nan() {
        let discipline = VerdictDiscipline::default();
        assert_eq!(discipline.negation_emission_rate(), 0.0);
        assert_eq!(discipline.surviving_negation_rate(), 0.0);
    }

    #[test]
    fn drops_alone_count_toward_emission_but_not_survival() {
        let mut discipline = VerdictDiscipline::default();
        discipline.add_sample(Label::Human, &[], 4);
        assert_eq!(discipline.emitted, 4);
        assert!((discipline.negation_emission_rate() - 1.0).abs() < f64::EPSILON);
        assert_eq!(discipline.human_kept, 0);
        assert_eq!(discipline.surviving_negation_rate(), 0.0);
    }

    #[test]
    fn negational_heuristic_matches_verdicts_the_guard_can_miss() {
        // Verdict-shaped survivors wider than the guard's narrow patterns.
        for explanation in [
            "The text does not hedge here.",
            "The sentence doesn t hedge a claim.",
            "This passage contains no hedge at all.",
            "The paragraph is acceptable as written.",
            "The text is compliant with the rule.",
            "This clause would not violate the rule.",
        ] {
            assert!(looks_negational(&finding(explanation)), "{explanation}");
        }
    }

    #[test]
    fn negational_heuristic_ignores_rubric_echo_negations() {
        // The rubrics phrase violations WITH negations; explanations echoing
        // them assert a violation and must not count as verdicts. These are
        // verbatim from the local-Qwen train run.
        for explanation in [
            "The sentence hedges a claim without saying what is uncertain or why.",
            "The sentence uses an empty hedge by stating the uncertainty without providing the reason.",
            "The sentence repeats the point of the previous sentence in different words without adding new information.",
            "This hedge adds no information.",
            "Vague qualifiers stack on one claim.",
            "",
        ] {
            assert!(!looks_negational(&finding(explanation)), "{explanation}");
        }
    }

    #[test]
    fn negational_heuristic_uses_the_finding_rule_name() {
        let mut padded = finding("There is no padded elaboration in this text.");
        padded.rule = "core/padded-elaboration".to_string();
        assert!(looks_negational(&padded));
        // Head word alone: "no elaboration".
        padded.explanation = "The paragraph contains no elaboration.".to_string();
        assert!(looks_negational(&padded));
        // A different rule's name is not this finding's verdict object.
        padded.explanation = "There is no empty hedge here.".to_string();
        assert!(!looks_negational(&padded));
    }
}
