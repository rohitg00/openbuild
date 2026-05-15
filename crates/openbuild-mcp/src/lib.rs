use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{oneshot, Mutex};

pub const PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "empty_schema")]
    pub input_schema: serde_json::Value,
}

fn empty_schema() -> serde_json::Value {
    serde_json::json!({"type": "object", "properties": {}})
}

#[derive(Debug, Serialize)]
struct Request<'a> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct Notification<'a> {
    jsonrpc: &'static str,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct Response {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

pub struct StdioClient {
    next_id: AtomicU64,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<BTreeMap<u64, oneshot::Sender<Result<serde_json::Value>>>>>,
    _child: Child,
}

impl StdioClient {
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<Arc<Self>> {
        let mut child = tokio::process::Command::new(command)
            .args(args)
            .envs(env.iter())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawn mcp server: {command}"))?;
        let stdin = child.stdin.take().context("mcp stdin missing")?;
        let stdout = child.stdout.take().context("mcp stdout missing")?;

        let pending: Arc<Mutex<BTreeMap<u64, oneshot::Sender<Result<serde_json::Value>>>>> =
            Arc::new(Mutex::new(BTreeMap::new()));
        let client = Arc::new(Self {
            next_id: AtomicU64::new(1),
            stdin: Arc::new(Mutex::new(stdin)),
            pending: pending.clone(),
            _child: child,
        });

        tokio::spawn(reader_loop(stdout, pending));
        client.initialize().await?;
        Ok(client)
    }

    async fn initialize(&self) -> Result<()> {
        let params = serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "clientInfo": {"name": "openbuild", "version": env!("CARGO_PKG_VERSION")},
        });
        let _ = self.call("initialize", Some(params)).await?;
        self.notify("notifications/initialized", None).await?;
        Ok(())
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        let v = self.call("tools/list", None).await?;
        let tools = v
            .get("tools")
            .ok_or_else(|| anyhow!("tools/list missing tools"))?
            .clone();
        Ok(serde_json::from_value(tools)?)
    }

    pub async fn call_tool(&self, name: &str, args: serde_json::Value) -> Result<String> {
        let params = serde_json::json!({"name": name, "arguments": args});
        let v = self.call("tools/call", Some(params)).await?;
        if let Some(content) = v.get("content").and_then(|c| c.as_array()) {
            let mut out = String::new();
            for item in content {
                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                    out.push_str(t);
                }
            }
            return Ok(out);
        }
        Ok(v.to_string())
    }

    async fn call(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let req = Request {
            jsonrpc: "2.0",
            id,
            method,
            params,
        };
        let line = serde_json::to_string(&req)? + "\n";
        self.stdin.lock().await.write_all(line.as_bytes()).await?;
        self.stdin.lock().await.flush().await?;
        rx.await.context("mcp response channel closed")?
    }

    async fn notify(&self, method: &str, params: Option<serde_json::Value>) -> Result<()> {
        let n = Notification {
            jsonrpc: "2.0",
            method,
            params,
        };
        let line = serde_json::to_string(&n)? + "\n";
        self.stdin.lock().await.write_all(line.as_bytes()).await?;
        self.stdin.lock().await.flush().await?;
        Ok(())
    }
}

async fn reader_loop(
    stdout: ChildStdout,
    pending: Arc<Mutex<BTreeMap<u64, oneshot::Sender<Result<serde_json::Value>>>>>,
) {
    let mut reader = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        let resp: Response = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let Some(id) = resp.id else { continue };
        let tx = pending.lock().await.remove(&id);
        if let Some(tx) = tx {
            let outcome = match (resp.result, resp.error) {
                (_, Some(e)) => Err(anyhow!("mcp error {}: {}", e.code, e.message)),
                (Some(v), None) => Ok(v),
                (None, None) => Err(anyhow!("mcp response missing result and error")),
            };
            let _ = tx.send(outcome);
        }
    }
}
