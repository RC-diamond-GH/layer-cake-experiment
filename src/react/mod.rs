//! Ordered, bounded ReAct rounds with explicit terminal states.
mod agent;
#[cfg(test)]
mod tests;

use crate::{
    context::LLMContext,
    model::{GenerateParams, ModelClient, ModelError},
    record::{RecordError, RunLog},
    tool::{ToolError, ToolRegistry},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepOutcome {
    Continue,
    Finished,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Finished,
    ModelError,
    ToolError,
    Truncated,
    ContentFiltered,
    ModelFinishError,
    UnknownFinish(String),
    Exhausted,
    Stopped,
    InvalidResponse,
    RecordError,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ReactState {
    Ready,
    Terminal { reason: StopReason },
}

#[derive(Debug, thiserror::Error)]
pub enum ReactError {
    #[error(transparent)]
    Record(#[from] RecordError),
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error("invalid model response: {0}")]
    InvalidResponse(String),
    #[error("invalid agent operation: {0}")]
    State(String),
}

pub struct ReAct {
    pub context: LLMContext,
    pub rounds: u64,
    pub log: RunLog,
    id: String,
    model_id: String,
    model: Arc<dyn ModelClient>,
    tools: Arc<ToolRegistry>,
    params: GenerateParams,
    max_rounds: u64,
    state: ReactState,
    context_cause: Option<String>,
    stop_signal: Arc<std::sync::atomic::AtomicBool>,
}
