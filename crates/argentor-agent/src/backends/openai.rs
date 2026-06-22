use super::LlmBackend;
use crate::config::{LlmProvider, ModelConfig};
use crate::llm::LlmResponse;
use crate::stream::StreamEvent;
use argentor_core::{ArgentorError, ArgentorResult, Message, Role, ToolCall};
use argentor_skills::SkillDescriptor;
use async_trait::async_trait;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// OpenAI-compatible API backend.
///
/// Works with OpenAI, OpenRouter, Groq, Ollama, and any other provider
/// that implements the OpenAI chat completions API.
pub struct OpenAiBackend {
    config: ModelConfig,
    http: reqwest::Client,
}

impl OpenAiBackend {
    /// Create a new OpenAI API backend with the given configuration.
    pub fn new(config: ModelConfig) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
        }
    }

    fn build_messages(
        &self,
        system_prompt: Option<&str>,
        messages: &[Message],
    ) -> Vec<serde_json::Value> {
        let mut api_messages: Vec<serde_json::Value> = Vec::new();

        if let Some(sys) = system_prompt {
            api_messages.push(serde_json::json!({
                "role": "system",
                "content": sys
            }));
        }

        for m in messages {
            if m.role == Role::System {
                continue;
            }
            api_messages.push(serde_json::json!({
                "role": match m.role {
                    Role::User | Role::Tool => "user",
                    Role::Assistant => "assistant",
                    Role::System => unreachable!(),
                },
                "content": m.content
            }));
        }

        api_messages
    }

    fn build_tools(&self, tools: &[SkillDescriptor]) -> Vec<serde_json::Value> {
        tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters_schema,
                    }
                })
            })
            .collect()
    }

    fn add_provider_headers(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let request = match self.config.provider {
            // Azure uses "api-key" header instead of Bearer token
            LlmProvider::AzureOpenAi => request.header("api-key", &self.config.api_key),
            _ => request.header("Authorization", format!("Bearer {}", self.config.api_key)),
        };

        let request = request.header("Content-Type", "application/json");

        // OpenRouter requires extra headers
        if matches!(self.config.provider, LlmProvider::OpenRouter) {
            request
                .header("HTTP-Referer", "https://github.com/fboiero/Argentor")
                .header("X-Title", "Argentor")
        } else {
            request
        }
    }
}

#[async_trait]
impl LlmBackend for OpenAiBackend {
    fn provider_name(&self) -> &str {
        match self.config.provider {
            LlmProvider::OpenAi => "openai",
            LlmProvider::OpenRouter => "openrouter",
            LlmProvider::Groq => "groq",
            LlmProvider::Ollama => "ollama",
            LlmProvider::Mistral => "mistral",
            LlmProvider::XAi => "xai",
            LlmProvider::AzureOpenAi => "azure_openai",
            LlmProvider::Cerebras => "cerebras",
            LlmProvider::Together => "together",
            LlmProvider::DeepSeek => "deepseek",
            LlmProvider::VLlm => "vllm",
            LlmProvider::Fireworks => "fireworks",
            LlmProvider::HuggingFace => "huggingface",
            _ => "openai",
        }
    }

    async fn chat(
        &self,
        system_prompt: Option<&str>,
        messages: &[Message],
        tools: &[SkillDescriptor],
    ) -> ArgentorResult<LlmResponse> {
        let url = format!("{}/v1/chat/completions", self.config.base_url());
        let api_messages = self.build_messages(system_prompt, messages);

        let mut body = serde_json::json!({
            "model": self.config.model_id,
            "max_tokens": self.config.max_tokens,
            "messages": api_messages,
        });

        if !tools.is_empty() {
            body["tools"] = serde_json::json!(self.build_tools(tools));
        }

        let request = self.add_provider_headers(self.http.post(&url));

        let resp = request
            .json(&body)
            .send()
            .await
            .map_err(|e| ArgentorError::Http(e.to_string()))?;

        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ArgentorError::Http(e.to_string()))?;

        if !status.is_success() {
            return Err(ArgentorError::Http(format!(
                "OpenAI API error {status}: {resp_body}"
            )));
        }

        parse_openai_response(&resp_body)
    }

    async fn chat_stream(
        &self,
        system_prompt: Option<&str>,
        messages: &[Message],
        tools: &[SkillDescriptor],
    ) -> ArgentorResult<(
        mpsc::Receiver<StreamEvent>,
        JoinHandle<ArgentorResult<LlmResponse>>,
    )> {
        let url = format!("{}/v1/chat/completions", self.config.base_url());
        let api_messages = self.build_messages(system_prompt, messages);

        let mut body = serde_json::json!({
            "model": self.config.model_id,
            "max_tokens": self.config.max_tokens,
            "messages": api_messages,
            "stream": true,
        });

        if !tools.is_empty() {
            body["tools"] = serde_json::json!(self.build_tools(tools));
        }

        let request = self.add_provider_headers(self.http.post(&url));

        let resp = request
            .json(&body)
            .send()
            .await
            .map_err(|e| ArgentorError::Http(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let error_body = resp
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(ArgentorError::Http(format!(
                "OpenAI API error {status}: {error_body}"
            )));
        }

        let (tx, rx) = mpsc::channel::<StreamEvent>(256);
        let byte_stream = resp.bytes_stream();

        let handle = tokio::spawn(async move {
            let mut stream = byte_stream;
            let mut buffer = String::new();
            let mut full_text = String::new();
            let mut tool_call_map: std::collections::HashMap<u64, (String, String, String)> =
                std::collections::HashMap::new();
            let mut finish_reason = String::from("stop");

            while let Some(chunk_result) = stream.next().await {
                let chunk = match chunk_result {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        let _ = tx
                            .send(StreamEvent::Error {
                                message: format!("Stream read error: {e}"),
                            })
                            .await;
                        return Err(ArgentorError::Http(format!("Stream read error: {e}")));
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }

                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            let _ = tx.send(StreamEvent::Done).await;
                            continue;
                        }

                        let event: serde_json::Value = match serde_json::from_str(data) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };

                        let choice = &event["choices"][0];

                        if let Some(fr) = choice["finish_reason"].as_str() {
                            finish_reason = fr.to_string();

                            if fr == "tool_calls" {
                                for (id, _name, _args) in tool_call_map.values() {
                                    let _ =
                                        tx.send(StreamEvent::ToolCallEnd { id: id.clone() }).await;
                                }
                            }

                            let _ = tx.send(StreamEvent::Done).await;
                            continue;
                        }

                        let delta = &choice["delta"];

                        if let Some(content) = delta["content"].as_str() {
                            if !content.is_empty() {
                                full_text.push_str(content);
                                let _ = tx
                                    .send(StreamEvent::TextDelta {
                                        text: content.to_string(),
                                    })
                                    .await;
                            }
                        }

                        if let Some(tc_array) = delta["tool_calls"].as_array() {
                            for tc in tc_array {
                                let idx = tc["index"].as_u64().unwrap_or(0);

                                if let Some(id) = tc["id"].as_str() {
                                    let name = tc["function"]["name"]
                                        .as_str()
                                        .unwrap_or_default()
                                        .to_string();
                                    tool_call_map
                                        .insert(idx, (id.to_string(), name.clone(), String::new()));
                                    let _ = tx
                                        .send(StreamEvent::ToolCallStart {
                                            id: id.to_string(),
                                            name,
                                        })
                                        .await;
                                }

                                if let Some(args_delta) = tc["function"]["arguments"].as_str() {
                                    if !args_delta.is_empty() {
                                        if let Some(entry) = tool_call_map.get_mut(&idx) {
                                            entry.2.push_str(args_delta);
                                            let _ = tx
                                                .send(StreamEvent::ToolCallDelta {
                                                    id: entry.0.clone(),
                                                    arguments_delta: args_delta.to_string(),
                                                })
                                                .await;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if !tool_call_map.is_empty() {
                let tool_calls: Vec<ToolCall> = tool_call_map
                    .into_values()
                    .map(|(id, name, args_json)| {
                        let arguments: serde_json::Value =
                            serde_json::from_str(&args_json).unwrap_or_default();
                        ToolCall {
                            id,
                            name,
                            arguments,
                        }
                    })
                    .collect();

                Ok(LlmResponse::ToolUse {
                    content: if full_text.is_empty() {
                        None
                    } else {
                        Some(full_text)
                    },
                    tool_calls,
                })
            } else if finish_reason == "stop" {
                Ok(LlmResponse::Done(full_text))
            } else {
                Ok(LlmResponse::Text(full_text))
            }
        });

        Ok((rx, handle))
    }
}

/// Parse an OpenAI API JSON response into an `LlmResponse`.
pub fn parse_openai_response(body: &serde_json::Value) -> ArgentorResult<LlmResponse> {
    let choice = &body["choices"][0];
    let message = &choice["message"];
    let content = message["content"].as_str().unwrap_or_default().to_string();

    if let Some(tool_calls_json) = message["tool_calls"].as_array() {
        let tool_calls: Vec<ToolCall> = tool_calls_json
            .iter()
            .filter_map(|tc| {
                let id = tc["id"].as_str()?.to_string();
                let name = tc["function"]["name"].as_str()?.to_string();
                let arguments: serde_json::Value =
                    serde_json::from_str(tc["function"]["arguments"].as_str()?).unwrap_or_default();
                Some(ToolCall {
                    id,
                    name,
                    arguments,
                })
            })
            .collect();

        Ok(LlmResponse::ToolUse {
            content: if content.is_empty() {
                None
            } else {
                Some(content)
            },
            tool_calls,
        })
    } else {
        let finish_reason = choice["finish_reason"].as_str().unwrap_or("stop");
        if finish_reason == "stop" {
            Ok(LlmResponse::Done(content))
        } else {
            Ok(LlmResponse::Text(content))
        }
    }
}
