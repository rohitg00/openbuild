use anyhow::{Context, Result};
use openbuild_core::Event;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

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
        let header = serde_json::json!({
            "type": "session_start",
            "id": self.id.to_string(),
            "started_at": chrono::Utc::now().to_rfc3339(),
            "version": env!("CARGO_PKG_VERSION"),
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
