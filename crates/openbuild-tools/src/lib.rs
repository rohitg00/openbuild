use async_trait::async_trait;
use openbuild_core::tool::ToolSchema;

#[async_trait]
pub trait Tool: Send + Sync {
    fn schema(&self) -> ToolSchema;
    async fn run(&self, input: serde_json::Value) -> Result<String, String>;
    fn is_write(&self) -> bool {
        false
    }
}

pub mod edit_file;
pub mod glob;
pub mod grep;
pub mod list_dir;
pub mod read_file;
pub mod run_terminal_cmd;
pub mod task;
pub mod write_file;

pub fn default_tools() -> Vec<Box<dyn Tool>> {
    default_tools_with_sandbox(None)
}

pub fn default_tools_with_sandbox(
    profile: Option<openbuild_sandbox::Profile>,
) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(read_file::ReadFile),
        Box::new(write_file::WriteFile),
        Box::new(edit_file::EditFile),
        Box::new(list_dir::ListDir),
        Box::new(glob::Glob),
        Box::new(grep::Grep),
        Box::new(run_terminal_cmd::RunTerminalCmd {
            sandbox_profile: profile,
        }),
    ]
}
