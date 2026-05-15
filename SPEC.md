# openbuild — model-agnostic Rust agent shell

Repo: `rohitg00/openbuild`. Open, provider-agnostic, single static binary. No vendor lock-in, no proprietary proxy, no telemetry.

## Prior art reference (internal only — never surface in openbuild copy)

Surface shape cross-referenced from multiple published agent shells. Universal patterns, not vendor-specific:
- Installer pattern: `~/.<shell>/{bin,bundled,skills,sessions,logs}/` (Claude Code, Cursor CLI, OpenCode, Aider, Cline, Codex, vendor-X shell).
- Provider abstraction: every shell now exposes BYOK + OpenAI-compatible + Ollama via base_url override.
- Subagent decomposition: role + persona + agent + skill is the cross-vendor convergence (Claude Code agents, Cursor modes, OpenCode profiles, vendor-X roles).
- Config import: shells leak each other's formats — `~/.claude/`, `~/.cursor/`, `~/.codex/`, `AGENTS.md`, `CLAUDE.md`, `.cursorrules`, `.aiderignore`. Universal because users carry config across tools.

openbuild = single shell that **reads every config**, **runs every model**, **ships zero vendor branding**.

## openbuild CLI surface

### Top-level flags
```
openbuild [--agent NAME] [--agents JSON] [--allow RULE] [--deny RULE]
     [--always-approve] [--best-of-n N] [-c|--continue] [--check]
     [--cwd PATH] [--disable-web-search] [--disallowed-tools T,T]
     [--effort low|medium|high|xhigh|max] [--experimental-memory] [--no-memory]
     [-m|--model ID] [--max-turns N] [--no-alt-screen] [--no-plan] [--no-subagents]
     [--oauth] [--output-format plain|json|streaming-json]
     [-p|--single PROMPT] [--prompt-file PATH] [--prompt-json JSON]
     [--permission-mode default|acceptEdits|auto|dontAsk|bypassPermissions|plan]
     [-r|--resume [SESSION_ID]] [--reasoning-effort EFFORT] [--restore-code]
     [--rules RULES] [--sandbox PROFILE] [--system-prompt-override PROMPT]
     [--tools T,T] [--verbatim] [-w|--worktree [NAME]]
```

### Subcommands
`agent {stdio|headless|serve|leader}` · `import` · `inspect` · `leader` ·
`login` · `mcp {list|add|remove|doctor}` · `memory` · `models` · `sessions` ·
`setup` · `share` · `ssh` · `trace` · `update` · `worktree`

### Bundled artifact taxonomy
```
~/.grok/bundled/
├── manifest.json              # sha256 of every shipped file
├── agents/                    # MD + YAML frontmatter, system prompts w/ ${{ tools.by_kind.X }} templating
│   ├── explore.md             # read-only, plan mode
│   ├── general-purpose.md     # full caps, recursive subagent spawn
│   └── plan.md                # read-only architect
├── roles/                     # TOML: capability_mode, reasoning_effort, default_fork_context
│   └── {explore,plan,implementer,reviewer,test-writer,security-auditor,...}.toml
├── personas/                  # TOML: description + instructions + [[inputs]] + [[outputs]]
│   └── {implementer,reviewer,researcher,design-doc-{writer,reviewer},...}.toml
└── skills/                    # Anthropic-style SKILL.md + scripts/ + tests/
    └── {design,implement,pr-babysit,review,shared}/
```

### Layered composition
- **Role** = capability profile (read-only|all, reasoning effort, fork policy)
- **Persona** = task contract (instructions + typed file inputs/outputs)
- **Agent** = system prompt + permission_mode + model + role binding
- **Skill** = Anthropic `SKILL.md` w/ optional scripts (Python/Bash) and tests

### Provider abstraction
```toml
[model.ollama-codellama]
base_url    = "http://localhost:11434/v1"
api_backend = "chat_completions"   # or "responses"
auth_scheme = "none"               # or "bearer"
context_window = 16384
```

## Rust dependency stack

| Concern | Crate |
|---|---|
| Runtime | `tokio` |
| HTTP server | `axum 0.8.6` + `hyper 1.8.1` |
| HTTP client | `reqwest` |
| WebSocket | `axum::extract::ws` |
| TUI | `ratatui` + `crossterm` (alt-screen flag implies it) |
| CLI parsing | `clap` |
| Retry | `backon 1.6.0` |
| Search/grep | `globset`, `ignore`, `matcher`, `searcher` (ripgrep internals) |
| Code parsing | `tree-sitter` (per-language grammars) |
| Full-text rank | `bm25 2.3.2` |
| Hashing | `blake3 1.8.2`, `sha2` |
| Cache | `cached 0.56.0` (SizedCache LRU) |
| Compression | `brotli-decompressor 5.0.0` |
| Sandbox (macOS) | `sandbox-exec` via `seatbelt` profiles |
| Sandbox (Linux) | `landlock` + `seccomp` + optional `bwrap` |

## openbuild crate layout

```
openbuild/
├── Cargo.toml                       # workspace
├── crates/
│   ├── openbuild-cli/               # main binary, clap wiring
│   ├── openbuild-core/              # agent loop, session, memory, tool dispatch
│   ├── openbuild-providers/         # trait + adapters
│   │   ├── src/anthropic.rs         # /v1/messages
│   │   ├── src/openai.rs            # /v1/chat/completions + /v1/responses
│   │   ├── src/xai.rs               # public xAI API (alias of openai responses)
│   │   ├── src/ollama.rs            # /api/chat or /v1/chat/completions
│   │   ├── src/openrouter.rs        # /api/v1/chat/completions
│   │   ├── src/bedrock.rs           # AWS SigV4 to /model/.../invoke
│   │   ├── src/vertex.rs            # GCP service account
│   │   └── src/local_llamacpp.rs    # /completion
│   ├── openbuild-tools/             # builtin tools (read/write/edit/bash/grep/glob/...)
│   ├── openbuild-skills/            # SKILL.md loader, frontmatter parse
│   ├── openbuild-mcp/               # rmcp client wiring (stdio + http + sse)
│   ├── openbuild-sandbox/           # macos seatbelt + linux landlock+seccomp
│   ├── openbuild-tui/               # ratatui frontend
│   ├── openbuild-session/           # jsonl session log, resume, share
│   ├── openbuild-permissions/       # allow/deny rules engine (parse Claude format too)
│   └── openbuild-config/            # TOML loader, role/persona/agent registry
└── bundled/                         # default agents/roles/personas/skills (CC0)
```

## Provider trait (model-agnostic seam)

```rust
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;
    fn supports(&self, cap: Capability) -> bool;          // tools, vision, reasoning, streaming
    async fn complete(&self, req: Request) -> Result<Stream<Event>>;
}

pub struct Request {
    pub model: String,
    pub system: Vec<Block>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSchema>,
    pub reasoning_effort: Option<Effort>,
    pub max_tokens: Option<u32>,
    pub stream: bool,
}

pub enum Event {
    TextDelta(String),
    ThinkingDelta(String),
    ToolCall { id: String, name: String, args: Value },
    ToolCallDelta { id: String, args_delta: String },
    Usage(Usage),
    Done(StopReason),
    Error(ProviderError),
}
```

One internal event stream. Each adapter translates from provider wire format (Anthropic SSE events, OpenAI chunk deltas, Ollama NDJSON) into `Event`. Tool-call schema normalized at the boundary — Anthropic `tool_use` ↔ OpenAI `tool_calls` ↔ xAI `responses` items.

## Permission + sandbox port

- Universal permission grammar: `Tool(arg-pattern)` syntax — same shape works for Claude `permissions.allow`, Cursor allowlist, Codex `approval_policy`, Aider `--auto-commits`, etc.
- Modes: `default | acceptEdits | auto | dontAsk | bypassPermissions | plan`. Each agent's mode maps to openbuild's via import-time normalization table.
- Config importers (read-only, in `openbuild-config/src/import/`):
  - `claude.rs` — `~/.claude/settings.json`, `.claude.json`, hooks, MCP
  - `cursor.rs` — `~/.cursor/`, `.cursorrules`, modes
  - `codex.rs` — `~/.codex/config.toml`, `AGENTS.md`
  - `opencode.rs` — `~/.config/opencode/`
  - `aider.rs` — `.aider.conf.yml`, `.aiderignore`
  - `cline.rs` — VSCode settings
  - `generic.rs` — `AGENTS.md` at repo root (cross-agent standard)
- Sandbox profile TOML extends builtins (`macos/seatbelt`, `linux/landlock-strict`):
  ```toml
  [profile.read-only]
  extends = "macos/seatbelt"
  read_only_paths = ["${workspace}"]
  deny_paths = ["${workspace}/.env*"]
  restrict_network = true
  ```
- macOS: write SBPL file, exec child under `sandbox-exec -f`.
- Linux: `landlock` for FS + `seccomp` for net (`socket(AF_INET*)` → `EACCES`).
- Secret regex pre-redact on tool output: `sk-[A-Za-z0-9_-]{20,}`, `AKIA[0-9A-Z]{16}`, `(?i)Bearer\s+[A-Za-z0-9._\-]{16,}`, configurable extras via TOML.

## Subagent model

Roles + personas + agents decompose:
- Parent spawns child with `capability_mode = "read-only"` for exploration.
- `default_fork_context = true` copies parent transcript into child (token-budget check first — abort if source > 80% of target ctx window).
- Children recursively spawn but inherit capability ceiling — never escalate above parent.
- `--best-of-n N` = N parallel child sessions, judge picks winner (headless only).

## Out of scope

- Any vendor-proprietary chat proxy.
- `--ssh` clipboard relay — niche.
- OAuth flow — API-key + env-var only.
- Telemetry/trace upload — local-only logs.
- Leader/follower IPC for shared backend — defer to v0.2.

## Differentiation

1. **Multi-provider native, not bolted on.** Each provider in its own crate, equal first-class status — no default-vendor hardcode.
2. **CC0 default agent bundle** — forkable, no derivative-prompt risk.
3. **Zero phone-home.** No proxy, no telemetry, no auto-update call.
4. **Reads every agent's config.** Claude Code, Cursor, Codex, OpenCode, Aider, Cline, generic `AGENTS.md`. One shell, every prior setup.
5. **Smaller binary.** Tree-sitter grammars load on demand, not statically linked.
6. **No vendor surface.** Zero brand names in CLI text, config keys, default prompts, error messages.

## Implementation order (4 milestones)

| MS | Scope | Done = |
|---|---|---|
| M0 | core loop + openai-compat provider + 5 builtin tools (read/write/edit/bash/grep) + jsonl session | `openbuild -p "..."` returns streamed response |
| M1 | provider matrix (Anthropic + xAI + Ollama + OpenRouter) + reasoning_effort routing + streaming JSON output | `--model claude-sonnet-4-6` + `--model grok-4` + `--model llama3` all work |
| M2 | TUI + permission engine + Claude config import + MCP stdio client | feature-parity interactive UX |
| M3 | sandbox (macOS+Linux) + subagent + skills loader + worktree + best-of-N | feature-complete v0.1 |

## Locked decisions

1. **License**: Apache-2.0 (matches iii family).
2. **Tool names**: snake_case primary (`read_file`, `write_file`, `edit_file`, `run_terminal_cmd`, `grep`, `glob`, `list_dir`, `web_search`, `web_fetch`, `task`). Aliases at import boundary, not main:
   - Claude: `Read`→`read_file`, `Edit`→`edit_file`, `Bash`→`run_terminal_cmd`
   - Cursor: same as Claude, plus `Codebase Search`→`grep`
   - Codex/OpenCode: already snake_case, identity map
   - Aider: `/add`→`read_file`, `/run`→`run_terminal_cmd`
3. **Default agent set**: design fresh (CC0 originals). Three roles to start: `explore` (read-only), `plan` (read-only architect), `general` (full-caps). No vendor names anywhere in default bundle.
4. **Repo**: `rohitg00/openbuild`, standalone. Model-agnostic, agent-agnostic, vendor-agnostic.

## Local references

- Local recon dir: `~/openbuild/`
- Models discovery cache format: see `~/.grok/models_cache.json` (TOML/JSON keys borrowed: `base_url`, `api_backend`, `auth_scheme`, `context_window`, `reasoning_effort`, `supports_reasoning_effort`, `stream_tool_calls`).
