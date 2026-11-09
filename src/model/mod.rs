//! Asynchronous model providers over a provider-independent context.
mod openai;
mod script;
pub use openai::OpenAiClient;
pub use script::ScriptModel;

use crate::context::{ChatMessage, LLMContext};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GenerateParams {
    pub model: String,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
}
impl Default for GenerateParams {
    fn default() -> Self {
        Self {
            model: "script".into(),
            temperature: None,
            max_tokens: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GenerateResponse {
    pub message: ChatMessage,
    pub finish_reason: FinishReason,
    pub usage: Option<Usage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    ToolCalls,
    Length,
    ContentFilter,
    Error,
    Unknown(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{kind}: {message}")]
pub struct ModelError {
    pub kind: String,
    pub message: String,
}
impl ModelError {
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
        }
    }
}

#[async_trait]
pub trait ModelClient: Send + Sync {
    async fn generate(
        &self,
        context: &LLMContext,
        params: &GenerateParams,
    ) -> Result<GenerateResponse, ModelError>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScriptStep {
    pub expected_last_user: Option<String>,
    pub response: Result<GenerateResponse, ModelError>,
}
