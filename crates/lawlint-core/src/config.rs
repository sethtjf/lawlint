//! Lint options. Verbatim from docs/engine-design.md §8. [skeleton — complete]

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::types::Severity;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LintOptions {
    pub enable: Option<Vec<String>>,
    pub disable: Option<Vec<String>>,
    pub severity: Option<HashMap<String, Severity>>,
    pub thresholds: Option<HashMap<String, f64>>,
    pub markdown: Option<bool>,
    /// Consumed by CLI/desktop, ignored by core `lint()`.
    pub rule_dirs: Option<Vec<String>>,
    pub judge: Option<JudgeOptions>,
    /// AI model preferences (written by `lawlint init`); consumed by
    /// CLI/desktop, ignored by core `lint()`.
    pub ai: Option<AiOptions>,
}

impl LintOptions {
    /// The preferred model spec for the AI `feature` ("judge", "learn", …):
    /// the per-feature override, else the default `ai.model`. The legacy
    /// `judge.model` outranks both for the judge — callers resolve that.
    pub fn ai_model(&self, feature: &str) -> Option<String> {
        let ai = self.ai.as_ref()?;
        ai.features
            .as_ref()
            .and_then(|features| features.get(feature).cloned())
            .or_else(|| ai.model.clone())
    }
}

/// The `ai` config section: which model powers AI features. Specs share the
/// judge's grammar (`local[:<hf-repo>[#<gguf>]]`, `anthropic:<model>`,
/// `openai:<base-url>#<model>`, `foundry:<deployment>`). API keys never live
/// here — they stay in the user-level credential store or the environment.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiOptions {
    /// Default model spec for every AI feature.
    pub model: Option<String>,
    /// Per-feature overrides keyed by feature name ("judge", "learn", …).
    pub features: Option<HashMap<String, String>>,
    /// `true` = the user has acknowledged the local-model constraints
    /// (multi-GB download, slower inference, measurably lower quality —
    /// docs/eval-corpus.md). Written by `lawlint init`'s advanced local
    /// path; while unset, consumers print a one-line constraints notice
    /// whenever a `local:` spec is used.
    pub local_acknowledged: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JudgeOptions {
    pub enabled: Option<bool>,
    pub model: Option<String>,
    pub floor: Option<f32>,
    /// Cap on tokens the backend may generate per chunk. Reasoning models
    /// spend this budget on hidden thinking before emitting the findings
    /// array, so a cap sized for the array alone truncates them to empty
    /// output and fails every chunk closed. `None` uses the backend default.
    pub max_tokens: Option<usize>,
    /// How many judge requests may be in flight at once. `None` uses the
    /// backend default; local in-process models ignore it and stay at 1.
    pub concurrency: Option<usize>,
    /// Max chars of document text per judge request. Larger units mean fewer
    /// requests and more context per call, at the cost of coarser cache
    /// invalidation — an edit anywhere in a unit re-runs that whole unit.
    /// `None` uses the model profile's budget.
    pub context_chars: Option<usize>,
    /// One request per rule (`true`) instead of one request per unit carrying
    /// every rubric. Per-rule requests give each rule the model's full
    /// attention and their own cache entry; `None` uses the model profile.
    pub per_rule: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_all_none() {
        let o = LintOptions::default();
        assert!(o.enable.is_none());
        assert!(o.disable.is_none());
        assert!(o.severity.is_none());
        assert!(o.thresholds.is_none());
        assert!(o.markdown.is_none());
        assert!(o.rule_dirs.is_none());
        assert!(o.judge.is_none());
    }

    #[test]
    fn deserializes_camel_case_with_missing_fields() {
        let o: LintOptions = serde_json::from_str(
            r#"{"ruleDirs": ["extra"], "markdown": true, "judge": {"floor": 0.7}}"#,
        )
        .unwrap();
        assert_eq!(o.rule_dirs, Some(vec!["extra".to_string()]));
        assert_eq!(o.markdown, Some(true));
        let j = o.judge.unwrap();
        assert_eq!(j.floor, Some(0.7));
        assert!(j.enabled.is_none());
        assert!(j.model.is_none());
    }

    #[test]
    fn severity_map_accepts_legacy_info() {
        let o: LintOptions =
            serde_json::from_str(r#"{"severity": {"no-hedging": "info"}}"#).unwrap();
        assert_eq!(
            o.severity.unwrap().get("no-hedging"),
            Some(&Severity::Suggestion)
        );
    }

    #[test]
    fn empty_object_deserializes() {
        let o: LintOptions = serde_json::from_str("{}").unwrap();
        assert!(o.enable.is_none());
        assert!(o.ai.is_none());
    }

    #[test]
    fn ai_section_deserializes_camel_case() {
        let o: LintOptions = serde_json::from_str(
            r#"{"ai": {"model": "local", "features": {"judge": "anthropic:m"}}}"#,
        )
        .unwrap();
        let ai = o.ai.as_ref().unwrap();
        assert_eq!(ai.model.as_deref(), Some("local"));
        assert_eq!(
            ai.features.as_ref().unwrap().get("judge").unwrap(),
            "anthropic:m"
        );
        // Absent acknowledgment field stays None (notice keeps printing).
        assert!(ai.local_acknowledged.is_none());

        let o: LintOptions =
            serde_json::from_str(r#"{"ai": {"model": "local", "localAcknowledged": true}}"#)
                .unwrap();
        assert_eq!(o.ai.unwrap().local_acknowledged, Some(true));
    }

    #[test]
    fn ai_model_prefers_feature_override_then_default() {
        let mut o = LintOptions::default();
        assert_eq!(o.ai_model("judge"), None);

        o.ai = Some(AiOptions {
            model: Some("local".into()),
            ..Default::default()
        });
        assert_eq!(o.ai_model("judge"), Some("local".into()));

        o.ai = Some(AiOptions {
            model: Some("local".into()),
            features: Some(
                [("judge".to_string(), "anthropic:m".to_string())]
                    .into_iter()
                    .collect(),
            ),
            ..Default::default()
        });
        assert_eq!(o.ai_model("judge"), Some("anthropic:m".into()));
        // Unknown feature falls back to the default model.
        assert_eq!(o.ai_model("learn"), Some("local".into()));
    }
}
