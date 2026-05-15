use crate::Tool;
use async_trait::async_trait;
use openbuild_core::tool::ToolSchema;
use serde::Deserialize;

pub struct MultiEdit;

#[derive(Deserialize)]
struct Input {
    path: String,
    edits: Vec<EditOp>,
}

#[derive(Deserialize)]
struct EditOp {
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

#[async_trait]
impl Tool for MultiEdit {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "multi_edit".into(),
            description: "Apply multiple sequential edits to one file atomically. Each edit operates on the result of the previous. Aborts the whole batch on first failure.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "edits": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "old_string": {"type": "string"},
                                "new_string": {"type": "string"},
                                "replace_all": {"type": "boolean"}
                            },
                            "required": ["old_string", "new_string"]
                        },
                        "minItems": 1
                    }
                },
                "required": ["path", "edits"]
            }),
        }
    }

    fn is_write(&self) -> bool {
        true
    }

    async fn run(&self, input: serde_json::Value) -> Result<String, String> {
        let i: Input = serde_json::from_value(input).map_err(|e| e.to_string())?;
        let mut text = tokio::fs::read_to_string(&i.path)
            .await
            .map_err(|e| e.to_string())?;
        let mut total = 0usize;
        for (idx, op) in i.edits.iter().enumerate() {
            let count = text.matches(&op.old_string).count();
            if count == 0 {
                return Err(format!("edit {idx}: old_string not found in {}", i.path));
            }
            if !op.replace_all && count > 1 {
                return Err(format!(
                    "edit {idx}: old_string appears {count} times in {}; set replace_all=true or add context",
                    i.path
                ));
            }
            text = if op.replace_all {
                text.replace(&op.old_string, &op.new_string)
            } else {
                text.replacen(&op.old_string, &op.new_string, 1)
            };
            total += count;
        }
        tokio::fs::write(&i.path, &text)
            .await
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "applied {} edit(s) totaling {total} replacement(s) to {}",
            i.edits.len(),
            i.path
        ))
    }
}
