//! Model-visible tool definitions and executable dispatch.
mod memory;
mod registry;
pub use memory::{Memory, memory_tools};

use crate::context::ToolDefinition;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{kind}: {message}")]
pub struct ToolError {
    pub kind: String,
    pub message: String,
}
impl ToolError {
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
        }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    async fn execute(&self, arguments: &Value) -> Result<Value, ToolError>;
}

/// An alternate execution boundary for strict replay, including rejected calls.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(
        &self,
        name: &str,
        visible: &[ToolDefinition],
        arguments: &Value,
    ) -> Result<Value, ToolError>;
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, (ToolDefinition, Arc<dyn Tool>)>,
    executor: Option<Arc<dyn ToolExecutor>>,
}
