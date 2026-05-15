use anyhow::Result;
use clap::Parser;
use futures::StreamExt;
use openbuild_core::{
    message::Message,
    provider::Provider,
    request::Request,
    event::Event,
};
use openbuild_providers::openai::OpenAi;
use std::io::Write;

#[derive(Parser, Debug)]
#[command(name = "openbuild", version, about = "Model-agnostic agent shell")]
struct Cli {
    #[arg(short = 'p', long = "single")]
    prompt: Option<String>,

    #[arg(short = 'm', long, default_value = "gpt-4o-mini")]
    model: String,

    #[arg(long, env = "OPENBUILD_BASE_URL", default_value = "https://api.openai.com/v1")]
    base_url: String,

    #[arg(long, env = "OPENBUILD_API_KEY")]
    api_key: Option<String>,

    #[arg(long, default_value = "plain", value_parser = ["plain", "json", "streaming-json"])]
    output_format: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let prompt = match cli.prompt {
        Some(p) => p,
        None => {
            anyhow::bail!("interactive mode not yet implemented; pass -p \"...\"");
        }
    };

    let api_key = cli.api_key.unwrap_or_default();
    let provider = OpenAi::new(cli.model.clone(), cli.base_url, api_key);

    let req = Request {
        model: cli.model,
        system: vec![],
        messages: vec![Message::user_text(prompt)],
        tools: vec![],
        reasoning_effort: None,
        max_tokens: None,
        stream: true,
    };

    let mut stream = provider.complete(req).await?;
    let mut stdout = std::io::stdout().lock();
    while let Some(ev) = stream.next().await {
        match ev? {
            Event::TextDelta { text } => {
                match cli.output_format.as_str() {
                    "json" | "streaming-json" => {
                        let line = serde_json::json!({"type": "text_delta", "text": text});
                        writeln!(stdout, "{line}")?;
                    }
                    _ => {
                        write!(stdout, "{text}")?;
                        stdout.flush()?;
                    }
                }
            }
            Event::ThinkingDelta { text } if cli.output_format != "plain" => {
                let line = serde_json::json!({"type": "thinking_delta", "text": text});
                writeln!(stdout, "{line}")?;
            }
            Event::Done(reason) => {
                if cli.output_format == "plain" {
                    writeln!(stdout)?;
                } else {
                    let line = serde_json::json!({"type": "done", "reason": reason});
                    writeln!(stdout, "{line}")?;
                }
                break;
            }
            Event::Error(e) => anyhow::bail!("{e}"),
            _ => {}
        }
    }
    Ok(())
}
