use crate::Tool;
use async_trait::async_trait;
use openbuild_core::tool::ToolSchema;
use serde::Deserialize;

pub struct WebSearch {
    pub disabled: bool,
}

#[derive(Deserialize)]
struct Input {
    query: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    10
}

#[async_trait]
impl Tool for WebSearch {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "web_search".into(),
            description:
                "Search the web (DuckDuckGo HTML). Returns title + URL + snippet for top results."
                    .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 25}
                },
                "required": ["query"]
            }),
        }
    }

    async fn run(&self, input: serde_json::Value) -> Result<String, String> {
        if self.disabled {
            return Err("web_search disabled by --disable-web-search".into());
        }
        let i: Input = serde_json::from_value(input).map_err(|e| e.to_string())?;
        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(&i.query)
        );
        let client = reqwest::Client::builder()
            .user_agent("openbuild/0.0.1")
            .build()
            .map_err(|e| e.to_string())?;
        let html = client
            .get(&url)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .text()
            .await
            .map_err(|e| e.to_string())?;
        let results = parse_ddg(&html, i.limit);
        let mut out = String::new();
        for r in results {
            out.push_str(&format!("{}\n{}\n{}\n\n", r.title, r.url, r.snippet));
        }
        if out.is_empty() {
            return Err("no results parsed; DuckDuckGo HTML format may have changed".into());
        }
        Ok(out)
    }
}

struct Hit {
    title: String,
    url: String,
    snippet: String,
}

fn parse_ddg(html: &str, limit: usize) -> Vec<Hit> {
    let mut out = Vec::new();
    let mut cursor = html;
    while out.len() < limit {
        let Some(title_start) = cursor.find("class=\"result__a\"") else {
            break;
        };
        cursor = &cursor[title_start..];
        let Some(href_start) = cursor.find("href=\"") else {
            break;
        };
        let after_href = &cursor[href_start + 6..];
        let Some(href_end) = after_href.find('"') else {
            break;
        };
        let url = decode_ddg_redirect(&after_href[..href_end]);
        let Some(title_open) = after_href.find('>') else {
            break;
        };
        let after_title = &after_href[title_open + 1..];
        let Some(title_close) = after_title.find("</a>") else {
            break;
        };
        let title = strip_tags(&after_title[..title_close]);
        let mut snippet = String::new();
        if let Some(snip_start) = after_title.find("class=\"result__snippet\"") {
            let after_snip = &after_title[snip_start..];
            if let Some(open) = after_snip.find('>') {
                if let Some(close) = after_snip[open + 1..].find("</a>") {
                    snippet = strip_tags(&after_snip[open + 1..open + 1 + close]);
                }
            }
        }
        out.push(Hit {
            title: title.trim().to_string(),
            url,
            snippet: snippet.trim().to_string(),
        });
        cursor = &after_title[title_close..];
    }
    out
}

fn decode_ddg_redirect(href: &str) -> String {
    let cleaned = href.trim_start_matches("//");
    if let Some(idx) = cleaned.find("uddg=") {
        let rest = &cleaned[idx + 5..];
        let raw = rest.split('&').next().unwrap_or(rest);
        return urlencoding::decode(raw)
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| raw.to_string());
    }
    if cleaned.starts_with("http") {
        cleaned.to_string()
    } else {
        format!("https://{cleaned}")
    }
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}
