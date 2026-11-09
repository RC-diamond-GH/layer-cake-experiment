use super::{Tool, ToolError, ToolExecutor, ToolRegistry};
use crate::context::ToolDefinition;
use serde_json::Value;
use std::sync::Arc;

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_executor(executor: Arc<dyn ToolExecutor>) -> Self {
        Self {
            executor: Some(executor),
            ..Self::default()
        }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<(), ToolError> {
        let definition = tool.definition();
        if definition.name.is_empty()
            || !definition
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(ToolError::new(
                "invalid_definition",
                "tool name must contain only ASCII letters, digits, underscores or hyphens",
            ));
        }
        if !definition.parameters.is_object() {
            return Err(ToolError::new(
                "invalid_definition",
                "tool parameters must be a JSON schema object",
            ));
        }
        if self.tools.contains_key(&definition.name) {
            return Err(ToolError::new(
                "duplicate_tool",
                format!("tool {} is already registered", definition.name),
            ));
        }
        self.tools
            .insert(definition.name.clone(), (definition, tool));
        Ok(())
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|(definition, _)| definition.clone())
            .collect()
    }

    pub async fn execute(
        &self,
        name: &str,
        visible: &[ToolDefinition],
        arguments: &Value,
    ) -> Result<Value, ToolError> {
        if let Some(executor) = &self.executor {
            return executor.execute(name, visible, arguments).await;
        }
        let (definition, tool) = self.tools.get(name).ok_or_else(|| {
            ToolError::new("unknown_tool", format!("tool {name} is not registered"))
        })?;
        let mut matches = visible.iter().filter(|entry| entry.name == name);
        let shown = matches.next().ok_or_else(|| {
            ToolError::new(
                "hidden_tool",
                format!("tool {name} is not visible in this context"),
            )
        })?;
        if matches.next().is_some() || shown != definition {
            return Err(ToolError::new(
                "definition_mismatch",
                format!("visible definition for {name} does not match its registered definition"),
            ));
        }
        if !arguments.is_object() {
            return Err(ToolError::new(
                "invalid_arguments",
                "tool arguments must be an object",
            ));
        }
        tool.execute(arguments).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{Memory, memory_tools};
    use serde_json::json;

    #[tokio::test]
    async fn visibility_duplicate_definitions_and_missing_values_are_enforced() {
        let memory = Memory::default();
        let mut registry = ToolRegistry::new();
        for tool in memory_tools(memory.clone()) {
            registry.register(tool).expect("register tool");
        }
        assert!(
            registry
                .register(memory_tools(memory.clone()).remove(0))
                .is_err()
        );
        let visible = registry.definitions();
        assert_eq!(
            visible.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            vec!["memory_read", "memory_write"]
        );
        assert_eq!(
            registry
                .execute("memory_write", &[], &json!({"key":"a","value":1}))
                .await
                .expect_err("hidden")
                .kind,
            "hidden_tool"
        );
        assert!(memory.lock().expect("memory").is_empty());
        let mut altered = visible.clone();
        altered[1].description = "different".into();
        assert_eq!(
            registry
                .execute("memory_write", &altered, &json!({"key":"a","value":1}))
                .await
                .expect_err("mismatch")
                .kind,
            "definition_mismatch"
        );
        registry
            .execute("memory_write", &visible, &json!({"key":"a","value":null}))
            .await
            .expect("write null");
        assert_eq!(
            registry
                .execute("memory_read", &visible, &json!({"key":"a"}))
                .await
                .expect("read null"),
            Value::Null
        );
        assert_eq!(
            registry
                .execute("memory_read", &visible, &json!({"key":"missing"}))
                .await
                .expect_err("missing")
                .kind,
            "missing_key"
        );
        assert!(
            registry
                .execute("memory_read", &visible, &json!({"key":"a","extra":1}))
                .await
                .is_err()
        );
    }
}
