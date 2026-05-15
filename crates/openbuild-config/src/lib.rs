use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub mod import;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub instructions: Vec<Instruction>,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServer>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub provenance: Vec<Source>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instruction {
    pub path: PathBuf,
    pub scope: Scope,
    pub bytes: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Global,
    User,
    Project,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Permissions {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
    #[serde(default)]
    pub default_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpServer {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
    },
    Sse {
        url: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub agent: AgentKind,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Claude,
    Cursor,
    Codex,
    OpenCode,
    Aider,
    Cline,
    Generic,
}

impl Config {
    pub fn merge(&mut self, other: Config) {
        self.instructions.extend(other.instructions);
        self.permissions.allow.extend(other.permissions.allow);
        self.permissions.deny.extend(other.permissions.deny);
        if self.permissions.default_mode.is_none() {
            self.permissions.default_mode = other.permissions.default_mode;
        }
        for (k, v) in other.mcp_servers {
            self.mcp_servers.entry(k).or_insert(v);
        }
        for (k, v) in other.env {
            self.env.entry(k).or_insert(v);
        }
        self.provenance.extend(other.provenance);
    }
}
