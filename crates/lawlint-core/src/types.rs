//! Core data model. Verbatim from docs/engine-design.md §2. [skeleton — complete]

use serde::{Deserialize, Serialize};

use crate::judge::JudgeStats;

/// Byte offsets into the ORIGINAL source, always.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}

impl TextRange {
    pub fn slice<'a>(&self, text: &'a str) -> &'a str {
        &text[self.start..self.end]
    }
    pub fn contains(&self, other: &TextRange) -> bool {
        other.start >= self.start && other.end <= self.end
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    #[serde(alias = "info")]
    Suggestion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Static,
    Statistical,
    Inferential,
}

/// Prose: paragraph + list-item sentences, excluding citation sentences.
/// Text:  Prose + headings + block quotes + citation sentences. (built-in default)
/// All:   everything including code blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Prose,
    Text,
    All,
}

/// What a rule's findings mean. Detection rules are evidence of AI authorship
/// and aggregate into the human-likeness score; style rules are drafting lint
/// only — they report diagnostics but never move the score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Intent {
    Style,
    #[default]
    Detection,
}

/// Namespaced "package/name", e.g. "core/no-em-dash". Stable forever.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuleId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub rule_id: RuleId,
    pub severity: Severity,
    pub tier: Tier,
    /// Detection findings count toward the score; style findings do not.
    /// Defaults to detection when absent (pre-intent serialized results).
    #[serde(default)]
    pub intent: Intent,
    pub span: TextRange,
    pub message: String,
    /// 1-based; filled by finalize
    pub line: usize,
    /// 1-based, UTF-16 code units; filled by finalize
    pub column: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_column: Option<usize>,
    /// trimmed source line; filled by finalize
    pub excerpt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<u32>,
    /// tier-3 only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fix {
    pub edits: Vec<Edit>,
    pub applicability: Applicability,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edit {
    pub range: TextRange,
    pub replacement: String,
}

/// Tier-3 fixes are ALWAYS MaybeIncorrect. Only MachineApplicable participates in --fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Applicability {
    MachineApplicable,
    MaybeIncorrect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub word_count: usize,
    pub sentence_count: usize,
    /// 0..=100
    pub score: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintResult {
    pub diagnostics: Vec<Diagnostic>,
    pub stats: Stats,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge: Option<JudgeStats>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_range_slice_and_contains() {
        let text = "hello world";
        let r = TextRange { start: 6, end: 11 };
        assert_eq!(r.slice(text), "world");
        let outer = TextRange { start: 0, end: 11 };
        assert!(outer.contains(&r));
        assert!(!r.contains(&outer));
        assert!(r.contains(&r));
    }

    #[test]
    fn severity_serde_lowercase_with_info_alias() {
        assert_eq!(
            serde_json::to_string(&Severity::Error).unwrap(),
            "\"error\""
        );
        assert_eq!(
            serde_json::to_string(&Severity::Warning).unwrap(),
            "\"warning\""
        );
        assert_eq!(
            serde_json::to_string(&Severity::Suggestion).unwrap(),
            "\"suggestion\""
        );
        // Legacy "info" must deserialize to Suggestion.
        let s: Severity = serde_json::from_str("\"info\"").unwrap();
        assert_eq!(s, Severity::Suggestion);
        let s: Severity = serde_json::from_str("\"suggestion\"").unwrap();
        assert_eq!(s, Severity::Suggestion);
    }

    #[test]
    fn tier_and_scope_serde_lowercase() {
        assert_eq!(
            serde_json::to_string(&Tier::Inferential).unwrap(),
            "\"inferential\""
        );
        assert_eq!(serde_json::to_string(&Scope::Prose).unwrap(), "\"prose\"");
        let sc: Scope = serde_json::from_str("\"all\"").unwrap();
        assert_eq!(sc, Scope::All);
    }

    #[test]
    fn rule_id_is_transparent() {
        let id = RuleId("core/no-em-dash".to_string());
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"core/no-em-dash\"");
        let back: RuleId = serde_json::from_str("\"core/no-em-dash\"").unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn applicability_kebab_case() {
        assert_eq!(
            serde_json::to_string(&Applicability::MachineApplicable).unwrap(),
            "\"machine-applicable\""
        );
        assert_eq!(
            serde_json::to_string(&Applicability::MaybeIncorrect).unwrap(),
            "\"maybe-incorrect\""
        );
    }

    #[test]
    fn diagnostic_json_field_name_contract() {
        let d = Diagnostic {
            rule_id: RuleId("core/no-em-dash-overuse".into()),
            severity: Severity::Error,
            tier: Tier::Static,
            intent: Intent::Detection,
            span: TextRange { start: 0, end: 1 },
            message: "m".into(),
            line: 1,
            column: 1,
            end_line: Some(1),
            end_column: Some(2),
            excerpt: "x".into(),
            suggestion: None,
            weight: None,
            confidence: None,
            fix: None,
        };
        let v: serde_json::Value = serde_json::to_value(&d).unwrap();
        assert!(v.get("ruleId").is_some());
        assert!(v.get("endLine").is_some());
        assert!(v.get("endColumn").is_some());
        assert_eq!(v["severity"], "error");
        assert_eq!(v["intent"], "detection");
        // None options are omitted entirely.
        assert!(v.get("suggestion").is_none());
        assert!(v.get("confidence").is_none());
    }

    #[test]
    fn intent_serde_lowercase_and_defaults_to_detection() {
        assert_eq!(serde_json::to_string(&Intent::Style).unwrap(), "\"style\"");
        assert_eq!(
            serde_json::to_string(&Intent::Detection).unwrap(),
            "\"detection\""
        );
        // Pre-intent serialized diagnostics deserialize as detection.
        let json = r#"{"ruleId":"core/x","severity":"error","tier":"static",
            "span":{"start":0,"end":1},"message":"m","line":1,"column":1,
            "excerpt":"x"}"#;
        let d: Diagnostic = serde_json::from_str(json).unwrap();
        assert_eq!(d.intent, Intent::Detection);
    }

    #[test]
    fn stats_json_field_names() {
        let s = Stats {
            word_count: 10,
            sentence_count: 2,
            score: 100,
        };
        let v: serde_json::Value = serde_json::to_value(&s).unwrap();
        assert!(v.get("wordCount").is_some());
        assert!(v.get("sentenceCount").is_some());
        assert_eq!(v["score"], 100);
    }
}
