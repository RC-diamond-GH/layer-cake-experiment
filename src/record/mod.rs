//! Thread-safe extensible observation storage and the shared call boundaries.
mod calls;
mod store;
pub use calls::{recorded_model, recorded_tool};

use crate::context::LLMContext;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunData {
    pub id: String,
    pub experiment: String,
    pub config: Value,
    pub objects: Vec<ObjectRecord>,
    pub records: Vec<Observation>,
    pub snapshots: Vec<LLMContext>,
    pub summary: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectRecord {
    pub id: String,
    pub kind: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub sequence: u64,
    pub run_id: String,
    pub object_id: String,
    pub cause: Option<String>,
    pub timestamp_ms: u64,
    pub kind: String,
    pub data: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallRecord {
    pub call_kind: String,
    pub input: Value,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "calls::deserialize_present_value"
    )]
    pub output: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "calls::deserialize_present_value"
    )]
    pub error: Option<Value>,
    pub elapsed_ms: u64,
    pub context_id: Option<String>,
    pub context_version: Option<u64>,
    pub tool_call_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RunLog {
    inner: Arc<Mutex<RunData>>,
}

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("run storage lock is poisoned")]
    Poisoned,
    #[error("invalid record: {0}")]
    Invalid(String),
    #[error("record serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("run file operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("system clock is before the Unix epoch")]
    Clock,
}
