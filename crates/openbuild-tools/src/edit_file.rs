use crate::Tool;
use async_trait::async_trait;
use openbuild_core::tool::ToolSchema;
use serde::Deserialize;

pub struct EditFile;

#[derive(Deserialize)]
struct Input {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

#[async_trait]
impl Tool for EditFile {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "edit_file".into(),
            description: "Replace exact text in a file. Errors if old_string is not unique unless replace_all=true.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "old_string": {"type": "string"},
                    "new_string": {"type": "string"},
                    "replace_all": {"type": "boolean"}
                },
                "required": ["path", "old_string", "new_string"]
            }),
        }
    }

    fn is_write(&self) -> bool {
        true
    }

    async fn run(&self, input: serde_json::Value) -> Result<String, String> {
        let i: Input = serde_json::from_value(input).map_err(|e| e.to_string())?;
        let text = tokio::fs::read_to_string(&i.path)
            .await
            .map_err(|e| e.to_string())?;
        let count = text.matches(&i.old_string).count();
        if count == 0 {
            return Err(format!("old_string not found in {}", i.path));
        }
        let new_text = if i.replace_all {
            text.replace(&i.old_string, &i.new_string)
        } else {
            if count > 1 {
                return Err(format!(
                    "old_string appears {count} times in {}, use replace_all=true or add context",
                    i.path
                ));
            }
            text.replacen(&i.old_string, &i.new_string, 1)
        };
        tokio::fs::write(&i.path, &new_text)
            .await
            .map_err(|e| e.to_string())?;
        Ok(format!("replaced {count} occurrence(s) in {}", i.path))
    }
}
