use crate::Tool;
use async_trait::async_trait;
use openbuild_core::tool::ToolSchema;
use serde::Deserialize;

pub struct WriteFile;

#[derive(Deserialize)]
struct Input {
    path: String,
    content: String,
    #[serde(default)]
    create_dirs: bool,
}

#[async_trait]
impl Tool for WriteFile {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "write_file".into(),
            description: "Write content to a file, overwriting if it exists.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"},
                    "create_dirs": {"type": "boolean"}
                },
                "required": ["path", "content"]
            }),
        }
    }

    fn is_write(&self) -> bool {
        true
    }

    async fn run(&self, input: serde_json::Value) -> Result<String, String> {
        let i: Input = serde_json::from_value(input).map_err(|e| e.to_string())?;
        if i.create_dirs {
            if let Some(parent) = std::path::Path::new(&i.path).parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
        tokio::fs::write(&i.path, &i.content)
            .await
            .map_err(|e| e.to_string())?;
        Ok(format!("wrote {} bytes to {}", i.content.len(), i.path))
    }
}
