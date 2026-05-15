use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    #[serde(default)]
    pub read_only_paths: Vec<PathBuf>,
    #[serde(default)]
    pub deny_paths: Vec<PathBuf>,
    #[serde(default)]
    pub restrict_network: bool,
}

impl Profile {
    pub fn read_only(workspace: PathBuf) -> Self {
        Self {
            name: "read-only".into(),
            read_only_paths: vec![workspace],
            deny_paths: Vec::new(),
            restrict_network: true,
        }
    }

    pub fn workspace_write(workspace: PathBuf) -> Self {
        Self {
            name: "workspace-write".into(),
            read_only_paths: Vec::new(),
            deny_paths: vec![],
            restrict_network: true,
        }
        .with_writable(workspace)
    }

    fn with_writable(self, _: PathBuf) -> Self {
        self
    }
}

#[cfg(target_os = "macos")]
pub fn sbpl(profile: &Profile) -> String {
    let mut sb = String::new();
    sb.push_str("(version 1)\n(deny default)\n");
    sb.push_str("(allow process-exec)\n(allow process-fork)\n");
    sb.push_str("(allow file-read*)\n");
    sb.push_str("(allow file-write* (subpath \"/tmp\"))\n");
    sb.push_str("(allow file-write* (subpath \"/var/folders\"))\n");
    sb.push_str("(allow file-write* (subpath \"/private/tmp\"))\n");
    sb.push_str("(allow file-write* (subpath \"/private/var\"))\n");
    sb.push_str("(allow sysctl-read)\n");
    sb.push_str("(allow ipc-posix-shm)\n");
    sb.push_str("(allow mach-lookup)\n");
    sb.push_str("(allow signal (target self))\n");
    for ro in &profile.read_only_paths {
        sb.push_str(&format!(
            "(deny file-write* (subpath \"{}\"))\n",
            ro.display()
        ));
    }
    for deny in &profile.deny_paths {
        sb.push_str(&format!("(deny file* (subpath \"{}\"))\n", deny.display()));
    }
    if profile.restrict_network {
        sb.push_str("(deny network*)\n(allow network* (remote ip \"localhost:*\"))\n");
    } else {
        sb.push_str("(allow network*)\n");
    }
    sb
}

#[cfg(target_os = "macos")]
pub fn wrap_command(profile: &Profile, mut cmd: Vec<String>) -> Result<Vec<String>> {
    let sbpl_text = sbpl(profile);
    let tmp = std::env::temp_dir().join(format!("openbuild-sandbox-{}.sb", std::process::id()));
    std::fs::write(&tmp, sbpl_text).with_context(|| format!("write {}", tmp.display()))?;
    let mut wrapped = vec![
        "sandbox-exec".into(),
        "-f".into(),
        tmp.to_string_lossy().into_owned(),
    ];
    wrapped.append(&mut cmd);
    Ok(wrapped)
}

#[cfg(target_os = "linux")]
pub fn wrap_command(profile: &Profile, mut cmd: Vec<String>) -> Result<Vec<String>> {
    if which::which("bwrap").is_ok() {
        let mut wrapped = vec![
            "bwrap".to_string(),
            "--ro-bind".into(),
            "/".into(),
            "/".into(),
            "--proc".into(),
            "/proc".into(),
            "--dev".into(),
            "/dev".into(),
            "--tmpfs".into(),
            "/tmp".into(),
        ];
        if profile.restrict_network {
            wrapped.push("--unshare-net".into());
        }
        for ro in &profile.read_only_paths {
            wrapped.push("--ro-bind".into());
            wrapped.push(ro.display().to_string());
            wrapped.push(ro.display().to_string());
        }
        wrapped.push("--".into());
        wrapped.append(&mut cmd);
        return Ok(wrapped);
    }
    eprintln!("[sandbox] bwrap not found, running without isolation");
    Ok(cmd)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn wrap_command(_profile: &Profile, cmd: Vec<String>) -> Result<Vec<String>> {
    Ok(cmd)
}

pub fn discover_profile(name: &str, cwd: &Path) -> Profile {
    if let Some(p) = load_user_profile(name) {
        return p;
    }
    match name {
        "off" => Profile::default(),
        "read-only" => Profile::read_only(cwd.to_path_buf()),
        _ => Profile::workspace_write(cwd.to_path_buf()),
    }
}

fn load_user_profile(name: &str) -> Option<Profile> {
    let home = dirs::home_dir()?;
    let path = home
        .join(".openbuild")
        .join("sandbox")
        .join(format!("{name}.toml"));
    let text = std::fs::read_to_string(&path).ok()?;
    let mut profile: Profile = toml::from_str(&text).ok()?;
    if profile.name.is_empty() {
        profile.name = name.to_string();
    }
    Some(profile)
}
