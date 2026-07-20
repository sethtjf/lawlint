//! lawlint-wasm — browser (playground) bindings for the lawlint v2 engine.
//!
//! Tiers 1–2 run in-process. **Tier-3 inference is a host concern**: judge
//! backends live in `crates/lawlint-judge` and are native-only (in-process
//! mistral.rs inference / cloud clients don't run under wasm32-unknown-unknown),
//! so this crate exposes core's host-driven pair instead. The JS host runs
//! inference however it likes (transformers.js/WebLLM on WebGPU, or cloud);
//! grounding, hallucination counters, the confidence floor, and the Warning
//! severity cap are all enforced inside wasm — the core invariant ("a finding
//! that cannot be grounded does not exist") holds in-browser.
//!
//! Exports:
//! - `lint(text, options)` — built-in rules; `options` may be
//!   null/undefined for defaults.
//! - `remediationPrompt(text, options)` — built-in rules (tiers 1–2, same as
//!   `lint`); returns a revision brief for the diagnostics, or `null` when
//!   there are none.
//! - `builtInRulesMeta()` — metadata for every built-in rule (camelCase).
//! - `lintWithRules(text, options, extraRules)` — `extraRules` is a JS array
//!   of `{name, yaml}`; rules load into a package named `user` merged over
//!   the built-ins. Loader validation failures throw a structured
//!   `{file, message}` error object.
//! - `validateRule(name, yaml)` — `{ok: true} | {ok: false, message}` for
//!   live editor feedback.
//! - `planJudge(text, options, extraRules?)` — tier-3 planning: the
//!   serialized `JudgeRequest[]` (snake_case fields, exactly core's shapes)
//!   for the enabled inferential rules; `[]` when none are active.
//! - `applyJudgeFindings(text, options, requests, findingsPerRequest,
//!   extraRules?)` — feed the host model's `JudgeFinding[][]` (parallel to
//!   `requests`) back through core's full pipeline; returns a complete
//!   `LintResult` (tiers 1–2 + grounded tier-3, stats/score, judge stats)
//!   identical in shape to native `lint_full` output.
//!
//! User rules go through `RuleSet::from_sources` + `RuleSet::merge`, so they
//! behave exactly like a loaded package: enable/disable/severity/threshold
//! options and suppression comments (`lawlint-disable …`) resolve user rule
//! names the same way they resolve built-in ones.

use std::collections::HashMap;
use std::sync::OnceLock;

use lawlint_core::loader::parse_rule;
use lawlint_core::{
    lint as core_lint, lint_full, lint_with, plan_judge, remediation_prompt, Judge, JudgeError,
    JudgeFinding, JudgeRequest, LintOptions, LintResult, LoadError, PromptSource, RubricFragment,
    RuleExample, RuleId, RuleMeta, RuleSet, Scope, Severity, Tier,
};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

/// Package name for playground-authored rules: ids become `user/<name>`.
const USER_PACKAGE: &str = "user";

fn built_in_set() -> &'static RuleSet {
    static SET: OnceLock<RuleSet> = OnceLock::new();
    SET.get_or_init(RuleSet::built_in)
}

// ---- Structured errors --------------------------------------------------

/// Loader validation failure, shaped for the playground editor:
/// `{file, message}`.
#[derive(Debug, Clone, Serialize)]
struct RuleError {
    file: String,
    message: String,
}

impl From<LoadError> for RuleError {
    fn from(e: LoadError) -> Self {
        RuleError {
            file: e.file().to_string(),
            message: e.to_string(),
        }
    }
}

// ---- User rule package --------------------------------------------------

/// One playground-authored rule file: `{name, yaml}`.
#[derive(Debug, Clone, Deserialize)]
struct ExtraRule {
    name: String,
    yaml: String,
}

/// Built-ins + a `user` package built from the YAML strings, as one merged
/// `RuleSet`. `user/` ids can never collide with `core/` ids, so the only
/// merge-time errors are the user package's own (bad YAML, duplicate ids).
fn merged_set(extra: &[ExtraRule]) -> Result<RuleSet, RuleError> {
    let files: Vec<(String, String)> = extra
        .iter()
        .map(|e| (e.name.clone(), e.yaml.clone()))
        .collect();
    let user = RuleSet::from_sources(USER_PACKAGE, &files)?;
    let mut set = built_in_set().clone();
    set.merge(user)?;
    Ok(set)
}

/// Tiers 1–2 lint over built-ins + a `user` package parsed from YAML
/// strings. Tier-3 user rules load and validate but emit nothing here (no
/// judge under wasm; see the module docs).
fn lint_with_user_rules(
    text: &str,
    options: &LintOptions,
    extra: &[ExtraRule],
) -> Result<LintResult, RuleError> {
    Ok(lint_with(text, options, &merged_set(extra)?))
}

// ---- Host-driven tier-3 -------------------------------------------------

/// The rubric fragments for the enabled inferential rules, resolved exactly
/// as `lint_full` resolves them (enable/disable/severity via
/// `RuleSet::instantiate`).
fn active_rubrics(set: &RuleSet, options: &LintOptions) -> Vec<RubricFragment> {
    set.instantiate(options)
        .iter()
        .filter_map(|r| r.rubric().cloned())
        .collect()
}

/// Plan tier-3 judge requests over built-ins + user rules. Empty when no
/// inferential rules are active.
fn plan_judge_impl(
    text: &str,
    options: &LintOptions,
    extra: &[ExtraRule],
) -> Result<Vec<JudgeRequest>, RuleError> {
    let set = merged_set(extra)?;
    let fragments = active_rubrics(&set, options);
    if fragments.is_empty() {
        return Ok(Vec::new());
    }
    let doc = lawlint_core::parse(text, options.markdown.unwrap_or(false));
    let refs: Vec<&RubricFragment> = fragments.iter().collect();
    Ok(plan_judge(&doc, text, &refs))
}

/// A `Judge` scripted from the host model's findings, keyed by the request's
/// deterministic `cache_key_base`. Planning is deterministic for a given
/// (text, options, rules), so `lint_full`'s re-planned requests carry the
/// same keys the host received from `planJudge`. A re-planned request with
/// no host findings (stale/foreign `requests`) fails that chunk closed —
/// counted in `judge.chunksFailed`, never a panic.
struct HostJudge {
    findings: HashMap<String, Vec<JudgeFinding>>,
}

impl HostJudge {
    /// `requests` and `findings_per_request` are parallel (caller validates
    /// lengths). Duplicate keys (identical chunks under the same rubric set)
    /// keep the first findings array, matching native cache semantics where
    /// one model response serves every identical chunk.
    fn new(requests: Vec<JudgeRequest>, findings_per_request: Vec<Vec<JudgeFinding>>) -> Self {
        let mut findings = HashMap::new();
        for (req, chunk_findings) in requests.into_iter().zip(findings_per_request) {
            findings.entry(req.cache_key_base).or_insert(chunk_findings);
        }
        HostJudge { findings }
    }
}

impl Judge for HostJudge {
    fn evaluate(&self, req: &JudgeRequest) -> Result<Vec<JudgeFinding>, JudgeError> {
        self.findings
            .get(&req.cache_key_base)
            .cloned()
            .ok_or_else(|| {
                JudgeError::Backend(
                    "no host findings for this chunk (requests do not match this \
                 text/options/rules)"
                        .to_string(),
                )
            })
    }

    fn model_id(&self) -> &str {
        "host"
    }
}

/// `applyJudgeFindings` failure: either a rule-package error (structured
/// `{file, message}`) or bad host input (plain message).
enum ApplyError {
    Rule(RuleError),
    Input(String),
}

impl From<RuleError> for ApplyError {
    fn from(e: RuleError) -> Self {
        ApplyError::Rule(e)
    }
}

/// Run host-supplied findings through core's full pipeline (`lint_full` with
/// a scripted judge): grounding with hallucination counters, confidence
/// floor, Warning severity cap, scope mask, suppression, scoring — identical
/// to native tier-3 output.
fn apply_judge_findings_impl(
    text: &str,
    options: &LintOptions,
    requests: Vec<JudgeRequest>,
    findings_per_request: Vec<Vec<JudgeFinding>>,
    extra: &[ExtraRule],
) -> Result<LintResult, ApplyError> {
    if requests.len() != findings_per_request.len() {
        return Err(ApplyError::Input(format!(
            "findingsPerRequest length ({}) does not match requests length ({})",
            findings_per_request.len(),
            requests.len()
        )));
    }
    let set = merged_set(extra)?;
    let judge = HostJudge::new(requests, findings_per_request);
    Ok(lint_full(text, options, &set, &judge, None))
}

// ---- Rule metadata (camelCase for JS) -----------------------------------

/// `RuleMeta` serialized with camelCase field names (`docsUrl`) for the
/// playground; core's `RuleMeta` serializes `docs_url` as snake_case.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MetaJs<'a> {
    id: &'a RuleId,
    tier: Tier,
    scope: Scope,
    severity: Severity,
    description: &'a str,
    docs_url: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    rationale: Option<&'a str>,
    examples: &'a [RuleExample],
}

fn meta_js(m: &RuleMeta) -> MetaJs<'_> {
    MetaJs {
        id: &m.id,
        tier: m.tier,
        scope: m.scope,
        severity: m.severity,
        description: &m.description,
        docs_url: &m.docs_url,
        rationale: m.rationale.as_deref(),
        examples: &m.examples,
    }
}

// ---- validateRule -------------------------------------------------------

#[derive(Debug, Serialize)]
struct Validation {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

fn validate_rule_impl(name: &str, yaml: &str) -> Validation {
    match parse_rule(name, yaml) {
        Ok(_) => Validation {
            ok: true,
            message: None,
        },
        Err(e) => Validation {
            ok: false,
            message: Some(e.to_string()),
        },
    }
}

// ---- wasm-bindgen exports -----------------------------------------------

fn options_from_js(options: JsValue) -> Result<LintOptions, JsValue> {
    if options.is_null() || options.is_undefined() {
        Ok(LintOptions::default())
    } else {
        serde_wasm_bindgen::from_value(options)
            .map_err(|error| JsValue::from_str(&format!("invalid options: {error}")))
    }
}

fn to_js<T: Serialize>(value: &T, what: &str) -> Result<JsValue, JsValue> {
    serde_wasm_bindgen::to_value(value)
        .map_err(|error| JsValue::from_str(&format!("failed to serialize {what}: {error}")))
}

/// Parse an optional `extraRules` argument: null/undefined means none.
fn extra_rules_from_js(extra_rules: JsValue) -> Result<Vec<ExtraRule>, JsValue> {
    if extra_rules.is_null() || extra_rules.is_undefined() {
        Ok(Vec::new())
    } else {
        serde_wasm_bindgen::from_value(extra_rules).map_err(|error| {
            JsValue::from_str(&format!(
                "invalid extraRules: expected an array of {{name, yaml}}: {error}"
            ))
        })
    }
}

fn rule_error_to_js(e: RuleError) -> JsValue {
    to_js(&e, "rule error").unwrap_or_else(|_| JsValue::from_str(&e.message))
}

/// Lint with the built-in rules, tiers 1–2. `options` may be
/// null/undefined for defaults.
#[wasm_bindgen]
pub fn lint(text: &str, options: JsValue) -> Result<JsValue, JsValue> {
    let options = options_from_js(options)?;
    to_js(&core_lint(text, &options), "lint result")
}

/// Build a remediation prompt from the built-in rules, tiers 1–2 (same rule
/// set as `lint`). `options` may be null/undefined for defaults. Returns the
/// revision brief for the diagnostics, or `null` when there are none.
#[wasm_bindgen(js_name = remediationPrompt)]
pub fn remediation_prompt_js(text: &str, options: JsValue) -> Result<JsValue, JsValue> {
    let options = options_from_js(options)?;
    let set = built_in_set();
    let result = lint_with(text, &options, set);
    Ok(
        match remediation_prompt(PromptSource::Text(text), &result, set) {
            Some(prompt) => JsValue::from_str(&prompt),
            None => JsValue::NULL,
        },
    )
}

/// Metadata for every built-in rule (camelCase field names).
#[wasm_bindgen(js_name = builtInRulesMeta)]
pub fn built_in_rules_meta() -> Result<JsValue, JsValue> {
    let metas: Vec<MetaJs> = built_in_set().metas().into_iter().map(meta_js).collect();
    to_js(&metas, "rules metadata")
}

/// Lint with built-ins plus playground-authored rules. `extraRules` is a JS
/// array of `{name, yaml}`; the rules form a package named `user` merged
/// over the built-ins. Loader validation failures throw a structured
/// `{file, message}` object.
#[wasm_bindgen(js_name = lintWithRules)]
pub fn lint_with_rules(
    text: &str,
    options: JsValue,
    extra_rules: JsValue,
) -> Result<JsValue, JsValue> {
    let options = options_from_js(options)?;
    let extra = extra_rules_from_js(extra_rules)?;
    match lint_with_user_rules(text, &options, &extra) {
        Ok(result) => to_js(&result, "lint result"),
        Err(e) => Err(rule_error_to_js(e)),
    }
}

/// Plan tier-3 judge requests for the enabled inferential rules (built-ins
/// plus optional `extraRules`, which may be null/undefined). Returns core's
/// `JudgeRequest[]` (snake_case fields: `chunk_range`, `chunk_text`, `rules`,
/// `prompt`, `cache_key_base`); `[]` when no inferential rules are active.
/// The host runs each `prompt` through its model and passes the requests
/// back to `applyJudgeFindings` with the findings it got.
#[wasm_bindgen(js_name = planJudge)]
pub fn plan_judge_js(
    text: &str,
    options: JsValue,
    extra_rules: JsValue,
) -> Result<JsValue, JsValue> {
    let options = options_from_js(options)?;
    let extra = extra_rules_from_js(extra_rules)?;
    match plan_judge_impl(text, &options, &extra) {
        Ok(reqs) => to_js(&reqs, "judge requests"),
        Err(e) => Err(rule_error_to_js(e)),
    }
}

/// Apply the host model's tier-3 findings. `requests` is the array from
/// `planJudge` (same text/options/extraRules); `findingsPerRequest` is a
/// parallel array of `JudgeFinding[]` (snake_case fields: `rule`, `quote`,
/// `explanation`, `confidence`, `suggested_rewrite`). Findings are grounded
/// (exact → fuzzy → discarded as hallucinated), gated by the confidence
/// floor, capped at Warning severity, scope-masked, and suppression-checked
/// inside wasm; the returned `LintResult` (tiers 1–2 + grounded tier-3, with
/// `judge` stats) is identical in shape to native `lint_full` output.
/// Requests that don't match the re-planned chunks fail closed as
/// `judge.chunksFailed`, never a crash.
#[wasm_bindgen(js_name = applyJudgeFindings)]
pub fn apply_judge_findings_js(
    text: &str,
    options: JsValue,
    requests: JsValue,
    findings_per_request: JsValue,
    extra_rules: JsValue,
) -> Result<JsValue, JsValue> {
    let options = options_from_js(options)?;
    let extra = extra_rules_from_js(extra_rules)?;
    let requests: Vec<JudgeRequest> =
        serde_wasm_bindgen::from_value(requests).map_err(|error| {
            JsValue::from_str(&format!(
                "invalid requests: expected the JudgeRequest array from planJudge: {error}"
            ))
        })?;
    let findings: Vec<Vec<JudgeFinding>> = serde_wasm_bindgen::from_value(findings_per_request)
        .map_err(|error| {
            JsValue::from_str(&format!(
                "invalid findingsPerRequest: expected an array of JudgeFinding arrays \
                 parallel to requests: {error}"
            ))
        })?;
    match apply_judge_findings_impl(text, &options, requests, findings, &extra) {
        Ok(result) => to_js(&result, "lint result"),
        Err(ApplyError::Rule(e)) => Err(rule_error_to_js(e)),
        Err(ApplyError::Input(message)) => Err(JsValue::from_str(&message)),
    }
}

/// Validate one YAML rule for live editor feedback:
/// `{ok: true} | {ok: false, message}`.
#[wasm_bindgen(js_name = validateRule)]
pub fn validate_rule(name: &str, yaml: &str) -> Result<JsValue, JsValue> {
    to_js(&validate_rule_impl(name, yaml), "validation result")
}

// ------------------------------------------------------------------------
// Native unit tests (`cargo test -p lawlint-wasm` on the host); they
// exercise the pure Rust layer, not the JsValue shims.
// ------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn extra(name: &str, yaml: &str) -> ExtraRule {
        ExtraRule {
            name: name.to_string(),
            yaml: yaml.to_string(),
        }
    }

    fn foo_rule() -> ExtraRule {
        extra(
            "no-foo.yaml",
            "id: no-foo\nengine: phrase\nseverity: warning\n\
             description: No foo.\nmessage: Avoid foo\npatterns: [\"(?i)\\\\bfoo\\\\b\"]\n",
        )
    }

    fn ids(result: &LintResult) -> Vec<&str> {
        result
            .diagnostics
            .iter()
            .map(|d| d.rule_id.0.as_str())
            .collect()
    }

    // ---- yaml merge path -------------------------------------------------

    #[test]
    fn empty_extra_rules_match_plain_lint() {
        let text = "We delve into the landscape of this matter.";
        let merged = lint_with_user_rules(text, &LintOptions::default(), &[]).unwrap();
        let plain = core_lint(text, &LintOptions::default());
        assert_eq!(ids(&merged), ids(&plain));
        assert_eq!(merged.stats.score, plain.stats.score);
        assert_eq!(merged.stats.word_count, plain.stats.word_count);
    }

    #[test]
    fn user_rules_merge_over_builtins_and_fire() {
        let text = "We delve into foo daily.";
        let result = lint_with_user_rules(text, &LintOptions::default(), &[foo_rule()]).unwrap();
        let ids = ids(&result);
        assert!(ids.contains(&"core/no-ai-cliches"), "{ids:?}");
        assert!(ids.contains(&"user/no-foo"), "{ids:?}");
        let d = result
            .diagnostics
            .iter()
            .find(|d| d.rule_id.0 == "user/no-foo")
            .unwrap();
        assert_eq!(d.span.slice(text), "foo");
        assert_eq!(d.message, "Avoid foo");
        assert_eq!(d.severity, Severity::Warning);
        // Finalized like any built-in diagnostic.
        assert_eq!(d.line, 1);
        assert!(!d.excerpt.is_empty());
    }

    #[test]
    fn duplicate_user_ids_are_a_structured_error() {
        let e = lint_with_user_rules(
            "text",
            &LintOptions::default(),
            &[
                foo_rule(),
                extra("again.yaml", "id: no-foo\nengine: phrase\npatterns: [x]\n"),
            ],
        )
        .unwrap_err();
        assert_eq!(e.file, "again.yaml");
        assert!(e.message.contains("user/no-foo"), "{}", e.message);
        assert!(e.message.contains("no-foo.yaml"), "{}", e.message);
    }

    #[test]
    fn loader_validation_error_carries_file_and_message() {
        let e = lint_with_user_rules(
            "text",
            &LintOptions::default(),
            &[extra(
                "bad.yaml",
                "id: no-x\nengine: phrase\nseverity: high\npatterns: [x]\n",
            )],
        )
        .unwrap_err();
        assert_eq!(e.file, "bad.yaml");
        assert!(e.message.contains("severity"), "{}", e.message);
        assert!(e.message.contains("bad.yaml"), "{}", e.message);
    }

    #[test]
    fn invalid_regex_is_a_structured_error_not_a_panic() {
        let e = lint_with_user_rules(
            "text",
            &LintOptions::default(),
            &[extra(
                "re.yaml",
                "id: no-x\nengine: phrase\npatterns: [\"(\"]\n",
            )],
        )
        .unwrap_err();
        assert_eq!(e.file, "re.yaml");
        assert!(e.message.contains("regex"), "{}", e.message);
    }

    // ---- option resolution over the merged set ---------------------------

    #[test]
    fn user_rule_disabled_by_bare_alias_and_full_id() {
        for name in ["no-foo", "user/no-foo"] {
            let o = LintOptions {
                disable: Some(vec![name.to_string()]),
                ..Default::default()
            };
            let result = lint_with_user_rules("foo", &o, &[foo_rule()]).unwrap();
            assert!(!ids(&result).contains(&"user/no-foo"), "disable {name}");
        }
    }

    #[test]
    fn enable_allowlist_with_user_rule_excludes_builtins() {
        let o = LintOptions {
            enable: Some(vec!["no-foo".into()]),
            ..Default::default()
        };
        let result = lint_with_user_rules("We delve into foo.", &o, &[foo_rule()]).unwrap();
        assert_eq!(ids(&result), vec!["user/no-foo"]);
    }

    #[test]
    fn severity_override_reaches_user_rule_via_alias() {
        let o = LintOptions {
            severity: Some(
                [("no-foo".to_string(), Severity::Error)]
                    .into_iter()
                    .collect(),
            ),
            ..Default::default()
        };
        let result = lint_with_user_rules("foo", &o, &[foo_rule()]).unwrap();
        let d = result
            .diagnostics
            .iter()
            .find(|d| d.rule_id.0 == "user/no-foo")
            .unwrap();
        assert_eq!(d.severity, Severity::Error);
    }

    #[test]
    fn bare_name_shadowing_a_builtin_is_ambiguous_and_ignored() {
        // A user rule named like a built-in makes the bare alias ambiguous:
        // disable by bare name then affects NEITHER; full ids still work.
        let shadow = extra(
            "no-em-dash.yaml",
            "id: no-em-dash\nengine: phrase\npatterns: [\"zzz\"]\n",
        );
        let text = "An em—dash and zzz.";
        let o = LintOptions {
            disable: Some(vec!["no-em-dash".into()]),
            ..Default::default()
        };
        let result = lint_with_user_rules(text, &o, std::slice::from_ref(&shadow)).unwrap();
        assert!(ids(&result).contains(&"core/no-em-dash"));
        assert!(ids(&result).contains(&"user/no-em-dash"));

        let o = LintOptions {
            disable: Some(vec!["core/no-em-dash".into(), "user/no-em-dash".into()]),
            ..Default::default()
        };
        let result = lint_with_user_rules(text, &o, &[shadow]).unwrap();
        assert!(!ids(&result).contains(&"core/no-em-dash"));
        assert!(!ids(&result).contains(&"user/no-em-dash"));
    }

    #[test]
    fn threshold_override_reaches_user_density_rule() {
        let dense = extra(
            "dense.yaml",
            "id: dense\nengine: density\nthreshold: 999\nmessage: Too much foo\n\
             patterns: [\"foo\"]\n",
        );
        let text = "foo bar baz";
        // Default threshold 999 per 1000 words: 1 match in 3 words ≈ 333 → quiet.
        let result =
            lint_with_user_rules(text, &LintOptions::default(), std::slice::from_ref(&dense))
                .unwrap();
        assert!(!ids(&result).contains(&"user/dense"));
        // Bare-alias threshold override to 0 → fires.
        let o = LintOptions {
            thresholds: Some([("dense".to_string(), 0.0)].into_iter().collect()),
            ..Default::default()
        };
        let result = lint_with_user_rules(text, &o, &[dense]).unwrap();
        assert!(ids(&result).contains(&"user/dense"), "{:?}", ids(&result));
    }

    #[test]
    fn inferential_user_rule_loads_but_emits_nothing_in_wasm() {
        // Tier-3 is native-only; the rule validates and instantiates but has
        // no runtime hooks here.
        let inf = extra(
            "inf.yaml",
            "id: no-fluff\nengine: inferential\ngranularity: sentence\nrubric: Flag fluff.\n\
             flag_examples: [a, b, c]\npass_examples: [x, y, z]\n",
        );
        let result = lint_with_user_rules(
            "It could perhaps be argued.",
            &LintOptions::default(),
            &[inf],
        )
        .unwrap();
        assert!(!ids(&result).iter().any(|id| id.starts_with("user/")));
    }

    #[test]
    fn user_meta_matches_registry_derivation() {
        let set = merged_set(&[
            extra(
                "x.yaml",
                "id: no-x\nengine: density\nscope: prose\nseverity: info\nthreshold: 8\n\
                 patterns: [x]\n",
            ),
            extra(
                "y.yaml",
                "id: no-y\nengine: leading\npatterns: [Certainly]\n",
            ),
        ])
        .unwrap();
        let metas = set.metas();
        let by_id = |id: &str| *metas.iter().find(|m| m.id.0 == id).unwrap();

        let meta = by_id("user/no-x");
        assert_eq!(meta.tier, Tier::Statistical);
        assert_eq!(meta.scope, Scope::Prose);
        assert_eq!(meta.severity, Severity::Suggestion); // legacy "info"
        assert_eq!(meta.docs_url, "https://lawlint.com/rules/no-x");

        let meta = by_id("user/no-y");
        assert_eq!(meta.tier, Tier::Static);
        assert_eq!(meta.scope, Scope::Text); // default
        assert_eq!(meta.severity, Severity::Warning); // default
    }

    // ---- validation path -------------------------------------------------

    #[test]
    fn validate_rule_ok() {
        let v = validate_rule_impl("ok.yaml", &foo_rule().yaml);
        assert!(v.ok);
        assert!(v.message.is_none());
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json, serde_json::json!({ "ok": true }));
    }

    #[test]
    fn validate_rule_reports_plain_english_errors() {
        let v = validate_rule_impl(
            "bad.yaml",
            "id: no-x\nengine: phrase\nseverity: high\npatterns: [x]\n",
        );
        assert!(!v.ok);
        let message = v.message.unwrap();
        assert!(message.contains("bad.yaml"), "{message}");
        assert!(message.contains("severity"), "{message}");

        let v = validate_rule_impl("bad.yaml", "not: [valid");
        assert!(!v.ok);
    }

    #[test]
    fn validate_rule_enforces_inferential_requirements() {
        let v = validate_rule_impl(
            "inf.yaml",
            "id: no-x\nengine: inferential\nrubric: Flag it.\nflag_examples: [a]\n\
             pass_examples: [x, y, z]\n",
        );
        assert!(!v.ok, "inferential rules need >= 3 flag examples");
    }

    // ---- metadata serialization -----------------------------------------

    #[test]
    fn built_in_meta_serializes_camel_case() {
        let metas = built_in_set().metas();
        assert_eq!(metas.len(), 22);
        let json =
            serde_json::to_value(metas.iter().map(|m| meta_js(m)).collect::<Vec<_>>()).unwrap();
        let first = &json[0];
        assert!(first["id"].as_str().unwrap().starts_with("core/"));
        assert!(first.get("docsUrl").is_some(), "camelCase docsUrl expected");
        assert!(first.get("docs_url").is_none());
        for key in ["tier", "scope", "severity", "description", "examples"] {
            assert!(first.get(key).is_some(), "missing {key}");
        }
        // Enums serialize lowercase.
        assert!(matches!(
            first["severity"].as_str().unwrap(),
            "error" | "warning" | "suggestion"
        ));
    }

    // ---- resolver --------------------------------------------------------

    #[test]
    fn resolver_full_ids_win_and_ambiguity_prefers_nothing() {
        let set = merged_set(&[foo_rule()]).unwrap();
        assert_eq!(set.resolve("user/no-foo").unwrap().0, "user/no-foo");
        assert_eq!(set.resolve("no-foo").unwrap().0, "user/no-foo");
        assert_eq!(set.resolve("no-em-dash").unwrap().0, "core/no-em-dash");
        assert!(set.resolve("nope").is_none());
    }

    // ---- host-driven tier-3 ----------------------------------------------

    fn judge_finding(rule: &str, quote: &str, confidence: f32) -> JudgeFinding {
        JudgeFinding {
            rule: rule.to_string(),
            quote: quote.to_string(),
            explanation: "flagged by the host model".into(),
            confidence,
            suggested_rewrite: None,
        }
    }

    fn fluff_rule() -> ExtraRule {
        extra(
            "no-fluff.yaml",
            "id: no-fluff\nengine: inferential\ngranularity: sentence\nrubric: Flag fluff.\n\
             flag_examples: [a, b, c]\npass_examples: [x, y, z]\n",
        )
    }

    #[test]
    fn plan_returns_requests_for_builtin_inferential_rules() {
        let text = "It could perhaps be argued that the claim fails.";
        let reqs = plan_judge_impl(text, &LintOptions::default(), &[]).unwrap();
        assert_eq!(reqs.len(), 1, "{reqs:?}");
        let req = &reqs[0];
        assert_eq!(req.chunk_text, text);
        assert!(
            req.rules.iter().any(|r| r.0 == "core/empty-hedge"),
            "{:?}",
            req.rules
        );
        assert!(req.prompt.contains("core/empty-hedge"));
        assert!(req.prompt.contains(text));
        assert_eq!(req.cache_key_base.len(), 64);
    }

    #[test]
    fn plan_honors_enable_and_disable_options() {
        let text = "It could perhaps be argued that the claim fails.";
        // Disabling every built-in inferential rule leaves nothing to plan.
        let o = LintOptions {
            disable: Some(vec!["empty-hedge".into(), "padded-elaboration".into()]),
            ..Default::default()
        };
        assert!(plan_judge_impl(text, &o, &[]).unwrap().is_empty());
        // An enable allowlist of only static rules likewise plans nothing.
        let o = LintOptions {
            enable: Some(vec!["no-em-dash".into()]),
            ..Default::default()
        };
        assert!(plan_judge_impl(text, &o, &[]).unwrap().is_empty());
        // Allowlisting one inferential rule keeps exactly its rubric.
        let o = LintOptions {
            enable: Some(vec!["empty-hedge".into()]),
            ..Default::default()
        };
        let reqs = plan_judge_impl(text, &o, &[]).unwrap();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].rules, vec![RuleId("core/empty-hedge".into())]);
    }

    #[test]
    fn plan_rule_package_errors_are_structured() {
        let e = plan_judge_impl(
            "text",
            &LintOptions::default(),
            &[extra(
                "bad.yaml",
                "id: no-x\nengine: phrase\nseverity: high\npatterns: [x]\n",
            )],
        )
        .unwrap_err();
        assert_eq!(e.file, "bad.yaml");
        assert!(e.message.contains("severity"), "{}", e.message);
    }

    #[test]
    fn apply_grounds_finding_into_full_diagnostic_merged_with_static_tiers() {
        let text = "We delve daily. It could perhaps be argued that the claim fails.";
        let options = LintOptions::default();
        let reqs = plan_judge_impl(text, &options, &[]).unwrap();
        assert_eq!(reqs.len(), 1);
        let findings = vec![vec![JudgeFinding {
            suggested_rewrite: Some("The claim fails because".into()),
            ..judge_finding("core/empty-hedge", "It could perhaps be argued that", 0.9)
        }]];
        let result = apply_judge_findings_impl(text, &options, reqs, findings, &[])
            .unwrap_or_else(|_| panic!("apply failed"));
        // Tiers 1–2 still present, merged in span order.
        assert!(
            ids(&result).contains(&"core/no-ai-cliches"),
            "{:?}",
            ids(&result)
        );
        // Grounded tier-3 diagnostic, finalized like any static one.
        let d = result
            .diagnostics
            .iter()
            .find(|d| d.tier == Tier::Inferential)
            .expect("tier-3 diagnostic");
        assert_eq!(d.rule_id.0, "core/empty-hedge");
        assert_eq!(d.span.slice(text), "It could perhaps be argued that");
        assert_eq!(d.line, 1);
        assert!(d.column > 0);
        assert!(!d.excerpt.is_empty());
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(d.confidence, Some(0.9));
        assert_eq!(d.suggestion.as_deref(), Some("The claim fails because"));
        // Judge stats attached, exactly like native lint_full.
        let stats = result.judge.as_ref().expect("judge stats");
        assert_eq!(stats.chunks, 1);
        assert_eq!(stats.grounded, 1);
        assert_eq!(stats.chunks_failed, 0);
        assert!(stats.hallucinated.is_empty());
    }

    #[test]
    fn apply_discards_and_counts_hallucinated_quotes() {
        let text = "It could perhaps be argued that the claim fails.";
        let options = LintOptions::default();
        let reqs = plan_judge_impl(text, &options, &[]).unwrap();
        let findings = vec![vec![judge_finding(
            "core/empty-hedge",
            "totally fabricated wording never in the text",
            0.95,
        )]];
        let result = apply_judge_findings_impl(text, &options, reqs, findings, &[])
            .ok()
            .unwrap();
        assert!(result
            .diagnostics
            .iter()
            .all(|d| d.tier != Tier::Inferential));
        let stats = result.judge.as_ref().unwrap();
        assert_eq!(stats.grounded, 0);
        assert_eq!(stats.hallucinated.get("core/empty-hedge"), Some(&1));
    }

    #[test]
    fn apply_drops_findings_below_confidence_floor() {
        let text = "It could perhaps be argued that the claim fails.";
        let options = LintOptions::default();
        let mk = |confidence| {
            let reqs = plan_judge_impl(text, &options, &[]).unwrap();
            let findings = vec![vec![judge_finding(
                "core/empty-hedge",
                "could perhaps be argued",
                confidence,
            )]];
            apply_judge_findings_impl(text, &options, reqs, findings, &[])
                .ok()
                .unwrap()
        };
        // Below the default 0.6 floor: grounded (it IS in the text) but gated.
        let result = mk(0.3);
        assert!(result
            .diagnostics
            .iter()
            .all(|d| d.tier != Tier::Inferential));
        assert_eq!(result.judge.as_ref().unwrap().grounded, 1);
        // Above the floor: kept.
        let result = mk(0.9);
        assert_eq!(
            result
                .diagnostics
                .iter()
                .filter(|d| d.tier == Tier::Inferential)
                .count(),
            1
        );
    }

    #[test]
    fn apply_mismatched_findings_length_is_a_clean_error() {
        let text = "It could perhaps be argued that the claim fails.";
        let options = LintOptions::default();
        let reqs = plan_judge_impl(text, &options, &[]).unwrap();
        assert_eq!(reqs.len(), 1);
        let e = apply_judge_findings_impl(text, &options, reqs, vec![], &[]);
        match e {
            Err(ApplyError::Input(message)) => {
                assert!(message.contains("findingsPerRequest"), "{message}");
                assert!(message.contains('0') && message.contains('1'), "{message}");
            }
            _ => panic!("expected a clean input error"),
        }
    }

    #[test]
    fn apply_with_foreign_requests_fails_chunks_closed_not_aborts() {
        let text = "It could perhaps be argued that the claim fails.";
        let options = LintOptions::default();
        // Requests planned for DIFFERENT text: keys can't match the re-plan.
        let stale = plan_judge_impl("Entirely different words here.", &options, &[]).unwrap();
        let findings = vec![vec![judge_finding(
            "core/empty-hedge",
            "different words",
            0.9,
        )]];
        let result = apply_judge_findings_impl(text, &options, stale, findings, &[])
            .ok()
            .unwrap();
        let stats = result.judge.as_ref().unwrap();
        assert_eq!(stats.chunks, 1);
        assert_eq!(stats.chunks_failed, 1);
        assert_eq!(stats.grounded, 0);
        assert!(result
            .diagnostics
            .iter()
            .all(|d| d.tier != Tier::Inferential));
    }

    #[test]
    fn roundtrip_with_user_inferential_rule() {
        let text = "This is pure fluff indeed.";
        let extra = [fluff_rule()];
        let options = LintOptions {
            // Severity override must still cap at Warning for tier-3.
            severity: Some(
                [("no-fluff".to_string(), Severity::Error)]
                    .into_iter()
                    .collect(),
            ),
            ..Default::default()
        };
        let reqs = plan_judge_impl(text, &options, &extra).unwrap();
        assert_eq!(reqs.len(), 1);
        assert!(
            reqs[0].rules.iter().any(|r| r.0 == "user/no-fluff"),
            "{:?}",
            reqs[0].rules
        );
        assert!(reqs[0].prompt.contains("Flag fluff."));
        let findings = reqs
            .iter()
            .map(|r| {
                if r.rules.iter().any(|id| id.0 == "user/no-fluff") {
                    vec![judge_finding("user/no-fluff", "pure fluff", 0.8)]
                } else {
                    vec![]
                }
            })
            .collect();
        let result = apply_judge_findings_impl(text, &options, reqs, findings, &extra)
            .ok()
            .unwrap();
        let d = result
            .diagnostics
            .iter()
            .find(|d| d.rule_id.0 == "user/no-fluff")
            .expect("user tier-3 diagnostic");
        assert_eq!(d.tier, Tier::Inferential);
        assert_eq!(d.span.slice(text), "pure fluff");
        assert_eq!(d.severity, Severity::Warning); // capped despite Error override
        assert_eq!(result.judge.as_ref().unwrap().grounded, 1);
    }

    #[test]
    fn apply_findings_naming_foreign_rules_are_counted_hallucinated() {
        let text = "It could perhaps be argued that the claim fails.";
        let options = LintOptions::default();
        let reqs = plan_judge_impl(text, &options, &[]).unwrap();
        let findings = vec![vec![judge_finding("core/not-a-rule", "could perhaps", 0.9)]];
        let result = apply_judge_findings_impl(text, &options, reqs, findings, &[])
            .ok()
            .unwrap();
        assert!(result
            .diagnostics
            .iter()
            .all(|d| d.tier != Tier::Inferential));
        assert_eq!(
            result
                .judge
                .as_ref()
                .unwrap()
                .hallucinated
                .get("core/not-a-rule"),
            Some(&1)
        );
    }

    #[test]
    fn plan_and_apply_serialize_through_json_roundtrip() {
        // The JS boundary is serde; requests and findings must survive a
        // serialize→deserialize cycle with keys intact.
        let text = "It could perhaps be argued that the claim fails.";
        let options = LintOptions::default();
        let reqs = plan_judge_impl(text, &options, &[]).unwrap();
        let json = serde_json::to_string(&reqs).unwrap();
        let back: Vec<JudgeRequest> = serde_json::from_str(&json).unwrap();
        assert_eq!(back[0].cache_key_base, reqs[0].cache_key_base);
        let findings = vec![vec![judge_finding(
            "core/empty-hedge",
            "could perhaps be argued",
            0.9,
        )]];
        let fjson = serde_json::to_string(&findings).unwrap();
        let fback: Vec<Vec<JudgeFinding>> = serde_json::from_str(&fjson).unwrap();
        let result = apply_judge_findings_impl(text, &options, back, fback, &[])
            .ok()
            .unwrap();
        assert_eq!(
            result
                .diagnostics
                .iter()
                .filter(|d| d.tier == Tier::Inferential)
                .count(),
            1
        );
    }

    // ---- suppression ----------------------------------------------------

    #[test]
    fn suppression_comments_silence_user_rules_by_name() {
        // Previously a documented limitation: directives could not name user
        // rules. With a real merged RuleSet the dispatcher resolves them.
        let text = "<!-- lawlint-disable-next-line no-foo -->\nSome foo here.\nMore foo again.";
        let o = LintOptions {
            markdown: Some(true),
            enable: Some(vec!["no-foo".into()]),
            ..Default::default()
        };
        let result = lint_with_user_rules(text, &o, &[foo_rule()]).unwrap();
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(result.diagnostics[0].rule_id.0, "user/no-foo");
        assert_eq!(result.diagnostics[0].line, 3);
    }
}
