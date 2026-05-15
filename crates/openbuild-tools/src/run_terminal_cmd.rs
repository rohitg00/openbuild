use crate::Tool;
use async_trait::async_trait;
use openbuild_core::tool::ToolSchema;
use serde::Deserialize;
use tokio::process::Command;

#[derive(Default)]
pub struct RunTerminalCmd {
    pub sandbox_profile: Option<openbuild_sandbox::Profile>,
}

#[derive(Deserialize)]
struct Input {
    command: String,
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
}

fn default_timeout() -> u64 {
    120_000
}

#[async_trait]
impl Tool for RunTerminalCmd {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "run_terminal_cmd".into(),
            description: "Execute a shell command with timeout. Returns stdout+stderr.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "timeout_ms": {"type": "integer", "minimum": 1, "maximum": 600000}
                },
                "required": ["command"]
            }),
        }
    }

    async fn run(&self, input: serde_json::Value) -> Result<String, String> {
        let i: Input = serde_json::from_value(input).map_err(|e| e.to_string())?;
        let dur = std::time::Duration::from_millis(i.timeout_ms);
        let argv: Vec<String> = vec!["sh".into(), "-c".into(), i.command.clone()];
        let argv = if let Some(p) = &self.sandbox_profile {
            openbuild_sandbox::wrap_command(p, argv).map_err(|e| e.to_string())?
        } else {
            argv
        };
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        let fut = cmd.output();
        let out = tokio::time::timeout(dur, fut)
            .await
            .map_err(|_| "command timed out".to_string())?
            .map_err(|e| e.to_string())?;
        let mut s = String::new();
        s.push_str(&String::from_utf8_lossy(&out.stdout));
        if !out.stderr.is_empty() {
            s.push_str("\n[stderr]\n");
            s.push_str(&String::from_utf8_lossy(&out.stderr));
        }
        Ok(s)
    }
}
