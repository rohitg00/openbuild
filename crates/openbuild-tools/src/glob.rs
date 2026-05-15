use crate::Tool;
use async_trait::async_trait;
use openbuild_core::tool::ToolSchema;
use serde::Deserialize;

pub struct Glob;

#[derive(Deserialize)]
struct Input {
    pattern: String,
    #[serde(default = "default_root")]
    root: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_root() -> String {
    ".".into()
}
fn default_limit() -> usize {
    200
}

#[async_trait]
impl Tool for Glob {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "glob".into(),
            description:
                "Find files matching a glob pattern under root. Sorted by modified time desc."
                    .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "root": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1}
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn run(&self, input: serde_json::Value) -> Result<String, String> {
        let i: Input = serde_json::from_value(input).map_err(|e| e.to_string())?;
        let glob = globset::Glob::new(&i.pattern)
            .map_err(|e| e.to_string())?
            .compile_matcher();
        let walker = ignore::WalkBuilder::new(&i.root).hidden(false).build();
        let mut hits: Vec<(std::time::SystemTime, String)> = Vec::new();
        for entry in walker.flatten() {
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.path();
            let rel = path.strip_prefix(&i.root).unwrap_or(path);
            if glob.is_match(rel) || glob.is_match(path) {
                let mtime = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .unwrap_or(std::time::UNIX_EPOCH);
                hits.push((mtime, path.display().to_string()));
            }
        }
        hits.sort_by_key(|h| std::cmp::Reverse(h.0));
        let mut out = String::new();
        for (_, p) in hits.into_iter().take(i.limit) {
            out.push_str(&p);
            out.push('\n');
        }
        Ok(out)
    }
}
