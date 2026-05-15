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
pub struct Anthropic {
    id: String,
    base_url: String,
    api_key: String,
    api_version: String,
    http: reqwest::Client,
}

impl Anthropic {
    pub fn new(
        id: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            base_url: base_url.into(),
            api_key: api_key.into(),
            api_version: "2023-06-01".into(),
            http: reqwest::Client::new(),
        }
    }

    pub fn with_api_version(mut self, v: impl Into<String>) -> Self {
        self.api_version = v.into();
        self
    }
}

#[derive(Serialize)]
struct MessagesReq<'a> {
    model: &'a str,
    messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    system: Vec<WireBlock>,
    max_tokens: u32,
    stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<WireTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<WireThinking>,
}

#[derive(Serialize)]
struct WireThinking {
    #[serde(rename = "type")]
    kind: &'static str,
    budget_tokens: u32,
}

#[derive(Serialize)]
struct WireMessage {
    role: &'static str,
    content: Vec<WireBlock>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
}

#[derive(Serialize)]
struct WireTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum StreamEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: MessageStartInfo },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: u32,
        content_block: BlockStart,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: u32, delta: BlockDelta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: u32 },
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: MessageDeltaInfo,
        #[serde(default)]
        usage: Option<UsageWire>,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "error")]
    Error { error: ErrorInfo },
}

#[derive(Deserialize, Debug)]
struct MessageStartInfo {
    #[serde(default)]
    usage: Option<UsageWire>,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(dead_code)]
enum BlockStart {
    Text {
        #[serde(default)]
        text: String,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
    },
    ToolUse {
        id: String,
        name: String,
    },
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::enum_variant_names)]
enum BlockDelta {
    TextDelta { text: String },
    ThinkingDelta { thinking: String },
    InputJsonDelta { partial_json: String },
    SignatureDelta {},
}

#[derive(Deserialize, Debug)]
struct MessageDeltaInfo {
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct UsageWire {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: u32,
}

#[derive(Deserialize, Debug)]
struct ErrorInfo {
    #[serde(rename = "type")]
    kind: String,
    message: String,
}

fn convert_messages(msgs: &[Message]) -> Vec<WireMessage> {
    let mut out = Vec::new();
    for m in msgs {
        let role = match m.role {
            Role::User | Role::Tool => "user",
            Role::Assistant => "assistant",
            Role::System => continue,
        };
        let mut blocks = Vec::new();
        for b in &m.content {
            match b {
                Block::Text { text } => blocks.push(WireBlock::Text { text: text.clone() }),
                Block::ToolUse { id, name, input } => blocks.push(WireBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                }),
                Block::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => blocks.push(WireBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: *is_error,
                }),
                _ => {}
            }
        }
        if !blocks.is_empty() {
            out.push(WireMessage {
                role,
                content: blocks,
            });
        }
    }
    out
}

#[async_trait]
impl Provider for Anthropic {
    fn id(&self) -> &str {
        &self.id
    }

    fn supports(&self, cap: Capability) -> bool {
        matches!(
            cap,
            Capability::Tools
                | Capability::Streaming
                | Capability::Vision
                | Capability::Reasoning
                | Capability::PromptCache
        )
    }

    async fn complete(&self, req: Request) -> Result<EventStream, ProviderError> {
        let system: Vec<WireBlock> = req
            .system
            .iter()
            .filter_map(|b| match b {
                Block::Text { text } => Some(WireBlock::Text { text: text.clone() }),
                _ => None,
            })
            .collect();

        let tools: Vec<WireTool> = req
            .tools
            .iter()
            .map(|t| WireTool {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.input_schema.clone(),
            })
            .collect();

        let thinking = req.reasoning_effort.map(|e| {
            let budget = match e {
                openbuild_core::request::Effort::Low => 1024,
                openbuild_core::request::Effort::Medium => 4096,
                openbuild_core::request::Effort::High => 16384,
                openbuild_core::request::Effort::Xhigh => 32768,
                openbuild_core::request::Effort::Max => 64000,
            };
            WireThinking {
                kind: "enabled",
                budget_tokens: budget,
            }
        });

        let body = MessagesReq {
            model: &req.model,
            messages: convert_messages(&req.messages),
            system,
            max_tokens: req.max_tokens.unwrap_or(8192),
            stream: req.stream,
            tools,
            thinking,
        };

        let url = format!("{}/messages", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", &self.api_version)
            .header("content-type", "application/json")
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
            let mut block_index_to_tool_id: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
            futures::pin_mut!(bytes);
            while let Some(chunk) = bytes.next().await {
                let chunk = chunk.map_err(|e| ProviderError::Http(e.to_string()))?;
                let text = String::from_utf8_lossy(&chunk).into_owned();
                for frame in decoder.push(&text) {
                    let parsed: StreamEvent = match serde_json::from_str(&frame) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    match parsed {
                        StreamEvent::MessageStart { message } => {
                            if let Some(u) = message.usage {
                                yield Event::Usage(Usage {
                                    input_tokens: u.input_tokens,
                                    output_tokens: u.output_tokens,
                                    cache_read_tokens: u.cache_read_input_tokens,
                                    cache_write_tokens: u.cache_creation_input_tokens,
                                    ..Default::default()
                                });
                            }
                            yield Event::MessageStart;
                        }
                        StreamEvent::ContentBlockStart { index, content_block } => {
                            if let BlockStart::ToolUse { id, name } = content_block {
                                block_index_to_tool_id.insert(index, id.clone());
                                yield Event::ToolCallStart { id, name };
                            }
                        }
                        StreamEvent::ContentBlockDelta { index, delta } => match delta {
                            BlockDelta::TextDelta { text } => yield Event::TextDelta { text },
                            BlockDelta::ThinkingDelta { thinking } => yield Event::ThinkingDelta { text: thinking },
                            BlockDelta::InputJsonDelta { partial_json } => {
                                if let Some(id) = block_index_to_tool_id.get(&index).cloned() {
                                    yield Event::ToolCallDelta { id, args_delta: partial_json };
                                }
                            }
                            BlockDelta::SignatureDelta {} => {}
                        },
                        StreamEvent::ContentBlockStop { index } => {
                            if let Some(id) = block_index_to_tool_id.remove(&index) {
                                yield Event::ToolCallEnd { id };
                            }
                        }
                        StreamEvent::MessageDelta { delta, usage } => {
                            if let Some(u) = usage {
                                yield Event::Usage(Usage {
                                    input_tokens: u.input_tokens,
                                    output_tokens: u.output_tokens,
                                    cache_read_tokens: u.cache_read_input_tokens,
                                    cache_write_tokens: u.cache_creation_input_tokens,
                                    ..Default::default()
                                });
                            }
                            if let Some(sr) = delta.stop_reason {
                                let reason = match sr.as_str() {
                                    "end_turn" => StopReason::EndTurn,
                                    "max_tokens" => StopReason::MaxTokens,
                                    "tool_use" => StopReason::ToolUse,
                                    "stop_sequence" => StopReason::StopSequence,
                                    _ => StopReason::EndTurn,
                                };
                                yield Event::Done(reason);
                            }
                        }
                        StreamEvent::MessageStop => return,
                        StreamEvent::Error { error } => {
                            yield Event::Error(match error.kind.as_str() {
                                "overloaded_error" | "rate_limit_error" => ProviderError::RateLimit,
                                "authentication_error" | "permission_error" => ProviderError::Auth(error.message),
                                _ => ProviderError::Upstream(error.message),
                            });
                            return;
                        }
                        StreamEvent::Ping => {}
                    }
                }
            }
        };
        Ok(Box::pin(stream))
    }
}
