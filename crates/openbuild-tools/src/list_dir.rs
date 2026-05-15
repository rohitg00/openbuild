use crate::Tool;
use async_trait::async_trait;
use openbuild_core::tool::ToolSchema;
use serde::Deserialize;

pub struct ListDir;

#[derive(Deserialize)]
struct Input {
    path: String,
    #[serde(default)]
    hidden: bool,
}

#[async_trait]
impl Tool for ListDir {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "list_dir".into(),
            description: "List entries in a directory. Set hidden=true to include dotfiles.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "hidden": {"type": "boolean"}
                },
                "required": ["path"]
            }),
        }
    }

    async fn run(&self, input: serde_json::Value) -> Result<String, String> {
        let i: Input = serde_json::from_value(input).map_err(|e| e.to_string())?;
        let mut rd = tokio::fs::read_dir(&i.path)
            .await
            .map_err(|e| e.to_string())?;
        let mut out = String::new();
        while let Some(entry) = rd.next_entry().await.map_err(|e| e.to_string())? {
            let name = entry.file_name();
            let n = name.to_string_lossy();
            if !i.hidden && n.starts_with('.') {
                continue;
            }
            let kind = match entry.file_type().await {
                Ok(t) if t.is_dir() => "d",
                Ok(t) if t.is_symlink() => "l",
                _ => "f",
            };
            out.push_str(kind);
            out.push(' ');
            out.push_str(&n);
            out.push('\n');
        }
        Ok(out)
    }
}
