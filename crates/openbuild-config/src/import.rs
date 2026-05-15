use crate::{AgentKind, Config, Instruction, McpServer, Scope, Source};
use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub fn discover_all(cwd: &Path) -> Config {
    let mut cfg = Config::default();
    let home = dirs::home_dir();
    if let Some(h) = &home {
        cfg.merge(claude(h, cwd).unwrap_or_default());
        cfg.merge(cursor(h, cwd).unwrap_or_default());
        cfg.merge(codex(h, cwd).unwrap_or_default());
        cfg.merge(opencode(h).unwrap_or_default());
        cfg.merge(cline(h).unwrap_or_default());
    }
    cfg.merge(aider(cwd).unwrap_or_default());
    cfg.merge(generic_agents_md(cwd).unwrap_or_default());
    cfg
}

pub fn claude(home: &Path, cwd: &Path) -> Result<Config> {
    let mut cfg = Config::default();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let user_dir = home.join(".claude");
    let project_dir = cwd.join(".claude");
    let mut dirs = vec![(Scope::User, user_dir.clone())];
    if project_dir != user_dir {
        dirs.push((Scope::Project, project_dir));
    }
    for (scope, dir) in dirs {
        let settings = dir.join("settings.json");
        if !seen.insert(settings.clone()) {
            continue;
        }
        if settings.exists() {
            if let Ok(text) = std::fs::read_to_string(&settings) {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    extract_claude_settings(&v, &mut cfg, scope);
                }
            }
            cfg.provenance.push(Source {
                agent: AgentKind::Claude,
                path: settings,
            });
        }
        let local = dir.join("settings.local.json");
        if local.exists() {
            if let Ok(text) = std::fs::read_to_string(&local) {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    extract_claude_settings(&v, &mut cfg, scope);
                }
            }
        }
        let md = dir.join("CLAUDE.md");
        if md.exists() {
            push_instruction(&mut cfg, &md, scope);
        }
    }
    let root_md = cwd.join("CLAUDE.md");
    if root_md.exists() {
        push_instruction(&mut cfg, &root_md, Scope::Project);
    }
    Ok(cfg)
}

fn extract_claude_settings(v: &Value, cfg: &mut Config, _scope: Scope) {
    if let Some(perms) = v.get("permissions") {
        if let Some(allow) = perms.get("allow").and_then(|a| a.as_array()) {
            cfg.permissions
                .allow
                .extend(allow.iter().filter_map(|s| s.as_str().map(String::from)));
        }
        if let Some(deny) = perms.get("deny").and_then(|d| d.as_array()) {
            cfg.permissions
                .deny
                .extend(deny.iter().filter_map(|s| s.as_str().map(String::from)));
        }
        if let Some(mode) = perms.get("defaultMode").and_then(|m| m.as_str()) {
            cfg.permissions.default_mode = Some(mode.to_string());
        }
    }
    if let Some(env) = v.get("env").and_then(|e| e.as_object()) {
        for (k, val) in env {
            if let Some(s) = val.as_str() {
                cfg.env.insert(k.clone(), s.to_string());
            }
        }
    }
    if let Some(servers) = v.get("mcpServers").and_then(|s| s.as_object()) {
        for (name, def) in servers {
            if let Some(s) = parse_mcp(def) {
                cfg.mcp_servers.insert(name.clone(), s);
            }
        }
    }
}

fn parse_mcp(def: &Value) -> Option<McpServer> {
    let cmd = def.get("command").and_then(|c| c.as_str());
    let url = def.get("url").and_then(|u| u.as_str());
    let kind = def.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match (cmd, url, kind) {
        (Some(c), _, "stdio") | (Some(c), None, _) => {
            let args = def
                .get("args")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let env: BTreeMap<String, String> = def
                .get("env")
                .and_then(|e| e.as_object())
                .map(|e| {
                    e.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            Some(McpServer::Stdio {
                command: c.into(),
                args,
                env,
            })
        }
        (_, Some(u), "sse") => Some(McpServer::Sse { url: u.into() }),
        (_, Some(u), _) => Some(McpServer::Http { url: u.into() }),
        _ => None,
    }
}

pub fn cursor(home: &Path, cwd: &Path) -> Result<Config> {
    let mut cfg = Config::default();
    let dir = home.join(".cursor");
    if dir.exists() {
        cfg.provenance.push(Source {
            agent: AgentKind::Cursor,
            path: dir.clone(),
        });
    }
    let rules = cwd.join(".cursorrules");
    if rules.exists() {
        push_instruction(&mut cfg, &rules, Scope::Project);
        cfg.provenance.push(Source {
            agent: AgentKind::Cursor,
            path: rules,
        });
    }
    let rules_dir = cwd.join(".cursor").join("rules");
    if rules_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&rules_dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) == Some("mdc") {
                    push_instruction(&mut cfg, &p, Scope::Project);
                }
            }
        }
    }
    Ok(cfg)
}

pub fn codex(home: &Path, cwd: &Path) -> Result<Config> {
    let mut cfg = Config::default();
    let toml_path = home.join(".codex").join("config.toml");
    if toml_path.exists() {
        if let Ok(text) = std::fs::read_to_string(&toml_path) {
            if let Ok(v) = toml::from_str::<toml::Value>(&text) {
                if let Some(approval) = v.get("approval_policy").and_then(|a| a.as_str()) {
                    cfg.permissions.default_mode = Some(approval.to_string());
                }
                if let Some(env) = v.get("env").and_then(|e| e.as_table()) {
                    for (k, val) in env {
                        if let Some(s) = val.as_str() {
                            cfg.env.insert(k.clone(), s.to_string());
                        }
                    }
                }
            }
        }
        cfg.provenance.push(Source {
            agent: AgentKind::Codex,
            path: toml_path,
        });
    }
    let agents_md = cwd.join("AGENTS.md");
    if agents_md.exists() {
        push_instruction(&mut cfg, &agents_md, Scope::Project);
    }
    Ok(cfg)
}

pub fn opencode(home: &Path) -> Result<Config> {
    let mut cfg = Config::default();
    let dir = home.join(".config").join("opencode");
    if dir.exists() {
        cfg.provenance.push(Source {
            agent: AgentKind::OpenCode,
            path: dir,
        });
    }
    Ok(cfg)
}

pub fn aider(cwd: &Path) -> Result<Config> {
    let mut cfg = Config::default();
    let conf = cwd.join(".aider.conf.yml");
    if conf.exists() {
        cfg.provenance.push(Source {
            agent: AgentKind::Aider,
            path: conf,
        });
    }
    let ignore = cwd.join(".aiderignore");
    if ignore.exists() {
        cfg.provenance.push(Source {
            agent: AgentKind::Aider,
            path: ignore,
        });
    }
    Ok(cfg)
}

pub fn cline(home: &Path) -> Result<Config> {
    let mut cfg = Config::default();
    let dir = home
        .join("Library")
        .join("Application Support")
        .join("Cline");
    if dir.exists() {
        cfg.provenance.push(Source {
            agent: AgentKind::Cline,
            path: dir,
        });
    }
    Ok(cfg)
}

pub fn generic_agents_md(cwd: &Path) -> Result<Config> {
    let mut cfg = Config::default();
    for name in ["AGENTS.md", "agents.md"] {
        let p = cwd.join(name);
        if p.exists() {
            push_instruction(&mut cfg, &p, Scope::Project);
            cfg.provenance.push(Source {
                agent: AgentKind::Generic,
                path: p,
            });
            break;
        }
    }
    Ok(cfg)
}

fn push_instruction(cfg: &mut Config, path: &Path, scope: Scope) {
    let bytes = std::fs::metadata(path)
        .map(|m| m.len() as usize)
        .unwrap_or(0);
    cfg.instructions.push(Instruction {
        path: PathBuf::from(path),
        scope,
        bytes,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn claude_settings_extracted() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".claude");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"permissions":{"allow":["Bash(ls)"],"deny":["Bash(rm *)"],"defaultMode":"auto"}}"#,
        )
        .unwrap();
        let cfg = claude(tmp.path(), tmp.path()).unwrap();
        assert_eq!(cfg.permissions.allow, vec!["Bash(ls)"]);
        assert_eq!(cfg.permissions.deny, vec!["Bash(rm *)"]);
        assert_eq!(cfg.permissions.default_mode.as_deref(), Some("auto"));
    }

    #[test]
    fn generic_agents_md_loaded() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), "# rules\nbe careful\n").unwrap();
        let cfg = generic_agents_md(tmp.path()).unwrap();
        assert_eq!(cfg.instructions.len(), 1);
        assert_eq!(cfg.instructions[0].scope, Scope::Project);
    }
}
