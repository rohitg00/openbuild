use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

#[cfg(not(target_os = "macos"))]
pub fn wrap_command(_profile: &Profile, cmd: Vec<String>) -> Result<Vec<String>> {
    // Linux landlock+seccomp would go here. Pending v0.2.
    Ok(cmd)
}

pub fn discover_profile(name: &str, cwd: &Path) -> Profile {
    match name {
        "off" => Profile::default(),
        "read-only" => Profile::read_only(cwd.to_path_buf()),
        _ => Profile::workspace_write(cwd.to_path_buf()),
    }
}
