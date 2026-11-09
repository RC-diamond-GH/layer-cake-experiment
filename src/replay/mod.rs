//! Strict call adapters for rerunning deterministic experiments without external effects.
mod tape;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub use tape::{ReplayModel, ReplayTape, ReplayTool};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplaySummary {
    pub run_id: String,
    pub calls: usize,
    pub model_calls: usize,
    pub tool_calls: usize,
}

#[derive(Clone, Debug, PartialEq, Error)]
pub enum ReplayError {
    #[error(
        "replay divergence at record {record_id}, field {field}: expected {expected}, actual {actual}"
    )]
    Divergence {
        record_id: String,
        field: String,
        expected: Value,
        actual: Value,
    },
    #[error("invalid replay record {record_id}: {message}")]
    InvalidRecord { record_id: String, message: String },
    #[error("replay state lock was poisoned")]
    Poisoned,
    #[error("replay execution failed: {0}")]
    Runtime(String),
}

/// Rerun the original experiment, including recorded boundary inputs, using replay adapters.
pub async fn verify(data: &crate::record::RunData) -> Result<ReplaySummary, ReplayError> {
    crate::experiment::replay(data).await
}
