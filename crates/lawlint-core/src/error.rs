//! Error types. [skeleton — complete]
//!
//! Validation is a product feature: every `LoadError` carries the file path,
//! the field, the given value, and the valid alternatives in plain English,
//! e.g. `builtin/rules/no-em-dash.yaml: severity: "high" is not a severity —
//! use error, warning, or suggestion`.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LoadError {
    /// Filesystem error while reading a package directory or rule file.
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// The file is not valid YAML at all.
    #[error("{file}: invalid YAML: {message}")]
    Yaml { file: String, message: String },

    /// A required field is absent.
    #[error("{file}: missing required field `{field}`")]
    MissingField { file: String, field: String },

    /// A field is present but its value is invalid. `message` must state the
    /// given value and the valid alternatives in plain English.
    #[error("{file}: {field}: {message}")]
    InvalidField {
        file: String,
        field: String,
        message: String,
    },

    /// A regex in a rule failed to compile. Never panic on bad user regexes.
    #[error("{file}: {field}: invalid regex {pattern:?}: {message}")]
    InvalidRegex {
        file: String,
        field: String,
        pattern: String,
        message: String,
    },

    /// The same rule id appears twice (within a package or across a merge).
    #[error("duplicate rule id {id:?}: defined in {first} and {second}")]
    DuplicateId {
        id: String,
        first: String,
        second: String,
    },
}

impl LoadError {
    /// The file (or path) the error is about, for structured error UIs that
    /// pair a message with its source file. For duplicate ids this is the
    /// second definition — the file that introduced the conflict.
    pub fn file(&self) -> &str {
        match self {
            LoadError::Io { path, .. } => path,
            LoadError::Yaml { file, .. }
            | LoadError::MissingField { file, .. }
            | LoadError::InvalidField { file, .. }
            | LoadError::InvalidRegex { file, .. } => file,
            LoadError::DuplicateId { second, .. } => second,
        }
    }

    /// Convenience constructor for the common invalid-field case.
    pub fn invalid_field(
        file: impl Into<String>,
        field: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        LoadError::InvalidField {
            file: file.into(),
            field: field.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum JudgeError {
    /// The backend (model, network, runtime) failed to produce a response.
    #[error("judge backend error: {0}")]
    Backend(String),

    /// The backend responded, but the response could not be parsed as the
    /// expected JSON findings array (after retry).
    #[error("malformed judge response: {0}")]
    MalformedResponse(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_error_display_carries_file_and_field() {
        let e = LoadError::invalid_field(
            "builtin/rules/no-em-dash.yaml",
            "severity",
            "\"high\" is not a severity — use error, warning, or suggestion",
        );
        let s = e.to_string();
        assert!(s.contains("builtin/rules/no-em-dash.yaml"));
        assert!(s.contains("severity"));
        assert!(s.contains("use error, warning, or suggestion"));
    }

    #[test]
    fn missing_field_display() {
        let e = LoadError::MissingField {
            file: "rules/x.yaml".into(),
            field: "rubric".into(),
        };
        assert_eq!(
            e.to_string(),
            "rules/x.yaml: missing required field `rubric`"
        );
    }

    #[test]
    fn invalid_regex_display() {
        let e = LoadError::InvalidRegex {
            file: "rules/x.yaml".into(),
            field: "patterns[0]".into(),
            pattern: "(".into(),
            message: "unclosed group".into(),
        };
        let s = e.to_string();
        assert!(s.contains("rules/x.yaml"));
        assert!(s.contains("patterns[0]"));
        assert!(s.contains("\"(\""));
        assert!(s.contains("unclosed group"));
    }

    #[test]
    fn duplicate_id_display() {
        let e = LoadError::DuplicateId {
            id: "core/no-em-dash".into(),
            first: "a.yaml".into(),
            second: "b.yaml".into(),
        };
        let s = e.to_string();
        assert!(s.contains("core/no-em-dash"));
        assert!(s.contains("a.yaml"));
        assert!(s.contains("b.yaml"));
    }

    #[test]
    fn judge_error_display() {
        assert_eq!(
            JudgeError::Backend("timeout".into()).to_string(),
            "judge backend error: timeout"
        );
        assert_eq!(
            JudgeError::MalformedResponse("not json".into()).to_string(),
            "malformed judge response: not json"
        );
    }
}
