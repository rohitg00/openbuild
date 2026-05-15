use anyhow::Result;
use async_trait::async_trait;
use clap::{Parser, Subcommand};
use openbuild_core::{
    AgentLoop, Effort, Event, Message, Provider, Request, Sink, ToolCall, ToolResult, ToolRunner,
    ToolSchema,
};
use openbuild_providers::{anthropic::Anthropic, ollama::Ollama, openai::OpenAi, xai::XAi};
use openbuild_session::Session;
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

    #[arg(long, value_parser = ["low", "medium", "high", "xhigh", "max"])]
    reasoning_effort: Option<String>,

    #[arg(long, default_value_t = false)]
    no_session_log: bool,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    Inspect,
    Models,
}

struct Stdout {
    format: String,
    session: Option<Session>,
}

#[async_trait]
impl Sink for Stdout {
    async fn on(&mut self, ev: Event) {
        if let Some(s) = &mut self.session {
            let _ = s.append_event(&ev);
        }
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

    if let Some(cmd) = &cli.cmd {
        return run_subcommand(cmd).await;
    }

    let prompt = cli
        .prompt
        .clone()
        .ok_or_else(|| anyhow::anyhow!("interactive mode pending; pass -p \"...\""))?;
    let api_key = cli.api_key.clone().unwrap_or_default();

    let base_override = (cli.base_url != "https://api.openai.com/v1").then(|| cli.base_url.clone());
    let provider: Arc<dyn Provider> = match cli.provider.as_str() {
        "anthropic" => Arc::new(Anthropic::new(
            cli.model.clone(),
            base_override.unwrap_or_else(|| "https://api.anthropic.com/v1".into()),
            api_key,
        )),
        "ollama" => Arc::new(Ollama::new(cli.model.clone(), base_override)),
        "xai" => Arc::new(XAi::new(cli.model.clone(), base_override, api_key)),
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

    let effort = cli.reasoning_effort.as_deref().map(|s| match s {
        "low" => Effort::Low,
        "medium" => Effort::Medium,
        "high" => Effort::High,
        "xhigh" => Effort::Xhigh,
        _ => Effort::Max,
    });

    let req = Request {
        model: cli.model,
        system: vec![],
        messages: vec![Message::user_text(prompt)],
        tools: vec![],
        reasoning_effort: effort,
        max_tokens: None,
        stream: true,
    };

    let session = (!cli.no_session_log).then(Session::create).transpose()?;
    if let Some(s) = &session {
        eprintln!("session: {}", s.path().display());
    }

    let sink = Stdout {
        format: cli.output_format,
        session,
    };

    agent.run(req, sink).await?;
    Ok(())
}

async fn run_subcommand(cmd: &Cmd) -> Result<()> {
    match cmd {
        Cmd::Inspect => {
            let cwd = std::env::current_dir()?;
            let cfg = openbuild_config::import::discover_all(&cwd);
            println!("openbuild inspect");
            println!("  cwd: {}", cwd.display());
            println!("  instructions ({}):", cfg.instructions.len());
            for i in &cfg.instructions {
                println!(
                    "    [{:?}] {} ({} bytes)",
                    i.scope,
                    i.path.display(),
                    i.bytes
                );
            }
            println!("  permissions:");
            println!("    allow: {}", cfg.permissions.allow.len());
            println!("    deny:  {}", cfg.permissions.deny.len());
            if let Some(m) = &cfg.permissions.default_mode {
                println!("    mode:  {m}");
            }
            println!("  mcp_servers ({}):", cfg.mcp_servers.len());
            for name in cfg.mcp_servers.keys() {
                println!("    {name}");
            }
            println!("  imported from:");
            for s in &cfg.provenance {
                println!("    [{:?}] {}", s.agent, s.path.display());
            }
        }
        Cmd::Models => {
            println!("openbuild provider matrix");
            println!("  openai     https://api.openai.com/v1");
            println!("  anthropic  https://api.anthropic.com/v1");
            println!("  xai        https://api.x.ai/v1");
            println!("  ollama     http://localhost:11434/v1");
            println!("  + any OpenAI-compatible endpoint via --base-url");
        }
    }
    Ok(())
}
