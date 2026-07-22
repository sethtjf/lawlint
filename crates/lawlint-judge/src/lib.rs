//! lawlint-judge — tier-3 judge backends (native only). Design doc §10.
//!
//! **ax (`axllm`) is the AI interface for ALL backends.** The judge is one
//! [`AxJudge`] whose prompt/JSON-contract layer is backend-independent;
//! backends are [`axllm::AxAIClient`] implementations, and every one of them
//! is an HTTP client:
//!
//! - Stock ax clients for Anthropic and any OpenAI-compatible endpoint.
//! - [`FoundryClient`] for Azure AI Foundry.
//!
//! **There is no in-process inference.** lawlint used to embed a mistral.rs
//! runtime behind `local:` specs; it was removed because the models small
//! enough to embed cannot do this job — on lawlint's own eval the default
//! scored F1 0.111 on `empty-hedge` and 0.000 on `padded-elaboration`, failing
//! 38 of 330 chunks outright (docs/eval-corpus.md). Running privately is still
//! supported and now works better: point `openai:<base-url>#<model>` at a local
//! OpenAI-compatible server such as Ollama or vLLM, which can serve a far
//! larger model than lawlint could reasonably bundle. That path needs no API
//! key — see [`credentials_ready`].
//!
//! `AxJudge` speaks OpenAI chat-completions-shaped payloads through
//! `AxAIClient::chat` and hand-rolls the `JudgeFinding[]` JSON parse from
//! `JudgeRequest.prompt` (core owns the canonical prompt and cache keys; ax's
//! signature layer would substitute its own prompt, so it is not used here —
//! see the crate README note in the repo docs). ax v23's `chat` is blocking,
//! which is why every backend here is a blocking HTTP client.

mod cache;
pub mod credentials;
mod foundry;

pub use cache::DiskCache;
pub use foundry::FoundryClient;

// Re-export the ax boundary so consumers can supply custom backends.
pub use axllm::{AxAIClient, AxError, AxResult};

use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Mutex;

use anyhow::{bail, Context};
use lawlint_core::{Judge, JudgeError, JudgeFinding, JudgeOptions, JudgePlan, JudgeRequest};
use serde::Deserialize;
use serde_json::{json, Value};

/// What to tell someone whose config still names the removed local backend.
/// Their intent was almost certainly privacy, so the message leads with the
/// replacement that preserves it rather than with the removal.
const LOCAL_REMOVED: &str = "in-process local models were removed in 0.9: the models small enough \
     to embed could not judge these rules (docs/eval-corpus.md). To keep text \
     on your machine, run an OpenAI-compatible server such as Ollama and use \
     \"openai:http://localhost:11434/v1#<model>\" — no API key needed. \
     Otherwise pick a hosted spec: \"anthropic:<model>\" or \"foundry:<deployment>\"";

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
    // and requires Send + Sync — hence the mutexes.
    //
    // One client per concurrent worker. A single client behind one mutex
    // would serialize every request no matter how many workers core spawns,
    // which is exactly the bottleneck this pool exists to remove. Backends
    // that cannot be duplicated would get a pool of one and stay sequential.
    clients: Vec<Mutex<Box<dyn AxAIClient + Send>>>,
    /// Round-robin starting point, so concurrent callers don't all contend on
    /// the first client before finding a free one.
    cursor: AtomicUsize,
    model_id: String,
    /// Per-call generation cap sent to the backend; `None` leaves the
    /// backend's own default in force.
    max_tokens: Option<usize>,
}

impl AxJudge {
    pub fn new(client: Box<dyn AxAIClient + Send>, model_id: impl Into<String>) -> Self {
        Self::with_clients(vec![client], model_id)
    }

    /// An `AxJudge` over a pool of interchangeable clients, one per concurrent
    /// worker. Panics on an empty pool: a judge with no client can only fail
    /// every chunk, and doing so lazily would report a backend error instead
    /// of the construction bug it is.
    pub fn with_clients(
        clients: Vec<Box<dyn AxAIClient + Send>>,
        model_id: impl Into<String>,
    ) -> Self {
        assert!(!clients.is_empty(), "AxJudge needs at least one client");
        AxJudge {
            clients: clients.into_iter().map(Mutex::new).collect(),
            cursor: AtomicUsize::new(0),
            model_id: model_id.into(),
            max_tokens: None,
        }
    }

    /// Cap generated tokens per chunk (config `judge.maxTokens`). Reasoning
    /// backends need headroom for hidden thinking on top of the findings
    /// array; see [`FoundryClient`]'s default.
    pub fn with_max_tokens(mut self, max_tokens: Option<usize>) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// The OpenAI chat-completions-shaped request sent to the backend.
    /// `model` is intentionally omitted: ax clients default to their
    /// configured model.
    /// `max_completion_tokens` is emitted only when configured, so backends
    /// keep their own defaults otherwise.
    fn chat_request(&self, req: &JudgeRequest) -> Value {
        let mut request = json!({
            "messages": [{"role": "user", "content": req.prompt}],
            "temperature": 0,
        });
        if let Some(max_tokens) = self.max_tokens {
            request["max_completion_tokens"] = json!(max_tokens);
        }
        request
    }

    /// Borrow a free client, else block on one. Starting the scan at a
    /// rotating cursor keeps concurrent callers from all queueing behind
    /// client 0 while later clients sit idle.
    fn checked_out(&self) -> std::sync::MutexGuard<'_, Box<dyn AxAIClient + Send>> {
        let count = self.clients.len();
        let start = self.cursor.fetch_add(1, AtomicOrdering::Relaxed);
        for offset in 0..count {
            let client = &self.clients[(start + offset) % count];
            if let Ok(guard) = client.try_lock() {
                return guard;
            }
        }
        // Every client is busy: wait for the one this call was assigned.
        // A poisoned mutex means another worker panicked mid-chat; the client
        // is still usable for the next request, so recover rather than
        // failing every remaining chunk.
        self.clients[start % count]
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl AxJudge {
    /// `Judge::evaluate`, but returning the findings dropped by the
    /// verdict-polarity guard alongside the kept ones. The #39 judged-eval
    /// harness measures the verdict-discipline rate from the dropped set;
    /// `evaluate` discards it, so trait behavior is unchanged.
    pub fn evaluate_with_stats(&self, req: &JudgeRequest) -> Result<ParsedFindings, JudgeError> {
        let request = self.chat_request(req);
        let response = self
            .checked_out()
            .chat(request)
            .map_err(|e| JudgeError::Backend(e.to_string()))?;
        let content = chat_content(&response).ok_or_else(|| {
            JudgeError::MalformedResponse(format!(
                "no textual content in chat response: {}",
                truncate(&response.to_string(), 300)
            ))
        })?;
        // Empty content with a length stop is a truncated generation, not
        // malformed JSON: a reasoning model spent the whole budget thinking.
        // Naming that distinctly is the difference between an actionable
        // error and a bare "the judge failed".
        if content.trim().is_empty() {
            let truncated = finish_reason(&response).is_some_and(|reason| reason == "length");
            return Err(JudgeError::MalformedResponse(if truncated {
                "model generated no output before hitting its token cap — \
                 raise `judge.maxTokens` in .lawlint/config.json (reasoning \
                 models spend the cap on hidden thinking)"
                    .to_string()
            } else {
                "model returned empty content".to_string()
            }));
        }
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

    fn max_concurrency(&self) -> usize {
        self.clients.len()
    }
}

/// Extract assistant text from a chat response. Accepts both the OpenAI
/// chat-completions shape (`choices[0].message.content` — any
/// OpenAI-compatible passthrough) and ax's normalized internal shape
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

/// Why the backend stopped generating, when it says. `"length"` means the
/// token cap was hit mid-generation — the response is truncated, not wrong.
pub fn finish_reason(response: &Value) -> Option<&str> {
    response
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("finish_reason"))
        .and_then(Value::as_str)
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

/// Lenient finding shape: smaller models sometimes omit optional fields.
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

/// Verdict-polarity guard (#39): smaller models sometimes emit their
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

/// Why a model spec cannot run right now. `None` from [`credentials_ready`]
/// means it can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotReady {
    /// A hosted backend whose key is in neither the environment nor the
    /// credential store. Carries the variable name(s) to name in the message.
    MissingKey(&'static str),
}

/// Whether `model` has the credentials it needs, without building a client or
/// touching the network. This is what lets the CLI run AI rules automatically
/// when they *can* run and skip them quietly when they cannot, instead of
/// erroring at call time.
///
/// A self-hosted `openai:` endpoint needs no credentials and is always
/// "ready" here, which is what lets the private path run with no setup beyond
/// the server itself.
pub fn credentials_ready(model: &str) -> Result<(), NotReady> {
    let need = |name: &'static str| {
        if credentials::lookup(name).is_some() {
            Ok(())
        } else {
            Err(NotReady::MissingKey(name))
        }
    };
    if model.starts_with("anthropic:") {
        return need("ANTHROPIC_API_KEY");
    }
    if model.starts_with("foundry:") {
        // The endpoint is the one that has no sensible default; check it first
        // so the message names the thing the user is most likely missing.
        need("AZURE_FOUNDRY_ENDPOINT")?;
        return need("AZURE_FOUNDRY_API_KEY");
    }
    // `openai:` covers self-hosted OpenAI-compatible servers, which routinely
    // need no key at all — create_client falls back to "unused" — so an absent
    // OPENAI_API_KEY is not a reason to skip.
    Ok(())
}

/// Build the raw ax client (plus its canonical model id) for a model spec —
/// the shared backend factory behind [`create_judge`] and any other AI
/// feature that speaks through the ax boundary (e.g. `lawlint learn`):
///
/// - `"anthropic:<model>"`: stock ax Anthropic client; requires
///   `ANTHROPIC_API_KEY` (environment or credential store).
/// - `"openai:<base-url>#<model>"`: stock ax OpenAI-compatible client. Covers
///   both api.openai.com and any self-hosted server — Ollama, vLLM,
///   llama.cpp — which is how lawlint runs without text leaving the machine.
///   Uses `OPENAI_API_KEY` when set, and works without one.
/// - `"foundry:<deployment>"`: [`FoundryClient`]; requires
///   `AZURE_FOUNDRY_ENDPOINT` + `AZURE_FOUNDRY_API_KEY`.
///
/// A `local:` spec is rejected with migration guidance rather than an
/// "unknown model" error: it used to work, so someone hitting it has a config
/// to fix, not a typo.
///
/// Hosted-provider keys resolve environment-first, then the user-level
/// credential store written by `lawlint init` ([`credentials`]).
pub fn create_client(model: &str) -> anyhow::Result<(Box<dyn AxAIClient + Send>, String)> {
    if model == "local" || model.starts_with("local:") {
        bail!("{LOCAL_REMOVED}");
    }

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

    if let Some(deployment) = model.strip_prefix("foundry:") {
        if deployment.is_empty() {
            bail!("foundry model must be \"foundry:<deployment>\"");
        }
        let client = FoundryClient::from_credentials(Some(deployment.to_string()))
            .map_err(|e| anyhow::anyhow!("failed to build foundry client: {e}"))?;
        return Ok((Box::new(client), model.to_string()));
    }

    bail!(
        "unknown model {model:?} — use \"anthropic:<model>\", \
         \"openai:<base-url>#<model>\", or \"foundry:<deployment>\""
    )
}

/// Default generation budget per request for a hosted backend.
///
/// Sized for reasoning deployments: on OpenAI-compatible routes this budget
/// also covers hidden reasoning tokens, and a thinking model that exhausts it
/// returns empty content and fails every chunk closed. Only tokens actually
/// generated are billed, so the headroom is free for non-reasoning models.
pub const DEFAULT_HOSTED_MAX_TOKENS: usize = 16_384;

/// Document chars per request. Deliberately far below any modern context
/// window: the limit on unit size is not what the model can read but how much
/// re-linting an edit should invalidate, since a unit is the cache granule.
/// ~24k chars is roughly a 4,000-word document — one request for most briefs
/// and memos, a handful for a long one.
pub const DEFAULT_CONTEXT_CHARS: usize = 24_000;

/// Default concurrent requests. Judge requests are network-bound, not
/// CPU-bound, so this tracks provider rate limits rather than core count; 4
/// keeps a document's requests overlapping without looking like a burst.
/// Override with `judge.concurrency`.
pub const DEFAULT_CONCURRENCY: usize = 4;

/// The default request-shaping profile: large units, one request per rule.
///
/// Every backend is now a remote endpoint, so there is nothing to branch on.
/// A small self-hosted model behind `openai:` may prefer the conservative
/// shape — smaller units, rubrics bundled — which is what
/// `judge.contextChars` and `judge.perRule` are for.
pub fn default_plan() -> JudgePlan {
    JudgePlan::for_context(DEFAULT_CONTEXT_CHARS)
}

/// How many clients to pool, honoring a configured override.
pub fn concurrency(configured: Option<usize>) -> usize {
    configured.unwrap_or(DEFAULT_CONCURRENCY).max(1)
}

/// Create a judge from `JudgeOptions.model`. See [`create_client`] for the
/// spec grammar. There is no default backend: `model: None` means the user
/// never configured one, and guessing a provider they may not have a key for
/// would be the wrong surprise — it errors with `lawlint init` guidance
/// instead.
///
/// Builds one client per concurrent worker ([`concurrency`]); clients are
/// independent, so this costs a little setup and no network traffic.
pub fn create_judge(options: &JudgeOptions) -> anyhow::Result<Box<dyn Judge>> {
    let Some(model) = options.model.as_deref() else {
        bail!(
            "no AI model is configured — run `lawlint init` to choose one, \
             or pass an explicit spec such as \"anthropic:<model>\" or \
             \"openai:http://localhost:11434/v1#<model>\""
        );
    };
    let (client, model_id) = create_client(model)?;
    let mut clients = vec![client];
    for _ in 1..concurrency(options.concurrency) {
        clients.push(create_client(model)?.0);
    }
    Ok(Box::new(
        AxJudge::with_clients(clients, model_id).with_max_tokens(options.max_tokens),
    ))
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
        // Unset by default, so each backend keeps its own budget.
        assert!(sent[0].get("max_completion_tokens").is_none());
    }

    /// A single-client judge must stay sequential, and a pool must advertise
    /// its real width — core spawns exactly this many workers.
    #[test]
    fn max_concurrency_reflects_the_client_pool() {
        let one = AxJudge::new(Box::new(FakeClient::new(vec![]).0), "m");
        assert_eq!(one.max_concurrency(), 1);

        let clients: Vec<Box<dyn AxAIClient + Send>> = (0..3)
            .map(|_| Box::new(FakeClient::new(vec![]).0) as Box<dyn AxAIClient + Send>)
            .collect();
        assert_eq!(AxJudge::with_clients(clients, "m").max_concurrency(), 3);
    }

    /// Every client in the pool must be reachable, or a pool of N would
    /// serialize onto client 0 and the concurrency would be a lie.
    #[test]
    fn pool_rotates_across_clients() {
        let mut clients: Vec<Box<dyn AxAIClient + Send>> = Vec::new();
        let mut logs = Vec::new();
        for _ in 0..3 {
            let (client, requests) =
                FakeClient::new(vec![choices_response("[]"), choices_response("[]")]);
            logs.push(requests);
            clients.push(Box::new(client));
        }
        let judge = AxJudge::with_clients(clients, "m");
        for _ in 0..3 {
            judge.evaluate(&req("p")).unwrap();
        }
        let used = logs
            .iter()
            .filter(|r| !r.lock().unwrap().is_empty())
            .count();
        assert_eq!(used, 3, "three sequential calls should touch three clients");
    }

    #[test]
    #[should_panic(expected = "at least one client")]
    fn empty_pool_panics() {
        AxJudge::with_clients(Vec::new(), "m");
    }

    #[test]
    fn concurrency_honors_the_override_and_never_reaches_zero() {
        assert_eq!(concurrency(None), DEFAULT_CONCURRENCY);
        assert_eq!(concurrency(Some(2)), 2);
        // 0 workers would mean no requests ever run.
        assert_eq!(concurrency(Some(0)), 1);
    }

    #[test]
    fn default_plan_is_large_units_one_request_per_rule() {
        let plan = default_plan();
        assert!(plan.per_rule);
        assert_eq!(plan.context_chars, DEFAULT_CONTEXT_CHARS);
    }

    /// A `local:` spec used to work, so someone hitting it has a config to
    /// migrate. The error has to name the replacement that preserves what they
    /// were after — keeping the text on their machine — not just refuse.
    #[test]
    fn local_specs_fail_with_migration_guidance() {
        for model in ["local", "local:Qwen/Qwen2.5-1.5B-Instruct-GGUF"] {
            let Err(err) = create_client(model) else {
                panic!("{model} should no longer build a client");
            };
            let err = err.to_string();
            assert!(err.contains("openai:http://localhost:11434/v1"), "{err}");
            assert!(err.contains("no API key"), "{err}");
            // Not the generic "unknown model" path.
            assert!(!err.contains("unknown model"), "{err}");
        }
    }

    #[test]
    fn unknown_specs_still_get_the_generic_error() {
        let Err(err) = create_client("bogus:whatever") else {
            panic!("expected an error for an unknown scheme");
        };
        let err = err.to_string();
        assert!(err.contains("unknown model"), "{err}");
        // The removed backend must not be advertised as an option.
        assert!(!err.contains("local["), "{err}");
    }

    #[test]
    fn configured_max_tokens_rides_along_on_the_request() {
        let (client, requests) = FakeClient::new(vec![choices_response("[]")]);
        let judge = AxJudge::new(Box::new(client), "test-model").with_max_tokens(Some(16384));
        judge.evaluate(&req("PROMPT TEXT")).unwrap();
        assert_eq!(requests.lock().unwrap()[0]["max_completion_tokens"], 16384);
    }

    /// A reasoning model that spends its whole budget thinking returns
    /// `finish_reason: "length"` with empty content. That is a budget
    /// problem, and the error has to say so — a generic parse failure sends
    /// the user looking at their rules or their API key instead.
    #[test]
    fn truncated_empty_generation_names_the_token_cap() {
        let truncated = || {
            Ok(json!({
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": ""},
                    "finish_reason": "length",
                }]
            }))
        };
        let (client, _) = FakeClient::new(vec![truncated()]);
        let judge = AxJudge::new(Box::new(client), "m");
        let Err(err) = judge.evaluate(&req("p")) else {
            panic!("expected an error for empty truncated content");
        };
        let err = err.to_string();
        assert!(err.contains("maxTokens"), "{err}");
        assert!(err.contains("token cap"), "{err}");
    }

    #[test]
    fn empty_content_without_truncation_is_reported_as_empty() {
        let (client, _) = FakeClient::new(vec![choices_response("   ")]);
        let judge = AxJudge::new(Box::new(client), "m");
        let Err(err) = judge.evaluate(&req("p")) else {
            panic!("expected an error for empty content");
        };
        let err = err.to_string();
        assert!(err.contains("empty content"), "{err}");
        assert!(!err.contains("maxTokens"), "{err}");
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
        // An unconfigured judge must error with actionable guidance rather
        // than guessing a provider the user may have no key for.
        let Err(err) = create_judge(&JudgeOptions::default()) else {
            panic!("expected error for unconfigured model");
        };
        let err = err.to_string();
        assert!(err.contains("lawlint init"), "{err}");
        assert!(err.contains("no AI model is configured"), "{err}");
    }

    #[test]
    fn create_judge_rejects_local_specs_with_migration_guidance() {
        let opts = JudgeOptions {
            model: Some("local".to_string()),
            ..Default::default()
        };
        let Err(err) = create_judge(&opts) else {
            panic!("local specs must no longer build a judge");
        };
        assert!(
            err.to_string().contains("openai:http://localhost:11434/v1"),
            "{err}"
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
        assert!(err.contains("anthropic:<model>"));
    }

    #[test]
    fn create_judge_openai_scheme_routes_to_a_compatible_client() {
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
