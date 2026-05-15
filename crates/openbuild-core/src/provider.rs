use crate::event::Event;
use crate::request::Request;
use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Tools,
    Vision,
    Reasoning,
    Streaming,
    PromptCache,
    StructuredOutput,
}

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderError {
    #[error("http error: {0}")]
    Http(String),
    #[error("auth error: {0}")]
    Auth(String),
    #[error("rate limit")]
    RateLimit,
    #[error("context overflow")]
    ContextOverflow,
    #[error("decode error: {0}")]
    Decode(String),
    #[error("upstream error: {0}")]
    Upstream(String),
}

pub type EventStream = BoxStream<'static, Result<Event, ProviderError>>;

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;
    fn supports(&self, cap: Capability) -> bool;
    async fn complete(&self, req: Request) -> Result<EventStream, ProviderError>;
}
