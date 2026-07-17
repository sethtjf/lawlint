//! `CandleClient` — a custom `axllm::AxAIClient` running candle inference
//! in-process. Default backend for the lawlint judge (design doc §10.2).
//!
//! Speaks OpenAI chat-completions shapes at the `chat` boundary: parses
//! `{messages: [{role, content}]}` requests, applies the model's chat
//! template (ChatML — Qwen instruct family), generates greedily (temp 0),
//! and returns `{choices: [{message: {role, content}}]}`.
//!
//! The model is a small quantized instruct GGUF, lazily downloaded through
//! hf-hub on first use (with progress on stderr) and cached under the
//! standard HF cache dir. CPU inference, Metal when available (macOS).
//! All failure paths return `AxResult` errors — never panics.

use std::path::PathBuf;

use axllm::{AxAIClient, AxError, AxResult};
use candle_core::quantized::gguf_file;
use candle_core::{Device, Tensor};
use candle_transformers::generation::{LogitsProcessor, Sampling};
use candle_transformers::models::quantized_qwen2::ModelWeights;
use hf_hub::api::sync::{Api, ApiRepo};
use serde_json::{json, Value};
use tokenizers::Tokenizer;

/// GGUF file used when the repo is the default and listing is unnecessary.
const DEFAULT_GGUF_FILE: &str = "qwen2.5-1.5b-instruct-q4_k_m.gguf";

/// Preferred quantizations, best quality/size tradeoff first. Q4_K_M is the
/// usual sweet spot for 1–3B instruct models on CPU.
const QUANT_PREFERENCE: [&str; 5] = ["q4_k_m", "q5_k_m", "q4_0", "q5_0", "q8_0"];

/// Cap on generated tokens per chat call (a findings array is short).
const MAX_NEW_TOKENS: usize = 1024;

/// Context window assumed when the GGUF carries no `<arch>.context_length`
/// metadata. Conservative for the small instruct GGUFs this client targets
/// (Qwen2.5 family: 32k).
const FALLBACK_CONTEXT_LENGTH: usize = 32_768;

pub struct CandleClient {
    repo: String,
    gguf_file: Option<String>,
    loaded: Option<Loaded>,
}

struct Loaded {
    model: ModelWeights,
    tokenizer: Tokenizer,
    device: Device,
    eos: Vec<u32>,
    /// Trained context window (from GGUF metadata) — the hard cap on
    /// prompt + generated positions.
    context_length: usize,
}

impl CandleClient {
    /// A lazy client over `<repo>` (an HF repo id, e.g.
    /// `Qwen/Qwen2.5-1.5B-Instruct-GGUF`), optionally pinning a specific
    /// GGUF file name. Nothing is downloaded until the first `chat`.
    pub fn new(repo: impl Into<String>, gguf_file: Option<String>) -> Self {
        CandleClient {
            repo: repo.into(),
            gguf_file,
            loaded: None,
        }
    }

    fn ensure_loaded(&mut self) -> AxResult<&mut Loaded> {
        if self.loaded.is_none() {
            self.loaded = Some(load_model(&self.repo, self.gguf_file.as_deref())?);
        }
        Ok(self.loaded.as_mut().expect("just set"))
    }
}

impl AxAIClient for CandleClient {
    fn chat(&mut self, request: Value) -> AxResult<Value> {
        let messages = parse_messages(&request)?;
        let prompt = chatml_prompt(&messages);
        let loaded = self.ensure_loaded()?;
        let (content, finish_reason) = generate(loaded, &prompt)?;
        Ok(json!({
            "object": "chat.completion",
            "model": self.repo,
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": content },
                "finish_reason": finish_reason,
            }],
        }))
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

// ---- Chat template -----------------------------------------------------

/// ChatML — the chat template of the Qwen instruct family (and the de facto
/// standard for small instruct GGUFs):
/// `<|im_start|>role\ncontent<|im_end|>\n … <|im_start|>assistant\n`.
fn chatml_prompt(messages: &[(String, String)]) -> String {
    let mut p = String::new();
    for (role, content) in messages {
        p.push_str("<|im_start|>");
        p.push_str(role);
        p.push('\n');
        p.push_str(content);
        p.push_str("<|im_end|>\n");
    }
    p.push_str("<|im_start|>assistant\n");
    p
}

// ---- Model loading -----------------------------------------------------

fn rt(context: &str, e: impl std::fmt::Display) -> AxError {
    AxError::runtime(format!("{context}: {e}"))
}

fn pick_device() -> Device {
    #[cfg(target_os = "macos")]
    {
        match Device::new_metal(0) {
            Ok(device) => {
                eprintln!("lawlint-judge: using Metal");
                return device;
            }
            Err(e) => eprintln!("lawlint-judge: Metal unavailable ({e}); using CPU"),
        }
    }
    Device::Cpu
}

/// Pick a GGUF file from a repo listing by quantization preference.
fn pick_gguf(filenames: &[String]) -> Option<String> {
    let ggufs: Vec<&String> = filenames
        .iter()
        .filter(|f| f.to_ascii_lowercase().ends_with(".gguf"))
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

/// Tokenizer lives in the non-GGUF base repo for the Qwen GGUF layout
/// (`…-Instruct-GGUF` → `…-Instruct`); fall back to the repo itself.
fn fetch_tokenizer(api: &Api, repo_id: &str) -> AxResult<PathBuf> {
    let mut candidates = Vec::new();
    for suffix in ["-GGUF", "-gguf"] {
        if let Some(base) = repo_id.strip_suffix(suffix) {
            candidates.push(base.to_string());
        }
    }
    candidates.push(repo_id.to_string());
    let mut last_err = None;
    for candidate in &candidates {
        eprintln!("lawlint-judge: fetching tokenizer.json from {candidate}…");
        match api.model(candidate.clone()).get("tokenizer.json") {
            Ok(path) => return Ok(path),
            Err(e) => last_err = Some(format!("{candidate}: {e}")),
        }
    }
    Err(AxError::runtime(format!(
        "could not fetch tokenizer.json (tried {}): {}",
        candidates.join(", "),
        last_err.unwrap_or_default()
    )))
}

fn load_model(repo_id: &str, gguf_file: Option<&str>) -> AxResult<Loaded> {
    let api = Api::new().map_err(|e| rt("failed to initialize hf-hub", e))?;
    let repo = api.model(repo_id.to_string());

    let gguf_name = match gguf_file {
        Some(f) => f.to_string(),
        None if repo_id == crate::DEFAULT_LOCAL_REPO => DEFAULT_GGUF_FILE.to_string(),
        None => resolve_gguf_file(&repo, repo_id)?,
    };
    eprintln!(
        "lawlint-judge: fetching {repo_id}/{gguf_name} (downloads once, then cached)…"
    );
    let gguf_path = repo
        .get(&gguf_name)
        .map_err(|e| rt(&format!("failed to fetch {repo_id}/{gguf_name}"), e))?;
    let tokenizer_path = fetch_tokenizer(&api, repo_id)?;

    let tokenizer = Tokenizer::from_file(&tokenizer_path)
        .map_err(|e| rt("failed to load tokenizer.json", e))?;

    let device = pick_device();
    eprintln!("lawlint-judge: loading {gguf_name}…");
    let mut file = std::fs::File::open(&gguf_path)
        .map_err(|e| rt(&format!("failed to open {}", gguf_path.display()), e))?;
    let content = gguf_file::Content::read(&mut file)
        .map_err(|e| rt("failed to read GGUF (is this a qwen2-family GGUF?)", e))?;
    let context_length = context_length_from_metadata(&content.metadata);
    let model = ModelWeights::from_gguf(content, &mut file, &device)
        .map_err(|e| rt("failed to load GGUF weights (qwen2-family GGUFs only)", e))?;

    let eos: Vec<u32> = ["<|im_end|>", "<|endoftext|>", "</s>"]
        .iter()
        .filter_map(|t| tokenizer.token_to_id(t))
        .collect();
    if eos.is_empty() {
        return Err(AxError::validation(
            "tokenizer has no known end-of-turn token (<|im_end|>, <|endoftext|>, </s>)",
        ));
    }

    Ok(Loaded {
        model,
        tokenizer,
        device,
        eos,
        context_length,
    })
}

/// Trained context window from GGUF metadata: `<arch>.context_length`
/// (e.g. `qwen2.context_length` — candle's quantized qwen2 precomputes rope
/// tables only up to this length, so positions past it hard-fail). Falls
/// back to [`FALLBACK_CONTEXT_LENGTH`] when absent or malformed.
fn context_length_from_metadata(
    metadata: &std::collections::HashMap<String, gguf_file::Value>,
) -> usize {
    metadata
        .iter()
        .find(|(key, _)| key.ends_with(".context_length"))
        .and_then(|(_, value)| value.to_u64().ok())
        .and_then(|v| usize::try_from(v).ok())
        .filter(|&v| v > 0)
        .unwrap_or(FALLBACK_CONTEXT_LENGTH)
}

// ---- Generation --------------------------------------------------------

/// Reject prompts that cannot fit in the model's trained context window
/// with room for at least one generated token. Checked before any forward
/// pass so oversized chunks fail in milliseconds, not minutes.
fn ensure_prompt_fits(prompt_tokens: usize, context_length: usize) -> AxResult<()> {
    if prompt_tokens >= context_length {
        return Err(AxError::validation(format!(
            "prompt is {prompt_tokens} tokens but the model's context window \
             is {context_length}; the chunk is too large for this model — \
             use a smaller input or a larger-context judge backend"
        )));
    }
    Ok(())
}

/// Greedy (temp-0) generation. Returns (text, finish_reason).
fn generate(loaded: &mut Loaded, prompt: &str) -> AxResult<(String, &'static str)> {
    // Fresh sequence: drop any KV state from a previous call.
    loaded.model.clear_kv_cache();

    let encoding = loaded
        .tokenizer
        .encode(prompt, true)
        .map_err(|e| rt("tokenization failed", e))?;
    let prompt_tokens = encoding.get_ids().to_vec();
    if prompt_tokens.is_empty() {
        return Err(AxError::validation("empty prompt after tokenization"));
    }
    // Fail fast BEFORE the O(len²) prompt forward: positions past the
    // trained context produce degenerate output or a hard candle error,
    // so an oversized prompt is a deterministic (and very expensive)
    // failure with no upfront check.
    ensure_prompt_fits(prompt_tokens.len(), loaded.context_length)?;

    let mut sampler = LogitsProcessor::from_sampling(0, Sampling::ArgMax);
    let sample = |sampler: &mut LogitsProcessor, logits: &Tensor| -> AxResult<u32> {
        let logits = logits
            .squeeze(0)
            .and_then(|l| l.to_dtype(candle_core::DType::F32))
            .map_err(|e| rt("logits reshape failed", e))?;
        sampler.sample(&logits).map_err(|e| rt("sampling failed", e))
    };

    // Prompt pass (full sequence), then token-by-token with KV cache.
    let input = Tensor::new(prompt_tokens.as_slice(), &loaded.device)
        .and_then(|t| t.unsqueeze(0))
        .map_err(|e| rt("prompt tensor build failed", e))?;
    let logits = loaded
        .model
        .forward(&input, 0)
        .map_err(|e| rt("model forward failed", e))?;
    let mut next = sample(&mut sampler, &logits)?;

    let mut generated: Vec<u32> = Vec::new();
    let mut index_pos = prompt_tokens.len();
    let mut finish_reason = "length";
    // Also stop before any generated position would exceed the window.
    while generated.len() < MAX_NEW_TOKENS && index_pos < loaded.context_length {
        if loaded.eos.contains(&next) {
            finish_reason = "stop";
            break;
        }
        generated.push(next);
        let input = Tensor::new(&[next], &loaded.device)
            .and_then(|t| t.unsqueeze(0))
            .map_err(|e| rt("token tensor build failed", e))?;
        let logits = loaded
            .model
            .forward(&input, index_pos)
            .map_err(|e| rt("model forward failed", e))?;
        index_pos += 1;
        next = sample(&mut sampler, &logits)?;
    }

    let text = loaded
        .tokenizer
        .decode(&generated, true)
        .map_err(|e| rt("detokenization failed", e))?;
    Ok((text, finish_reason))
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

    // ---- chat template ---------------------------------------------------

    #[test]
    fn chatml_prompt_wraps_roles_and_opens_assistant_turn() {
        let messages = vec![
            ("system".to_string(), "S".to_string()),
            ("user".to_string(), "U".to_string()),
        ];
        assert_eq!(
            chatml_prompt(&messages),
            "<|im_start|>system\nS<|im_end|>\n<|im_start|>user\nU<|im_end|>\n<|im_start|>assistant\n"
        );
    }

    // ---- gguf selection --------------------------------------------------

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

    // ---- context-window guard (regression: oversized prompts must fail
    // fast, before any forward pass) --------------------------------------

    #[test]
    fn ensure_prompt_fits_rejects_oversized_prompts_with_token_counts() {
        // At or past the window: fail fast with an actionable message.
        for prompt_tokens in [32_768, 32_769, 50_000] {
            let err = ensure_prompt_fits(prompt_tokens, 32_768).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains(&prompt_tokens.to_string()), "{msg}");
            assert!(msg.contains("32768"), "{msg}");
            assert!(msg.contains("context window"), "{msg}");
        }
    }

    #[test]
    fn ensure_prompt_fits_accepts_prompts_with_generation_room() {
        // Strictly inside the window (room for >= 1 generated token): ok.
        assert!(ensure_prompt_fits(1, 32_768).is_ok());
        assert!(ensure_prompt_fits(32_767, 32_768).is_ok());
    }

    #[test]
    fn context_length_from_metadata_reads_arch_key_and_falls_back() {
        use std::collections::HashMap;
        let md = |entries: &[(&str, gguf_file::Value)]| -> HashMap<String, gguf_file::Value> {
            entries
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect()
        };

        // Arch-prefixed key (qwen2 GGUF layout), u32-valued.
        let qwen = md(&[
            ("general.name", gguf_file::Value::String("q".into())),
            ("qwen2.context_length", gguf_file::Value::U32(32_768)),
        ]);
        assert_eq!(context_length_from_metadata(&qwen), 32_768);

        // Any architecture prefix works.
        let llama = md(&[("llama.context_length", gguf_file::Value::U64(4_096))]);
        assert_eq!(context_length_from_metadata(&llama), 4_096);

        // Missing, zero, or non-integer values fall back conservatively.
        assert_eq!(context_length_from_metadata(&md(&[])), FALLBACK_CONTEXT_LENGTH);
        let zero = md(&[("qwen2.context_length", gguf_file::Value::U32(0))]);
        assert_eq!(context_length_from_metadata(&zero), FALLBACK_CONTEXT_LENGTH);
        let bad = md(&[(
            "qwen2.context_length",
            gguf_file::Value::String("32k".into()),
        )]);
        assert_eq!(context_length_from_metadata(&bad), FALLBACK_CONTEXT_LENGTH);
    }

    // ---- laziness --------------------------------------------------------

    #[test]
    fn constructing_client_downloads_nothing_and_bad_request_fails_before_load() {
        let mut client = CandleClient::new("no-such-org/no-such-model-GGUF", None);
        assert!(client.loaded.is_none());
        // Request validation happens before any model load/download.
        let err = client.chat(json!({"nope": true})).unwrap_err();
        assert!(err.to_string().contains("messages"));
        assert!(client.loaded.is_none());
    }
}
