//! lawlint-judge — tier-3 judge backends (native only). Design doc §10.
//!
//! **ax (`axllm`) is the AI interface for ALL backends.** The judge is one
//! [`AxJudge`] whose prompt/JSON-contract layer is backend-independent;
//! backends are [`axllm::AxAIClient`] implementations:
//!
//! - [`MistralRsClient`] (`local:` specs, explicit opt-in — see #50; there
//!   is no default backend): in-process mistral.rs inference — a
//!   small quantized instruct GGUF (Qwen2.5-1.5B-Instruct by default) or a
//!   safetensors repo quantized in situ (the Gemma 4 series runs this way),
//!   lazily downloaded into the standard HF hub cache. CPU, with Metal on
//!   macOS.
//! - Cloud backends (feature `cloud`): stock ax clients (Anthropic,
//!   OpenAI-compatible with a custom base URL).
//!
//! `AxJudge` speaks OpenAI chat-completions-shaped payloads through
//! `AxAIClient::chat` and hand-rolls the `JudgeFinding[]` JSON parse from
//! `JudgeRequest.prompt` (core owns the canonical prompt and cache keys; ax's
//! signature layer would substitute its own prompt, so it is not used here —
//! see the crate README note in the repo docs). ax v23's `chat` is blocking;
//! the local client bridges to mistral.rs' async SDK with a runtime it owns.

mod cache;
pub mod credentials;
#[cfg(feature = "cloud")]
mod foundry;
mod mistralrs_client;

pub use cache::DiskCache;
#[cfg(feature = "cloud")]
pub use foundry::FoundryClient;
pub use mistralrs_client::MistralRsClient;

// Re-export the ax boundary so consumers can supply custom backends.
pub use axllm::{AxAIClient, AxError, AxResult};

use std::sync::Mutex;

use anyhow::bail;
#[cfg(feature = "cloud")]
use anyhow::Context;
use lawlint_core::{Judge, JudgeError, JudgeFinding, JudgeOptions, JudgeRequest};
use serde::Deserialize;
use serde_json::{json, Value};

/// Default local model repo (small quantized instruct GGUF).
pub const DEFAULT_LOCAL_REPO: &str = "Qwen/Qwen2.5-1.5B-Instruct-GGUF";

/// Gemma catalog repo (Google's Gemma 4 E4B instruct model, safetensors —
/// mistral.rs auto-detects it as multimodal and quantizes it in situ to
/// Q4K at load; the repo is not gated). Gemma GGUFs are a different story:
/// mistral.rs' GGUF loader has no gemma architecture, so
/// [`MistralRsClient`] rejects them up front with a pointer here. The repo
/// id stays config-editable (`local:<repo>`).
pub const DEFAULT_GEMMA_REPO: &str = "google/gemma-4-E4B-it";

// ---- AxJudge -----------------------------------------------------------

/// `lawlint_core::Judge` over any `axllm::AxAIClient` backend.
///
/// Sends `JudgeRequest.prompt` as a single user message in an OpenAI
/// chat-completions-shaped request and parses the response content as a
/// `JudgeFinding[]` JSON array. Malformed output surfaces as
/// `JudgeError::MalformedResponse` so core's `run_judge` can retry once and
/// then fail the chunk closed.
pub struct AxJudge {
    // `AxAIClient::chat` takes `&mut self`; `Judge::evaluate` takes `&self`
    // and requires Send + Sync — hence the mutex.
    client: Mutex<Box<dyn AxAIClient + Send>>,
    model_id: String,
}

impl AxJudge {
    pub fn new(client: Box<dyn AxAIClient + Send>, model_id: impl Into<String>) -> Self {
        AxJudge {
            client: Mutex::new(client),
            model_id: model_id.into(),
        }
    }

    /// The OpenAI chat-completions-shaped request sent to the backend.
    /// `model` is intentionally omitted: ax clients default to their
    /// configured model, and `MistralRsClient` has exactly one model.
    fn chat_request(req: &JudgeRequest) -> Value {
        json!({
            "messages": [{"role": "user", "content": req.prompt}],
            "temperature": 0,
        })
    }
}

impl AxJudge {
    /// `Judge::evaluate`, but returning the findings dropped by the
    /// verdict-polarity guard alongside the kept ones. The #39 judged-eval
    /// harness measures the verdict-discipline rate from the dropped set;
    /// `evaluate` discards it, so trait behavior is unchanged.
    pub fn evaluate_with_stats(&self, req: &JudgeRequest) -> Result<ParsedFindings, JudgeError> {
        let request = Self::chat_request(req);
        let response = self
            .client
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .chat(request)
            .map_err(|e| JudgeError::Backend(e.to_string()))?;
        let content = chat_content(&response).ok_or_else(|| {
            JudgeError::MalformedResponse(format!(
                "no textual content in chat response: {}",
                truncate(&response.to_string(), 300)
            ))
        })?;
        parse_findings_with_stats(&content)
    }
}

impl Judge for AxJudge {
    fn evaluate(&self, req: &JudgeRequest) -> Result<Vec<JudgeFinding>, JudgeError> {
        self.evaluate_with_stats(req).map(|parsed| parsed.kept)
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}

/// Extract assistant text from a chat response. Accepts both the OpenAI
/// chat-completions shape (`choices[0].message.content` — `MistralRsClient`,
/// any OpenAI-compatible passthrough) and ax's normalized internal shape
/// (`results[0].content` — stock ax clients). Public so other AI features
/// speaking through the ax boundary (e.g. `lawlint learn`) parse responses
/// the same way the judge does.
pub fn chat_content(response: &Value) -> Option<String> {
    let node = response
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .or_else(|| {
            response
                .get("results")
                .and_then(|r| r.get(0))
                .and_then(|r| r.get("content"))
        })?;
    value_as_text(node)
}

/// A content node is either a plain string or an array of
/// `{type: "text", text}` parts.
fn value_as_text(node: &Value) -> Option<String> {
    match node {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => {
            let mut out = String::new();
            for part in parts {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    out.push_str(text);
                } else if let Some(text) = part.as_str() {
                    out.push_str(text);
                }
            }
            Some(out)
        }
        _ => None,
    }
}

// ---- Findings parse ----------------------------------------------------

/// Lenient finding shape: small local models sometimes omit optional fields.
/// `rule` and `quote` are required (a finding without them is useless —
/// grounding needs the quote, routing needs the rule); missing `explanation`
/// defaults empty, missing `confidence` defaults to 0.5 (below core's default
/// 0.6 floor, so under-specified findings drop out unless the user lowers the
/// floor deliberately).
#[derive(Deserialize)]
struct LenientFinding {
    rule: String,
    quote: String,
    #[serde(default)]
    explanation: String,
    #[serde(default = "default_confidence")]
    confidence: f32,
    #[serde(default)]
    suggested_rewrite: Option<String>,
}

fn default_confidence() -> f32 {
    0.5
}

impl From<LenientFinding> for JudgeFinding {
    fn from(f: LenientFinding) -> Self {
        JudgeFinding {
            rule: f.rule,
            quote: f.quote,
            explanation: f.explanation,
            confidence: f.confidence,
            suggested_rewrite: f.suggested_rewrite,
        }
    }
}

/// One parsed model response, partitioned by the verdict-polarity guard:
/// `kept` goes on to grounding; `dropped_negative` is the guard's discard
/// pile, exposed so the #39 judged-eval harness can compute the
/// verdict-discipline rate (dropped-negative / all model-emitted findings).
#[derive(Debug, Clone)]
pub struct ParsedFindings {
    pub kept: Vec<JudgeFinding>,
    pub dropped_negative: Vec<JudgeFinding>,
}

/// Parse model output into findings. Tolerates markdown code fences and
/// leading/trailing prose around the JSON array; anything else is
/// `MalformedResponse` (core retries once, then fails the chunk closed).
/// Negative-verdict findings are dropped into `dropped_negative` (see
/// [`is_negative_verdict`]) and stay observable there — the #39 judged-eval
/// harness counts them; `Judge::evaluate` discards them.
pub fn parse_findings_with_stats(content: &str) -> Result<ParsedFindings, JudgeError> {
    let stripped = strip_code_fences(content.trim());

    let mut candidates: Vec<&str> = vec![stripped];
    // Fall back to the outermost bracketed slice (models sometimes wrap the
    // array in prose).
    if let (Some(start), Some(end)) = (stripped.find('['), stripped.rfind(']')) {
        if start < end {
            candidates.push(&stripped[start..=end]);
        }
    }

    for candidate in candidates {
        if let Ok(found) = serde_json::from_str::<Vec<LenientFinding>>(candidate) {
            let (dropped_negative, kept) = found
                .into_iter()
                .map(JudgeFinding::from)
                .partition(is_negative_verdict);
            return Ok(ParsedFindings {
                kept,
                dropped_negative,
            });
        }
    }
    Err(JudgeError::MalformedResponse(truncate(content, 300)))
}

/// Verdict-polarity guard (#39): small local models sometimes emit their
/// *pass* verdict as a finding object ("The text does not flag any empty
/// hedge") instead of returning `[]`; such findings quote real text with high
/// confidence, so core's confidence floor and grounding both pass them.
///
/// A finding is a negative verdict only when its explanation negates the
/// violation itself — either a rule-agnostic verdict phrase ("does not
/// violate", "no violation") or a negation whose object is the rule's own
/// name ("no empty hedge", "is not an empty hedge"). The patterns are
/// deliberately narrow: a real explanation that merely contains a negation
/// ("this hedge adds no information", "does not commit to a position") must
/// survive — a missed negative verdict is one bogus diagnostic, a dropped
/// real finding is silently lost.
fn is_negative_verdict(finding: &JudgeFinding) -> bool {
    let explanation = finding.explanation.to_lowercase();

    const VERDICT_NEGATIONS: &[&str] = &[
        "does not violate",
        "do not violate",
        "doesn't violate",
        "not a violation",
        "not violated",
        "no violation",
        "does not flag",
        "do not flag",
        "doesn't flag",
        "nothing to flag",
        "no issues found",
        "no problems found",
        "complies with the rule",
    ];
    if VERDICT_NEGATIONS.iter().any(|p| explanation.contains(p)) {
        return true;
    }

    // Negations naming the rule itself as their object. Rule phrase = the id
    // minus its namespace, hyphens as spaces ("core/empty-hedge" → "empty
    // hedge").
    let rule_phrase = finding
        .rule
        .rsplit('/')
        .next()
        .unwrap_or(&finding.rule)
        .replace('-', " ")
        .to_lowercase();
    if rule_phrase.trim().is_empty() {
        return false;
    }
    [
        format!("no {rule_phrase}"),
        format!("not a {rule_phrase}"),
        format!("not an {rule_phrase}"),
    ]
    .iter()
    .any(|p| explanation.contains(p.as_str()))
}

/// Strip a single wrapping markdown code fence (``` or ```json).
fn strip_code_fences(s: &str) -> &str {
    let s = s.trim();
    let Some(rest) = s.strip_prefix("```") else {
        return s;
    };
    // Drop the info string ("json", …) on the opening fence line.
    let rest = match rest.find('\n') {
        Some(idx) => &rest[idx + 1..],
        None => rest,
    };
    rest.trim_end().strip_suffix("```").unwrap_or(rest).trim()
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max_chars).collect();
        format!("{cut}…")
    }
}

// ---- Backend selection -------------------------------------------------

/// Build the raw ax client (plus its canonical model id) for a model spec —
/// the shared backend factory behind [`create_judge`] and any other AI
/// feature that speaks through the ax boundary (e.g. `lawlint learn`):
///
/// - `"local"` / `"local:<hf-repo>"` (optionally `#<gguf-file>`):
///   in-process [`MistralRsClient`] (lazy model download on first chat).
/// - `"anthropic:<model>"` (feature `cloud`): stock ax Anthropic client;
///   requires `ANTHROPIC_API_KEY` (environment or credential store).
/// - `"openai:<base-url>#<model>"` (feature `cloud`): stock ax
///   OpenAI-compatible client (covers any local OpenAI-compatible server);
///   uses `OPENAI_API_KEY` when set.
/// - `"foundry:<deployment>"` (feature `cloud`): [`FoundryClient`]; requires
///   `AZURE_FOUNDRY_ENDPOINT` + `AZURE_FOUNDRY_API_KEY`.
///
/// Hosted-provider keys resolve environment-first, then the user-level
/// credential store written by `lawlint init` ([`credentials`]).
pub fn create_client(model: &str) -> anyhow::Result<(Box<dyn AxAIClient + Send>, String)> {
    if model == "local" || model.starts_with("local:") {
        let spec = model.strip_prefix("local:").unwrap_or("");
        let (repo, file) = match spec.split_once('#') {
            Some((repo, file)) => (repo, Some(file.to_string())),
            None => (spec, None),
        };
        let repo = if repo.is_empty() {
            DEFAULT_LOCAL_REPO
        } else {
            repo
        };
        let model_id = match &file {
            Some(f) => format!("local:{repo}#{f}"),
            None => format!("local:{repo}"),
        };
        let client = MistralRsClient::new(repo, file);
        return Ok((Box::new(client), model_id));
    }

    #[cfg(feature = "cloud")]
    if let Some(anthropic_model) = model.strip_prefix("anthropic:") {
        let key = credentials::lookup("ANTHROPIC_API_KEY").context(
            "ANTHROPIC_API_KEY must be set (or stored via `lawlint init`) for \
             anthropic:<model> backends",
        )?;
        let client = axllm::ai(
            "anthropic",
            json!({ "model": anthropic_model, "api_key": key }),
        )
        .map_err(|e| anyhow::anyhow!("failed to build anthropic ax client: {e}"))?;
        return Ok((Box::new(client), model.to_string()));
    }

    #[cfg(feature = "cloud")]
    if let Some(spec) = model.strip_prefix("openai:") {
        let (base_url, openai_model) = spec.rsplit_once('#').with_context(|| {
            format!("openai model {model:?} must be \"openai:<base-url>#<model>\"")
        })?;
        let key = credentials::lookup("OPENAI_API_KEY").unwrap_or_else(|| "unused".to_string());
        let mut client = axllm::OpenAICompatibleClient::new(key, openai_model)
            .with_api_url(base_url.to_string());
        client.base_url_override = Some(base_url.to_string());
        return Ok((Box::new(client), model.to_string()));
    }

    #[cfg(feature = "cloud")]
    if let Some(deployment) = model.strip_prefix("foundry:") {
        if deployment.is_empty() {
            bail!("foundry model must be \"foundry:<deployment>\"");
        }
        let client = FoundryClient::from_credentials(Some(deployment.to_string()))
            .map_err(|e| anyhow::anyhow!("failed to build foundry client: {e}"))?;
        return Ok((Box::new(client), model.to_string()));
    }

    #[cfg(not(feature = "cloud"))]
    if model.starts_with("anthropic:")
        || model.starts_with("openai:")
        || model.starts_with("foundry:")
    {
        bail!(
            "model {model:?} needs the `cloud` feature — \
             rebuild lawlint-judge with `--features cloud`"
        );
    }

    bail!(
        "unknown model {model:?} — use \"local[:<hf-repo>[#<gguf-file>]]\", \
         \"anthropic:<model>\", \"openai:<base-url>#<model>\", or \"foundry:<deployment>\""
    )
}

/// Create a judge from `JudgeOptions.model`. See [`create_client`] for the
/// spec grammar. There is no default backend (#50): `model: None` means the
/// user never configured one, and silently downloading a multi-GB local
/// model would be the wrong surprise — it errors with `lawlint init`
/// guidance instead. Explicit specs (including `local:`) always work.
pub fn create_judge(options: &JudgeOptions) -> anyhow::Result<Box<dyn Judge>> {
    let Some(model) = options.model.as_deref() else {
        bail!(
            "no AI model is configured — run `lawlint init` to choose one \
             (hosted providers recommended), or pass an explicit spec such as \
             \"anthropic:<model>\" or \"local:<hf-repo>\""
        );
    };
    let (client, model_id) = create_client(model)?;
    Ok(Box::new(AxJudge::new(client, model_id)))
}

// ------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex as StdMutex};

    // A scripted AxAIClient. Implementing `axllm::AxAIClient` here is the
    // compile-checked round-trip guarding against axllm upgrades: if the
    // pinned trait shape changes, this file stops compiling.
    struct FakeClient {
        responses: VecDeque<AxResult<Value>>,
        requests: Arc<StdMutex<Vec<Value>>>,
    }

    impl FakeClient {
        fn new(responses: Vec<AxResult<Value>>) -> (Self, Arc<StdMutex<Vec<Value>>>) {
            let requests = Arc::new(StdMutex::new(Vec::new()));
            (
                FakeClient {
                    responses: responses.into(),
                    requests: Arc::clone(&requests),
                },
                requests,
            )
        }
    }

    impl AxAIClient for FakeClient {
        fn chat(&mut self, request: Value) -> AxResult<Value> {
            self.requests.lock().unwrap().push(request);
            self.responses
                .pop_front()
                .unwrap_or_else(|| Err(AxError::runtime("fake client exhausted")))
        }
    }

    fn req(prompt: &str) -> JudgeRequest {
        JudgeRequest {
            chunk_range: lawlint_core::TextRange { start: 0, end: 10 },
            chunk_text: "chunk text".to_string(),
            rules: vec![lawlint_core::RuleId("core/empty-hedge".to_string())],
            prompt: prompt.to_string(),
            cache_key_base: "base".to_string(),
        }
    }

    fn choices_response(content: &str) -> AxResult<Value> {
        Ok(json!({
            "choices": [{"index": 0, "message": {"role": "assistant", "content": content}}]
        }))
    }

    const VALID: &str = r#"[{"rule": "core/empty-hedge", "quote": "could perhaps", "explanation": "hedge", "confidence": 0.8, "suggested_rewrite": null}]"#;

    /// The parse as `Judge::evaluate` sees it: guard drops discarded.
    fn parse_findings(content: &str) -> Result<Vec<JudgeFinding>, JudgeError> {
        parse_findings_with_stats(content).map(|parsed| parsed.kept)
    }

    // ---- chat request shape (axllm contract round-trip) -----------------

    #[test]
    fn evaluate_sends_openai_chat_completions_shaped_request() {
        let (client, requests) = FakeClient::new(vec![choices_response("[]")]);
        let judge = AxJudge::new(Box::new(client), "test-model");
        judge.evaluate(&req("PROMPT TEXT")).unwrap();

        let sent = requests.lock().unwrap();
        assert_eq!(sent.len(), 1);
        let messages = sent[0]["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "PROMPT TEXT");
        assert_eq!(sent[0]["temperature"], 0);
    }

    // ---- response parsing ----------------------------------------------

    #[test]
    fn parses_findings_from_choices_shaped_response() {
        let (client, _) = FakeClient::new(vec![choices_response(VALID)]);
        let judge = AxJudge::new(Box::new(client), "m");
        let findings = judge.evaluate(&req("p")).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, "core/empty-hedge");
        assert_eq!(findings[0].quote, "could perhaps");
        assert_eq!(findings[0].confidence, 0.8);
        assert!(findings[0].suggested_rewrite.is_none());
    }

    #[test]
    fn parses_findings_from_ax_results_shaped_response() {
        // Stock ax clients return ax's normalized shape, not raw OpenAI.
        let (client, _) = FakeClient::new(vec![Ok(json!({ "results": [{"content": VALID}] }))]);
        let judge = AxJudge::new(Box::new(client), "m");
        let findings = judge.evaluate(&req("p")).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, "core/empty-hedge");
    }

    #[test]
    fn parses_fenced_and_prose_wrapped_json() {
        let fenced = format!("```json\n{VALID}\n```");
        let prose = format!("Here are the findings:\n{VALID}\nDone.");
        for content in [fenced.as_str(), prose.as_str()] {
            let (client, _) = FakeClient::new(vec![choices_response(content)]);
            let judge = AxJudge::new(Box::new(client), "m");
            let findings = judge.evaluate(&req("p")).unwrap();
            assert_eq!(findings.len(), 1, "content: {content}");
        }
    }

    #[test]
    fn lenient_parse_defaults_optional_fields() {
        let minimal = r#"[{"rule": "core/empty-hedge", "quote": "q"}]"#;
        let (client, _) = FakeClient::new(vec![choices_response(minimal)]);
        let judge = AxJudge::new(Box::new(client), "m");
        let findings = judge.evaluate(&req("p")).unwrap();
        assert_eq!(findings[0].explanation, "");
        assert_eq!(findings[0].confidence, 0.5); // below core's default floor
        assert!(findings[0].suggested_rewrite.is_none());
    }

    #[test]
    fn missing_rule_or_quote_is_malformed() {
        let no_quote = r#"[{"rule": "core/empty-hedge", "explanation": "e"}]"#;
        let (client, _) = FakeClient::new(vec![choices_response(no_quote)]);
        let judge = AxJudge::new(Box::new(client), "m");
        assert!(matches!(
            judge.evaluate(&req("p")),
            Err(JudgeError::MalformedResponse(_))
        ));
    }

    #[test]
    fn empty_array_is_ok_and_empty() {
        let (client, _) = FakeClient::new(vec![choices_response("  []  ")]);
        let judge = AxJudge::new(Box::new(client), "m");
        assert!(judge.evaluate(&req("p")).unwrap().is_empty());
    }

    // ---- verdict-polarity guard (#39 part 1) ----------------------------

    #[test]
    fn canned_negative_verdict_response_is_dropped() {
        // Regression (#39): on clean chunks the small local model emits its
        // pass verdict as a finding — quoting real text with confidence ≥ 0.6,
        // so core's confidence floor and grounding both pass it.
        let canned = r#"[{"rule": "core/empty-hedge", "quote": "the parties shall meet", "explanation": "The text does not flag any empty hedge.", "confidence": 0.9, "suggested_rewrite": null}]"#;
        assert!(parse_findings(canned).unwrap().is_empty());
    }

    #[test]
    fn negative_verdict_variants_are_dropped() {
        for (rule, explanation) in [
            (
                "core/empty-hedge",
                "The text does not flag any empty hedge.",
            ),
            ("core/empty-hedge", "No empty hedge found in this passage."),
            ("core/empty-hedge", "This is not an empty hedge."),
            (
                "core/empty-hedge",
                "The sentence does not violate the rule.",
            ),
            ("core/empty-hedge", "No violations found."),
            ("core/empty-hedge", "The paragraph complies with the rule."),
            (
                "core/padded-elaboration",
                "There is no padded elaboration here.",
            ),
            (
                "core/padded-elaboration",
                "This is not a padded elaboration.",
            ),
        ] {
            let content = format!(
                r#"[{{"rule": "{rule}", "quote": "q", "explanation": "{explanation}", "confidence": 0.9}}]"#
            );
            assert!(
                parse_findings(&content).unwrap().is_empty(),
                "not dropped: {explanation}"
            );
        }
    }

    #[test]
    fn positive_findings_with_incidental_negation_survive() {
        // The guard must prefer false negatives: real explanations often
        // contain negations describing the violation, not negating it.
        for (rule, explanation) in [
            ("core/empty-hedge", "This hedge adds no information."),
            (
                "core/empty-hedge",
                "The sentence hedges but does not commit to any position.",
            ),
            (
                "core/empty-hedge",
                "An empty hedge: the qualifier carries no substance.",
            ),
            (
                "core/padded-elaboration",
                "Padded elaboration that restates the point without new content.",
            ),
        ] {
            let content = format!(
                r#"[{{"rule": "{rule}", "quote": "q", "explanation": "{explanation}", "confidence": 0.9}}]"#
            );
            assert_eq!(
                parse_findings(&content).unwrap().len(),
                1,
                "wrongly dropped: {explanation}"
            );
        }
    }

    #[test]
    fn negative_verdict_dropped_alongside_kept_real_finding() {
        let mixed = r#"[
            {"rule": "core/empty-hedge", "quote": "could perhaps", "explanation": "Hedge that adds no information.", "confidence": 0.8},
            {"rule": "core/padded-elaboration", "quote": "as noted above", "explanation": "The text does not violate this rule.", "confidence": 0.9}
        ]"#;
        let findings = parse_findings(mixed).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, "core/empty-hedge");
    }

    // ---- stats-exposing parse (#39 part 2) -------------------------------

    #[test]
    fn parse_with_stats_partitions_negative_verdicts() {
        let mixed = r#"[
            {"rule": "core/empty-hedge", "quote": "could perhaps", "explanation": "Hedge that adds no information.", "confidence": 0.8},
            {"rule": "core/padded-elaboration", "quote": "as noted above", "explanation": "The text does not violate this rule.", "confidence": 0.9},
            {"rule": "core/empty-hedge", "quote": "the parties shall meet", "explanation": "No empty hedge found in this passage.", "confidence": 0.9}
        ]"#;
        let parsed = parse_findings_with_stats(mixed).unwrap();
        assert_eq!(parsed.kept.len(), 1);
        assert_eq!(parsed.kept[0].rule, "core/empty-hedge");
        assert_eq!(parsed.kept[0].quote, "could perhaps");
        assert_eq!(parsed.dropped_negative.len(), 2);
        // Dropped findings keep their full content — the harness caches and
        // recounts them across runs.
        assert_eq!(parsed.dropped_negative[0].rule, "core/padded-elaboration");
        assert_eq!(
            parsed.dropped_negative[1].explanation,
            "No empty hedge found in this passage."
        );
        // The silent public parse sees exactly the kept set.
        let silent = parse_findings(mixed).unwrap();
        assert_eq!(silent.len(), 1);
        assert_eq!(silent[0].quote, parsed.kept[0].quote);
    }

    #[test]
    fn parse_with_stats_clean_response_has_no_drops() {
        let parsed = parse_findings_with_stats("[]").unwrap();
        assert!(parsed.kept.is_empty());
        assert!(parsed.dropped_negative.is_empty());
    }

    #[test]
    fn parse_with_stats_malformed_still_errors() {
        assert!(matches!(
            parse_findings_with_stats("I found no problems."),
            Err(JudgeError::MalformedResponse(_))
        ));
    }

    #[test]
    fn evaluate_with_stats_surfaces_dropped_negatives() {
        let canned = r#"[
            {"rule": "core/empty-hedge", "quote": "could perhaps", "explanation": "hedge", "confidence": 0.8},
            {"rule": "core/empty-hedge", "quote": "q", "explanation": "The text does not flag any empty hedge.", "confidence": 0.9}
        ]"#;
        let (client, _) = FakeClient::new(vec![choices_response(canned)]);
        let judge = AxJudge::new(Box::new(client), "m");
        let parsed = judge.evaluate_with_stats(&req("p")).unwrap();
        assert_eq!(parsed.kept.len(), 1);
        assert_eq!(parsed.dropped_negative.len(), 1);
    }

    #[test]
    fn missing_explanation_is_not_a_negative_verdict() {
        // Lenient parse defaults explanation to "" — the guard only fires on
        // explicit negation, so such findings pass through (core's confidence
        // floor handles under-specified findings).
        let minimal = r#"[{"rule": "core/empty-hedge", "quote": "q", "confidence": 0.9}]"#;
        assert_eq!(parse_findings(minimal).unwrap().len(), 1);
    }

    #[test]
    fn malformed_json_surfaces_malformed_response() {
        let (client, _) = FakeClient::new(vec![choices_response("I found no problems.")]);
        let judge = AxJudge::new(Box::new(client), "m");
        match judge.evaluate(&req("p")) {
            Err(JudgeError::MalformedResponse(raw)) => {
                assert!(raw.contains("I found no problems."))
            }
            other => panic!("expected MalformedResponse, got {other:?}"),
        }
    }

    #[test]
    fn backend_error_surfaces_backend() {
        let (client, _) = FakeClient::new(vec![Err(AxError::runtime("connection refused"))]);
        let judge = AxJudge::new(Box::new(client), "m");
        match judge.evaluate(&req("p")) {
            Err(JudgeError::Backend(msg)) => assert!(msg.contains("connection refused")),
            other => panic!("expected Backend, got {other:?}"),
        }
    }

    #[test]
    fn response_without_content_is_malformed() {
        let (client, _) = FakeClient::new(vec![Ok(json!({"unexpected": true}))]);
        let judge = AxJudge::new(Box::new(client), "m");
        assert!(matches!(
            judge.evaluate(&req("p")),
            Err(JudgeError::MalformedResponse(_))
        ));
    }

    // ---- end-to-end through core's run_judge (retry contract) -----------

    #[test]
    fn core_run_judge_retries_then_fails_chunk_closed_on_malformed() {
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let client = FakeClient {
            responses: vec![
                choices_response("not json at all"),
                choices_response("still not json"),
            ]
            .into(),
            requests: Arc::clone(&calls),
        };
        let judge = AxJudge::new(Box::new(client), "m");
        let r = req("p");
        let (out, stats) =
            lawlint_core::run_judge(&judge, None, std::slice::from_ref(&r), "chunk text");
        assert!(out.is_empty());
        assert_eq!(stats.chunks_failed, 1);
        assert_eq!(calls.lock().unwrap().len(), 2); // exactly one retry
    }

    #[test]
    fn core_run_judge_grounds_valid_findings() {
        let (client, _) = FakeClient::new(vec![choices_response(
            r#"[{"rule": "core/empty-hedge", "quote": "could perhaps", "explanation": "hedge", "confidence": 0.9, "suggested_rewrite": null}]"#,
        )]);
        let judge = AxJudge::new(Box::new(client), "m");
        let source = "It could perhaps be argued.";
        let r = JudgeRequest {
            chunk_range: lawlint_core::TextRange {
                start: 0,
                end: source.len(),
            },
            chunk_text: source.to_string(),
            rules: vec![lawlint_core::RuleId("core/empty-hedge".to_string())],
            prompt: "p".to_string(),
            cache_key_base: "base".to_string(),
        };
        let (out, stats) = lawlint_core::run_judge(&judge, None, &[r], source);
        assert_eq!(stats.grounded, 1);
        assert_eq!(out[0].2.slice(source), "could perhaps");
    }

    // ---- create_judge routing -------------------------------------------

    #[test]
    fn create_judge_unconfigured_errors_with_init_guidance() {
        // #50: no silent local default — an unconfigured judge must error
        // with actionable guidance, never start a model download.
        let Err(err) = create_judge(&JudgeOptions::default()) else {
            panic!("expected error for unconfigured model");
        };
        let err = err.to_string();
        assert!(err.contains("lawlint init"), "{err}");
        assert!(err.contains("no AI model is configured"), "{err}");
    }

    #[test]
    fn create_judge_local_with_repo_and_file() {
        let opts = |model: &str| JudgeOptions {
            model: Some(model.to_string()),
            ..Default::default()
        };
        assert_eq!(
            create_judge(&opts("local")).unwrap().model_id(),
            format!("local:{DEFAULT_LOCAL_REPO}")
        );
        assert_eq!(
            create_judge(&opts("local:foo/bar-GGUF"))
                .unwrap()
                .model_id(),
            "local:foo/bar-GGUF"
        );
        assert_eq!(
            create_judge(&opts("local:foo/bar-GGUF#m-q4_0.gguf"))
                .unwrap()
                .model_id(),
            "local:foo/bar-GGUF#m-q4_0.gguf"
        );
    }

    #[test]
    fn create_judge_unknown_scheme_errors() {
        let opts = JudgeOptions {
            model: Some("bogus:whatever".to_string()),
            ..Default::default()
        };
        let Err(err) = create_judge(&opts) else {
            panic!("expected error for unknown scheme");
        };
        let err = err.to_string();
        assert!(err.contains("bogus:whatever"));
        assert!(err.contains("local"));
    }

    #[cfg(not(feature = "cloud"))]
    #[test]
    fn create_judge_cloud_schemes_error_without_cloud_feature() {
        for model in [
            "anthropic:claude-sonnet-4-5",
            "openai:http://localhost:8080/v1#m",
            "foundry:gpt-5.5",
        ] {
            let opts = JudgeOptions {
                model: Some(model.to_string()),
                ..Default::default()
            };
            let Err(err) = create_judge(&opts) else {
                panic!("expected error without cloud feature for {model}");
            };
            assert!(err.to_string().contains("cloud"), "{err}");
        }
    }

    #[cfg(feature = "cloud")]
    #[test]
    fn create_judge_openai_scheme_routes_with_cloud_feature() {
        let opts = JudgeOptions {
            model: Some("openai:http://localhost:8080/v1#qwen".to_string()),
            ..Default::default()
        };
        let judge = create_judge(&opts).unwrap();
        assert_eq!(judge.model_id(), "openai:http://localhost:8080/v1#qwen");

        // Malformed spec (no '#') errors.
        let bad = JudgeOptions {
            model: Some("openai:http://localhost:8080/v1".to_string()),
            ..Default::default()
        };
        assert!(create_judge(&bad).is_err());
    }

    #[cfg(feature = "cloud")]
    #[test]
    fn create_judge_foundry_scheme_requires_deployment() {
        // Empty deployment fails before any credential lookup.
        let opts = JudgeOptions {
            model: Some("foundry:".to_string()),
            ..Default::default()
        };
        let Err(err) = create_judge(&opts) else {
            panic!("expected error for empty foundry deployment");
        };
        let err = err.to_string();
        assert!(err.contains("foundry:<deployment>"), "{err}");
    }

    // ---- helpers ---------------------------------------------------------

    #[test]
    fn strip_code_fences_variants() {
        assert_eq!(strip_code_fences("[]"), "[]");
        assert_eq!(strip_code_fences("```json\n[]\n```"), "[]");
        assert_eq!(strip_code_fences("```\n[]\n```"), "[]");
        assert_eq!(strip_code_fences("  ```json\n[1]\n```  "), "[1]");
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("héllo", 10), "héllo");
        assert_eq!(truncate("ééééé", 2), "éé…");
    }
}
