use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::context::ToolDefinition;
use crate::tool::{Tool, ToolError};

use super::Simulation;

struct SimulationTool {
    simulation: Arc<Mutex<Simulation>>,
    operation: &'static str,
    prefix: &'static str,
}

#[async_trait]
impl Tool for SimulationTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: format!("{}_{}", self.prefix, self.operation),
            description: match self.operation {
                "start" => {
                    "Start a deterministic job; return job identity and current resource snapshot."
                }
                "query" => "Query the true current job and versioned resources.",
                _ => "Collect completed job output from the current resource state.",
            }
            .into(),
            parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
        }
    }

    async fn execute(&self, arguments: &Value) -> Result<Value, ToolError> {
        if !arguments
            .as_object()
            .is_some_and(|object| object.is_empty())
        {
            return Err(ToolError {
                kind: "arguments".into(),
                message: "expected an empty object".into(),
            });
        }
        let mut simulation = self.simulation.lock().map_err(|error| ToolError {
            kind: "lock".into(),
            message: error.to_string(),
        })?;
        match self.operation {
            "start" => simulation.start(),
            "query" => Ok(simulation.query()),
            _ => simulation.take(),
        }
        .map_err(|error| ToolError {
            kind: "environment".into(),
            message: error.to_string(),
        })
    }
}

pub fn simulation_tools(simulation: Arc<Mutex<Simulation>>, timer: bool) -> Vec<Arc<dyn Tool>> {
    let prefix = if timer { "timer" } else { "furnace" };
    ["start", "query", "take"]
        .into_iter()
        .map(|operation| {
            Arc::new(SimulationTool {
                simulation: simulation.clone(),
                operation,
                prefix,
            }) as Arc<dyn Tool>
        })
        .collect()
}
