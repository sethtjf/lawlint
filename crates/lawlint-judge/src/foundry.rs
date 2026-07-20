//! Azure AI Foundry backend as an `axllm::AxAIClient` (feature `cloud`).
//!
//! Promoted out of `lawlint-eval` so every AI feature reaches Foundry
//! through the ax boundary; the eval sourcing binaries consume it from here.
//! Foundry serves two wire protocols behind one resource: Claude deployments
//! go through the Anthropic passthrough (`/anthropic/v1/messages`,
//! `x-api-key`), everything else through the OpenAI-compatible route
//! (`/models/chat/completions`, `api-key`). Responses are normalized to the
//! OpenAI chat-completions shape at the `chat` boundary, like `MistralRsClient`.

use axllm::{AxAIClient, AxError, AxResult};
use serde::Deserialize;
use serde_json::{json, Value};
use std::thread;
use std::time::Duration;

use crate::credentials;

const API_VERSION: &str = "2024-05-01-preview";

/// Cap on generated tokens per judge chat call (a findings array is short);
/// `completion` callers pass their own budget.
const CHAT_MAX_TOKENS: usize = 1024;

#[derive(Debug)]
pub struct FoundryClient {
    host: String,
    key: String,
    /// Deployment used by `chat` when the request names no model.
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiMessage {
    content: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContent {
    text: String,
}

/// Claude deployments answer on Foundry's Anthropic passthrough; everything
/// else is OpenAI-compatible.
fn uses_anthropic_route(model: &str) -> bool {
    model.starts_with("claude")
}

/// Reduce a full endpoint URL to `scheme://host` (Foundry portal endpoints
/// often carry a path segment that the chat routes must not repeat).
fn normalize_host(endpoint: &str) -> String {
    endpoint
        .split_once("://")
        .map(|(scheme, remainder)| {
            format!(
                "{}://{}",
                scheme,
                remainder.split('/').next().unwrap_or(remainder)
            )
        })
        .unwrap_or_else(|| endpoint.to_string())
}

fn request_url(host: &str, model: &str) -> String {
    if uses_anthropic_route(model) {
        format!("{host}/anthropic/v1/messages?api-version={API_VERSION}")
    } else {
        format!("{host}/models/chat/completions?api-version={API_VERSION}")
    }
}

fn request_body(model: &str, system: &str, user: &str, max_tokens: usize) -> Value {
    if uses_anthropic_route(model) {
        json!({
            "model": model,
            "system": system,
            "messages": [{"role": "user", "content": user}],
            "max_tokens": max_tokens,
        })
    } else {
        json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ],
            "max_completion_tokens": max_tokens,
        })
    }
}

fn extract_text(model: &str, value: Value) -> Result<String, String> {
    if uses_anthropic_route(model) {
        serde_json::from_value::<AnthropicResponse>(value)
            .map_err(|error| format!("invalid Anthropic response: {error}"))?
            .content
            .into_iter()
            .next()
            .map(|content| content.text)
            .ok_or_else(|| "Anthropic response had no content".to_string())
    } else {
        serde_json::from_value::<OpenAiResponse>(value)
            .map_err(|error| format!("invalid OpenAI response: {error}"))?
            .choices
            .into_iter()
            .next()
            .map(|choice| choice.message.content)
            .ok_or_else(|| "OpenAI response had no choices".to_string())
    }
}

/// Collapse an OpenAI chat-completions-shaped request into the
/// (system, user) pair `completion` speaks. System turns concatenate into the
/// system prompt; every other turn joins the user prompt.
fn collapse_messages(request: &Value) -> AxResult<(String, String)> {
    let messages = request
        .get("messages")
        .and_then(Value::as_array)
        .filter(|messages| !messages.is_empty())
        .ok_or_else(|| {
            AxError::validation("chat request must carry a non-empty `messages` array")
        })?;
    let mut system = Vec::new();
    let mut user = Vec::new();
    for message in messages {
        let content = message
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match message.get("role").and_then(Value::as_str) {
            Some("system") => system.push(content),
            _ => user.push(content),
        }
    }
    Ok((system.join("\n\n"), user.join("\n\n")))
}

impl FoundryClient {
    pub fn new(endpoint: &str, key: impl Into<String>, model: Option<String>) -> Self {
        Self {
            host: normalize_host(endpoint),
            key: key.into(),
            model,
        }
    }

    /// Build from `AZURE_FOUNDRY_ENDPOINT` / `AZURE_FOUNDRY_API_KEY` — the
    /// environment first, then the user-level credential store.
    pub fn from_credentials(model: Option<String>) -> Result<Self, String> {
        let endpoint = credentials::lookup("AZURE_FOUNDRY_ENDPOINT").ok_or(
            "AZURE_FOUNDRY_ENDPOINT is not set (export it or store it via `lawlint init`)",
        )?;
        let key = credentials::lookup("AZURE_FOUNDRY_API_KEY")
            .ok_or("AZURE_FOUNDRY_API_KEY is not set (export it or store it via `lawlint init`)")?;
        Ok(Self::new(&endpoint, key, model))
    }

    /// Environment-or-credential-store construction with no default
    /// deployment; callers pass the model per `completion` call.
    pub fn from_env() -> Result<Self, String> {
        Self::from_credentials(None)
    }

    /// One system+user completion against the named deployment, with
    /// exponential backoff on 429/5xx (3 attempts).
    pub fn completion(
        &self,
        model: &str,
        system: &str,
        user: &str,
        max_tokens: usize,
    ) -> Result<String, String> {
        let url = request_url(&self.host, model);
        let body = request_body(model, system, user, max_tokens);

        let mut last_error = String::new();
        for attempt in 0..3 {
            let request = ureq::post(&url)
                .set("Content-Type", "application/json")
                .set(
                    if uses_anthropic_route(model) {
                        "x-api-key"
                    } else {
                        "api-key"
                    },
                    &self.key,
                );
            let request = if uses_anthropic_route(model) {
                request.set("anthropic-version", "2023-06-01")
            } else {
                request
            };
            match request.send_string(&body.to_string()) {
                Ok(response) => {
                    let value: Value = serde_json::from_str(
                        &response
                            .into_string()
                            .map_err(|error| format!("invalid model response: {error}"))?,
                    )
                    .map_err(|error| format!("invalid model JSON: {error}"))?;
                    return extract_text(model, value);
                }
                Err(ureq::Error::Status(status, response)) => {
                    let detail = response.into_string().unwrap_or_default();
                    last_error = format!("status code {status}: {detail}");
                    if status < 500 && status != 429 {
                        break;
                    }
                    if attempt < 2 {
                        thread::sleep(Duration::from_secs(2_u64.pow(attempt)));
                    }
                }
                Err(error) => {
                    last_error = error.to_string();
                    if attempt < 2 {
                        thread::sleep(Duration::from_secs(2_u64.pow(attempt)));
                    }
                }
            }
        }
        Err(last_error)
    }
}

impl AxAIClient for FoundryClient {
    fn chat(&mut self, request: Value) -> AxResult<Value> {
        let model = request
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| self.model.clone())
            .ok_or_else(|| {
                AxError::validation(
                    "foundry chat needs a model: configure a deployment or name one in the request",
                )
            })?;
        let (system, user) = collapse_messages(&request)?;
        let content = self
            .completion(&model, &system, &user, CHAT_MAX_TOKENS)
            .map_err(AxError::runtime)?;
        Ok(json!({
            "object": "chat.completion",
            "model": model,
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": content },
                "finish_reason": "stop",
            }],
        }))
    }
}

// ------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_deployments_use_the_anthropic_route() {
        assert!(uses_anthropic_route("claude-opus-4-8"));
        assert!(uses_anthropic_route("claude-haiku-4-5"));
        assert!(!uses_anthropic_route("gpt-5.5"));
        assert!(!uses_anthropic_route("FW-GLM-5.1"));
    }

    #[test]
    fn normalize_host_strips_path_and_keeps_scheme() {
        assert_eq!(
            normalize_host("https://res.services.ai.azure.com/models"),
            "https://res.services.ai.azure.com"
        );
        assert_eq!(
            normalize_host("https://res.azure.com"),
            "https://res.azure.com"
        );
        // No scheme: passed through untouched.
        assert_eq!(normalize_host("res.azure.com"), "res.azure.com");
    }

    #[test]
    fn request_shapes_differ_per_route() {
        assert!(request_url("https://h", "claude-opus-4-8").contains("/anthropic/v1/messages"));
        assert!(request_url("https://h", "gpt-5.5").contains("/models/chat/completions"));

        let anthropic = request_body("claude-opus-4-8", "s", "u", 9);
        assert_eq!(anthropic["system"], "s");
        assert_eq!(anthropic["max_tokens"], 9);

        let openai = request_body("gpt-5.5", "s", "u", 9);
        assert_eq!(openai["messages"][0]["role"], "system");
        assert_eq!(openai["max_completion_tokens"], 9);
    }

    #[test]
    fn extract_text_parses_both_response_shapes() {
        let anthropic = json!({"content": [{"text": "hi"}]});
        assert_eq!(extract_text("claude-opus-4-8", anthropic).unwrap(), "hi");
        let openai = json!({"choices": [{"message": {"content": "ok"}}]});
        assert_eq!(extract_text("gpt-5.5", openai).unwrap(), "ok");
        assert!(extract_text("gpt-5.5", json!({"choices": []})).is_err());
    }

    #[test]
    fn collapse_messages_splits_system_from_user_turns() {
        let request = json!({
            "messages": [
                {"role": "system", "content": "be strict"},
                {"role": "user", "content": "first"},
                {"role": "user", "content": "second"},
            ]
        });
        let (system, user) = collapse_messages(&request).unwrap();
        assert_eq!(system, "be strict");
        assert_eq!(user, "first\n\nsecond");
        assert!(collapse_messages(&json!({"messages": []})).is_err());
    }

    #[test]
    fn chat_without_a_model_is_a_validation_error() {
        let mut client = FoundryClient::new("https://h.example", "k", None);
        let err = client
            .chat(json!({"messages": [{"role": "user", "content": "x"}]}))
            .unwrap_err();
        assert!(err.to_string().contains("model"));
    }
}
