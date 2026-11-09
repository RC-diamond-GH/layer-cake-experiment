//! Deterministic reactive and polling drivers over the same core components.
mod driver;
mod scripted;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::tool::{ToolError, ToolRegistry};

pub use driver::{ReactiveExperiment, assemble, assemble_with};
pub use scripted::ScenarioModel;

pub type RegistryWrapper =
    Arc<dyn Fn(ToolRegistry) -> Result<ToolRegistry, ToolError> + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScenarioConfig {
    pub duration: u64,
    pub completion_before_subscribe: bool,
    pub duplicate_notification: bool,
    pub mutate_while_waiting: bool,
}

impl Default for ScenarioConfig {
    fn default() -> Self {
        Self {
            duration: 3,
            completion_before_subscribe: false,
            duplicate_notification: false,
            mutate_while_waiting: false,
        }
    }
}

#[derive(Debug, Error)]
pub enum ReactiveError {
    #[error(transparent)]
    Environment(#[from] crate::environment::EnvironmentError),
    #[error(transparent)]
    Event(#[from] crate::events::EventError),
    #[error(transparent)]
    Record(#[from] crate::record::RecordError),
    #[error(transparent)]
    React(#[from] crate::react::ReactError),
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error("simulation lock poisoned: {0}")]
    Lock(String),
    #[error("invalid reactive input: {0}")]
    Input(String),
    #[error("reactive scenario failed: {0}")]
    Scenario(String),
}

#[cfg(test)]
mod tests;
