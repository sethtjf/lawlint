//! `MistralRsClient` — a custom `axllm::AxAIClient` running mistral.rs
//! inference in-process. Default backend for the lawlint judge (design doc
//! §10.2).
//!
//! Speaks OpenAI chat-completions shapes at the `chat` boundary: parses
//! `{messages: [{role, content}]}` requests, hands the conversation to
//! mistral.rs (which owns tokenization, chat templates, and sampling), and
//! returns `{choices: [{message: {role, content}}]}`.
//!
//! Two loading paths, chosen from the spec:
//!
//! - **GGUF repos** (repo id mentions "gguf", or a `#<file>` is pinned):
//!   mistral.rs' GGUF loader. The tokenizer and chat template come from the
//!   GGUF's own metadata — no extra downloads beyond the model file.
//! - **Safetensors repos** (everything else — the Gemma 4 series among
//!   them): mistral.rs' auto-detecting loader (text or multimodal, from the
//!   repo's `config.json`) with in-situ quantization to Q4K, so a bf16 repo
//!   like `google/gemma-4-E4B-it` runs in a ~4-bit memory footprint.
//!
//! mistral.rs' GGUF loader has **no gemma architecture at all** (v0.9.0
//! supports llama/phi/starcoder2/qwen/mistral3 GGUFs), so gemma-family GGUF
//! repos are rejected up front — before any multi-GB download — with a
//! pointer at the safetensors path that does work.
//!
//! Models download lazily through the standard Hugging Face hub cache
//! (`~/.cache/huggingface`), shared with every other hf-hub consumer.
//! Metal is used automatically on macOS (crate feature `metal`, enabled for
//! that target); CPU elsewhere. The mistral.rs SDK is async; the ax
//! boundary is blocking, so each client owns a small tokio runtime and
//! bridges with `block_on`. All failure paths return `AxResult` errors —
//! never panics.

use axllm::{AxAIClient, AxError, AxResult};
use hf_hub::api::sync::{Api, ApiRepo};
use mistralrs::{GgufModelBuilder, IsqType, Model, ModelBuilder, RequestBuilder, TextMessageRole};
use serde_json::{json, Value};

/// GGUF file used when the repo is the default and listing is unnecessary.
const DEFAULT_GGUF_FILE: &str = "qwen2.5-1.5b-instruct-q4_k_m.gguf";

/// Preferred quantizations, best quality/size tradeoff first. Q4_K_M is the
/// usual sweet spot for 1–3B instruct models.
const QUANT_PREFERENCE: [&str; 5] = ["q4_k_m", "q5_k_m", "q4_0", "q5_0", "q8_0"];

/// Cap on generated tokens per chat call (a findings array is short).
const MAX_NEW_TOKENS: usize = 1024;

pub struct MistralRsClient {
    repo: String,
    gguf_file: Option<String>,
    loaded: Option<Loaded>,
}

struct Loaded {
    /// Bridges mistral.rs' async SDK to the blocking ax boundary. Owned by
    /// the client: `block_on` would panic inside a foreign runtime.
    runtime: tokio::runtime::Runtime,
    model: Model,
}

impl MistralRsClient {
    /// A lazy client over `<repo>` (an HF repo id, e.g.
    /// `Qwen/Qwen2.5-1.5B-Instruct-GGUF` or `google/gemma-4-E4B-it`),
    /// optionally pinning a specific GGUF file name. Nothing is downloaded
    /// until the first `chat`.
    pub fn new(repo: impl Into<String>, gguf_file: Option<String>) -> Self {
        MistralRsClient {
            repo: repo.into(),
            gguf_file,
            loaded: None,
        }
    }

    fn ensure_loaded(&mut self) -> AxResult<&Loaded> {
        if self.loaded.is_none() {
            self.loaded = Some(load_model(&self.repo, self.gguf_file.as_deref())?);
        }
        Ok(self.loaded.as_ref().expect("just set"))
    }
}

impl AxAIClient for MistralRsClient {
    fn chat(&mut self, request: Value) -> AxResult<Value> {
        // Validate the request before any model load/download.
        let messages = parse_messages(&request)?;
        let repo = self.repo.clone();
        let loaded = self.ensure_loaded()?;

        let mut builder = RequestBuilder::new()
            .set_deterministic_sampler() // the judge runs at temperature 0
            .set_sampler_max_len(MAX_NEW_TOKENS);
        for (role, content) in &messages {
            builder = builder.add_message(map_role(role), content);
        }

        let response = loaded
            .runtime
            .block_on(loaded.model.send_chat_request(builder))
            .map_err(|e| rt("mistral.rs chat request failed", e))?;
        let choice = response
            .choices
            .first()
            .ok_or_else(|| AxError::runtime("mistral.rs returned no choices"))?;
        let content = choice.message.content.clone().unwrap_or_default();
        Ok(json!({
            "object": "chat.completion",
            "model": repo,
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": content },
                "finish_reason": choice.finish_reason,
            }],
        }))
    }
}

/// OpenAI role string → mistral.rs role.
fn map_role(role: &str) -> TextMessageRole {
    match role {
        "system" => TextMessageRole::System,
        "assistant" => TextMessageRole::Assistant,
        "tool" => TextMessageRole::Tool,
        "user" => TextMessageRole::User,
        other => TextMessageRole::Custom(other.to_string()),
    }
}

// ---- Request parsing ---------------------------------------------------

/// Parse an OpenAI chat-completions-shaped request into (role, content)
/// pairs. Also accepts ax's internal `chat_prompt`/`chatPrompt` aliases so
/// the client keeps working if routed through ax's own layers.
fn parse_messages(request: &Value) -> AxResult<Vec<(String, String)>> {
    let messages = request
        .get("messages")
        .or_else(|| request.get("chat_prompt"))
        .or_else(|| request.get("chatPrompt"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            AxError::validation("chat request must carry a `messages` array of {role, content}")
        })?;
    if messages.is_empty() {
        return Err(AxError::validation("chat request `messages` is empty"));
    }
    let mut out = Vec::with_capacity(messages.len());
    for (i, message) in messages.iter().enumerate() {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user")
            .to_string();
        let content = message
            .get("content")
            .and_then(content_as_text)
            .ok_or_else(|| {
                AxError::validation(format!("messages[{i}] has no textual `content`"))
            })?;
        out.push((role, content));
    }
    Ok(out)
}

/// Content is a string or an array of `{type: "text", text}` parts.
fn content_as_text(content: &Value) -> Option<String> {
    match content {
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

// ---- Model loading -----------------------------------------------------

fn rt(context: &str, e: impl std::fmt::Display) -> AxError {
    AxError::runtime(format!("{context}: {e}"))
}

/// GGUF repos take mistral.rs' GGUF loader; anything else is a safetensors
/// repo for the auto-detecting loader with in-situ quantization. A pinned
/// `#<file>` always means GGUF.
fn is_gguf_spec(repo: &str, gguf_file: Option<&str>) -> bool {
    gguf_file.is_some() || repo.to_ascii_lowercase().contains("gguf")
}

/// mistral.rs v0.9.0's GGUF loader supports no gemma architecture (any
/// version), so gemma-family GGUF repos fail fast — before the multi-GB
/// download — with a pointer at the safetensors path that works.
fn check_gguf_support(repo: &str, gguf_file: Option<&str>) -> AxResult<()> {
    let spec = format!("{repo}/{}", gguf_file.unwrap_or_default()).to_ascii_lowercase();
    if spec.contains("gemma") {
        return Err(AxError::validation(format!(
            "gemma GGUFs are not supported by the bundled mistral.rs runtime \
             (its GGUF loader has no gemma architecture) — use the safetensors \
             repo instead, which runs quantized in-process: e.g. \
             \"local:{}\" rather than \"local:{repo}\"",
            crate::DEFAULT_GEMMA_REPO
        )));
    }
    Ok(())
}

/// Pick a GGUF file from a repo listing by quantization preference.
/// Multimodal projector files (`mmproj`) are never the language model.
fn pick_gguf(filenames: &[String]) -> Option<String> {
    let ggufs: Vec<&String> = filenames
        .iter()
        .filter(|f| {
            let lower = f.to_ascii_lowercase();
            lower.ends_with(".gguf") && !lower.contains("mmproj")
        })
        .collect();
    for quant in QUANT_PREFERENCE {
        if let Some(hit) = ggufs
            .iter()
            .find(|f| f.to_ascii_lowercase().contains(quant))
        {
            return Some((**hit).clone());
        }
    }
    ggufs.first().map(|f| (*f).clone())
}

fn resolve_gguf_file(repo: &ApiRepo, repo_id: &str) -> AxResult<String> {
    match repo.info() {
        Ok(info) => {
            let names: Vec<String> = info.siblings.into_iter().map(|s| s.rfilename).collect();
            pick_gguf(&names).ok_or_else(|| {
                AxError::validation(format!("no .gguf file found in HF repo {repo_id}"))
            })
        }
        // Offline / API hiccup: for the default repo we know the filename.
        Err(e) if repo_id == crate::DEFAULT_LOCAL_REPO => {
            eprintln!("lawlint-judge: could not list {repo_id} ({e}); using {DEFAULT_GGUF_FILE}");
            Ok(DEFAULT_GGUF_FILE.to_string())
        }
        Err(e) => Err(rt(&format!("failed to list HF repo {repo_id}"), e)),
    }
}

fn load_model(repo_id: &str, gguf_file: Option<&str>) -> AxResult<Loaded> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| rt("failed to start tokio runtime for mistral.rs", e))?;

    let model = if is_gguf_spec(repo_id, gguf_file) {
        check_gguf_support(repo_id, gguf_file)?;
        let gguf_name = match gguf_file {
            Some(f) => f.to_string(),
            None if repo_id == crate::DEFAULT_LOCAL_REPO => DEFAULT_GGUF_FILE.to_string(),
            None => {
                let api = Api::new().map_err(|e| rt("failed to initialize hf-hub", e))?;
                resolve_gguf_file(&api.model(repo_id.to_string()), repo_id)?
            }
        };
        eprintln!(
            "lawlint-judge: loading {repo_id}/{gguf_name} via mistral.rs \
             (downloads once, then cached)…"
        );
        runtime
            .block_on(GgufModelBuilder::new(repo_id, vec![gguf_name]).build())
            .map_err(|e| rt(&format!("failed to load GGUF model {repo_id}"), e))?
    } else {
        eprintln!(
            "lawlint-judge: loading {repo_id} via mistral.rs with in-situ Q4K \
             quantization (downloads once, then cached)…"
        );
        runtime
            .block_on(ModelBuilder::new(repo_id).with_isq(IsqType::Q4K).build())
            .map_err(|e| rt(&format!("failed to load model {repo_id}"), e))?
    };

    Ok(Loaded { runtime, model })
}

// ------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- request parsing (no model required) ----------------------------

    #[test]
    fn parse_messages_openai_shape() {
        let req = json!({
            "messages": [
                {"role": "system", "content": "be strict"},
                {"role": "user", "content": "evaluate this"},
            ]
        });
        let messages = parse_messages(&req).unwrap();
        assert_eq!(
            messages,
            vec![
                ("system".to_string(), "be strict".to_string()),
                ("user".to_string(), "evaluate this".to_string()),
            ]
        );
    }

    #[test]
    fn parse_messages_accepts_ax_chat_prompt_alias_and_parts() {
        let req = json!({
            "chat_prompt": [
                {"role": "user", "content": [{"type": "text", "text": "part one "}, {"type": "text", "text": "part two"}]},
            ]
        });
        let messages = parse_messages(&req).unwrap();
        assert_eq!(messages[0].1, "part one part two");
    }

    #[test]
    fn parse_messages_defaults_missing_role_to_user() {
        let req = json!({"messages": [{"content": "hi"}]});
        assert_eq!(parse_messages(&req).unwrap()[0].0, "user");
    }

    #[test]
    fn parse_messages_rejects_bad_requests_without_panicking() {
        for bad in [
            json!({}),
            json!({"messages": []}),
            json!({"messages": "not an array"}),
            json!({"messages": [{"role": "user", "content": 42}]}),
        ] {
            assert!(parse_messages(&bad).is_err(), "accepted: {bad}");
        }
    }

    // ---- role mapping ----------------------------------------------------

    #[test]
    fn map_role_covers_openai_roles() {
        assert!(matches!(map_role("system"), TextMessageRole::System));
        assert!(matches!(map_role("user"), TextMessageRole::User));
        assert!(matches!(map_role("assistant"), TextMessageRole::Assistant));
        assert!(matches!(map_role("tool"), TextMessageRole::Tool));
        assert!(matches!(map_role("weird"), TextMessageRole::Custom(_)));
    }

    // ---- loading-path selection ------------------------------------------

    #[test]
    fn gguf_repos_and_pinned_files_take_the_gguf_path() {
        // The default local repo is a GGUF repo.
        assert!(is_gguf_spec(crate::DEFAULT_LOCAL_REPO, None));
        assert!(is_gguf_spec("bartowski/Model-GGUF", None));
        // A pinned file always means GGUF, whatever the repo id says.
        assert!(is_gguf_spec("some/repo", Some("m-q4_0.gguf")));
    }

    #[test]
    fn safetensors_repos_take_the_isq_path() {
        // The Gemma 4 catalog repo is safetensors + in-situ quantization.
        assert!(!is_gguf_spec(crate::DEFAULT_GEMMA_REPO, None));
        assert!(!is_gguf_spec("Qwen/Qwen3-4B", None));
    }

    // ---- gemma GGUF gate (fail before download, point at safetensors) ----

    #[test]
    fn gemma_ggufs_are_rejected_with_the_safetensors_alternative() {
        for repo in [
            "google/gemma-4-E4B-it-qat-q4_0-gguf",
            "google/gemma-3-4b-it-qat-q4_0-gguf",
            "unsloth/gemma-4-E4B-it-GGUF",
        ] {
            let err = check_gguf_support(repo, None).unwrap_err().to_string();
            assert!(err.contains("gemma GGUFs"), "{err}");
            assert!(err.contains(crate::DEFAULT_GEMMA_REPO), "{err}");
        }
        // Pinned gemma file under a neutral repo id is caught too.
        assert!(check_gguf_support("some/repo", Some("gemma-4-q4_0.gguf")).is_err());
        // Non-gemma GGUFs pass.
        assert!(check_gguf_support(crate::DEFAULT_LOCAL_REPO, None).is_ok());
        assert!(check_gguf_support("some/repo", Some("m-q4_0.gguf")).is_ok());
    }

    // ---- gguf selection --------------------------------------------------

    #[test]
    fn pick_gguf_skips_multimodal_projector_files() {
        let names = vec![
            "model-mmproj.gguf".to_string(),
            "model-q4_0.gguf".to_string(),
        ];
        assert_eq!(pick_gguf(&names), Some("model-q4_0.gguf".to_string()));
        // Only a projector file: nothing usable.
        assert_eq!(pick_gguf(&names[..1]), None);
    }

    #[test]
    fn pick_gguf_prefers_q4_k_m_then_falls_back() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            pick_gguf(&names(&["m-q8_0.gguf", "m-Q4_K_M.gguf", "README.md"])),
            Some("m-Q4_K_M.gguf".to_string())
        );
        assert_eq!(
            pick_gguf(&names(&["m-q8_0.gguf", "m-q5_k_m.gguf"])),
            Some("m-q5_k_m.gguf".to_string())
        );
        // No preferred quant: first .gguf.
        assert_eq!(
            pick_gguf(&names(&["m-iq2_xs.gguf"])),
            Some("m-iq2_xs.gguf".to_string())
        );
        assert_eq!(pick_gguf(&names(&["README.md"])), None);
    }

    // ---- laziness --------------------------------------------------------

    #[test]
    fn constructing_client_downloads_nothing_and_bad_request_fails_before_load() {
        let mut client = MistralRsClient::new("no-such-org/no-such-model-GGUF", None);
        assert!(client.loaded.is_none());
        // Request validation happens before any model load/download.
        let err = client.chat(json!({"nope": true})).unwrap_err();
        assert!(err.to_string().contains("messages"));
        assert!(client.loaded.is_none());
    }
}
