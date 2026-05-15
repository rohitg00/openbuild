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
    pub fn new(
        id: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<WireToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize)]
struct WireToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    function: WireToolCallFn,
}

#[derive(Serialize)]
struct WireToolCallFn {
    name: String,
    arguments: String,
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
    #[serde(default)]
    tool_calls: Vec<DeltaToolCall>,
}

#[derive(Deserialize, Default)]
struct DeltaToolCall {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<DeltaToolCallFn>,
}

#[derive(Deserialize, Default)]
struct DeltaToolCallFn {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
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

fn flatten(msg: &Message) -> Vec<WireMessage> {
    let role = match msg.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    };
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut tool_results: Vec<WireMessage> = Vec::new();
    for b in &msg.content {
        match b {
            Block::Text { text: t } => text.push_str(t),
            Block::ToolUse { id, name, input } => tool_calls.push(WireToolCall {
                id: id.clone(),
                kind: "function",
                function: WireToolCallFn {
                    name: name.clone(),
                    arguments: serde_json::to_string(input).unwrap_or_default(),
                },
            }),
            Block::ToolResult {
                tool_use_id,
                content,
                ..
            } => tool_results.push(WireMessage {
                role: "tool".into(),
                content: Some(content.clone()),
                tool_calls: Vec::new(),
                tool_call_id: Some(tool_use_id.clone()),
            }),
            _ => {}
        }
    }
    let mut out = Vec::new();
    if !text.is_empty() || !tool_calls.is_empty() {
        out.push(WireMessage {
            role: role.into(),
            content: (!text.is_empty()).then_some(text),
            tool_calls,
            tool_call_id: None,
        });
    }
    out.extend(tool_results);
    out
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
            messages.push(WireMessage {
                role: "system".into(),
                content: Some(sys),
                tool_calls: Vec::new(),
                tool_call_id: None,
            });
        }
        for m in &req.messages {
            messages.extend(flatten(m));
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
            stream_options: req.stream.then_some(StreamOptions {
                include_usage: true,
            }),
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
            let mut active_tool_calls: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
            futures::pin_mut!(bytes);
            while let Some(chunk) = bytes.next().await {
                let chunk = chunk.map_err(|e| ProviderError::Http(e.to_string()))?;
                let text = String::from_utf8_lossy(&chunk).into_owned();
                for frame in decoder.push(&text) {
                    if frame == "[DONE]" {
                        for id in active_tool_calls.values() {
                            yield Event::ToolCallEnd { id: id.clone() };
                        }
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
                        for tc in choice.delta.tool_calls {
                            let id = match tc.id {
                                Some(new_id) => {
                                    let name = tc.function.as_ref().and_then(|f| f.name.clone()).unwrap_or_default();
                                    active_tool_calls.insert(tc.index, new_id.clone());
                                    yield Event::ToolCallStart { id: new_id.clone(), name };
                                    new_id
                                }
                                None => active_tool_calls.get(&tc.index).cloned().unwrap_or_default(),
                            };
                            if let Some(f) = tc.function {
                                if let Some(args) = f.arguments {
                                    if !args.is_empty() {
                                        yield Event::ToolCallDelta { id, args_delta: args };
                                    }
                                }
                            }
                        }
                        if let Some(fr) = choice.finish_reason {
                            for id in active_tool_calls.values() {
                                yield Event::ToolCallEnd { id: id.clone() };
                            }
                            active_tool_calls.clear();
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
