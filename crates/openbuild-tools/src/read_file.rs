use crate::Tool;
use async_trait::async_trait;
use openbuild_core::tool::ToolSchema;
use serde::Deserialize;

pub struct ReadFile;

#[derive(Deserialize)]
struct Input {
    path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl Tool for ReadFile {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "read_file".into(),
            description: "Read a file from the workspace. Optional offset (lines) and limit (lines).".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "offset": {"type": "integer", "minimum": 0},
                    "limit": {"type": "integer", "minimum": 1}
                },
                "required": ["path"]
            }),
        }
    }

    async fn run(&self, input: serde_json::Value) -> Result<String, String> {
        let i: Input = serde_json::from_value(input).map_err(|e| e.to_string())?;
        let bytes = tokio::fs::read(&i.path).await.map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&bytes);
        if i.offset.is_some() || i.limit.is_some() {
            let off = i.offset.unwrap_or(0);
            let lim = i.limit.unwrap_or(2000);
            let lines: Vec<&str> = text.lines().skip(off).take(lim).collect();
            return Ok(lines.join("\n"));
        }
        Ok(text.into_owned())
    }
}
