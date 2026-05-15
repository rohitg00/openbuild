use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Frontmatter {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub prompt_mode: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub capability_mode: Option<String>,
    #[serde(default)]
    pub agents_md: Option<bool>,
    #[serde(default)]
    pub default_fork_context: Option<bool>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Agent {
    pub frontmatter: Frontmatter,
    pub system_prompt: String,
    pub path: PathBuf,
    pub source: Source,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Builtin,
    User,
    Project,
}

pub fn load_by_name(name: &str, bundled_dir: Option<&Path>) -> Result<Agent> {
    if let Ok(cwd) = std::env::current_dir() {
        let p = cwd
            .join(".openbuild")
            .join("agents")
            .join(format!("{name}.md"));
        if p.exists() {
            return load(&p, Source::Project);
        }
    }
    if let Some(home) = dirs::home_dir() {
        let p = home
            .join(".openbuild")
            .join("agents")
            .join(format!("{name}.md"));
        if p.exists() {
            return load(&p, Source::User);
        }
    }
    if let Some(dir) = bundled_dir {
        let p = dir.join(format!("{name}.md"));
        if p.exists() {
            return load(&p, Source::Builtin);
        }
    }
    anyhow::bail!("agent not found: {name}")
}

pub fn load(path: &Path, source: Source) -> Result<Agent> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("read agent {}", path.display()))?;
    let (front, body) = split_frontmatter(&text);
    let frontmatter: Frontmatter = if front.is_empty() {
        Frontmatter::default()
    } else {
        serde_yaml::from_str(front).unwrap_or_default()
    };
    let mut fm = frontmatter;
    if fm.name.is_empty() {
        fm.name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string();
    }
    Ok(Agent {
        frontmatter: fm,
        system_prompt: body.trim().to_string(),
        path: path.to_path_buf(),
        source,
    })
}

pub fn discover_all(bundled_dir: Option<&Path>) -> Vec<Agent> {
    let mut out = Vec::new();
    if let Some(dir) = bundled_dir {
        out.extend(scan(dir, Source::Builtin));
    }
    if let Some(home) = dirs::home_dir() {
        out.extend(scan(&home.join(".openbuild").join("agents"), Source::User));
    }
    if let Ok(cwd) = std::env::current_dir() {
        out.extend(scan(
            &cwd.join(".openbuild").join("agents"),
            Source::Project,
        ));
    }
    out
}

fn scan(dir: &Path, source: Source) -> Vec<Agent> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        if let Ok(a) = load(&p, source) {
            out.push(a);
        }
    }
    out
}

fn split_frontmatter(text: &str) -> (&str, &str) {
    if let Some(rest) = text.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            let body_start = end + 4;
            let body = if rest.as_bytes().get(body_start) == Some(&b'\n') {
                &rest[body_start + 1..]
            } else {
                &rest[body_start..]
            };
            return (&rest[..end], body);
        }
    }
    ("", text)
}

pub fn render_prompt(prompt: &str, tool_names: &[String]) -> String {
    let mut out = prompt.to_string();
    let by_kind_keys = [
        "read", "list", "search", "execute", "write", "edit", "spawn", "web",
    ];
    for key in by_kind_keys {
        let needle = format!("${{{{ tools.by_kind.{key} }}}}");
        let candidates = match key {
            "read" => vec!["read_file"],
            "list" => vec!["list_dir", "glob"],
            "search" => vec!["grep", "glob"],
            "execute" => vec!["run_terminal_cmd"],
            "write" => vec!["write_file"],
            "edit" => vec!["edit_file"],
            "spawn" => vec!["task"],
            "web" => vec!["web_search", "web_fetch"],
            _ => vec![],
        };
        let names: Vec<&str> = candidates
            .into_iter()
            .filter(|c| tool_names.iter().any(|t| t == c))
            .collect();
        let replacement = if names.is_empty() {
            "(none)".to_string()
        } else {
            names.join(", ")
        };
        out = out.replace(&needle, &replacement);
    }
    out
}
