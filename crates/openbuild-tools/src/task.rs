use crate::Tool;
use async_trait::async_trait;
use openbuild_core::tool::ToolSchema;
use openbuild_core::{
    message::Message, provider::Provider, request::Request, AgentLoop, Effort, Event, Sink,
    ToolCall, ToolResult, ToolRunner, ToolSchema as Schema,
};
use serde::Deserialize;
use std::sync::Arc;

pub struct TaskSpawn {
    pub provider: Arc<dyn Provider>,
    pub max_turns: u32,
    pub depth: u32,
    pub max_depth: u32,
    pub child_tools: Arc<dyn ChildToolFactory>,
}

#[async_trait]
pub trait ChildToolFactory: Send + Sync {
    fn build(&self, capability: &str) -> Vec<Box<dyn Tool>>;
}

#[derive(Deserialize)]
struct Input {
    description: String,
    prompt: String,
    #[serde(default = "default_capability")]
    capability: String,
    #[serde(default)]
    reasoning_effort: Option<String>,
}

fn default_capability() -> String {
    "read-only".into()
}

#[async_trait]
impl Tool for TaskSpawn {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "task".into(),
            description:
                "Spawn a subagent to complete a focused task. capability: 'read-only' | 'all'."
                    .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "description": {"type": "string", "description": "short 3-5 word title"},
                    "prompt": {"type": "string", "description": "self-contained instructions for the subagent"},
                    "capability": {"type": "string", "enum": ["read-only", "all"]},
                    "reasoning_effort": {"type": "string", "enum": ["low","medium","high","xhigh","max"]}
                },
                "required": ["description", "prompt"]
            }),
        }
    }

    async fn run(&self, input: serde_json::Value) -> Result<String, String> {
        if self.depth >= self.max_depth {
            return Err(format!("subagent depth limit reached ({})", self.max_depth));
        }
        let i: Input = serde_json::from_value(input).map_err(|e| e.to_string())?;
        let runner: Arc<dyn ToolRunner> = Arc::new(StaticRunner {
            tools: self.child_tools.build(&i.capability),
        });
        let agent = AgentLoop {
            provider: self.provider.clone(),
            tools: runner,
            max_turns: self.max_turns,
        };
        let effort = i.reasoning_effort.as_deref().map(|s| match s {
            "low" => Effort::Low,
            "medium" => Effort::Medium,
            "high" => Effort::High,
            "xhigh" => Effort::Xhigh,
            _ => Effort::Max,
        });
        let req = Request {
            model: self.provider.id().to_string(),
            system: vec![],
            messages: vec![Message::user_text(format!(
                "[subagent: {}]\n{}",
                i.description, i.prompt
            ))],
            tools: vec![],
            reasoning_effort: effort,
            max_tokens: None,
            stream: true,
            temperature: None,
            top_p: None,
            seed: None,
            stop: Vec::new(),
        };
        let mut buf = String::new();
        let sink = Collector { buf: &mut buf };
        agent.run(req, sink).await.map_err(|e| e.to_string())?;
        Ok(buf)
    }
}

struct StaticRunner {
    tools: Vec<Box<dyn Tool>>,
}

#[async_trait]
impl ToolRunner for StaticRunner {
    fn schemas(&self) -> Vec<Schema> {
        self.tools.iter().map(|t| t.schema()).collect()
    }
    async fn run(&self, call: ToolCall) -> ToolResult {
        for t in &self.tools {
            if t.schema().name == call.name {
                return match t.run(call.input.clone()).await {
                    Ok(c) => ToolResult {
                        call_id: call.id,
                        content: c,
                        is_error: false,
                    },
                    Err(e) => ToolResult {
                        call_id: call.id,
                        content: e,
                        is_error: true,
                    },
                };
            }
        }
        ToolResult {
            call_id: call.id,
            content: format!("unknown tool: {}", call.name),
            is_error: true,
        }
    }
}

struct Collector<'a> {
    buf: &'a mut String,
}

#[async_trait]
impl<'a> Sink for Collector<'a> {
    async fn on(&mut self, ev: Event) {
        if let Event::TextDelta { text } = ev {
            self.buf.push_str(&text);
        }
    }
}
