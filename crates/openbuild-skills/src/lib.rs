use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub body: String,
    pub source: Source,
}

#[derive(Debug, Clone, Copy)]
pub enum Source {
    Builtin,
    User,
    Project,
}

pub fn discover() -> Result<Vec<Skill>> {
    let mut out = Vec::new();
    if let Some(home) = dirs::home_dir() {
        out.extend(scan_dir(
            &home.join(".openbuild").join("skills"),
            Source::User,
        ));
    }
    if let Ok(cwd) = std::env::current_dir() {
        out.extend(scan_dir(
            &cwd.join(".openbuild").join("skills"),
            Source::Project,
        ));
    }
    Ok(out)
}

fn scan_dir(dir: &Path, source: Source) -> Vec<Skill> {
    let mut skills = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return skills;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let skill_md = p.join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }
        if let Ok(s) = load(&skill_md, source) {
            skills.push(s);
        }
    }
    skills
}

pub fn load(path: &Path, source: Source) -> Result<Skill> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let (front, body) = split_frontmatter(&text);
    let mut name = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed")
        .to_string();
    let mut description = String::new();
    for line in front.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("name:") {
            name = v.trim().trim_matches('"').to_string();
        } else if let Some(v) = line.strip_prefix("description:") {
            description = v.trim().trim_matches('"').to_string();
        }
    }
    Ok(Skill {
        name,
        description,
        path: path.to_path_buf(),
        body: body.to_string(),
        source,
    })
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
