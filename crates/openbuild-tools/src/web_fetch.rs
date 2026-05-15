use crate::Tool;
use async_trait::async_trait;
use openbuild_core::tool::ToolSchema;
use serde::Deserialize;

pub struct WebFetch {
    pub disabled: bool,
}

#[derive(Deserialize)]
struct Input {
    url: String,
    #[serde(default = "default_limit")]
    max_bytes: usize,
}

fn default_limit() -> usize {
    200_000
}

#[async_trait]
impl Tool for WebFetch {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "web_fetch".into(),
            description: "Fetch a URL and return the response body. Strips HTML tags. Truncates at max_bytes."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string"},
                    "max_bytes": {"type": "integer", "minimum": 1024}
                },
                "required": ["url"]
            }),
        }
    }

    async fn run(&self, input: serde_json::Value) -> Result<String, String> {
        if self.disabled {
            return Err("web_fetch disabled by --disable-web-search".into());
        }
        let i: Input = serde_json::from_value(input).map_err(|e| e.to_string())?;
        let client = reqwest::Client::builder()
            .user_agent("openbuild/0.0.1")
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client.get(&i.url).send().await.map_err(|e| e.to_string())?;
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = resp.text().await.map_err(|e| e.to_string())?;
        let mut text = if ct.contains("text/html") {
            html_to_text(&body)
        } else {
            body
        };
        if text.len() > i.max_bytes {
            text.truncate(i.max_bytes);
            text.push_str("\n[truncated]");
        }
        Ok(text)
    }
}

fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut in_script = false;
    let mut buf = String::new();
    for ch in html.chars() {
        if in_tag {
            buf.push(ch);
            if ch == '>' {
                let lower = buf.to_ascii_lowercase();
                if lower.starts_with("<script") {
                    in_script = true;
                } else if lower.starts_with("</script") {
                    in_script = false;
                }
                buf.clear();
                in_tag = false;
            }
            continue;
        }
        if ch == '<' {
            in_tag = true;
            buf.push(ch);
            continue;
        }
        if !in_script {
            out.push(ch);
        }
    }
    let mut compact = String::with_capacity(out.len());
    let mut last_blank = false;
    for line in out.lines() {
        let t = line.trim();
        if t.is_empty() {
            if !last_blank {
                compact.push('\n');
            }
            last_blank = true;
        } else {
            compact.push_str(t);
            compact.push('\n');
            last_blank = false;
        }
    }
    compact
}
