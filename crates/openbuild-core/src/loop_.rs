use crate::{
    event::Event,
    message::{Block, Message, Role},
    provider::{Provider, ProviderError},
    request::{Request, StopReason},
    tool::{ToolCall, ToolResult, ToolSchema},
};
use async_trait::async_trait;
use futures::StreamExt;
use std::sync::Arc;

#[async_trait]
pub trait ToolRunner: Send + Sync {
    fn schemas(&self) -> Vec<ToolSchema>;
    async fn run(&self, call: ToolCall) -> ToolResult;
}

pub struct TurnOutcome {
    pub stop_reason: StopReason,
    pub assistant: Message,
    pub tool_results: Vec<Block>,
}

#[async_trait]
pub trait Sink: Send {
    async fn on(&mut self, ev: Event);
}

pub struct AgentLoop {
    pub provider: Arc<dyn Provider>,
    pub tools: Arc<dyn ToolRunner>,
    pub max_turns: u32,
}

impl AgentLoop {
    pub async fn run(
        &self,
        mut request: Request,
        mut sink: impl Sink,
    ) -> Result<Vec<Message>, ProviderError> {
        let mut history = request.messages.clone();
        let tool_schemas = self.tools.schemas();
        for turn in 0..self.max_turns {
            request.messages = history.clone();
            request.tools = tool_schemas.clone();
            let outcome = self.drive_turn(&request, &mut sink).await?;
            history.push(outcome.assistant);
            if outcome.tool_results.is_empty()
                || matches!(outcome.stop_reason, StopReason::EndTurn | StopReason::Error)
            {
                if matches!(outcome.stop_reason, StopReason::Error) {
                    return Ok(history);
                }
                if outcome.tool_results.is_empty() {
                    return Ok(history);
                }
            }
            history.push(Message {
                role: Role::User,
                content: outcome.tool_results,
            });
            if turn + 1 == self.max_turns {
                return Ok(history);
            }
        }
        Ok(history)
    }

    async fn drive_turn(
        &self,
        req: &Request,
        sink: &mut impl Sink,
    ) -> Result<TurnOutcome, ProviderError> {
        let mut stream = self.provider.complete(req.clone()).await?;
        let mut text = String::new();
        let mut active: std::collections::HashMap<String, (String, String)> =
            std::collections::HashMap::new();
        let mut finished_calls: Vec<(String, String, String)> = Vec::new();
        let mut stop_reason = StopReason::EndTurn;

        while let Some(ev) = stream.next().await {
            let ev = ev?;
            match &ev {
                Event::TextDelta { text: t } => text.push_str(t),
                Event::ToolCallStart { id, name } => {
                    active.insert(id.clone(), (name.clone(), String::new()));
                }
                Event::ToolCallDelta { id, args_delta } => {
                    if let Some(entry) = active.get_mut(id) {
                        entry.1.push_str(args_delta);
                    }
                }
                Event::ToolCallEnd { id } => {
                    if let Some((name, args)) = active.remove(id) {
                        finished_calls.push((id.clone(), name, args));
                    }
                }
                Event::Done(reason) => {
                    stop_reason = *reason;
                }
                Event::Error(e) => return Err(e.clone()),
                _ => {}
            }
            sink.on(ev).await;
        }

        for (id, (name, args)) in active.drain() {
            finished_calls.push((id, name, args));
        }

        let mut assistant_blocks = Vec::new();
        if !text.is_empty() {
            assistant_blocks.push(Block::Text { text });
        }
        for (id, name, args) in &finished_calls {
            let input: serde_json::Value =
                serde_json::from_str(args).unwrap_or(serde_json::Value::Object(Default::default()));
            assistant_blocks.push(Block::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input,
            });
        }

        let mut tool_results = Vec::new();
        for (id, name, args) in finished_calls {
            let input: serde_json::Value = serde_json::from_str(&args)
                .unwrap_or(serde_json::Value::Object(Default::default()));
            let result = self
                .tools
                .run(ToolCall {
                    id: id.clone(),
                    name,
                    input,
                })
                .await;
            tool_results.push(Block::ToolResult {
                tool_use_id: id,
                content: result.content,
                is_error: result.is_error,
            });
        }

        Ok(TurnOutcome {
            stop_reason,
            assistant: Message {
                role: Role::Assistant,
                content: assistant_blocks,
            },
            tool_results,
        })
    }
}
