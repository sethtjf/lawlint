use serde::Deserialize;
use serde_json::{json, Value};
use std::thread;
use std::time::Duration;

const API_VERSION: &str = "2024-05-01-preview";

#[derive(Debug)]
pub struct FoundryClient {
    host: String,
    key: String,
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

impl FoundryClient {
    pub fn from_env() -> Result<Self, String> {
        let endpoint = std::env::var("AZURE_FOUNDRY_ENDPOINT")
            .map_err(|_| "AZURE_FOUNDRY_ENDPOINT is not set".to_string())?;
        let key = std::env::var("AZURE_FOUNDRY_API_KEY")
            .map_err(|_| "AZURE_FOUNDRY_API_KEY is not set".to_string())?;
        let host = endpoint
            .split_once("://")
            .map(|(scheme, remainder)| {
                format!(
                    "{}://{}",
                    scheme,
                    remainder.split('/').next().unwrap_or(remainder)
                )
            })
            .unwrap_or(endpoint);
        Ok(Self { host, key })
    }

    pub fn complete(
        &self,
        model: &str,
        system: &str,
        user: &str,
        max_tokens: usize,
    ) -> Result<String, String> {
        let body = if model == "claude-opus-4-8" {
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
        };
        let url = if model == "claude-opus-4-8" {
            format!(
                "{}/anthropic/v1/messages?api-version={API_VERSION}",
                self.host
            )
        } else {
            format!(
                "{}/models/chat/completions?api-version={API_VERSION}",
                self.host
            )
        };

        let mut last_error = String::new();
        for attempt in 0..3 {
            let request = ureq::post(&url)
                .set("Content-Type", "application/json")
                .set(
                    if model == "claude-opus-4-8" {
                        "x-api-key"
                    } else {
                        "api-key"
                    },
                    &self.key,
                );
            let request = if model == "claude-opus-4-8" {
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
                    let text = if model == "claude-opus-4-8" {
                        serde_json::from_value::<AnthropicResponse>(value)
                            .map_err(|error| format!("invalid Anthropic response: {error}"))?
                            .content
                            .into_iter()
                            .next()
                            .map(|content| content.text)
                            .ok_or_else(|| "Anthropic response had no content".to_string())?
                    } else {
                        serde_json::from_value::<OpenAiResponse>(value)
                            .map_err(|error| format!("invalid OpenAI response: {error}"))?
                            .choices
                            .into_iter()
                            .next()
                            .map(|choice| choice.message.content)
                            .ok_or_else(|| "OpenAI response had no choices".to_string())?
                    };
                    return Ok(text);
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
