//! YAML rule parsing + validation, with optional Markdown skill references. [agent D]
//!
//! Package = directory: `style.yaml` (`name`, `version`, optional
//! `description`) + `rules/*.yaml`, one rule per file. Built-in package
//! embedded via `include_dir!` (see registry.rs). Inferential YAML rules may
//! reference a Claude Code-style Markdown skill file.
//!
//! Validation is a product feature: errors must carry file path, field, given
//! value, and valid alternatives in plain English (see `LoadError`).
//! Enum-ish fields are kept as raw `String`s here so validation can produce
//! those messages instead of opaque serde errors.

use serde::Deserialize;
use std::path::Path;

use crate::engines::statistical::{Direction, Metric};
use crate::error::LoadError;
use crate::judge::Granularity;
use crate::rule::RuleExample;
use crate::types::{Intent, Scope, Severity, Tier};

/// `style.yaml` package manifest.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageManifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// One `rules/*.yaml` file, capturing the full schema of design doc §6.
/// Raw (pre-validation) representation; `RuleSet` stores validated defs.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleDef {
    /// Bare name; full id becomes `<package>/<id>`.
    pub id: String,
    /// phrase | leading | density | statistical | inferential
    pub engine: String,
    /// prose | text | all (default: text)
    #[serde(default)]
    pub scope: Option<String>,
    /// error | warning | suggestion (accepts legacy "info")
    #[serde(default)]
    pub severity: Option<String>,
    /// style | detection (default: detection). Style rules lint but never
    /// move the human-likeness score.
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub rationale: Option<String>,
    /// Defaults to `https://lawlint.com/rules/<id>`.
    #[serde(default)]
    pub docs: Option<String>,
    /// Default message.
    #[serde(default)]
    pub message: Option<String>,
    /// Single `{bad, good}` object or a list of them.
    #[serde(default, deserialize_with = "one_or_many_examples")]
    pub examples: Vec<RuleExample>,
    /// phrase/leading/density; bare string or object form.
    #[serde(default)]
    pub patterns: Vec<PatternDef>,
    /// phrase only.
    #[serde(default)]
    pub allow_context: Option<AllowContextDef>,
    /// density: matches per 1000 words. statistical document-level metrics:
    /// the flag boundary for `direction`.
    #[serde(default)]
    pub threshold: Option<f64>,
    /// statistical only (non-exhaustive; unknown metric = load error). Two
    /// per-sentence metrics (sentence-length, repetitive-openers) plus the
    /// document-level metrics of `engines::statistical::Metric`.
    #[serde(default)]
    pub metric: Option<String>,
    /// statistical only, e.g. `{ max_words: 45 }` / `{ run_length: 3 }`.
    #[serde(default)]
    pub params: Option<std::collections::HashMap<String, f64>>,
    /// statistical document-level metrics only: above | below — which side of
    /// `threshold` the measured value must fall on to flag.
    #[serde(default)]
    pub direction: Option<String>,
    /// inferential only: sentence | paragraph | document.
    #[serde(default)]
    pub granularity: Option<String>,
    /// inferential only.
    #[serde(default)]
    pub rubric: Option<String>,
    /// inferential only; >= 3 required.
    #[serde(default)]
    pub flag_examples: Vec<String>,
    /// inferential only; >= 3 required.
    #[serde(default)]
    pub pass_examples: Vec<String>,
    /// inferential only: path to a Claude Code-style Markdown skill file.
    #[serde(default)]
    pub skill: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillFrontmatter {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    granularity: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    intent: Option<String>,
    #[serde(default)]
    docs: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    rationale: Option<String>,
    #[serde(default)]
    flag_examples: Vec<String>,
    #[serde(default)]
    pass_examples: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct SkillContent {
    pub(crate) description: Option<String>,
    pub(crate) severity: Option<String>,
    pub(crate) granularity: Option<String>,
    pub(crate) scope: Option<String>,
    pub(crate) intent: Option<String>,
    pub(crate) docs: Option<String>,
    pub(crate) message: Option<String>,
    pub(crate) rationale: Option<String>,
    pub(crate) rubric: String,
    pub(crate) flag_examples: Vec<String>,
    pub(crate) pass_examples: Vec<String>,
}

/// A `patterns:` list item — bare string or detailed object.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PatternDef {
    Bare(String),
    Detailed {
        pattern: String,
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
        suggestion: Option<String>,
        /// Replacement string → MachineApplicable single-edit Fix.
        #[serde(default)]
        fix: Option<String>,
    },
}

impl PatternDef {
    pub fn pattern(&self) -> &str {
        match self {
            PatternDef::Bare(p) => p,
            PatternDef::Detailed { pattern, .. } => pattern,
        }
    }
}

/// `allow_context: { pattern, window }` (phrase only): expand match by
/// `window` bytes each side (clamped to char boundaries); if `pattern`
/// matches the expanded slice, skip the match.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllowContextDef {
    pub pattern: String,
    pub window: usize,
}

/// `examples:` accepts a single `{bad, good}` object or a list of them.
fn one_or_many_examples<'de, D>(deserializer: D) -> Result<Vec<RuleExample>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(RuleExample),
        Many(Vec<RuleExample>),
    }
    Ok(match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(x) => vec![x],
        OneOrMany::Many(xs) => xs,
    })
}

// ---- Enum-ish string helpers (post-validation these never fail) ---------

pub(crate) fn parse_severity(s: &str) -> Option<Severity> {
    match s {
        "error" => Some(Severity::Error),
        "warning" => Some(Severity::Warning),
        "suggestion" | "info" => Some(Severity::Suggestion),
        _ => None,
    }
}

pub(crate) fn parse_intent(s: &str) -> Option<Intent> {
    match s {
        "style" => Some(Intent::Style),
        "detection" => Some(Intent::Detection),
        _ => None,
    }
}

pub(crate) fn parse_scope(s: &str) -> Option<Scope> {
    match s {
        "prose" => Some(Scope::Prose),
        "text" => Some(Scope::Text),
        "all" => Some(Scope::All),
        _ => None,
    }
}

pub(crate) fn parse_granularity(s: &str) -> Option<Granularity> {
    match s {
        "sentence" => Some(Granularity::Sentence),
        "paragraph" => Some(Granularity::Paragraph),
        "document" => Some(Granularity::Document),
        _ => None,
    }
}

/// Derived tier: phrase/leading → static; density/statistical → statistical;
/// inferential → inferential. Only call on a validated engine name.
pub(crate) fn engine_tier(engine: &str) -> Option<Tier> {
    match engine {
        "phrase" | "leading" => Some(Tier::Static),
        "density" | "statistical" => Some(Tier::Statistical),
        "inferential" => Some(Tier::Inferential),
        _ => None,
    }
}

const ENGINES: [&str; 5] = ["phrase", "leading", "density", "statistical", "inferential"];

/// Turn a serde_yaml error into the friendliest `LoadError` we can manage:
/// missing required fields and unknown fields get their own variants; anything
/// else is surfaced as invalid YAML with serde's message.
fn map_serde_error(file: &str, err: serde_yaml::Error) -> LoadError {
    let msg = err.to_string();
    if let Some(rest) = msg.strip_prefix("missing field `") {
        if let Some(field) = rest.split('`').next() {
            return LoadError::MissingField {
                file: file.to_string(),
                field: field.to_string(),
            };
        }
    }
    if msg.starts_with("unknown field `") {
        let field = msg.split('`').nth(1).unwrap_or("?").to_string();
        return LoadError::invalid_field(file, field, msg);
    }
    LoadError::Yaml {
        file: file.to_string(),
        message: msg,
    }
}

fn compile_regex(file: &str, field: &str, pattern: &str) -> Result<(), LoadError> {
    regex::Regex::new(pattern)
        .map(|_| ())
        .map_err(|e| LoadError::InvalidRegex {
            file: file.to_string(),
            field: field.to_string(),
            pattern: pattern.to_string(),
            message: e.to_string(),
        })
}

pub fn parse_rule_def(file: &str, yaml: &str) -> Result<RuleDef, LoadError> {
    serde_yaml::from_str(yaml).map_err(|e| map_serde_error(file, e))
}

/// Parse + validate a single rule file. `file` is used for error context.
pub fn parse_rule(file: &str, yaml: &str) -> Result<RuleDef, LoadError> {
    let def = parse_rule_def(file, yaml)?;
    if def.skill.is_some() {
        return Err(LoadError::invalid_field(
            file,
            "skill",
            "skill references must be loaded from a rule file",
        ));
    }
    validate_rule(file, &def)?;
    Ok(def)
}

/// Parse a YAML rule file and resolve an optional skill reference relative to
/// that file's location.
pub fn parse_rule_file(path: &Path) -> Result<RuleDef, LoadError> {
    let file = path.display().to_string();
    let markdown = std::fs::read_to_string(path).map_err(|source| LoadError::Io {
        path: file.clone(),
        source,
    })?;
    let def = parse_rule_def(&file, &markdown)?;
    let Some(skill) = def.skill.clone() else {
        validate_rule(&file, &def)?;
        return Ok(def);
    };
    let skill_path = path.parent().unwrap_or_else(|| Path::new("")).join(skill);
    let skill_file = skill_path.display().to_string();
    let skill_markdown = std::fs::read_to_string(&skill_path).map_err(|source| LoadError::Io {
        path: skill_file.clone(),
        source,
    })?;
    parse_rule_with_skill(&file, &markdown, &skill_file, &skill_markdown)
}

pub(crate) fn parse_rule_with_skill(
    file: &str,
    yaml: &str,
    skill_file: &str,
    markdown: &str,
) -> Result<RuleDef, LoadError> {
    let mut def = parse_rule_def(file, yaml)?;
    if def.skill.is_none() {
        return Err(LoadError::invalid_field(
            file,
            "skill",
            "a skill file can only be merged when the rule declares skill",
        ));
    }
    if def.engine != "inferential" {
        return Err(LoadError::invalid_field(
            file,
            "skill",
            format!(
                "skill references only apply to inferential rules — this rule uses the {} engine",
                def.engine
            ),
        ));
    }
    if def.rubric.is_some() {
        return Err(LoadError::invalid_field(
            file,
            "rubric",
            "a rule cannot set both rubric and skill — use the skill file as the rubric source",
        ));
    }
    let skill = parse_skill_content(skill_file, markdown)?;
    def.description = def.description.or(skill.description);
    def.severity = def.severity.or(skill.severity);
    def.granularity = def.granularity.or(skill.granularity);
    def.scope = def.scope.or(skill.scope);
    def.intent = def.intent.or(skill.intent);
    def.docs = def.docs.or(skill.docs);
    def.message = def.message.or(skill.message);
    def.rationale = def.rationale.or(skill.rationale);
    def.rubric = Some(skill.rubric);
    if def.flag_examples.is_empty() {
        def.flag_examples = skill.flag_examples;
    }
    if def.pass_examples.is_empty() {
        def.pass_examples = skill.pass_examples;
    }
    validate_rule(file, &def)?;
    Ok(def)
}

pub(crate) fn parse_skill_content(file: &str, markdown: &str) -> Result<SkillContent, LoadError> {
    let mut lines = markdown.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Err(LoadError::invalid_field(
            file,
            "frontmatter",
            "a skill file must start with YAML frontmatter between --- fences",
        ));
    }
    let mut frontmatter = String::new();
    let mut found_end = false;
    for line in &mut lines {
        if line.trim() == "---" {
            found_end = true;
            break;
        }
        frontmatter.push_str(line);
        frontmatter.push('\n');
    }
    if !found_end {
        return Err(LoadError::invalid_field(
            file,
            "frontmatter",
            "missing closing --- fence",
        ));
    }
    let fm: SkillFrontmatter =
        serde_yaml::from_str(&frontmatter).map_err(|e| map_serde_error(file, e))?;
    let body = lines.collect::<Vec<_>>();
    let (rubric, body_flags, body_passes) = extract_skill_sections(&body);
    Ok(SkillContent {
        description: fm.description,
        severity: fm.severity,
        granularity: fm.granularity,
        scope: fm.scope,
        intent: fm.intent,
        docs: fm.docs,
        message: fm.message,
        rationale: fm.rationale,
        rubric,
        flag_examples: if fm.flag_examples.is_empty() {
            body_flags
        } else {
            fm.flag_examples
        },
        pass_examples: if fm.pass_examples.is_empty() {
            body_passes
        } else {
            fm.pass_examples
        },
    })
}

fn extract_skill_sections(lines: &[&str]) -> (String, Vec<String>, Vec<String>) {
    let mut rubric = Vec::new();
    let mut flags = Vec::new();
    let mut passes = Vec::new();
    let mut section: Option<&str> = None;
    for line in lines {
        if let Some(title) = line.strip_prefix("## ") {
            let normalized = title.trim().to_ascii_lowercase();
            section = match normalized.as_str() {
                "flag examples" => Some("flags"),
                "pass examples" => Some("passes"),
                _ => None,
            };
            if section.is_none() {
                rubric.push(*line);
            }
            continue;
        }
        if let Some(kind) = section {
            let item = line
                .trim()
                .strip_prefix("- ")
                .or_else(|| line.trim().strip_prefix("* "))
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if let Some(item) = item {
                match kind {
                    "flags" => flags.push(item.to_string()),
                    "passes" => passes.push(item.to_string()),
                    _ => unreachable!(),
                }
            }
        } else {
            rubric.push(*line);
        }
    }
    (rubric.join("\n").trim().to_string(), flags, passes)
}

fn validate_rule(file: &str, def: &RuleDef) -> Result<(), LoadError> {
    // id: a bare name; the package prefix is added by the registry.
    let id = def.id.trim();
    if id.is_empty() || id.contains('/') || id.contains(char::is_whitespace) {
        return Err(LoadError::invalid_field(
            file,
            "id",
            format!(
                "{:?} is not a valid rule name — use a bare name like no-em-dash \
                 (no slashes or spaces; the package prefix is added automatically)",
                def.id
            ),
        ));
    }

    // engine
    if !ENGINES.contains(&def.engine.as_str()) {
        return Err(LoadError::invalid_field(
            file,
            "engine",
            format!(
                "{:?} is not an engine — use phrase, leading, density, statistical, or inferential",
                def.engine
            ),
        ));
    }

    // scope / severity / granularity values
    if let Some(scope) = &def.scope {
        if parse_scope(scope).is_none() {
            return Err(LoadError::invalid_field(
                file,
                "scope",
                format!("{scope:?} is not a scope — use prose, text, or all"),
            ));
        }
    }
    if let Some(severity) = &def.severity {
        if parse_severity(severity).is_none() {
            return Err(LoadError::invalid_field(
                file,
                "severity",
                format!("{severity:?} is not a severity — use error, warning, or suggestion"),
            ));
        }
    }
    if let Some(intent) = &def.intent {
        if parse_intent(intent).is_none() {
            return Err(LoadError::invalid_field(
                file,
                "intent",
                format!("{intent:?} is not an intent — use style or detection"),
            ));
        }
    }
    if let Some(granularity) = &def.granularity {
        if parse_granularity(granularity).is_none() {
            return Err(LoadError::invalid_field(
                file,
                "granularity",
                format!(
                    "{granularity:?} is not a granularity — use sentence, paragraph, or document"
                ),
            ));
        }
    }

    // allow_context is a phrase-engine feature.
    if def.allow_context.is_some() && def.engine != "phrase" {
        return Err(LoadError::invalid_field(
            file,
            "allow_context",
            format!(
                "allow_context only applies to phrase rules — this rule uses the {} engine",
                def.engine
            ),
        ));
    }

    // direction is a statistical-engine feature (per-metric fit is checked in
    // the statistical arm below, after the metric itself validates).
    if def.direction.is_some() && def.engine != "statistical" {
        return Err(LoadError::invalid_field(
            file,
            "direction",
            format!(
                "direction only applies to statistical rules — this rule uses the {} engine",
                def.engine
            ),
        ));
    }

    match def.engine.as_str() {
        "phrase" => {
            if def.patterns.is_empty() {
                return Err(LoadError::invalid_field(
                    file,
                    "patterns",
                    "a phrase rule needs at least one pattern — add a patterns list",
                ));
            }
            for (i, p) in def.patterns.iter().enumerate() {
                compile_regex(file, &format!("patterns[{i}]"), p.pattern())?;
            }
            if let Some(ac) = &def.allow_context {
                compile_regex(file, "allow_context.pattern", &ac.pattern)?;
            }
        }
        "leading" => {
            if def.patterns.is_empty() {
                return Err(LoadError::invalid_field(
                    file,
                    "patterns",
                    "a leading rule needs at least one sentence-opener pattern — add a patterns list",
                ));
            }
            // Needles are regex fragments; the engine compiles them as
            // `(?i)^(?:<needle>)`. Compile the same wrapper here so load
            // errors carry the author's fragment, not an engine panic.
            for (i, p) in def.patterns.iter().enumerate() {
                let needle = p.pattern();
                regex::Regex::new(&format!("(?i)^(?:{needle})")).map_err(|e| {
                    LoadError::InvalidRegex {
                        file: file.to_string(),
                        field: format!("patterns[{i}]"),
                        pattern: needle.to_string(),
                        message: e.to_string(),
                    }
                })?;
            }
        }
        "density" => {
            let threshold = def.threshold.ok_or_else(|| LoadError::MissingField {
                file: file.to_string(),
                field: "threshold".to_string(),
            })?;
            if !threshold.is_finite() || threshold < 0.0 {
                return Err(LoadError::invalid_field(
                    file,
                    "threshold",
                    format!(
                        "{threshold} is not a valid threshold — use a non-negative number of \
                         matches per 1000 words, like 8"
                    ),
                ));
            }
            if def.patterns.len() != 1 {
                return Err(LoadError::invalid_field(
                    file,
                    "patterns",
                    format!(
                        "a density rule needs exactly one pattern — found {}",
                        def.patterns.len()
                    ),
                ));
            }
            compile_regex(file, "patterns[0]", def.patterns[0].pattern())?;
        }
        "statistical" => {
            let metric_str = def
                .metric
                .as_deref()
                .ok_or_else(|| LoadError::MissingField {
                    file: file.to_string(),
                    field: "metric".to_string(),
                })?;
            let Some(metric) = Metric::parse(metric_str) else {
                return Err(LoadError::invalid_field(
                    file,
                    "metric",
                    format!(
                        "{metric_str:?} is not a known metric — use {}",
                        Metric::alternatives()
                    ),
                ));
            };
            // Params must be valid HERE: RuleSet::instantiate treats a
            // constructor failure on a validated def as a bug (panic), so
            // mirror StatisticalEngine::from_def's checks.
            let param = |name: &str| def.params.as_ref().and_then(|p| p.get(name)).copied();
            if let Some(run_length) = param("run_length") {
                if !run_length.is_finite() || run_length < 1.0 || run_length.fract() != 0.0 {
                    return Err(LoadError::invalid_field(
                        file,
                        "params.run_length",
                        format!(
                            "{run_length} is not a valid run length — use a whole number of \
                             at least 1"
                        ),
                    ));
                }
            }
            if let Some(max_words) = param("max_words") {
                if !max_words.is_finite() || max_words < 1.0 {
                    return Err(LoadError::invalid_field(
                        file,
                        "params.max_words",
                        format!(
                            "{max_words} is not a valid sentence length — use a positive \
                             number of words like 45"
                        ),
                    ));
                }
            }
            if !metric.is_document_level() && def.direction.is_some() {
                return Err(LoadError::invalid_field(
                    file,
                    "direction",
                    format!("direction only applies to document-level metrics, not {metric_str}"),
                ));
            }
            if metric.is_document_level() {
                let threshold = def.threshold.ok_or_else(|| LoadError::MissingField {
                    file: file.to_string(),
                    field: "threshold".to_string(),
                })?;
                // Unlike density, negative thresholds are legal: cadence
                // autocorrelation lives in [-1, 1].
                if !threshold.is_finite() {
                    return Err(LoadError::invalid_field(
                        file,
                        "threshold",
                        format!("{threshold} is not a valid threshold — use a finite number"),
                    ));
                }
                let direction =
                    def.direction
                        .as_deref()
                        .ok_or_else(|| LoadError::MissingField {
                            file: file.to_string(),
                            field: "direction".to_string(),
                        })?;
                if Direction::parse(direction).is_none() {
                    return Err(LoadError::invalid_field(
                        file,
                        "direction",
                        format!("{direction:?} is not a direction — use above or below"),
                    ));
                }
            }
        }
        "inferential" => {
            if def
                .rubric
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .is_none()
            {
                return Err(LoadError::MissingField {
                    file: file.to_string(),
                    field: "rubric".to_string(),
                });
            }
            if def.flag_examples.len() < 3 {
                return Err(LoadError::invalid_field(
                    file,
                    "flag_examples",
                    format!(
                        "an inferential rule needs at least 3 flag_examples — found {}",
                        def.flag_examples.len()
                    ),
                ));
            }
            if def.pass_examples.len() < 3 {
                return Err(LoadError::invalid_field(
                    file,
                    "pass_examples",
                    format!(
                        "an inferential rule needs at least 3 pass_examples — found {}",
                        def.pass_examples.len()
                    ),
                ));
            }
            if def.severity.as_deref().and_then(parse_severity) == Some(Severity::Error) {
                return Err(LoadError::invalid_field(
                    file,
                    "severity",
                    "\"error\" is too severe for an inferential rule — judge findings are \
                     advisory; use warning or suggestion",
                ));
            }
        }
        _ => unreachable!("engine validated above"),
    }

    Ok(())
}

/// Shared package-name validation for `parse_manifest` and
/// `RuleSet::from_sources`.
pub(crate) fn validate_package_name(file: &str, name: &str) -> Result<(), LoadError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.contains('/') || trimmed.contains(char::is_whitespace) {
        return Err(LoadError::invalid_field(
            file,
            "name",
            format!(
                "{name:?} is not a valid package name — use a bare name like core \
                 (no slashes or spaces)"
            ),
        ));
    }
    Ok(())
}

/// Parse + validate a package manifest (`style.yaml`).
pub fn parse_manifest(file: &str, yaml: &str) -> Result<PackageManifest, LoadError> {
    let manifest: PackageManifest =
        serde_yaml::from_str(yaml).map_err(|e| map_serde_error(file, e))?;
    validate_package_name(file, &manifest.name)?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    const F: &str = "rules/test.yaml";

    fn ok(yaml: &str) -> RuleDef {
        parse_rule(F, yaml).expect("rule should parse")
    }

    fn err(yaml: &str) -> String {
        parse_rule(F, yaml)
            .expect_err("rule should not parse")
            .to_string()
    }

    // ---- happy paths, one per engine kind ------------------------------

    #[test]
    fn phrase_rule_full_schema() {
        let def = ok(r#"
id: no-em-dash
engine: phrase
scope: text
severity: error
description: "Em dashes are a hallmark of AI-generated prose."
rationale: "Humans reach for commas."
message: "Avoid em dashes"
examples:
  - { bad: "It was—frankly—wrong.", good: "It was, frankly, wrong." }
patterns:
  - "—"
  - { pattern: "(?i)\\bdelve\\b", message: "No delving", suggestion: "examine", fix: "examine" }
allow_context: { pattern: '\d\s?–\s?\d', window: 8 }
"#);
        assert_eq!(def.id, "no-em-dash");
        assert_eq!(def.engine, "phrase");
        assert_eq!(def.scope.as_deref(), Some("text"));
        assert_eq!(def.severity.as_deref(), Some("error"));
        assert_eq!(def.examples.len(), 1);
        assert_eq!(def.examples[0].bad, "It was—frankly—wrong.");
        assert_eq!(def.patterns.len(), 2);
        assert_eq!(def.patterns[0].pattern(), "—");
        match &def.patterns[1] {
            PatternDef::Detailed {
                pattern,
                message,
                suggestion,
                fix,
            } => {
                assert_eq!(pattern, "(?i)\\bdelve\\b");
                assert_eq!(message.as_deref(), Some("No delving"));
                assert_eq!(suggestion.as_deref(), Some("examine"));
                assert_eq!(fix.as_deref(), Some("examine"));
            }
            other => panic!("expected detailed pattern, got {other:?}"),
        }
        let ac = def.allow_context.unwrap();
        assert_eq!(ac.pattern, r"\d\s?–\s?\d");
        assert_eq!(ac.window, 8);
    }

    #[test]
    fn leading_rule_minimal() {
        let def = ok(r#"
id: no-throat-clearing
engine: leading
severity: error
message: "Get to the point"
patterns: ["It is important to note that", "Needless to say"]
"#);
        assert_eq!(def.engine, "leading");
        assert_eq!(def.patterns.len(), 2);
        assert!(def.scope.is_none());
    }

    #[test]
    fn density_rule_minimal() {
        let def = ok(r#"
id: no-hedging
engine: density
severity: warning
threshold: 10
patterns: ["(?i)\\b(arguably|perhaps)\\b"]
"#);
        assert_eq!(def.threshold, Some(10.0));
        assert_eq!(def.patterns.len(), 1);
    }

    #[test]
    fn statistical_rule_with_params() {
        let def = ok(r#"
id: sentence-length
engine: statistical
metric: sentence-length
params: { max_words: 45 }
"#);
        assert_eq!(def.metric.as_deref(), Some("sentence-length"));
        assert_eq!(def.params.unwrap().get("max_words"), Some(&45.0));
    }

    #[test]
    fn inferential_rule_full() {
        let def = ok(r#"
id: empty-hedge
engine: inferential
severity: warning
granularity: sentence
rubric: >-
  Flag hedges that carry no information about actual uncertainty.
flag_examples: ["a", "b", "c"]
pass_examples: ["x", "y", "z"]
"#);
        assert_eq!(def.granularity.as_deref(), Some("sentence"));
        assert!(def.rubric.unwrap().contains("Flag hedges"));
        assert_eq!(def.flag_examples.len(), 3);
        assert_eq!(def.pass_examples.len(), 3);
    }

    #[test]
    fn skill_file_extracts_frontmatter_and_examples() {
        let skill = parse_skill_content(
            "rules/hedging.md",
            r#"---
description: Flags empty hedges.
severity: warning
granularity: paragraph
scope: prose
---
# Rubric

Flag a hedge when it adds no useful uncertainty.

## Flag examples
- Perhaps this is good.
- It may arguably work.
- It could possibly pass.

## Pass examples
- The result may vary with jurisdiction.
- Perhaps the witness misunderstood the question.
- It may be true if the contract says so.
"#,
        )
        .unwrap();
        assert_eq!(skill.granularity.as_deref(), Some("paragraph"));
        assert!(skill.rubric.contains("adds no useful"));
        assert!(!skill.rubric.contains("Flag examples"));
        assert_eq!(skill.flag_examples.len(), 3);
        assert_eq!(skill.pass_examples.len(), 3);
    }

    #[test]
    fn yaml_rule_merges_skill_with_yaml_precedence() {
        let def = parse_rule_with_skill(
            "rules/my-rule.yaml",
            "id: my-rule\nengine: inferential\nseverity: warning\nskill: ./my-rule.md\ndescription: YAML description\nflag_examples: [yaml-a, yaml-b, yaml-c]\n",
            "rules/my-rule.md",
            "---\ndescription: Skill description\nmessage: Skill message\nflag_examples: [skill-a, skill-b, skill-c]\npass_examples: [x, y, z]\n---\nSkill rubric.\n",
        )
        .unwrap();
        assert_eq!(def.description.as_deref(), Some("YAML description"));
        assert_eq!(def.message.as_deref(), Some("Skill message"));
        assert_eq!(def.rubric.as_deref(), Some("Skill rubric."));
        assert_eq!(def.flag_examples, ["yaml-a", "yaml-b", "yaml-c"]);
        assert_eq!(def.pass_examples, ["x", "y", "z"]);
    }

    #[test]
    fn yaml_rule_rejects_rubric_and_skill_together() {
        let e = parse_rule_with_skill(
            "rules/my-rule.yaml",
            "id: my-rule\nengine: inferential\nrubric: YAML rubric\nskill: ./my-rule.md\n",
            "rules/my-rule.md",
            "---\nflag_examples: [a, b, c]\npass_examples: [x, y, z]\n---\nSkill rubric.\n",
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("rules/my-rule.yaml: rubric"), "{e}");
    }

    #[test]
    fn yaml_rule_skill_validation_uses_yaml_file_context() {
        let e = parse_rule_with_skill(
            "rules/my-rule.yaml",
            "id: my-rule\nengine: inferential\nskill: ./my-rule.md\n",
            "rules/my-rule.md",
            "---\nseverity: error\nflag_examples: [a, b, c]\npass_examples: [x, y, z]\n---\nA rubric.\n",
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("rules/my-rule.yaml: severity"), "{e}");
        assert!(e.contains("too severe"), "{e}");
    }

    // ---- schema flexibility --------------------------------------------

    #[test]
    fn examples_accepts_single_object() {
        let def = ok(r#"
id: x
engine: phrase
patterns: ["—"]
examples: { bad: "b", good: "g" }
"#);
        assert_eq!(def.examples.len(), 1);
        assert_eq!(def.examples[0].good, "g");
    }

    #[test]
    fn intent_parses_and_defaults_to_none() {
        let def = ok("id: x\nengine: phrase\nintent: style\npatterns: [\"—\"]\n");
        assert_eq!(def.intent.as_deref(), Some("style"));
        assert_eq!(parse_intent("style"), Some(Intent::Style));
        assert_eq!(parse_intent("detection"), Some(Intent::Detection));
        assert_eq!(parse_intent("lint"), None);
        let def = ok("id: x\nengine: phrase\npatterns: [\"—\"]\n");
        assert!(def.intent.is_none());
    }

    #[test]
    fn bad_intent_lists_alternatives() {
        let e = err("id: x\nengine: phrase\nintent: scoring\npatterns: [\"—\"]\n");
        assert_eq!(
            e,
            "rules/test.yaml: intent: \"scoring\" is not an intent — use style or detection"
        );
    }

    #[test]
    fn severity_accepts_legacy_info() {
        let def = ok(r#"
id: x
engine: phrase
severity: info
patterns: ["—"]
"#);
        assert_eq!(
            parse_severity(def.severity.as_deref().unwrap()),
            Some(Severity::Suggestion)
        );
    }

    // ---- validation errors ---------------------------------------------

    #[test]
    fn bad_severity_matches_doc_example() {
        let e = parse_rule(
            "builtin/rules/no-em-dash.yaml",
            "id: no-em-dash\nengine: phrase\nseverity: high\npatterns: [\"—\"]\n",
        )
        .unwrap_err();
        assert_eq!(
            e.to_string(),
            "builtin/rules/no-em-dash.yaml: severity: \"high\" is not a severity — \
             use error, warning, or suggestion"
        );
    }

    #[test]
    fn bad_engine_lists_alternatives() {
        let e = err("id: x\nengine: regexp\npatterns: [\"a\"]\n");
        assert_eq!(
            e,
            "rules/test.yaml: engine: \"regexp\" is not an engine — \
             use phrase, leading, density, statistical, or inferential"
        );
    }

    #[test]
    fn bad_scope_lists_alternatives() {
        let e = err("id: x\nengine: phrase\nscope: body\npatterns: [\"a\"]\n");
        assert_eq!(
            e,
            "rules/test.yaml: scope: \"body\" is not a scope — use prose, text, or all"
        );
    }

    #[test]
    fn bad_granularity_lists_alternatives() {
        let e = err("id: x\nengine: inferential\ngranularity: word\nrubric: r\n\
             flag_examples: [a, b, c]\npass_examples: [x, y, z]\n");
        assert_eq!(
            e,
            "rules/test.yaml: granularity: \"word\" is not a granularity — \
             use sentence, paragraph, or document"
        );
    }

    #[test]
    fn invalid_regex_surfaces_regex_error_text() {
        let e = err("id: x\nengine: phrase\npatterns: [\"(unclosed\"]\n");
        assert!(
            e.starts_with("rules/test.yaml: patterns[0]: invalid regex \"(unclosed\":"),
            "{e}"
        );
        assert!(e.contains("unclosed group"), "{e}");
    }

    #[test]
    fn invalid_allow_context_regex_reports_its_field() {
        let e = err(
            "id: x\nengine: phrase\npatterns: [\"a\"]\nallow_context: { pattern: \"[\", window: 4 }\n",
        );
        assert!(e.contains("allow_context.pattern"), "{e}");
        assert!(e.contains("invalid regex"), "{e}");
    }

    #[test]
    fn allow_context_on_non_phrase_is_an_error() {
        let e = err(
            "id: x\nengine: leading\npatterns: [\"a\"]\nallow_context: { pattern: \"b\", window: 4 }\n",
        );
        assert_eq!(
            e,
            "rules/test.yaml: allow_context: allow_context only applies to phrase rules — \
             this rule uses the leading engine"
        );
    }

    #[test]
    fn phrase_needs_patterns() {
        let e = err("id: x\nengine: phrase\n");
        assert_eq!(
            e,
            "rules/test.yaml: patterns: a phrase rule needs at least one pattern — \
             add a patterns list"
        );
    }

    #[test]
    fn density_needs_threshold() {
        let e = err("id: x\nengine: density\npatterns: [\"a\"]\n");
        assert_eq!(e, "rules/test.yaml: missing required field `threshold`");
    }

    #[test]
    fn density_needs_exactly_one_pattern() {
        let e = err("id: x\nengine: density\nthreshold: 8\npatterns: [\"a\", \"b\"]\n");
        assert_eq!(
            e,
            "rules/test.yaml: patterns: a density rule needs exactly one pattern — found 2"
        );
        let e = err("id: x\nengine: density\nthreshold: 8\n");
        assert_eq!(
            e,
            "rules/test.yaml: patterns: a density rule needs exactly one pattern — found 0"
        );
    }

    #[test]
    fn density_threshold_must_be_non_negative() {
        let e = err("id: x\nengine: density\nthreshold: -3\npatterns: [\"a\"]\n");
        assert_eq!(
            e,
            "rules/test.yaml: threshold: -3 is not a valid threshold — use a non-negative \
             number of matches per 1000 words, like 8"
        );
    }

    #[test]
    fn statistical_needs_known_metric() {
        let e = err("id: x\nengine: statistical\n");
        assert_eq!(e, "rules/test.yaml: missing required field `metric`");
        let e = err("id: x\nengine: statistical\nmetric: word-frequency\n");
        assert!(
            e.starts_with(
                "rules/test.yaml: metric: \"word-frequency\" is not a known metric — \
                 use sentence-length, repetitive-openers,"
            ),
            "{e}"
        );
    }

    #[test]
    fn statistical_document_metric_full_schema() {
        let def = ok(r#"
id: uniform-sentence-rhythm
engine: statistical
metric: sentence-length-variance
threshold: 105
direction: below
"#);
        assert_eq!(def.metric.as_deref(), Some("sentence-length-variance"));
        assert_eq!(def.threshold, Some(105.0));
        assert_eq!(def.direction.as_deref(), Some("below"));
        // Negative thresholds are legal for document metrics.
        ok("id: x\nengine: statistical\nmetric: cadence-autocorrelation\nthreshold: -0.5\ndirection: below\n");
    }

    #[test]
    fn statistical_document_metric_needs_threshold_and_direction() {
        let e = err("id: x\nengine: statistical\nmetric: triad-density\ndirection: above\n");
        assert_eq!(e, "rules/test.yaml: missing required field `threshold`");
        let e = err("id: x\nengine: statistical\nmetric: triad-density\nthreshold: 2\n");
        assert_eq!(e, "rules/test.yaml: missing required field `direction`");
        let e =
            err("id: x\nengine: statistical\nmetric: triad-density\nthreshold: 2\ndirection: up\n");
        assert_eq!(
            e,
            "rules/test.yaml: direction: \"up\" is not a direction — use above or below"
        );
        let e = err(
            "id: x\nengine: statistical\nmetric: triad-density\nthreshold: .nan\ndirection: above\n",
        );
        assert!(e.contains("threshold"), "{e}");
    }

    #[test]
    fn direction_outside_document_metrics_is_an_error() {
        let e = err("id: x\nengine: phrase\npatterns: [\"a\"]\ndirection: above\n");
        assert_eq!(
            e,
            "rules/test.yaml: direction: direction only applies to statistical rules — \
             this rule uses the phrase engine"
        );
        let e = err("id: x\nengine: statistical\nmetric: sentence-length\ndirection: above\n");
        assert_eq!(
            e,
            "rules/test.yaml: direction: direction only applies to document-level metrics, \
             not sentence-length"
        );
    }

    #[test]
    fn statistical_params_are_validated_at_load_time() {
        // Regression: these used to pass validation, then the FIRST lint
        // panicked in RuleSet::instantiate ("validated rule failed to build").
        let e = err(
            "id: rep\nengine: statistical\nmetric: repetitive-openers\nparams: { run_length: 0 }\n",
        );
        assert_eq!(
            e,
            "rules/test.yaml: params.run_length: 0 is not a valid run length — \
             use a whole number of at least 1"
        );
        let e = err("id: rep\nengine: statistical\nmetric: repetitive-openers\nparams: { run_length: 2.5 }\n");
        assert!(e.contains("params.run_length"), "{e}");
        let e = err("id: rep\nengine: statistical\nmetric: repetitive-openers\nparams: { run_length: .nan }\n");
        assert!(e.contains("params.run_length"), "{e}");
        // NaN max_words used to load fine and made sentence-length never fire
        // with no signal.
        let e = err(
            "id: sl\nengine: statistical\nmetric: sentence-length\nparams: { max_words: .nan }\n",
        );
        assert!(e.contains("params.max_words"), "{e}");
        let e =
            err("id: sl\nengine: statistical\nmetric: sentence-length\nparams: { max_words: 0 }\n");
        assert!(e.contains("params.max_words"), "{e}");
        // Valid params still load.
        ok("id: sl\nengine: statistical\nmetric: sentence-length\nparams: { max_words: 45, run_length: 3 }\n");
    }

    #[test]
    fn inferential_needs_rubric_and_examples() {
        let e =
            err("id: x\nengine: inferential\nflag_examples: [a, b, c]\npass_examples: [x, y, z]\n");
        assert_eq!(e, "rules/test.yaml: missing required field `rubric`");

        let e = err("id: x\nengine: inferential\nrubric: r\nflag_examples: [a, b]\npass_examples: [x, y, z]\n");
        assert_eq!(
            e,
            "rules/test.yaml: flag_examples: an inferential rule needs at least 3 \
             flag_examples — found 2"
        );

        let e = err(
            "id: x\nengine: inferential\nrubric: r\nflag_examples: [a, b, c]\npass_examples: []\n",
        );
        assert_eq!(
            e,
            "rules/test.yaml: pass_examples: an inferential rule needs at least 3 \
             pass_examples — found 0"
        );
    }

    #[test]
    fn inferential_severity_above_warning_is_an_error() {
        let e = err("id: x\nengine: inferential\nseverity: error\nrubric: r\n\
             flag_examples: [a, b, c]\npass_examples: [x, y, z]\n");
        assert_eq!(
            e,
            "rules/test.yaml: severity: \"error\" is too severe for an inferential rule — \
             judge findings are advisory; use warning or suggestion"
        );
    }

    #[test]
    fn unknown_field_is_an_error() {
        let e = err("id: x\nengine: phrase\npatterns: [\"a\"]\nseverityy: error\n");
        assert!(e.contains("rules/test.yaml"), "{e}");
        assert!(e.contains("unknown field `severityy`"), "{e}");
    }

    #[test]
    fn missing_required_fields_are_named() {
        let e = err("engine: phrase\npatterns: [\"a\"]\n");
        assert_eq!(e, "rules/test.yaml: missing required field `id`");
        let e = err("id: x\npatterns: [\"a\"]\n");
        assert_eq!(e, "rules/test.yaml: missing required field `engine`");
    }

    #[test]
    fn invalid_yaml_reports_file() {
        let e = err(": : :");
        assert!(e.starts_with("rules/test.yaml: invalid YAML:"), "{e}");
    }

    #[test]
    fn bad_rule_id_is_an_error() {
        let e = err("id: \"core/x\"\nengine: phrase\npatterns: [\"a\"]\n");
        assert!(e.contains("\"core/x\" is not a valid rule name"), "{e}");
        let e = err("id: \"\"\nengine: phrase\npatterns: [\"a\"]\n");
        assert!(e.contains("is not a valid rule name"), "{e}");
    }

    // ---- manifest -------------------------------------------------------

    #[test]
    fn manifest_happy_path() {
        let m = parse_manifest(
            "pkg/style.yaml",
            "name: core\nversion: 0.1.0\ndescription: d\n",
        )
        .unwrap();
        assert_eq!(m.name, "core");
        assert_eq!(m.version, "0.1.0");
        assert_eq!(m.description.as_deref(), Some("d"));
    }

    #[test]
    fn manifest_missing_name() {
        let e = parse_manifest("pkg/style.yaml", "version: 0.1.0\n")
            .unwrap_err()
            .to_string();
        assert_eq!(e, "pkg/style.yaml: missing required field `name`");
    }

    #[test]
    fn manifest_bad_name() {
        let e = parse_manifest("pkg/style.yaml", "name: \"a/b\"\nversion: 0.1.0\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("\"a/b\" is not a valid package name"), "{e}");
    }

    // ---- helper coverage -------------------------------------------------

    #[test]
    fn enum_helpers() {
        assert_eq!(parse_severity("error"), Some(Severity::Error));
        assert_eq!(parse_severity("warning"), Some(Severity::Warning));
        assert_eq!(parse_severity("suggestion"), Some(Severity::Suggestion));
        assert_eq!(parse_severity("info"), Some(Severity::Suggestion));
        assert_eq!(parse_severity("high"), None);
        assert_eq!(parse_scope("prose"), Some(Scope::Prose));
        assert_eq!(parse_scope("nope"), None);
        assert_eq!(parse_granularity("document"), Some(Granularity::Document));
        assert_eq!(engine_tier("phrase"), Some(Tier::Static));
        assert_eq!(engine_tier("leading"), Some(Tier::Static));
        assert_eq!(engine_tier("density"), Some(Tier::Statistical));
        assert_eq!(engine_tier("statistical"), Some(Tier::Statistical));
        assert_eq!(engine_tier("inferential"), Some(Tier::Inferential));
        assert_eq!(engine_tier("x"), None);
    }
}
