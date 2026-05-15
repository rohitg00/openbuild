use crate::Tool;
use async_trait::async_trait;
use openbuild_core::tool::ToolSchema;
use regex::Regex;
use serde::Deserialize;

pub struct Grep;

#[derive(Deserialize)]
struct Input {
    pattern: String,
    #[serde(default = "default_root")]
    root: String,
    #[serde(default)]
    case_insensitive: bool,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    include: Option<String>,
}

fn default_root() -> String {
    ".".into()
}
fn default_limit() -> usize {
    200
}

#[async_trait]
impl Tool for Grep {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "grep".into(),
            description: "Search files for a regex. Returns file:line:text matches.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "root": {"type": "string"},
                    "case_insensitive": {"type": "boolean"},
                    "limit": {"type": "integer", "minimum": 1},
                    "include": {"type": "string", "description": "glob filter for files"}
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn run(&self, input: serde_json::Value) -> Result<String, String> {
        let i: Input = serde_json::from_value(input).map_err(|e| e.to_string())?;
        let re = if i.case_insensitive {
            Regex::new(&format!("(?i){}", i.pattern))
        } else {
            Regex::new(&i.pattern)
        }
        .map_err(|e| e.to_string())?;
        let include = i
            .include
            .as_deref()
            .map(|p| globset::Glob::new(p).map(|g| g.compile_matcher()))
            .transpose()
            .map_err(|e| e.to_string())?;

        let walker = ignore::WalkBuilder::new(&i.root).hidden(false).build();
        let mut out = String::new();
        let mut count = 0;
        for entry in walker.flatten() {
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.path();
            if let Some(g) = &include {
                if !g.is_match(path) {
                    continue;
                }
            }
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            for (n, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    out.push_str(&format!("{}:{}:{}\n", path.display(), n + 1, line));
                    count += 1;
                    if count >= i.limit {
                        out.push_str(&format!("[truncated at {} matches]\n", i.limit));
                        return Ok(out);
                    }
                }
            }
        }
        Ok(out)
    }
}
