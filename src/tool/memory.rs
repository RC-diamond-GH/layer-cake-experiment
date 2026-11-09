use super::{Tool, ToolError};
use crate::context::ToolDefinition;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

pub type Memory = Arc<Mutex<BTreeMap<String, Value>>>;

struct MemoryRead {
    memory: Memory,
}
struct MemoryWrite {
    memory: Memory,
}

pub fn memory_tools(memory: Memory) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(MemoryRead {
            memory: memory.clone(),
        }),
        Arc::new(MemoryWrite { memory }),
    ]
}

fn key(arguments: &Value, write: bool) -> Result<&str, ToolError> {
    let object = arguments
        .as_object()
        .ok_or_else(|| ToolError::new("invalid_arguments", "expected an object"))?;
    if object
        .keys()
        .any(|key| key != "key" && !(write && key == "value"))
        || (write && !object.contains_key("value"))
    {
        return Err(ToolError::new(
            "invalid_arguments",
            "unexpected argument or missing value",
        ));
    }
    object
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("invalid_arguments", "key must be a string"))
}

#[async_trait]
impl Tool for MemoryRead {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "memory_read".into(),
            description: "Read a shared memory value by key.".into(),
            parameters: json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"],"additionalProperties":false}),
        }
    }
    async fn execute(&self, arguments: &Value) -> Result<Value, ToolError> {
        let key = key(arguments, false)?;
        self.memory
            .lock()
            .map_err(|_| ToolError::new("memory_lock", "shared memory lock poisoned"))?
            .get(key)
            .cloned()
            .ok_or_else(|| {
                ToolError::new("missing_key", format!("memory key {key:?} does not exist"))
            })
    }
}

#[async_trait]
impl Tool for MemoryWrite {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "memory_write".into(),
            description: "Write a shared memory value by key.".into(),
            parameters: json!({"type":"object","properties":{"key":{"type":"string"},"value":{}},"required":["key","value"],"additionalProperties":false}),
        }
    }
    async fn execute(&self, arguments: &Value) -> Result<Value, ToolError> {
        let key = key(arguments, true)?;
        let value = arguments
            .get("value")
            .ok_or_else(|| ToolError::new("invalid_arguments", "value is required"))?;
        self.memory
            .lock()
            .map_err(|_| ToolError::new("memory_lock", "shared memory lock poisoned"))?
            .insert(key.to_owned(), value.clone());
        Ok(json!({"key":key,"value":value}))
    }
}
