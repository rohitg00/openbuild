use async_trait::async_trait;
use openbuild_core::tool::ToolSchema;

#[async_trait]
pub trait Tool: Send + Sync {
    fn schema(&self) -> ToolSchema;
    async fn run(&self, input: serde_json::Value) -> Result<String, String>;
}

pub mod read_file;
pub mod run_terminal_cmd;
