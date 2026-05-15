use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Store {
    pub entries: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub value: String,
    pub created_at: String,
    pub updated_at: String,
}

pub fn path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("home dir not found")?
        .join(".openbuild")
        .join("memory.json"))
}

pub fn load() -> Result<Store> {
    let p = path()?;
    if !p.exists() {
        return Ok(Store::default());
    }
    let text = std::fs::read_to_string(&p)?;
    Ok(serde_json::from_str(&text).unwrap_or_default())
}

pub fn save(store: &Store) -> Result<()> {
    let p = path()?;
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(store)?;
    std::fs::write(&p, text)?;
    Ok(())
}

pub fn set(key: &str, value: &str) -> Result<()> {
    let mut store = load()?;
    let now = chrono::Utc::now().to_rfc3339();
    store
        .entries
        .entry(key.to_string())
        .and_modify(|e| {
            e.value = value.to_string();
            e.updated_at = now.clone();
        })
        .or_insert(Entry {
            value: value.to_string(),
            created_at: now.clone(),
            updated_at: now,
        });
    save(&store)
}

pub fn get(key: &str) -> Result<Option<String>> {
    Ok(load()?.entries.get(key).map(|e| e.value.clone()))
}

pub fn remove(key: &str) -> Result<bool> {
    let mut store = load()?;
    let existed = store.entries.remove(key).is_some();
    save(&store)?;
    Ok(existed)
}

pub fn render_for_system_prompt() -> Result<String> {
    let store = load()?;
    if store.entries.is_empty() {
        return Ok(String::new());
    }
    let mut out = String::from("# Cross-session memory\n");
    for (k, e) in &store.entries {
        out.push_str(&format!("- {k}: {}\n", e.value));
    }
    Ok(out)
}
