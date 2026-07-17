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
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JudgeOptions {
    pub enabled: Option<bool>,
    pub model: Option<String>,
    pub floor: Option<f32>,
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
    }
}
