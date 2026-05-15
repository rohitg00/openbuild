// Universal config importers — one per agent.
// All read-only, all idempotent, all produce a canonical openbuild::Config.
//
// - claude.rs     ~/.claude/settings*.json, .claude.json, hooks/, MCP
// - cursor.rs     ~/.cursor/, .cursorrules, modes
// - codex.rs      ~/.codex/config.toml, AGENTS.md
// - opencode.rs   ~/.config/opencode/
// - aider.rs      .aider.conf.yml, .aiderignore
// - cline.rs      VSCode settings
// - generic.rs    AGENTS.md at repo root (vendor-neutral standard)
