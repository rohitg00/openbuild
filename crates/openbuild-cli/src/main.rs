use anyhow::Result;
use async_trait::async_trait;
use clap::Parser;
use openbuild_core::{
    AgentLoop, Event, Message, Provider, Request, Sink, ToolCall, ToolResult, ToolRunner,
    ToolSchema,
};
use openbuild_providers::{anthropic::Anthropic, openai::OpenAi};
use openbuild_tools::{read_file::ReadFile, run_terminal_cmd::RunTerminalCmd, Tool};
use std::io::Write;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "openbuild", version, about = "Model-agnostic agent shell")]
struct Cli {
    #[arg(short = 'p', long = "single")]
    prompt: Option<String>,

    #[arg(short = 'm', long, default_value = "gpt-4o-mini")]
    model: String,

    #[arg(long, env = "OPENBUILD_PROVIDER", default_value = "openai")]
    provider: String,

    #[arg(
        long,
        env = "OPENBUILD_BASE_URL",
        default_value = "https://api.openai.com/v1"
    )]
    base_url: String,

    #[arg(long, env = "OPENBUILD_API_KEY")]
    api_key: Option<String>,

    #[arg(long, default_value = "plain", value_parser = ["plain", "json", "streaming-json"])]
    output_format: String,

    #[arg(long, default_value_t = 10)]
    max_turns: u32,

    #[arg(long, default_value_t = false)]
    no_tools: bool,
}

struct Stdout {
    format: String,
}

#[async_trait]
impl Sink for Stdout {
    async fn on(&mut self, ev: Event) {
        let mut out = std::io::stdout().lock();
        match ev {
            Event::TextDelta { text } => match self.format.as_str() {
                "json" | "streaming-json" => {
                    let _ = writeln!(
                        out,
                        "{}",
                        serde_json::json!({"type":"text_delta","text":text})
                    );
                }
                _ => {
                    let _ = write!(out, "{text}");
                    let _ = out.flush();
                }
            },
            Event::ThinkingDelta { text } if self.format != "plain" => {
                let _ = writeln!(
                    out,
                    "{}",
                    serde_json::json!({"type":"thinking_delta","text":text})
                );
            }
            Event::ToolCallStart { id, name } => {
                if self.format == "plain" {
                    let _ = writeln!(out, "\n[{name}]");
                } else {
                    let _ = writeln!(
                        out,
                        "{}",
                        serde_json::json!({"type":"tool_call_start","id":id,"name":name})
                    );
                }
            }
            Event::Done(reason) => {
                if self.format == "plain" {
                    let _ = writeln!(out);
                } else {
                    let _ = writeln!(
                        out,
                        "{}",
                        serde_json::json!({"type":"done","reason":reason})
                    );
                }
            }
            _ => {}
        }
    }
}

struct BuiltinTools {
    tools: Vec<Box<dyn Tool>>,
}

#[async_trait]
impl ToolRunner for BuiltinTools {
    fn schemas(&self) -> Vec<ToolSchema> {
        self.tools.iter().map(|t| t.schema()).collect()
    }

    async fn run(&self, call: ToolCall) -> ToolResult {
        for t in &self.tools {
            if t.schema().name == call.name {
                return match t.run(call.input.clone()).await {
                    Ok(content) => ToolResult {
                        call_id: call.id,
                        content,
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let prompt = cli
        .prompt
        .clone()
        .ok_or_else(|| anyhow::anyhow!("interactive mode pending; pass -p \"...\""))?;
    let api_key = cli.api_key.clone().unwrap_or_default();

    let provider: Arc<dyn Provider> = match cli.provider.as_str() {
        "anthropic" => Arc::new(Anthropic::new(
            cli.model.clone(),
            if cli.base_url == "https://api.openai.com/v1" {
                "https://api.anthropic.com/v1".into()
            } else {
                cli.base_url.clone()
            },
            api_key,
        )),
        _ => Arc::new(OpenAi::new(
            cli.model.clone(),
            cli.base_url.clone(),
            api_key,
        )),
    };

    let tools = if cli.no_tools {
        Vec::<Box<dyn Tool>>::new()
    } else {
        vec![
            Box::new(ReadFile) as Box<dyn Tool>,
            Box::new(RunTerminalCmd) as Box<dyn Tool>,
        ]
    };
    let runner = Arc::new(BuiltinTools { tools });

    let agent = AgentLoop {
        provider,
        tools: runner,
        max_turns: cli.max_turns,
    };

    let req = Request {
        model: cli.model,
        system: vec![],
        messages: vec![Message::user_text(prompt)],
        tools: vec![],
        reasoning_effort: None,
        max_tokens: None,
        stream: true,
    };

    let sink = Stdout {
        format: cli.output_format,
    };

    agent.run(req, sink).await?;
    Ok(())
}
