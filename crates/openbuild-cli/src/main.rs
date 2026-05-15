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

    #[arg(short = 'm', long)]
    model: Option<String>,

    #[arg(long)]
    provider: Option<String>,

    #[arg(long)]
    base_url: Option<String>,

    #[arg(long)]
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

    #[arg(long)]
    agent_profile: Option<PathBuf>,

    #[arg(long, default_value_t = false)]
    restore_code: bool,

    #[arg(long, short = 'o')]
    output: Option<PathBuf>,

    #[arg(long)]
    plan_file: Option<PathBuf>,

    #[arg(long, default_value_t = 0)]
    auto_compact_after: u32,

    #[arg(long)]
    max_tokens: Option<u32>,

    #[arg(long)]
    temperature: Option<f32>,

    #[arg(long)]
    top_p: Option<f32>,

    #[arg(long)]
    seed: Option<u64>,

    #[arg(long = "stop")]
    stop: Vec<String>,

    #[arg(long, default_value_t = false)]
    no_context_inject: bool,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    Inspect,
    Models {
        #[command(subcommand)]
        action: Option<ModelsCmd>,
    },
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
    Agents {
        #[command(subcommand)]
        action: AgentsCmd,
    },
    Memory {
        #[command(subcommand)]
        action: MemoryCmd,
    },
    Import {
        #[command(subcommand)]
        action: ImportCmd,
    },
    Trace {
        #[command(subcommand)]
        action: TraceCmd,
    },
    Agent {
        #[command(subcommand)]
        action: AgentRunCmd,
    },
    Hooks {
        #[command(subcommand)]
        action: HooksCmd,
    },
    Tui,
    Setup,
    Update,
    Cost {
        #[arg(default_value = "")]
        session: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Completions {
        shell: String,
    },
    Sandbox {
        #[command(subcommand)]
        action: SandboxCmd,
    },
}

#[derive(Subcommand, Debug)]
enum ModelsCmd {
    Live {
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        api_key: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum SandboxCmd {
    List,
    Show { name: String },
}

#[derive(Subcommand, Debug)]
enum HooksCmd {
    List,
    Test {
        event: String,
        #[arg(default_value = "")]
        matcher: String,
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
    Add {
        name: String,
        command: String,
        #[arg(num_args = 0..)]
        args: Vec<String>,
    },
    AddHttp {
        name: String,
        url: String,
    },
    AddSse {
        name: String,
        url: String,
    },
    Remove {
        name: String,
    },
}

#[derive(Subcommand, Debug)]
enum SkillsCmd {
    List,
    Show {
        name: String,
    },
    Install {
        source: String,
        #[arg(long)]
        name: Option<String>,
    },
    Remove {
        name: String,
    },
}

#[derive(Subcommand, Debug)]
enum AgentsCmd {
    List,
    Show {
        name: String,
    },
    Add {
        name: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        capability: Option<String>,
        #[arg(long)]
        permission_mode: Option<String>,
        #[arg(long)]
        prompt: Option<String>,
        #[arg(long)]
        prompt_file: Option<PathBuf>,
    },
    Remove {
        name: String,
    },
}

#[derive(Subcommand, Debug)]
enum MemoryCmd {
    List,
    Get { key: String },
    Set { key: String, value: String },
    Remove { key: String },
    Clear,
    Edit,
}

#[derive(Subcommand, Debug)]
enum ImportCmd {
    Claude { path: std::path::PathBuf },
    Cursor { path: std::path::PathBuf },
    Codex { path: std::path::PathBuf },
    OpenCode { path: std::path::PathBuf },
    ClaudeAll,
    ClaudeAgents,
    ClaudeSkills,
    ClaudeMcp,
}

#[derive(Subcommand, Debug)]
enum TraceCmd {
    Export { id: String },
}

#[derive(Subcommand, Debug)]
enum AgentRunCmd {
    Stdio,
    Headless,
    Serve {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 7424)]
        port: u16,
    },
    Acp,
}

struct Stdout {
    format: String,
    session: Option<Session>,
    output_file: Option<std::sync::Mutex<std::fs::File>>,
    capture: String,
    tokens_used: u64,
    compact_threshold: u32,
    hooks: Option<Arc<openbuild_hooks::Registry>>,
}

#[async_trait]
impl Sink for Stdout {
    async fn on(&mut self, ev: Event) {
        if let Some(s) = &mut self.session {
            let _ = s.append_event(&ev);
        }
        if let Event::Usage(u) = &ev {
            self.tokens_used += u.input_tokens as u64 + u.output_tokens as u64;
            if self.compact_threshold > 0 && self.tokens_used >= self.compact_threshold as u64 {
                eprintln!(
                    "[auto-compact threshold reached at {} tokens]",
                    self.tokens_used
                );
                let trigger = self.hooks.clone();
                let tokens = self.tokens_used;
                self.tokens_used = 0;
                if let Some(h) = trigger {
                    let payload = serde_json::json!({"tokens": tokens});
                    let _ = h
                        .fire(openbuild_hooks::Event::PreCompact, "", &payload)
                        .await;
                }
                return;
            }
        }
        let mut out = std::io::stdout().lock();
        match ev {
            Event::TextDelta { text } => {
                self.capture.push_str(&text);
                match self.format.as_str() {
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
                }
            }
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
                if let Some(f) = &self.output_file {
                    if let Ok(mut f) = f.lock() {
                        let _ = std::io::Write::write_all(&mut *f, self.capture.as_bytes());
                        self.capture.clear();
                    }
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
    hooks: Arc<openbuild_hooks::Registry>,
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
        if let Err(msg) = validate_schema(&tool.schema().input_schema, &call.input) {
            return ToolResult {
                call_id: call.id,
                content: format!("input failed schema validation: {msg}"),
                is_error: true,
            };
        }
        let pre_payload = serde_json::json!({
            "tool_name": call.name,
            "tool_input": call.input,
            "tool_use_id": call.id,
        });
        for outcome in self
            .hooks
            .fire(openbuild_hooks::Event::PreToolUse, &call.name, &pre_payload)
            .await
        {
            if outcome.blocked {
                return ToolResult {
                    call_id: call.id,
                    content: format!(
                        "blocked by PreToolUse hook (exit {}): {}",
                        outcome.exit_code, outcome.stderr
                    ),
                    is_error: true,
                };
            }
        }
        let decision = self
            .engine
            .evaluate(&call.name, &call.input, tool.is_write());
        match decision {
            Decision::Deny => {
                return ToolResult {
                    call_id: call.id,
                    content: format!(
                        "denied by permission policy: {} (mode={:?})",
                        call.name, self.engine.mode
                    ),
                    is_error: true,
                };
            }
            Decision::Plan => {
                return ToolResult {
                    call_id: call.id,
                    content: format!(
                        "[plan mode] would run {} with input {} — skipped (no side effects)",
                        call.name,
                        serde_json::to_string(&call.input).unwrap_or_default()
                    ),
                    is_error: false,
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
        let outcome = tool.run(call.input.clone()).await;
        let (content, is_error) = match outcome {
            Ok(c) => (self.secret_filter.redact(&c), false),
            Err(e) => (e, true),
        };
        let post_payload = serde_json::json!({
            "tool_name": call.name,
            "tool_input": call.input,
            "tool_use_id": call.id,
            "tool_response": content,
            "is_error": is_error,
        });
        self.hooks
            .fire(
                openbuild_hooks::Event::PostToolUse,
                &call.name,
                &post_payload,
            )
            .await;
        ToolResult {
            call_id: call.id,
            content,
            is_error,
        }
    }
}

struct McpToolAdapter {
    client: Arc<openbuild_mcp::StdioClient>,
    name: String,
    description: String,
    input_schema: serde_json::Value,
    qualified: String,
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.qualified.clone(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
        }
    }
    async fn run(&self, input: serde_json::Value) -> Result<String, String> {
        self.client
            .call_tool(&self.name, input)
            .await
            .map_err(|e| e.to_string())
    }
}

struct DefaultChildFactory;

impl openbuild_tools::task::ChildToolFactory for DefaultChildFactory {
    fn build(&self, capability: &str) -> Vec<Box<dyn Tool>> {
        let base = openbuild_tools::default_tools();
        if capability == "read-only" {
            base.into_iter()
                .filter(|t| !t.is_write() && t.schema().name != "run_terminal_cmd")
                .collect()
        } else {
            base
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

fn validate_schema(schema: &serde_json::Value, input: &serde_json::Value) -> Result<(), String> {
    let Some(required) = schema.get("required").and_then(|r| r.as_array()) else {
        return Ok(());
    };
    let obj = input
        .as_object()
        .ok_or_else(|| "input must be an object".to_string())?;
    for r in required {
        let Some(key) = r.as_str() else { continue };
        if !obj.contains_key(key) {
            return Err(format!("missing required field: {key}"));
        }
    }
    if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
        for (k, v) in obj {
            if let Some(prop_schema) = props.get(k) {
                if let Some(expected) = prop_schema.get("type").and_then(|t| t.as_str()) {
                    let ok = match expected {
                        "string" => v.is_string(),
                        "integer" => v.is_i64() || v.is_u64(),
                        "number" => v.is_number(),
                        "boolean" => v.is_boolean(),
                        "array" => v.is_array(),
                        "object" => v.is_object(),
                        _ => true,
                    };
                    if !ok {
                        return Err(format!("field {k} must be {expected}"));
                    }
                }
            }
        }
    }
    Ok(())
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

    use std::io::IsTerminal as _;
    let no_prompt_sources = cli.prompt.is_none()
        && cli.prompt_file.is_none()
        && cli.prompt_json.is_none()
        && std::io::stdin().is_terminal();
    if no_prompt_sources {
        return cmd_tui().await;
    }

    let prompt = resolve_prompt(&cli)?;

    let bundled_for_agent = bundled_dir();
    let agent_def = if let Some(path) = &cli.agent_profile {
        Some(openbuild_agents::load(
            path,
            openbuild_agents::Source::User,
        )?)
    } else if let Some(name) = &cli.agent {
        Some(openbuild_agents::load_by_name(
            name,
            bundled_for_agent.as_deref(),
        )?)
    } else {
        None
    };

    let (provider_name, mut effective_model, default_base, picked_key) = resolve_provider(
        cli.provider.as_deref(),
        cli.model.as_deref(),
        cli.base_url.as_deref(),
        cli.api_key.as_deref(),
    );
    if let Some(def) = &agent_def {
        if cli.model.is_none() {
            if let Some(m) = &def.frontmatter.model {
                effective_model = m.clone();
            }
        }
    }
    let api_key = picked_key;
    let base_url = cli.base_url.clone().unwrap_or_else(|| default_base.clone());
    let base_override = (base_url != default_base).then(|| base_url.clone());
    let provider: Arc<dyn Provider> = match provider_name.as_str() {
        "anthropic" => Arc::new(Anthropic::new(
            effective_model.clone(),
            base_override.unwrap_or_else(|| "https://api.anthropic.com/v1".into()),
            api_key,
        )),
        "ollama" => Arc::new(Ollama::new(effective_model.clone(), base_override)),
        "xai" => Arc::new(XAi::new(effective_model.clone(), base_override, api_key)),
        "openrouter" => Arc::new(OpenAi::new(
            effective_model.clone(),
            base_url.clone(),
            api_key,
        )),
        "groq" => Arc::new(OpenAi::new(
            effective_model.clone(),
            base_url.clone(),
            api_key,
        )),
        "together" => Arc::new(OpenAi::new(
            effective_model.clone(),
            base_url.clone(),
            api_key,
        )),
        _ => Arc::new(OpenAi::new(
            effective_model.clone(),
            base_url.clone(),
            api_key,
        )),
    };
    eprintln!("[openbuild] provider={provider_name} model={effective_model}");

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

    let mut tools: Vec<Box<dyn Tool>> = if cli.no_tools {
        Vec::new()
    } else {
        openbuild_tools::default_tools_with(openbuild_tools::BuildOpts {
            sandbox_profile,
            web_disabled: cli.disable_web_search,
        })
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

    if !cli.no_subagents && !cli.no_tools {
        tools.push(Box::new(openbuild_tools::task::TaskSpawn {
            provider: provider.clone(),
            max_turns: cli.max_turns,
            depth: 0,
            max_depth: 3,
            child_tools: Arc::new(DefaultChildFactory),
        }) as Box<dyn Tool>);
    }

    let cwd_now = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let cfg = openbuild_config::import::discover_all(&cwd_now);
    let mut mcp_clients: Vec<(String, Arc<openbuild_mcp::StdioClient>)> = Vec::new();
    for (name, server) in &cfg.mcp_servers {
        if let openbuild_config::McpServer::Stdio { command, args, env } = server {
            match openbuild_mcp::StdioClient::spawn(command, args, env).await {
                Ok(client) => {
                    if let Ok(remote_tools) = client.list_tools().await {
                        for rt in remote_tools {
                            tools.push(Box::new(McpToolAdapter {
                                client: client.clone(),
                                name: rt.name.clone(),
                                description: rt.description.clone(),
                                input_schema: rt.input_schema.clone(),
                                qualified: format!("mcp__{}__{}", name, rt.name),
                            }) as Box<dyn Tool>);
                        }
                    }
                    mcp_clients.push((name.clone(), client));
                }
                Err(e) => {
                    eprintln!("[mcp] {name}: spawn failed: {e}");
                }
            }
        }
    }
    if !mcp_clients.is_empty() {
        eprintln!("[mcp] {} server(s) connected", mcp_clients.len());
    }

    let mut engine = Engine::default();
    if cli.always_approve {
        engine.mode = PermMode::BypassPermissions;
    } else if let Some(m) = &cli.permission_mode {
        engine.mode = match m.as_str() {
            "acceptEdits" => PermMode::AcceptEdits,
            "auto" => PermMode::Auto,
            "dontAsk" => PermMode::DontAsk,
            "bypassPermissions" => PermMode::BypassPermissions,
            "plan" if !cli.no_plan => PermMode::Plan,
            "plan" => PermMode::Default,
            _ => PermMode::Default,
        };
    } else if let Some(def) = &agent_def {
        if let Some(pm) = &def.frontmatter.permission_mode {
            engine.mode = match pm.as_str() {
                "acceptEdits" => PermMode::AcceptEdits,
                "auto" => PermMode::Auto,
                "dontAsk" => PermMode::DontAsk,
                "bypassPermissions" => PermMode::BypassPermissions,
                "plan" if !cli.no_plan => PermMode::Plan,
                _ => engine.mode,
            };
        }
    }
    for r in &cli.allow {
        engine.add_allow(r)?;
    }
    for r in &cli.deny {
        engine.add_deny(r)?;
    }

    let hooks = Arc::new(openbuild_hooks::Registry::discover().unwrap_or_default());
    if !hooks.hooks.is_empty() {
        eprintln!("[hooks] {} hook(s) loaded", hooks.hooks.len());
    }
    let session_payload =
        serde_json::json!({"cwd": std::env::current_dir().ok().map(|p| p.display().to_string())});
    hooks
        .fire(openbuild_hooks::Event::SessionStart, "", &session_payload)
        .await;
    let user_prompt_payload = serde_json::json!({"prompt": prompt.clone()});
    hooks
        .fire(
            openbuild_hooks::Event::UserPromptSubmit,
            "",
            &user_prompt_payload,
        )
        .await;
    let tool_names = runner_tools_snapshot(&tools);
    let runner = Arc::new(GatedTools {
        tools,
        engine,
        secret_filter: openbuild_redact::Filter::new(),
        hooks: hooks.clone(),
    });

    if cli.restore_code && (cli.resume.is_some() || cli.continue_recent) {
        let resume_path = if let Some(id) = cli.resume.as_deref() {
            if id.is_empty() {
                openbuild_session::most_recent()?
            } else {
                openbuild_session::find_by_id(id)?
            }
        } else {
            openbuild_session::most_recent()?
        };
        if let Some(p) = resume_path {
            if let Ok(text) = std::fs::read_to_string(&p) {
                for line in text.lines() {
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                        continue;
                    };
                    if v.get("type").and_then(|t| t.as_str()) == Some("session_start") {
                        if let Some(sha) = v.get("git_head").and_then(|s| s.as_str()) {
                            let cwd = std::env::current_dir()?;
                            git_checkout(&cwd, sha)?;
                            eprintln!("[restore-code] checked out {sha}");
                        }
                        break;
                    }
                }
            }
        }
    }

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

    if let Some(def) = &agent_def {
        let rendered = openbuild_agents::render_prompt(&def.system_prompt, &tool_names);
        system_blocks.push(Block::Text { text: rendered });
    }

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

    if cli.experimental_memory && !cli.no_memory {
        let mem = openbuild_memory::render_for_system_prompt().unwrap_or_default();
        if !mem.is_empty() {
            system_blocks.push(Block::Text { text: mem });
        }
    }

    for instr in cfg.instructions.iter().take(3) {
        if let Ok(text) = std::fs::read_to_string(&instr.path) {
            system_blocks.push(Block::Text { text });
        }
    }

    if !cli.no_context_inject {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let cwd = std::env::current_dir()
            .ok()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let user = std::env::var("USER").unwrap_or_default();
        let os = std::env::consts::OS;
        let ctx = format!(
            "# Environment\nDate: {today}\nWorking directory: {cwd}\nUser: {user}\nOS: {os}\n"
        );
        system_blocks.push(Block::Text { text: ctx });
    }

    let prompt = if cli.check && !cli.verbatim {
        format!("{prompt}\n\nAfter you finish, run a self-verification loop: list each requirement from the request, verify whether it was met, and fix any gaps before stopping.")
    } else {
        prompt
    };

    let mut messages = if cli.continue_recent || cli.resume.is_some() {
        rebuild_messages_from_session(cli.resume.as_deref(), cli.continue_recent)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    messages.push(Message::user_text(prompt));

    let req = Request {
        model: effective_model.clone(),
        system: system_blocks,
        messages,
        tools: vec![],
        reasoning_effort: effort,
        max_tokens: cli.max_tokens,
        stream: true,
        temperature: cli.temperature,
        top_p: cli.top_p,
        seed: cli.seed,
        stop: cli.stop.clone(),
    };

    let session = resolve_session(&cli)?;
    if let Some(s) = &session {
        eprintln!("session: {}", s.path().display());
    }

    let output_file = if let Some(path) = &cli.output {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        Some(std::sync::Mutex::new(
            std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?,
        ))
    } else {
        None
    };
    let sink = Stdout {
        format: cli.output_format.clone(),
        session,
        output_file,
        capture: String::new(),
        tokens_used: 0,
        compact_threshold: cli.auto_compact_after,
        hooks: Some(hooks.clone()),
    };

    let outcome = if let Some(n) = cli.best_of_n {
        run_best_of_n(agent, req, n, sink).await
    } else if cli.check {
        let agent_arc = Arc::new(agent);
        let pass1 = agent_arc.run(req.clone(), sink).await;
        if pass1.is_ok() {
            eprintln!("[check] running verification pass");
            let mut req2 = req;
            req2.messages
                .push(Message::user_text(
                    "Verification pass: re-read your previous response. List each requirement from the original request. For each, mark done/partial/missing. Fix any gap. Output only the corrections.",
                ));
            let sink2 = Stdout {
                format: cli.output_format.clone(),
                session: None,
                output_file: None,
                capture: String::new(),
                tokens_used: 0,
                compact_threshold: cli.auto_compact_after,
                hooks: Some(hooks.clone()),
            };
            let _ = agent_arc.run(req2, sink2).await;
        }
        pass1.map(|_| ()).map_err(Into::into)
    } else {
        agent.run(req, sink).await.map(|_| ()).map_err(Into::into)
    };
    let stop_outcomes = hooks
        .fire(openbuild_hooks::Event::Stop, "", &serde_json::json!({}))
        .await;
    let refused = stop_outcomes.iter().any(|o| o.exit_code == 2);
    if refused {
        eprintln!("[stop hook] refused stop (exit 2); rerun --continue to keep going");
    }
    hooks
        .fire(
            openbuild_hooks::Event::SessionEnd,
            "",
            &serde_json::json!({}),
        )
        .await;
    outcome
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
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        let trimmed = buf.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    anyhow::bail!("no prompt; use -p, --prompt-file, --prompt-json, or pipe via stdin")
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
        Cmd::Models { action } => match action {
            None => cmd_models(),
            Some(ModelsCmd::Live {
                provider,
                base_url,
                api_key,
            }) => {
                let (p, _m, b, k) = resolve_provider(
                    provider.as_deref(),
                    None,
                    base_url.as_deref(),
                    api_key.as_deref(),
                );
                cmd_models_live(&p, &b, &k).await
            }
        },
        Cmd::Sessions { action } => cmd_sessions(action),
        Cmd::Mcp { action } => cmd_mcp(action).await,
        Cmd::Skills { action } => cmd_skills(action),
        Cmd::Agents { action } => cmd_agents(action),
        Cmd::Memory { action } => cmd_memory(action),
        Cmd::Import { action } => cmd_import(action),
        Cmd::Trace { action } => cmd_trace(action),
        Cmd::Agent { action } => cmd_agent_ipc(action).await,
        Cmd::Hooks { action } => cmd_hooks(action).await,
        Cmd::Setup => cmd_setup(),
        Cmd::Update => cmd_update(),
        Cmd::Sandbox { action } => cmd_sandbox(action),
        Cmd::Tui => cmd_tui().await,
        Cmd::Cost { session, json } => cmd_cost(session, *json),
        Cmd::Completions { shell } => cmd_completions(shell),
    }
}

fn cmd_cost(session_id: &str, json: bool) -> Result<()> {
    let path = if session_id.is_empty() {
        openbuild_session::most_recent()?
    } else {
        openbuild_session::find_by_id(session_id)?
    };
    let Some(path) = path else {
        anyhow::bail!("no session found");
    };
    let text = std::fs::read_to_string(&path)?;
    let mut input = 0u64;
    let mut output = 0u64;
    let mut reasoning = 0u64;
    let mut cache_read = 0u64;
    let mut cache_write = 0u64;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let t = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if t != "usage" {
            continue;
        }
        input += v.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
        output += v.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
        reasoning += v
            .get("reasoning_tokens")
            .and_then(|x| x.as_u64())
            .unwrap_or(0);
        cache_read += v
            .get("cache_read_tokens")
            .and_then(|x| x.as_u64())
            .unwrap_or(0);
        cache_write += v
            .get("cache_write_tokens")
            .and_then(|x| x.as_u64())
            .unwrap_or(0);
    }
    if json {
        let out = serde_json::json!({
            "session": path.display().to_string(),
            "input_tokens": input,
            "output_tokens": output,
            "reasoning_tokens": reasoning,
            "cache_read_tokens": cache_read,
            "cache_write_tokens": cache_write,
            "total_tokens": input + output + reasoning,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("session: {}", path.display());
        println!("  input tokens:      {input}");
        println!("  output tokens:     {output}");
        println!("  reasoning tokens:  {reasoning}");
        println!("  cache read:        {cache_read}");
        println!("  cache write:       {cache_write}");
        println!("  total:             {}", input + output + reasoning);
    }
    Ok(())
}

fn cmd_completions(shell: &str) -> Result<()> {
    use clap::CommandFactory;
    use clap_complete::{generate, Shell};
    let s = match shell {
        "bash" => Shell::Bash,
        "zsh" => Shell::Zsh,
        "fish" => Shell::Fish,
        "powershell" => Shell::PowerShell,
        "elvish" => Shell::Elvish,
        _ => anyhow::bail!("unsupported shell: {shell}"),
    };
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    generate(s, &mut cmd, bin_name, &mut std::io::stdout());
    Ok(())
}

async fn cmd_tui() -> Result<()> {
    let (provider_name, model, base_url, api_key) = resolve_provider(None, None, None, None);
    eprintln!("[openbuild] provider={provider_name} model={model}");

    let provider: Arc<dyn Provider> = match provider_name.as_str() {
        "anthropic" => Arc::new(Anthropic::new(model.clone(), base_url, api_key)),
        "ollama" => Arc::new(Ollama::new(model.clone(), Some(base_url))),
        "xai" => Arc::new(XAi::new(model.clone(), Some(base_url), api_key)),
        _ => Arc::new(OpenAi::new(model.clone(), base_url, api_key)),
    };

    let tools = openbuild_tools::default_tools_with(openbuild_tools::BuildOpts {
        sandbox_profile: None,
        web_disabled: false,
    });
    let engine = openbuild_permissions::Engine {
        mode: openbuild_permissions::Mode::BypassPermissions,
        ..Default::default()
    };
    let hooks = Arc::new(openbuild_hooks::Registry::default());
    let runner = Arc::new(GatedTools {
        tools,
        engine,
        secret_filter: openbuild_redact::Filter::new(),
        hooks,
    });
    let agent = Arc::new(AgentLoop {
        provider,
        tools: runner,
        max_turns: 10,
    });

    struct ChatBackend {
        agent: Arc<AgentLoop>,
        model: String,
        history: Vec<Message>,
    }

    #[async_trait]
    impl openbuild_tui::Backend for ChatBackend {
        async fn send(
            &mut self,
            prompt: String,
            out: tokio::sync::mpsc::Sender<openbuild_core::Event>,
        ) {
            self.history.push(Message::user_text(prompt));
            let req = Request {
                model: self.model.clone(),
                system: vec![],
                messages: self.history.clone(),
                tools: vec![],
                reasoning_effort: None,
                max_tokens: None,
                stream: true,
                temperature: None,
                top_p: None,
                seed: None,
                stop: Vec::new(),
            };
            struct Forward(tokio::sync::mpsc::Sender<openbuild_core::Event>);
            #[async_trait]
            impl openbuild_core::Sink for Forward {
                async fn on(&mut self, ev: openbuild_core::Event) {
                    let _ = self.0.send(ev).await;
                }
            }
            let sink = Forward(out);
            let _ = self.agent.run(req, sink).await;
        }

        fn slash(&mut self, cmd: &str, arg: &str) -> Option<String> {
            match cmd {
                "/help" => Some("/quit /clear /cost /agent NAME /model NAME /help".into()),
                "/cost" => Some("(cost tracker — pending TUI integration)".into()),
                "/model" => {
                    if arg.is_empty() {
                        Some(format!("current model: {}", self.model))
                    } else {
                        self.model = arg.to_string();
                        Some(format!("model set to {}", self.model))
                    }
                }
                "/agent" => Some(format!(
                    "(agent switch not wired in TUI yet; restart with --agent {arg})"
                )),
                _ => None,
            }
        }

        fn header(&self) -> String {
            format!("openbuild — {}", self.model)
        }
    }

    let mut backend = ChatBackend {
        agent,
        model,
        history: Vec::new(),
    };
    openbuild_tui::run_streaming(&mut backend, true).await
}

fn cmd_sandbox(action: &SandboxCmd) -> Result<()> {
    let dir = dirs::home_dir()
        .context("no home")?
        .join(".openbuild")
        .join("sandbox");
    match action {
        SandboxCmd::List => {
            for builtin in ["off", "read-only", "workspace-write"] {
                println!("{builtin}\tbuiltin");
            }
            if dir.exists() {
                for e in std::fs::read_dir(&dir)?.flatten() {
                    if let Some(stem) = e.path().file_stem().and_then(|s| s.to_str()) {
                        println!("{stem}\tuser");
                    }
                }
            }
        }
        SandboxCmd::Show { name } => {
            let cwd = std::env::current_dir()?;
            let p = openbuild_sandbox::discover_profile(name, &cwd);
            println!("{}", toml::to_string_pretty(&p)?);
        }
    }
    Ok(())
}

async fn cmd_hooks(action: &HooksCmd) -> Result<()> {
    let reg = openbuild_hooks::Registry::discover()?;
    match action {
        HooksCmd::List => {
            println!("loaded {} hooks", reg.hooks.len());
            for h in &reg.hooks {
                println!(
                    "  [{:?}] matcher={:?} timeout={}ms blocking={} cmd={}",
                    h.event, h.matcher, h.timeout_ms, h.blocking, h.command
                );
            }
        }
        HooksCmd::Test { event, matcher } => {
            let ev = match event.as_str() {
                "PreToolUse" => openbuild_hooks::Event::PreToolUse,
                "PostToolUse" => openbuild_hooks::Event::PostToolUse,
                "Stop" => openbuild_hooks::Event::Stop,
                "SessionStart" => openbuild_hooks::Event::SessionStart,
                "SessionEnd" => openbuild_hooks::Event::SessionEnd,
                "UserPromptSubmit" => openbuild_hooks::Event::UserPromptSubmit,
                "PreCompact" => openbuild_hooks::Event::PreCompact,
                "Notification" => openbuild_hooks::Event::Notification,
                _ => anyhow::bail!("unknown event: {event}"),
            };
            let payload = serde_json::json!({"matcher": matcher});
            for outcome in reg.fire(ev, matcher, &payload).await {
                println!(
                    "cmd={} exit={} blocked={}",
                    outcome.command, outcome.exit_code, outcome.blocked
                );
                if !outcome.stdout.is_empty() {
                    println!("stdout: {}", outcome.stdout);
                }
                if !outcome.stderr.is_empty() {
                    eprintln!("stderr: {}", outcome.stderr);
                }
            }
        }
    }
    Ok(())
}

fn cmd_setup() -> Result<()> {
    let home = dirs::home_dir().context("no home dir")?;
    let dir = home.join(".openbuild");
    for sub in ["", "sessions", "skills", "agents", "hooks", "sandbox"] {
        std::fs::create_dir_all(dir.join(sub))?;
    }
    let config = dir.join("config.toml");
    if !config.exists() {
        std::fs::write(&config, "[cli]\nprovider = \"openai\"\n\n[mcp_servers]\n")?;
    }
    println!("openbuild setup complete at {}", dir.display());
    Ok(())
}

fn cmd_update() -> Result<()> {
    println!("openbuild update: install via `cargo install --git https://github.com/rohitg00/openbuild openbuild-cli`");
    println!("auto-update not enabled (zero phone-home policy)");
    Ok(())
}

fn cmd_agents(action: &AgentsCmd) -> Result<()> {
    let bundled = bundled_dir();
    match action {
        AgentsCmd::List => {
            for a in openbuild_agents::discover_all(bundled.as_deref()) {
                println!(
                    "{}\t{:?}\t{}",
                    a.frontmatter.name, a.source, a.frontmatter.description
                );
            }
        }
        AgentsCmd::Show { name } => {
            let a = openbuild_agents::load_by_name(name, bundled.as_deref())?;
            println!("{}", a.system_prompt);
        }
        AgentsCmd::Add {
            name,
            description,
            capability,
            permission_mode,
            prompt,
            prompt_file,
        } => {
            let dir = dirs::home_dir()
                .context("no home")?
                .join(".openbuild")
                .join("agents");
            std::fs::create_dir_all(&dir)?;
            let body = if let Some(p) = prompt_file {
                std::fs::read_to_string(p)?
            } else {
                prompt.clone().unwrap_or_else(|| {
                    format!("You are the '{name}' agent. Complete the user's task.")
                })
            };
            let mut front = String::from("---\n");
            front.push_str(&format!("name: {name}\n"));
            if let Some(d) = description {
                front.push_str(&format!("description: {d}\n"));
            }
            if let Some(c) = capability {
                front.push_str(&format!("capability_mode: {c}\n"));
            }
            if let Some(pm) = permission_mode {
                front.push_str(&format!("permission_mode: {pm}\n"));
            }
            front.push_str("---\n\n");
            front.push_str(&body);
            let path = dir.join(format!("{name}.md"));
            std::fs::write(&path, front)?;
            println!("wrote agent to {}", path.display());
        }
        AgentsCmd::Remove { name } => {
            let dir = dirs::home_dir()
                .context("no home")?
                .join(".openbuild")
                .join("agents");
            let path = dir.join(format!("{name}.md"));
            let existed = path.exists();
            if existed {
                std::fs::remove_file(&path)?;
            }
            println!("removed: {existed}");
        }
    }
    Ok(())
}

fn cmd_memory(action: &MemoryCmd) -> Result<()> {
    match action {
        MemoryCmd::List => {
            let store = openbuild_memory::load()?;
            for (k, e) in &store.entries {
                println!("{k}\t{}\t{}", e.updated_at, e.value);
            }
        }
        MemoryCmd::Get { key } => {
            if let Some(v) = openbuild_memory::get(key)? {
                println!("{v}");
            }
        }
        MemoryCmd::Set { key, value } => openbuild_memory::set(key, value)?,
        MemoryCmd::Remove { key } => {
            let existed = openbuild_memory::remove(key)?;
            println!("removed: {existed}");
        }
        MemoryCmd::Clear => {
            openbuild_memory::save(&openbuild_memory::Store::default())?;
            println!("cleared");
        }
        MemoryCmd::Edit => {
            let path = openbuild_memory::path()?;
            if !path.exists() {
                openbuild_memory::save(&openbuild_memory::Store::default())?;
            }
            let editor = std::env::var("EDITOR")
                .or_else(|_| std::env::var("VISUAL"))
                .unwrap_or_else(|_| "vi".into());
            let status = std::process::Command::new(&editor).arg(&path).status()?;
            if !status.success() {
                anyhow::bail!("editor exited non-zero");
            }
        }
    }
    Ok(())
}

fn cmd_import(action: &ImportCmd) -> Result<()> {
    match action {
        ImportCmd::Claude { path } => import_session(path, "claude", claude_event_map),
        ImportCmd::Cursor { path } => import_session(path, "cursor", cursor_event_map),
        ImportCmd::Codex { path } => import_session(path, "codex", codex_event_map),
        ImportCmd::OpenCode { path } => import_session(path, "opencode", opencode_event_map),
        ImportCmd::ClaudeAll => import_claude_all(),
        ImportCmd::ClaudeAgents => import_claude_agents(),
        ImportCmd::ClaudeSkills => import_claude_skills(),
        ImportCmd::ClaudeMcp => import_claude_mcp(),
    }
}

fn import_claude_all() -> Result<()> {
    let home = dirs::home_dir().context("no home")?;
    let projects = home.join(".claude").join("projects");
    if !projects.exists() {
        anyhow::bail!("no Claude projects dir at {}", projects.display());
    }
    let mut count = 0;
    for entry in walk(&projects) {
        if entry.extension().and_then(|s| s.to_str()) == Some("jsonl")
            && import_session(&entry, "claude", claude_event_map).is_ok()
        {
            count += 1;
        }
    }
    println!("imported {count} session(s) from {}", projects.display());
    Ok(())
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

fn import_claude_agents() -> Result<()> {
    let home = dirs::home_dir().context("no home")?;
    let src = home.join(".claude").join("agents");
    let dst = home.join(".openbuild").join("agents");
    if !src.exists() {
        anyhow::bail!("no Claude agents dir at {}", src.display());
    }
    std::fs::create_dir_all(&dst)?;
    let mut count = 0;
    for entry in std::fs::read_dir(&src)?.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) == Some("md") {
            if let Some(name) = p.file_name() {
                std::fs::copy(&p, dst.join(name))?;
                count += 1;
            }
        }
    }
    println!("imported {count} agent(s)");
    Ok(())
}

fn import_claude_skills() -> Result<()> {
    let home = dirs::home_dir().context("no home")?;
    let src = home.join(".claude").join("skills");
    let dst = home.join(".openbuild").join("skills");
    if !src.exists() {
        anyhow::bail!("no Claude skills dir at {}", src.display());
    }
    std::fs::create_dir_all(&dst)?;
    let mut count = 0;
    for entry in std::fs::read_dir(&src)?.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(name) = p.file_name() {
                let target = dst.join(name);
                copy_dir(&p, &target).ok();
                count += 1;
            }
        }
    }
    println!("imported {count} skill(s)");
    Ok(())
}

fn import_claude_mcp() -> Result<()> {
    let home = dirs::home_dir().context("no home")?;
    let settings = home.join(".claude").join("settings.json");
    if !settings.exists() {
        anyhow::bail!("no Claude settings.json at {}", settings.display());
    }
    let text = std::fs::read_to_string(&settings)?;
    let v: serde_json::Value = serde_json::from_str(&text)?;
    let Some(servers) = v.get("mcpServers").and_then(|s| s.as_object()) else {
        println!("no mcpServers in {}", settings.display());
        return Ok(());
    };
    let mut count = 0;
    for (name, def) in servers {
        if let Some(cmd) = def.get("command").and_then(|c| c.as_str()) {
            let args: Vec<String> = def
                .get("args")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            user_config_add_mcp_stdio(name, cmd, &args)?;
            count += 1;
        } else if let Some(url) = def.get("url").and_then(|u| u.as_str()) {
            user_config_add_mcp_http(name, url, "http")?;
            count += 1;
        }
    }
    println!("imported {count} MCP server(s)");
    Ok(())
}

fn import_session(
    path: &std::path::Path,
    agent: &str,
    map_fn: fn(&serde_json::Value) -> Option<&'static str>,
) -> Result<()> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let dest_dir = openbuild_session::sessions_dir()?;
    std::fs::create_dir_all(&dest_dir)?;
    let new_id = uuid::Uuid::new_v4();
    let dest = dest_dir.join(format!("{new_id}.jsonl"));
    let mut out = String::new();
    out.push_str(
        &serde_json::json!({
            "type": "session_start",
            "id": new_id.to_string(),
            "imported_from": path.display().to_string(),
            "agent": agent,
        })
        .to_string(),
    );
    out.push('\n');
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(mapped) = map_fn(&v) {
            let entry = serde_json::json!({"type": mapped, "payload": v});
            out.push_str(&entry.to_string());
            out.push('\n');
        }
    }
    std::fs::write(&dest, out)?;
    println!("imported -> {}", dest.display());
    Ok(())
}

fn claude_event_map(v: &serde_json::Value) -> Option<&'static str> {
    match v.get("type").and_then(|t| t.as_str())? {
        "user" => Some("user_message"),
        "assistant" => Some("assistant_message"),
        "summary" => Some("summary"),
        _ => None,
    }
}

fn cursor_event_map(v: &serde_json::Value) -> Option<&'static str> {
    let role = v
        .get("role")
        .and_then(|r| r.as_str())
        .or_else(|| v.get("type").and_then(|t| t.as_str()))?;
    match role {
        "user" => Some("user_message"),
        "assistant" | "ai" => Some("assistant_message"),
        "tool" | "tool_result" => Some("tool_result"),
        _ => None,
    }
}

fn codex_event_map(v: &serde_json::Value) -> Option<&'static str> {
    match v.get("type").and_then(|t| t.as_str())? {
        "message" => {
            let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("");
            match role {
                "user" => Some("user_message"),
                "assistant" => Some("assistant_message"),
                _ => None,
            }
        }
        "tool_call" => Some("tool_call"),
        "tool_result" => Some("tool_result"),
        _ => None,
    }
}

fn opencode_event_map(v: &serde_json::Value) -> Option<&'static str> {
    let role = v.get("role").and_then(|r| r.as_str())?;
    match role {
        "user" => Some("user_message"),
        "assistant" => Some("assistant_message"),
        "tool" => Some("tool_result"),
        _ => None,
    }
}

fn cmd_trace(action: &TraceCmd) -> Result<()> {
    match action {
        TraceCmd::Export { id } => {
            let path = openbuild_session::find_by_id(id)?
                .ok_or_else(|| anyhow::anyhow!("session not found: {id}"))?;
            let text = std::fs::read_to_string(&path)?;
            let mut spans = Vec::new();
            for line in text.lines() {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                    continue;
                };
                if let Some(t) = v.get("type").and_then(|t| t.as_str()) {
                    spans.push(serde_json::json!({
                        "name": t,
                        "attributes": v,
                    }));
                }
            }
            let out = serde_json::json!({
                "session_id": id,
                "source": path.display().to_string(),
                "spans": spans,
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
    }
    Ok(())
}

async fn cmd_agent_ipc(action: &AgentRunCmd) -> Result<()> {
    match action {
        AgentRunCmd::Stdio | AgentRunCmd::Headless => {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
            let mut stdin = BufReader::new(tokio::io::stdin()).lines();
            let mut stdout = tokio::io::stdout();
            let ready = serde_json::json!({"type":"ready","protocol":"openbuild-ipc/v0"});
            stdout.write_all(ready.to_string().as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            while let Ok(Some(line)) = stdin.next_line().await {
                let req: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(e) => {
                        let err = serde_json::json!({"type":"error","error":e.to_string()});
                        stdout.write_all(err.to_string().as_bytes()).await?;
                        stdout.write_all(b"\n").await?;
                        continue;
                    }
                };
                let prompt = req
                    .get("prompt")
                    .and_then(|p| p.as_str())
                    .unwrap_or("")
                    .to_string();
                let ack = serde_json::json!({
                    "type": "ack",
                    "received_prompt_len": prompt.len(),
                });
                stdout.write_all(ack.to_string().as_bytes()).await?;
                stdout.write_all(b"\n").await?;
            }
            Ok(())
        }
        AgentRunCmd::Serve { host, port } => agent_serve(host, *port).await,
        AgentRunCmd::Acp => agent_acp().await,
    }
}

async fn agent_acp() -> Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut stdin = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    let ready = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialize",
        "params": {
            "protocolVersion": "0.1.0",
            "serverInfo": {"name": "openbuild", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {
                "prompts": {},
                "tools": {},
                "resources": {}
            }
        }
    });
    stdout.write_all(ready.to_string().as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    while let Ok(Some(line)) = stdin.next_line().await {
        let req: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": {"code": -32700, "message": e.to_string()},
                });
                stdout.write_all(err.to_string().as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                continue;
            }
        };
        let id = req.get("id").cloned();
        let method = req
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        let result = match method.as_str() {
            "initialize" => serde_json::json!({"protocolVersion":"0.1.0"}),
            "prompts/list" => serde_json::json!({"prompts": []}),
            "tools/list" => {
                let tools = openbuild_tools::default_tools();
                let schemas: Vec<_> = tools.iter().map(|t| t.schema()).collect();
                serde_json::json!({"tools": schemas})
            }
            "shutdown" => serde_json::json!({}),
            _ => serde_json::json!({"unsupported_method": method}),
        };
        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        });
        stdout.write_all(resp.to_string().as_bytes()).await?;
        stdout.write_all(b"\n").await?;
    }
    Ok(())
}

async fn agent_serve(host: &str, port: u16) -> Result<()> {
    use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
    use axum::response::Response;
    use axum::routing::{any, get};
    use axum::Router;

    async fn ws_handler(ws: WebSocketUpgrade) -> Response {
        ws.on_upgrade(handle_socket)
    }

    async fn handle_socket(mut socket: WebSocket) {
        let ready = serde_json::json!({"type":"ready","protocol":"openbuild-ws/v0"});
        let _ = socket.send(WsMessage::Text(ready.to_string().into())).await;
        while let Some(Ok(msg)) = socket.recv().await {
            let text = match msg {
                WsMessage::Text(t) => t.to_string(),
                WsMessage::Close(_) => break,
                _ => continue,
            };
            let req: serde_json::Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(e) => {
                    let err = serde_json::json!({"type":"error","error":e.to_string()});
                    let _ = socket.send(WsMessage::Text(err.to_string().into())).await;
                    continue;
                }
            };
            let prompt = req
                .get("prompt")
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .to_string();
            let ack = serde_json::json!({
                "type": "ack",
                "received_prompt_len": prompt.len(),
            });
            let _ = socket.send(WsMessage::Text(ack.to_string().into())).await;
        }
    }

    async fn health() -> &'static str {
        "openbuild agent serve\n"
    }

    let app = Router::new()
        .route("/", get(health))
        .route("/ws", any(ws_handler));

    let addr: std::net::SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("openbuild agent serve: ws://{host}:{port}/ws");
    axum::serve(listener, app).await?;
    Ok(())
}

fn git_checkout(cwd: &std::path::Path, sha: &str) -> Result<()> {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("checkout")
        .arg(sha)
        .status()
        .context("git checkout")?;
    if !status.success() {
        anyhow::bail!("git checkout {sha} failed");
    }
    Ok(())
}

fn runner_tools_snapshot(tools: &[Box<dyn Tool>]) -> Vec<String> {
    tools.iter().map(|t| t.schema().name).collect()
}

fn resolve_provider(
    cli_provider: Option<&str>,
    cli_model: Option<&str>,
    cli_base_url: Option<&str>,
    cli_api_key: Option<&str>,
) -> (String, String, String, String) {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());

    let provider = cli_provider
        .map(str::to_string)
        .or_else(|| env("OPENBUILD_PROVIDER"))
        .or_else(|| {
            if env("ANTHROPIC_API_KEY").is_some() {
                Some("anthropic".into())
            } else if env("XAI_API_KEY").is_some() || env("GROK_API_KEY").is_some() {
                Some("xai".into())
            } else if env("OLLAMA_HOST").is_some() {
                Some("ollama".into())
            } else if env("OPENROUTER_API_KEY").is_some() {
                Some("openrouter".into())
            } else if env("GROQ_API_KEY").is_some() {
                Some("groq".into())
            } else if env("TOGETHER_API_KEY").is_some() {
                Some("together".into())
            } else {
                Some("openai".into())
            }
        })
        .unwrap();

    let (default_model, default_base, key_var) = match provider.as_str() {
        "anthropic" => (
            "claude-sonnet-4-5",
            "https://api.anthropic.com/v1",
            "ANTHROPIC_API_KEY",
        ),
        "xai" => ("grok-4", "https://api.x.ai/v1", "XAI_API_KEY"),
        "ollama" => ("llama3.2", "http://localhost:11434/v1", ""),
        "openrouter" => (
            "openrouter/auto",
            "https://openrouter.ai/api/v1",
            "OPENROUTER_API_KEY",
        ),
        "groq" => (
            "llama-3.3-70b-versatile",
            "https://api.groq.com/openai/v1",
            "GROQ_API_KEY",
        ),
        "together" => (
            "meta-llama/Llama-3.3-70B-Instruct-Turbo",
            "https://api.together.xyz/v1",
            "TOGETHER_API_KEY",
        ),
        _ => ("gpt-4o-mini", "https://api.openai.com/v1", "OPENAI_API_KEY"),
    };

    let model = cli_model
        .map(str::to_string)
        .or_else(|| env("OPENBUILD_MODEL"))
        .unwrap_or_else(|| default_model.to_string());

    let base_url = cli_base_url
        .map(str::to_string)
        .or_else(|| env("OPENBUILD_BASE_URL"))
        .or_else(|| env(&format!("{}_BASE_URL", provider.to_uppercase())))
        .unwrap_or_else(|| default_base.to_string());

    let api_key = cli_api_key
        .map(str::to_string)
        .or_else(|| env("OPENBUILD_API_KEY"))
        .or_else(|| {
            if key_var.is_empty() {
                Some(String::new())
            } else {
                env(key_var)
            }
        })
        .or_else(|| {
            if provider == "xai" {
                env("GROK_API_KEY")
            } else {
                None
            }
        })
        .unwrap_or_default();

    (provider, model, base_url.clone(), api_key)
}

fn rebuild_messages_from_session(
    resume_id: Option<&str>,
    continue_recent: bool,
) -> Result<Vec<Message>> {
    let path = if continue_recent {
        openbuild_session::most_recent()?
    } else if let Some(id) = resume_id {
        if id.is_empty() {
            openbuild_session::most_recent()?
        } else {
            openbuild_session::find_by_id(id)?
        }
    } else {
        None
    };
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let text = std::fs::read_to_string(&path)?;
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let t = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match t {
            "user_message" => {
                if let Some(txt) = v.get("text").and_then(|x| x.as_str()) {
                    out.push(Message::user_text(txt));
                } else if let Some(payload) = v.get("payload") {
                    if let Some(s) = extract_text_from_payload(payload) {
                        out.push(Message::user_text(s));
                    }
                }
            }
            "assistant_message" => {
                if let Some(payload) = v.get("payload") {
                    if let Some(s) = extract_text_from_payload(payload) {
                        out.push(Message::assistant_text(s));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

fn extract_text_from_payload(v: &serde_json::Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    if let Some(content) = v.get("content") {
        if let Some(s) = content.as_str() {
            return Some(s.to_string());
        }
        if let Some(arr) = content.as_array() {
            let mut out = String::new();
            for b in arr {
                if let Some(s) = b.get("text").and_then(|t| t.as_str()) {
                    out.push_str(s);
                }
            }
            if !out.is_empty() {
                return Some(out);
            }
        }
    }
    if let Some(text) = v.get("text").and_then(|t| t.as_str()) {
        return Some(text.to_string());
    }
    None
}

fn bundled_dir() -> Option<std::path::PathBuf> {
    let candidates = [
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .map(|p| p.join("..").join("bundled").join("agents")),
        Some(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("bundled")
                .join("agents"),
        ),
    ];
    candidates.into_iter().flatten().find(|c| c.exists())
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
    println!();
    println!("set OPENBUILD_PROVIDER + OPENBUILD_API_KEY then run `openbuild models live` to enumerate live model ids");
    Ok(())
}

async fn cmd_models_live(provider: &str, base_url: &str, api_key: &str) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent("openbuild/0.0.1")
        .build()?;
    let url = match provider {
        "anthropic" => format!("{}/models", base_url.trim_end_matches('/')),
        _ => format!("{}/models", base_url.trim_end_matches('/')),
    };
    let mut req = client.get(&url);
    if provider == "anthropic" {
        req = req
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01");
    } else {
        req = req.bearer_auth(api_key);
    }
    let resp = req.send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        anyhow::bail!("models endpoint returned {status}: {body}");
    }
    let v: serde_json::Value = serde_json::from_str(&body)?;
    let arr = v
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| anyhow::anyhow!("response had no .data array"))?;
    for item in arr {
        if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
            println!("{id}");
        }
    }
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
                    openbuild_config::McpServer::Http { url } => {
                        print!("{name} (http: {url}) ... ");
                        std::io::stdout().flush().ok();
                        match openbuild_mcp::HttpClient::connect(url).await {
                            Ok(c) => match c.list_tools().await {
                                Ok(tools) => println!("ok ({} tools)", tools.len()),
                                Err(e) => println!("FAIL list_tools: {e}"),
                            },
                            Err(e) => println!("FAIL connect: {e}"),
                        }
                    }
                    openbuild_config::McpServer::Sse { url } => {
                        println!("{name} (sse: {url}) — uses HTTP streamable transport");
                    }
                }
            }
        }
        McpCmd::Add {
            name,
            command,
            args,
        } => {
            user_config_add_mcp_stdio(name, command, args)?;
            println!("added stdio MCP server '{name}'");
        }
        McpCmd::AddHttp { name, url } => {
            user_config_add_mcp_http(name, url, "http")?;
            println!("added http MCP server '{name}'");
        }
        McpCmd::AddSse { name, url } => {
            user_config_add_mcp_http(name, url, "sse")?;
            println!("added sse MCP server '{name}'");
        }
        McpCmd::Remove { name } => {
            let removed = user_config_remove_mcp(name)?;
            println!("removed: {removed}");
        }
    }
    Ok(())
}

fn user_config_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("no home dir")?
        .join(".openbuild")
        .join("config.toml"))
}

fn read_user_config() -> Result<toml::Value> {
    let path = user_config_path()?;
    if !path.exists() {
        return Ok(toml::Value::Table(toml::value::Table::new()));
    }
    let text = std::fs::read_to_string(&path)?;
    Ok(toml::from_str(&text).unwrap_or(toml::Value::Table(toml::value::Table::new())))
}

fn write_user_config(v: &toml::Value) -> Result<()> {
    let path = user_config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, toml::to_string_pretty(v)?)?;
    Ok(())
}

fn user_config_add_mcp_stdio(name: &str, command: &str, args: &[String]) -> Result<()> {
    let mut cfg = read_user_config()?;
    let table = cfg.as_table_mut().context("config root must be table")?;
    let servers = table
        .entry("mcp_servers".to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()))
        .as_table_mut()
        .context("mcp_servers must be table")?;
    let mut entry = toml::value::Table::new();
    entry.insert("type".into(), toml::Value::String("stdio".into()));
    entry.insert("command".into(), toml::Value::String(command.into()));
    entry.insert(
        "args".into(),
        toml::Value::Array(
            args.iter()
                .map(|a| toml::Value::String(a.clone()))
                .collect(),
        ),
    );
    servers.insert(name.into(), toml::Value::Table(entry));
    write_user_config(&cfg)
}

fn user_config_add_mcp_http(name: &str, url: &str, kind: &str) -> Result<()> {
    let mut cfg = read_user_config()?;
    let table = cfg.as_table_mut().context("config root must be table")?;
    let servers = table
        .entry("mcp_servers".to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()))
        .as_table_mut()
        .context("mcp_servers must be table")?;
    let mut entry = toml::value::Table::new();
    entry.insert("type".into(), toml::Value::String(kind.into()));
    entry.insert("url".into(), toml::Value::String(url.into()));
    servers.insert(name.into(), toml::Value::Table(entry));
    write_user_config(&cfg)
}

fn user_config_remove_mcp(name: &str) -> Result<bool> {
    let mut cfg = read_user_config()?;
    let Some(table) = cfg.as_table_mut() else {
        return Ok(false);
    };
    let Some(servers) = table.get_mut("mcp_servers").and_then(|v| v.as_table_mut()) else {
        return Ok(false);
    };
    let existed = servers.remove(name).is_some();
    write_user_config(&cfg)?;
    Ok(existed)
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
        SkillsCmd::Install { source, name } => {
            let user_dir = dirs::home_dir()
                .context("no home")?
                .join(".openbuild")
                .join("skills");
            std::fs::create_dir_all(&user_dir)?;
            let target_name = name
                .clone()
                .or_else(|| {
                    source
                        .rsplit('/')
                        .next()
                        .map(|s| s.trim_end_matches(".git").to_string())
                })
                .ok_or_else(|| anyhow::anyhow!("can't derive skill name from source"))?;
            let dest = user_dir.join(&target_name);
            if source.starts_with("http")
                && (source.ends_with(".git") || source.contains("github.com"))
            {
                let status = std::process::Command::new("git")
                    .arg("clone")
                    .arg("--depth=1")
                    .arg(source)
                    .arg(&dest)
                    .status()
                    .context("git clone")?;
                if !status.success() {
                    anyhow::bail!("git clone failed");
                }
            } else if std::path::Path::new(source).is_dir() {
                std::fs::create_dir_all(&dest)?;
                copy_dir(std::path::Path::new(source), &dest)?;
            } else {
                anyhow::bail!("unsupported skill source: {source}");
            }
            println!("installed -> {}", dest.display());
        }
        SkillsCmd::Remove { name } => {
            let dir = dirs::home_dir()
                .context("no home")?
                .join(".openbuild")
                .join("skills")
                .join(name);
            let existed = dir.exists();
            if existed {
                std::fs::remove_dir_all(&dir)?;
            }
            println!("removed: {existed}");
        }
    }
    Ok(())
}

fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}
