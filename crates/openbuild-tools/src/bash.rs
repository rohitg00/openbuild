use crate::Tool;
use async_trait::async_trait;
use openbuild_core::tool::ToolSchema;

pub struct Bash {
    inner: crate::run_terminal_cmd::RunTerminalCmd,
}

impl Bash {
    pub fn new(sandbox_profile: Option<openbuild_sandbox::Profile>) -> Self {
        Self {
            inner: crate::run_terminal_cmd::RunTerminalCmd { sandbox_profile },
        }
    }
}

#[async_trait]
impl Tool for Bash {
    fn schema(&self) -> ToolSchema {
        let mut s = self.inner.schema();
        s.name = "bash".into();
        s
    }

    async fn run(&self, input: serde_json::Value) -> Result<String, String> {
        self.inner.run(input).await
    }
}
