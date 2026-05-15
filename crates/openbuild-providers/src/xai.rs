use crate::openai::OpenAi;
use openbuild_core::provider::{Capability, EventStream, Provider, ProviderError};
use openbuild_core::request::Request;

const DEFAULT_BASE: &str = "https://api.x.ai/v1";

#[derive(Debug, Clone)]
pub struct XAi(OpenAi);

impl XAi {
    pub fn new(
        id: impl Into<String>,
        base_url: Option<String>,
        api_key: impl Into<String>,
    ) -> Self {
        let base = base_url.unwrap_or_else(|| DEFAULT_BASE.into());
        Self(OpenAi::new(id, base, api_key))
    }
}

#[async_trait::async_trait]
impl Provider for XAi {
    fn id(&self) -> &str {
        self.0.id()
    }
    fn supports(&self, cap: Capability) -> bool {
        matches!(
            cap,
            Capability::Streaming | Capability::Tools | Capability::Reasoning
        )
    }
    async fn complete(&self, req: Request) -> Result<EventStream, ProviderError> {
        self.0.complete(req).await
    }
}
