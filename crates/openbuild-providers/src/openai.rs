use async_trait::async_trait;
use futures::stream::StreamExt;
use openbuild_core::{
    event::Event,
    message::{Block, Message, Role},
    provider::{Capability, EventStream, Provider, ProviderError},
    request::{Request, StopReason, Usage},
};
use serde::{Deserialize, Serialize};

use crate::sse::SseDecoder;

#[derive(Debug, Clone)]
pub struct OpenAi {
    id: String,
    base_url: String,
    api_key: String,
    http: reqwest::Client,
}

impl OpenAi {
    pub fn new(id: impl Into<String>, base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            base_url: base_url.into(),
            api_key: api_key.into(),
            http: reqwest::Client::new(),
        }
    }
}

#[derive(Serialize)]
struct ChatReq<'a> {
    model: &'a str,
    messages: Vec<WireMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<WireTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Serialize)]
struct WireMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct WireTool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: WireFn,
}

#[derive(Serialize)]
struct WireFn {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Deserialize)]
struct Chunk {
    #[serde(default)]
    choices: Vec<ChunkChoice>,
    #[serde(default)]
    usage: Option<UsageWire>,
}

#[derive(Deserialize)]
struct ChunkChoice {
    delta: Delta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
}

#[derive(Deserialize)]
struct UsageWire {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
    #[serde(default)]
    reasoning_tokens: u32,
}

fn flatten(msg: &Message) -> WireMessage {
    let role = match msg.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    };
    let mut text = String::new();
    for b in &msg.content {
        if let Block::Text { text: t } = b {
            text.push_str(t);
        }
    }
    WireMessage { role: role.into(), content: text }
}

#[async_trait]
impl Provider for OpenAi {
    fn id(&self) -> &str {
        &self.id
    }

    fn supports(&self, cap: Capability) -> bool {
        matches!(
            cap,
            Capability::Tools | Capability::Streaming | Capability::StructuredOutput
        )
    }

    async fn complete(&self, req: Request) -> Result<EventStream, ProviderError> {
        let mut messages = Vec::new();
        if !req.system.is_empty() {
            let mut sys = String::new();
            for b in &req.system {
                if let Block::Text { text } = b {
                    sys.push_str(text);
                }
            }
            messages.push(WireMessage { role: "system".into(), content: sys });
        }
        for m in &req.messages {
            messages.push(flatten(m));
        }

        let tools: Vec<WireTool> = req
            .tools
            .iter()
            .map(|t| WireTool {
                kind: "function",
                function: WireFn {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.input_schema.clone(),
                },
            })
            .collect();

        let body = ChatReq {
            model: &req.model,
            messages,
            stream: req.stream,
            max_tokens: req.max_tokens,
            tools,
            reasoning_effort: req.reasoning_effort.map(|e| match e {
                openbuild_core::request::Effort::Low => "low",
                openbuild_core::request::Effort::Medium => "medium",
                openbuild_core::request::Effort::High => "high",
                openbuild_core::request::Effort::Xhigh => "high",
                openbuild_core::request::Effort::Max => "high",
            }),
            stream_options: req.stream.then(|| StreamOptions { include_usage: true }),
        };

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Upstream(format!("{status}: {body}")));
        }

        let bytes = resp.bytes_stream();
        let stream = ::async_stream::try_stream! {
            let mut decoder = SseDecoder::new();
            futures::pin_mut!(bytes);
            while let Some(chunk) = bytes.next().await {
                let chunk = chunk.map_err(|e| ProviderError::Http(e.to_string()))?;
                let text = String::from_utf8_lossy(&chunk).into_owned();
                for frame in decoder.push(&text) {
                    if frame == "[DONE]" {
                        yield Event::Done(StopReason::EndTurn);
                        return;
                    }
                    let parsed: Chunk = serde_json::from_str(&frame)
                        .map_err(|e| ProviderError::Decode(e.to_string()))?;
                    if let Some(u) = parsed.usage {
                        yield Event::Usage(Usage {
                            input_tokens: u.prompt_tokens,
                            output_tokens: u.completion_tokens,
                            reasoning_tokens: u.reasoning_tokens,
                            ..Default::default()
                        });
                    }
                    for choice in parsed.choices {
                        if let Some(t) = choice.delta.reasoning {
                            yield Event::ThinkingDelta { text: t };
                        }
                        if let Some(t) = choice.delta.content {
                            yield Event::TextDelta { text: t };
                        }
                        if let Some(fr) = choice.finish_reason {
                            let reason = match fr.as_str() {
                                "stop" => StopReason::EndTurn,
                                "length" => StopReason::MaxTokens,
                                "tool_calls" => StopReason::ToolUse,
                                _ => StopReason::EndTurn,
                            };
                            yield Event::Done(reason);
                        }
                    }
                }
            }
        };
        Ok(Box::pin(stream))
    }
}

