use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Event {
    PreToolUse,
    PostToolUse,
    Stop,
    SessionStart,
    SessionEnd,
    UserPromptSubmit,
    PreCompact,
    Notification,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hook {
    pub event: Event,
    #[serde(default)]
    pub matcher: Option<String>,
    pub command: String,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub blocking: bool,
}

fn default_timeout() -> u64 {
    5_000
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Registry {
    pub hooks: Vec<Hook>,
}

impl Registry {
    pub fn discover() -> Result<Self> {
        let mut reg = Self::default();
        if let Some(home) = dirs::home_dir() {
            reg.merge(&load_dir(&home.join(".openbuild").join("hooks"))?);
            reg.merge(&load_claude_hooks(&home.join(".claude"))?);
        }
        if let Ok(cwd) = std::env::current_dir() {
            reg.merge(&load_dir(&cwd.join(".openbuild").join("hooks"))?);
            reg.merge(&load_claude_hooks(&cwd.join(".claude"))?);
        }
        Ok(reg)
    }

    pub fn merge(&mut self, other: &Registry) {
        self.hooks.extend(other.hooks.iter().cloned());
    }

    pub fn for_event<'a>(
        &'a self,
        ev: Event,
        matcher_input: &'a str,
    ) -> impl Iterator<Item = &'a Hook> {
        self.hooks.iter().filter(move |h| {
            if h.event != ev {
                return false;
            }
            match &h.matcher {
                None => true,
                Some(pat) => match Regex::new(pat) {
                    Ok(re) => re.is_match(matcher_input),
                    Err(_) => pat == matcher_input,
                },
            }
        })
    }

    pub async fn fire(
        &self,
        ev: Event,
        matcher_input: &str,
        payload: &serde_json::Value,
    ) -> Vec<HookOutcome> {
        let mut out = Vec::new();
        for hook in self.for_event(ev, matcher_input) {
            out.push(run_hook(hook, payload).await);
        }
        out
    }
}

#[derive(Debug, Clone)]
pub struct HookOutcome {
    pub event: Event,
    pub command: String,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub blocked: bool,
}

async fn run_hook(hook: &Hook, payload: &serde_json::Value) -> HookOutcome {
    let payload_str = payload.to_string();
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(&hook.command);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return HookOutcome {
                event: hook.event,
                command: hook.command.clone(),
                exit_code: -1,
                stdout: String::new(),
                stderr: e.to_string(),
                blocked: false,
            };
        }
    };
    let mut child = child;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload_str.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }
    let dur = std::time::Duration::from_millis(hook.timeout_ms);
    let out = match tokio::time::timeout(dur, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return HookOutcome {
                event: hook.event,
                command: hook.command.clone(),
                exit_code: -1,
                stdout: String::new(),
                stderr: e.to_string(),
                blocked: false,
            };
        }
        Err(_) => {
            return HookOutcome {
                event: hook.event,
                command: hook.command.clone(),
                exit_code: -1,
                stdout: String::new(),
                stderr: format!("timeout after {}ms", hook.timeout_ms),
                blocked: hook.blocking,
            };
        }
    };
    let exit = out.status.code().unwrap_or(-1);
    HookOutcome {
        event: hook.event,
        command: hook.command.clone(),
        exit_code: exit,
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        blocked: hook.blocking && exit != 0,
    }
}

fn load_dir(dir: &Path) -> Result<Registry> {
    let mut reg = Registry::default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(reg);
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        if let Ok(file) = serde_json::from_str::<HooksFile>(&text) {
            reg.hooks.extend(file.flatten());
        }
    }
    Ok(reg)
}

fn load_claude_hooks(claude_dir: &Path) -> Result<Registry> {
    let mut reg = Registry::default();
    let settings = claude_dir.join("settings.json");
    if !settings.exists() {
        return Ok(reg);
    }
    let Ok(text) = std::fs::read_to_string(&settings) else {
        return Ok(reg);
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Ok(reg);
    };
    let hooks_v = match v.get("hooks").and_then(|h| h.as_object()) {
        Some(o) => o,
        None => return Ok(reg),
    };
    for (event_name, value) in hooks_v {
        let Some(event) = parse_event(event_name) else {
            continue;
        };
        if let Some(arr) = value.as_array() {
            for group in arr {
                let matcher = group
                    .get("matcher")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string());
                if let Some(handlers) = group.get("hooks").and_then(|h| h.as_array()) {
                    for h in handlers {
                        if let Some(cmd) = h.get("command").and_then(|c| c.as_str()) {
                            let timeout =
                                h.get("timeout").and_then(|t| t.as_u64()).unwrap_or(5_000);
                            reg.hooks.push(Hook {
                                event,
                                matcher: matcher.clone(),
                                command: cmd.to_string(),
                                timeout_ms: timeout,
                                blocking: false,
                            });
                        }
                    }
                }
            }
        }
    }
    Ok(reg)
}

fn parse_event(name: &str) -> Option<Event> {
    match name {
        "PreToolUse" => Some(Event::PreToolUse),
        "PostToolUse" => Some(Event::PostToolUse),
        "Stop" => Some(Event::Stop),
        "SessionStart" => Some(Event::SessionStart),
        "SessionEnd" => Some(Event::SessionEnd),
        "UserPromptSubmit" => Some(Event::UserPromptSubmit),
        "PreCompact" => Some(Event::PreCompact),
        "Notification" => Some(Event::Notification),
        _ => None,
    }
}

#[derive(Debug, Deserialize, Default)]
struct HooksFile {
    #[serde(default)]
    hooks: BTreeMap<String, serde_json::Value>,
}

impl HooksFile {
    fn flatten(&self) -> Vec<Hook> {
        let mut out = Vec::new();
        for (event_name, value) in &self.hooks {
            let Some(event) = parse_event(event_name) else {
                continue;
            };
            let Some(arr) = value.as_array() else {
                continue;
            };
            for group in arr {
                let matcher = group
                    .get("matcher")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string());
                if let Some(handlers) = group.get("hooks").and_then(|h| h.as_array()) {
                    for h in handlers {
                        if let Some(cmd) = h.get("command").and_then(|c| c.as_str()) {
                            out.push(Hook {
                                event,
                                matcher: matcher.clone(),
                                command: cmd.to_string(),
                                timeout_ms: h
                                    .get("timeout")
                                    .and_then(|t| t.as_u64())
                                    .unwrap_or(5_000),
                                blocking: h
                                    .get("blocking")
                                    .and_then(|b| b.as_bool())
                                    .unwrap_or(false),
                            });
                        }
                    }
                }
            }
        }
        out
    }
}

pub fn user_hooks_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".openbuild").join("hooks"))
}
