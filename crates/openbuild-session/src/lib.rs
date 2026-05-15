use anyhow::{Context, Result};
use openbuild_core::Event;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use uuid::Uuid;

pub fn sessions_dir() -> Result<PathBuf> {
    let dir = dirs::home_dir()
        .context("home dir not found")?
        .join(".openbuild")
        .join("sessions");
    Ok(dir)
}

pub fn most_recent() -> Result<Option<PathBuf>> {
    let dir = sessions_dir()?;
    if !dir.exists() {
        return Ok(None);
    }
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(&dir)?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let mt = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let replace = match &best {
            None => true,
            Some((b, _)) => mt > *b,
        };
        if replace {
            best = Some((mt, path));
        }
    }
    Ok(best.map(|(_, p)| p))
}

pub fn find_by_id(id: &str) -> Result<Option<PathBuf>> {
    let dir = sessions_dir()?;
    if !dir.exists() {
        return Ok(None);
    }
    for entry in std::fs::read_dir(&dir)?.flatten() {
        let path = entry.path();
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            if stem == id || stem.starts_with(id) {
                return Ok(Some(path));
            }
        }
    }
    Ok(None)
}

pub struct Session {
    id: Uuid,
    path: PathBuf,
    file: File,
}

impl Session {
    pub fn create() -> Result<Self> {
        let id = Uuid::new_v4();
        let dir = dirs::home_dir()
            .context("home dir not found")?
            .join(".openbuild")
            .join("sessions");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{id}.jsonl"));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open session log {}", path.display()))?;
        let mut s = Self { id, path, file };
        s.append_header()?;
        Ok(s)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let id = Uuid::new_v4();
        Ok(Self { id, path, file })
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn append_header(&mut self) -> Result<()> {
        let git_head = std::process::Command::new("git")
            .arg("rev-parse")
            .arg("HEAD")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        let header = serde_json::json!({
            "type": "session_start",
            "id": self.id.to_string(),
            "started_at": chrono::Utc::now().to_rfc3339(),
            "version": env!("CARGO_PKG_VERSION"),
            "cwd": std::env::current_dir().ok().map(|p| p.display().to_string()),
            "git_head": git_head,
        });
        writeln!(self.file, "{header}")?;
        Ok(())
    }

    pub fn append_event(&mut self, ev: &Event) -> Result<()> {
        let line = serde_json::to_string(ev)?;
        writeln!(self.file, "{line}")?;
        Ok(())
    }

    pub fn append_user_message(&mut self, text: &str) -> Result<()> {
        let line = serde_json::json!({"type": "user_message", "text": text});
        writeln!(self.file, "{line}")?;
        Ok(())
    }
}
