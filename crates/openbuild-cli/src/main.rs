use anyhow::{Context, Result};
use async_trait::async_trait;
use clap::{Parser, Subcommand};
use openbuild_core::{
    AgentLoop, Block, Effort, Event, Message, Provider, Request, Sink, ToolCall, ToolResult,
    ToolRunner, ToolSchema,
};
use openbuild_permissions::{Decision, Engine, Mode as PermMode};
use openbuild_providers::{anthropic::Anthropic, ollama::Ollama, openai::OpenAi, xai::XAi};
use openbuild_session::Session;
use openbuild_tools::Tool;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "openbuild", version, about = "Model-agnostic agent shell")]
struct Cli {
    #[arg(short = 'p', long = "single")]
    prompt: Option<String>,

    #[arg(long)]
    prompt_file: Option<PathBuf>,

    #[arg(long)]
    prompt_json: Option<String>,

    #[arg(long, default_value_t = false)]
    verbatim: bool,

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

    #[arg(long)]
    tools: Option<String>,

    #[arg(long)]
    disallowed_tools: Option<String>,

    #[arg(long, value_parser = ["low", "medium", "high", "xhigh", "max"])]
    reasoning_effort: Option<String>,

    #[arg(long, alias = "effort", value_parser = ["low", "medium", "high", "xhigh", "max"])]
    effort: Option<String>,

    #[arg(long)]
    rules: Option<String>,

    #[arg(long)]
    system_prompt_override: Option<String>,

    #[arg(long)]
    cwd: Option<PathBuf>,

    #[arg(long, default_value_t = false)]
    no_session_log: bool,

    #[arg(long, default_value_t = false)]
    no_alt_screen: bool,

    #[arg(long, default_value_t = false)]
    no_plan: bool,

    #[arg(long, default_value_t = false)]
    no_subagents: bool,

    #[arg(long, default_value_t = false)]
    disable_web_search: bool,

    #[arg(long, default_value_t = false)]
    experimental_memory: bool,

    #[arg(long, default_value_t = false)]
    no_memory: bool,

    #[arg(long, default_value_t = false)]
    check: bool,

    #[arg(long)]
    sandbox: Option<String>,

    #[arg(long = "allow", value_name = "RULE")]
    allow: Vec<String>,

    #[arg(long = "deny", value_name = "RULE")]
    deny: Vec<String>,

    #[arg(long, default_value_t = false)]
    always_approve: bool,

    #[arg(long, value_parser = ["default", "acceptEdits", "auto", "dontAsk", "bypassPermissions", "plan"])]
    permission_mode: Option<String>,

    #[arg(long)]
    best_of_n: Option<u32>,

    #[arg(short = 'c', long = "continue", default_value_t = false)]
    continue_recent: bool,

    #[arg(short = 'r', long = "resume", num_args = 0..=1, default_missing_value = "")]
    resume: Option<String>,

    #[arg(short = 'w', long = "worktree", num_args = 0..=1, default_missing_value = "")]
    worktree: Option<String>,

    #[arg(long)]
    agent: Option<String>,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    Inspect,
    Models,
    Sessions {
        #[command(subcommand)]
        action: SessionsCmd,
    },
    Mcp {
        #[command(subcommand)]
        action: McpCmd,
    },
    Skills {
        #[command(subcommand)]
        action: SkillsCmd,
    },
}

#[derive(Subcommand, Debug)]
enum SessionsCmd {
    List,
    Show { id: String },
}

#[derive(Subcommand, Debug)]
enum McpCmd {
    List,
    Doctor,
}

#[derive(Subcommand, Debug)]
enum SkillsCmd {
    List,
    Show { name: String },
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

struct GatedTools {
    tools: Vec<Box<dyn Tool>>,
    engine: Engine,
    secret_filter: openbuild_redact::Filter,
}

#[async_trait]
impl ToolRunner for GatedTools {
    fn schemas(&self) -> Vec<ToolSchema> {
        self.tools.iter().map(|t| t.schema()).collect()
    }

    async fn run(&self, call: ToolCall) -> ToolResult {
        let tool = self.tools.iter().find(|t| t.schema().name == call.name);
        let Some(tool) = tool else {
            return ToolResult {
                call_id: call.id,
                content: format!("unknown tool: {}", call.name),
                is_error: true,
            };
        };
        let decision = self
            .engine
            .evaluate(&call.name, &call.input, tool.is_write());
        match decision {
            Decision::Deny | Decision::Plan => {
                return ToolResult {
                    call_id: call.id,
                    content: format!(
                        "denied by permission policy: {} (mode={:?})",
                        call.name, self.engine.mode
                    ),
                    is_error: true,
                };
            }
            Decision::Ask => {
                eprintln!(
                    "[permission] tool '{}' wants to run; auto-deny under mode {:?}",
                    call.name, self.engine.mode
                );
                return ToolResult {
                    call_id: call.id,
                    content: "permission required; rerun with --always-approve or add --allow rule"
                        .into(),
                    is_error: true,
                };
            }
            Decision::Allow => {}
        }
        match tool.run(call.input.clone()).await {
            Ok(content) => ToolResult {
                call_id: call.id,
                content: self.secret_filter.redact(&content),
                is_error: false,
            },
            Err(e) => ToolResult {
                call_id: call.id,
                content: e,
                is_error: true,
            },
        }
    }
}

mod worktree {
    use anyhow::{Context, Result};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    pub fn create_or_attach(cwd: &Path, name: &str) -> Result<PathBuf> {
        let parent = cwd.parent().unwrap_or(cwd);
        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
        let dir_name = if name.is_empty() {
            format!("openbuild-{stamp}")
        } else {
            name.to_string()
        };
        let target = parent.join(&dir_name);
        if target.exists() {
            return Ok(target);
        }
        let branch = if name.is_empty() {
            format!("openbuild/{stamp}")
        } else {
            format!("openbuild/{name}")
        };
        let status = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .arg("worktree")
            .arg("add")
            .arg("-b")
            .arg(&branch)
            .arg(&target)
            .status()
            .context("git worktree add")?;
        if !status.success() {
            anyhow::bail!("git worktree add failed");
        }
        Ok(target)
    }
}

mod openbuild_redact {
    use regex::Regex;

    pub struct Filter {
        patterns: Vec<Regex>,
    }

    impl Filter {
        pub fn new() -> Self {
            let raw = [
                r"sk-[A-Za-z0-9_-]{20,}",
                r"xai-[A-Za-z0-9_-]{20,}",
                r"AKIA[0-9A-Z]{16}",
                r"(?i)Bearer\s+[A-Za-z0-9._\-]{16,}",
                r"ghp_[A-Za-z0-9]{30,}",
                r"github_pat_[A-Za-z0-9_]{30,}",
            ];
            Self {
                patterns: raw.iter().filter_map(|p| Regex::new(p).ok()).collect(),
            }
        }

        pub fn redact(&self, s: &str) -> String {
            let mut out = s.to_string();
            for re in &self.patterns {
                out = re.replace_all(&out, "[redacted]").into_owned();
            }
            out
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

    if let Some(cwd) = &cli.cwd {
        std::env::set_current_dir(cwd).with_context(|| format!("cd {}", cwd.display()))?;
    }

    if let Some(name) = &cli.worktree {
        let cwd = std::env::current_dir()?;
        let wt = worktree::create_or_attach(&cwd, name)?;
        eprintln!("worktree: {}", wt.display());
        std::env::set_current_dir(&wt).with_context(|| format!("cd {}", wt.display()))?;
    }

    if let Some(cmd) = &cli.cmd {
        return run_subcommand(cmd).await;
    }

    let prompt = resolve_prompt(&cli)?;
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

    let allowed: Option<std::collections::HashSet<String>> = cli
        .tools
        .as_deref()
        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());
    let denied: std::collections::HashSet<String> = cli
        .disallowed_tools
        .as_deref()
        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect())
        .unwrap_or_default();

    let sandbox_profile = cli.sandbox.as_deref().map(|name| {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        openbuild_sandbox::discover_profile(name, &cwd)
    });

    let tools: Vec<Box<dyn Tool>> = if cli.no_tools {
        Vec::new()
    } else {
        openbuild_tools::default_tools_with_sandbox(sandbox_profile)
            .into_iter()
            .filter(|t| {
                let n = t.schema().name;
                if denied.contains(&n) {
                    return false;
                }
                if let Some(a) = &allowed {
                    return a.contains(&n);
                }
                true
            })
            .collect()
    };

    let mut engine = Engine::default();
    if cli.always_approve {
        engine.mode = PermMode::BypassPermissions;
    } else if let Some(m) = &cli.permission_mode {
        engine.mode = match m.as_str() {
            "acceptEdits" => PermMode::AcceptEdits,
            "auto" => PermMode::Auto,
            "dontAsk" => PermMode::DontAsk,
            "bypassPermissions" => PermMode::BypassPermissions,
            "plan" => PermMode::Plan,
            _ => PermMode::Default,
        };
    }
    for r in &cli.allow {
        engine.add_allow(r)?;
    }
    for r in &cli.deny {
        engine.add_deny(r)?;
    }

    let runner = Arc::new(GatedTools {
        tools,
        engine,
        secret_filter: openbuild_redact::Filter::new(),
    });

    let agent = AgentLoop {
        provider,
        tools: runner,
        max_turns: cli.max_turns,
    };

    let effort = cli
        .reasoning_effort
        .as_deref()
        .or(cli.effort.as_deref())
        .map(|s| match s {
            "low" => Effort::Low,
            "medium" => Effort::Medium,
            "high" => Effort::High,
            "xhigh" => Effort::Xhigh,
            _ => Effort::Max,
        });

    let mut system_blocks: Vec<Block> = Vec::new();
    if let Some(s) = &cli.system_prompt_override {
        system_blocks.push(Block::Text { text: s.clone() });
    }
    if let Some(rules) = &cli.rules {
        let text = if let Some(path) = rules.strip_prefix('@') {
            std::fs::read_to_string(path).with_context(|| format!("read --rules @{path}"))?
        } else {
            rules.clone()
        };
        system_blocks.push(Block::Text { text });
    }

    let prompt = if cli.check && !cli.verbatim {
        format!("{prompt}\n\nAfter you finish, run a self-verification loop: list each requirement from the request, verify whether it was met, and fix any gaps before stopping.")
    } else {
        prompt
    };

    let req = Request {
        model: cli.model.clone(),
        system: system_blocks,
        messages: vec![Message::user_text(prompt)],
        tools: vec![],
        reasoning_effort: effort,
        max_tokens: None,
        stream: true,
    };

    let session = resolve_session(&cli)?;
    if let Some(s) = &session {
        eprintln!("session: {}", s.path().display());
    }

    let sink = Stdout {
        format: cli.output_format,
        session,
    };

    if let Some(n) = cli.best_of_n {
        run_best_of_n(agent, req, n, sink).await
    } else {
        agent.run(req, sink).await?;
        Ok(())
    }
}

fn resolve_prompt(cli: &Cli) -> Result<String> {
    if let Some(p) = &cli.prompt {
        return Ok(p.clone());
    }
    if let Some(f) = &cli.prompt_file {
        return std::fs::read_to_string(f).with_context(|| format!("read prompt {}", f.display()));
    }
    if let Some(j) = &cli.prompt_json {
        let v: serde_json::Value =
            serde_json::from_str(j).context("--prompt-json must be valid JSON")?;
        if let Some(s) = v.as_str() {
            return Ok(s.into());
        }
        return Ok(v.to_string());
    }
    anyhow::bail!("no prompt; use -p, --prompt-file, or --prompt-json")
}

fn resolve_session(cli: &Cli) -> Result<Option<Session>> {
    if cli.no_session_log {
        return Ok(None);
    }
    if cli.continue_recent {
        if let Some(path) = openbuild_session::most_recent()? {
            return Ok(Some(Session::open(path)?));
        }
    }
    if let Some(r) = &cli.resume {
        if r.is_empty() {
            if let Some(path) = openbuild_session::most_recent()? {
                return Ok(Some(Session::open(path)?));
            }
        } else if let Some(path) = openbuild_session::find_by_id(r)? {
            return Ok(Some(Session::open(path)?));
        }
    }
    Ok(Some(Session::create()?))
}

async fn run_best_of_n(agent: AgentLoop, req: Request, n: u32, mut sink: Stdout) -> Result<()> {
    sink.on(Event::TextDelta {
        text: format!("[best-of-{n}] running parallel sessions\n"),
    })
    .await;
    let agent = Arc::new(agent);
    let mut handles = Vec::new();
    for i in 0..n {
        let a = agent.clone();
        let r = req.clone();
        handles.push(tokio::spawn(async move {
            let mut buf = String::new();
            let sink = CollectSink {
                buf: &mut buf as *mut String,
            };
            let _ = a.run(r, sink).await;
            (i, buf)
        }));
    }
    let mut results = Vec::new();
    for h in handles {
        if let Ok((i, text)) = h.await {
            results.push((i, text));
        }
    }
    let winner = results
        .iter()
        .max_by_key(|(_, t)| t.len())
        .cloned()
        .unwrap_or((0, String::new()));
    sink.on(Event::TextDelta {
        text: format!("[best-of-{n}] winner: candidate {}\n{}", winner.0, winner.1),
    })
    .await;
    Ok(())
}

struct CollectSink {
    buf: *mut String,
}
unsafe impl Send for CollectSink {}

#[async_trait]
impl Sink for CollectSink {
    async fn on(&mut self, ev: Event) {
        if let Event::TextDelta { text } = ev {
            unsafe {
                if let Some(b) = self.buf.as_mut() {
                    b.push_str(&text);
                }
            }
        }
    }
}

async fn run_subcommand(cmd: &Cmd) -> Result<()> {
    match cmd {
        Cmd::Inspect => cmd_inspect(),
        Cmd::Models => cmd_models(),
        Cmd::Sessions { action } => cmd_sessions(action),
        Cmd::Mcp { action } => cmd_mcp(action).await,
        Cmd::Skills { action } => cmd_skills(action),
    }
}

fn cmd_inspect() -> Result<()> {
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
    Ok(())
}

fn cmd_models() -> Result<()> {
    println!("openbuild provider matrix");
    println!("  openai     https://api.openai.com/v1");
    println!("  anthropic  https://api.anthropic.com/v1");
    println!("  xai        https://api.x.ai/v1");
    println!("  ollama     http://localhost:11434/v1");
    println!("  + any OpenAI-compatible endpoint via --base-url");
    Ok(())
}

fn cmd_sessions(action: &SessionsCmd) -> Result<()> {
    match action {
        SessionsCmd::List => {
            let dir = openbuild_session::sessions_dir()?;
            if !dir.exists() {
                println!("no sessions yet at {}", dir.display());
                return Ok(());
            }
            let mut entries: Vec<_> = std::fs::read_dir(&dir)?.flatten().collect();
            entries.sort_by_key(|e| {
                e.metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .unwrap_or(std::time::UNIX_EPOCH)
            });
            entries.reverse();
            for e in entries.into_iter().take(50) {
                let p = e.path();
                let bytes = e.metadata().map(|m| m.len()).unwrap_or(0);
                println!("{}  {}b", p.display(), bytes);
            }
        }
        SessionsCmd::Show { id } => {
            let path = openbuild_session::find_by_id(id)?
                .ok_or_else(|| anyhow::anyhow!("session not found: {id}"))?;
            let text = std::fs::read_to_string(&path)?;
            print!("{text}");
        }
    }
    Ok(())
}

async fn cmd_mcp(action: &McpCmd) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let cfg = openbuild_config::import::discover_all(&cwd);
    match action {
        McpCmd::List => {
            if cfg.mcp_servers.is_empty() {
                println!("no MCP servers configured");
                return Ok(());
            }
            for (name, server) in &cfg.mcp_servers {
                println!("{name}: {server:?}");
            }
        }
        McpCmd::Doctor => {
            for (name, server) in &cfg.mcp_servers {
                match server {
                    openbuild_config::McpServer::Stdio { command, args, env } => {
                        print!("{name} (stdio: {command}) ... ");
                        std::io::stdout().flush().ok();
                        match openbuild_mcp::StdioClient::spawn(command, args, env).await {
                            Ok(c) => match c.list_tools().await {
                                Ok(tools) => println!("ok ({} tools)", tools.len()),
                                Err(e) => println!("FAIL list_tools: {e}"),
                            },
                            Err(e) => println!("FAIL spawn: {e}"),
                        }
                    }
                    openbuild_config::McpServer::Http { url }
                    | openbuild_config::McpServer::Sse { url } => {
                        println!("{name} (http/sse: {url}) — pending in v0.1");
                    }
                }
            }
        }
    }
    Ok(())
}

fn cmd_skills(action: &SkillsCmd) -> Result<()> {
    let skills = openbuild_skills::discover()?;
    match action {
        SkillsCmd::List => {
            for s in skills {
                println!("{}  {} ({})", s.name, s.description, s.path.display());
            }
        }
        SkillsCmd::Show { name } => {
            let s = skills
                .into_iter()
                .find(|s| &s.name == name)
                .ok_or_else(|| anyhow::anyhow!("skill not found: {name}"))?;
            println!("{}", s.body);
        }
    }
    Ok(())
}
