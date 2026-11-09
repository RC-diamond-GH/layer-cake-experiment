use super::{CallRecord, RecordError, RunLog};
use crate::{
    context::{LLMContext, ToolCall, ToolDefinition},
    model::{GenerateParams, GenerateResponse, ModelClient, ModelError},
    tool::{ToolError, ToolRegistry},
};
use serde_json::{Value, json};
use std::time::Instant;

pub(super) fn deserialize_present_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    use serde::Deserialize;
    Value::deserialize(deserializer).map(Some)
}

pub async fn recorded_model(
    log: &RunLog,
    object_id: &str,
    cause: Option<&str>,
    client: &dyn ModelClient,
    context: &LLMContext,
    params: &GenerateParams,
) -> Result<(String, Result<GenerateResponse, ModelError>), RecordError> {
    log.validate_source(object_id, cause)?;
    log.snapshot(context)?;
    let input = json!({"context":context,"params":params});
    let started = Instant::now();
    let result = client.generate(context, params).await;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let (output, error) = match &result {
        Ok(response) => (Some(serde_json::to_value(response)?), None),
        Err(error) => (None, Some(serde_json::to_value(error)?)),
    };
    let call = CallRecord {
        call_kind: "model".into(),
        input,
        output,
        error,
        elapsed_ms,
        context_id: Some(context.id.clone()),
        context_version: Some(context.version),
        tool_call_id: None,
    };
    let id = log.observe(object_id, "call", cause, serde_json::to_value(call)?)?;
    Ok((id, result))
}

pub async fn recorded_tool(
    log: &RunLog,
    object_id: &str,
    cause: Option<&str>,
    registry: &ToolRegistry,
    visible: &[ToolDefinition],
    call: &ToolCall,
) -> Result<(String, Result<Value, ToolError>), RecordError> {
    log.validate_source(object_id, cause)?;
    let (context_id, context_version) = log.call_context(cause)?;
    let input = json!({"name":call.name,"arguments":call.arguments,"visible":visible});
    let started = Instant::now();
    let result = registry.execute(&call.name, visible, &call.arguments).await;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let (output, error) = match &result {
        Ok(response) => (Some(response.clone()), None),
        Err(error) => (None, Some(serde_json::to_value(error)?)),
    };
    let record = CallRecord {
        call_kind: "tool".into(),
        input,
        output,
        error,
        elapsed_ms,
        context_id,
        context_version,
        tool_call_id: Some(call.id.clone()),
    };
    let id = log.observe(object_id, "call", cause, serde_json::to_value(record)?)?;
    Ok((id, result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ScriptModel, ScriptStep};
    use crate::tool::{Memory, memory_tools};

    #[test]
    fn successful_null_output_survives_persistence_without_becoming_absent() {
        let record = CallRecord {
            call_kind: "tool".into(),
            input: json!({}),
            output: Some(Value::Null),
            error: None,
            elapsed_ms: 0,
            context_id: None,
            context_version: None,
            tool_call_id: None,
        };
        let encoded = serde_json::to_value(&record).expect("encode");
        assert!(encoded.get("error").is_none());
        assert_eq!(encoded.get("output"), Some(&Value::Null));
        let restored: CallRecord = serde_json::from_value(encoded).expect("decode");
        assert_eq!(restored.output, Some(Value::Null));
        assert_eq!(restored, record);
    }

    #[tokio::test]
    async fn error_records_preserve_actual_context_and_originating_model() {
        let log = RunLog::new("run".into(), "test".into(), Value::Null);
        log.register("model", "model", "Model")
            .expect("register model");
        log.register("memory_read", "tool", "memory_read")
            .expect("register tool");
        let model = ScriptModel::new(vec![ScriptStep {
            expected_last_user: None,
            response: Err(ModelError::new("actual_failure", "provider failed")),
        }]);
        let mut context = LLMContext::new("actual-context", "system");
        context.append(crate::context::ChatMessage::User {
            content: "question".into(),
        });
        let (model_id, result) = recorded_model(
            &log,
            "model",
            None,
            &model,
            &context,
            &GenerateParams::default(),
        )
        .await
        .expect("record model");
        assert_eq!(result.expect_err("model failed").kind, "actual_failure");
        let mut registry = ToolRegistry::new();
        for tool in memory_tools(Memory::default()) {
            registry.register(tool).expect("register");
        }
        let call = ToolCall {
            id: "call".into(),
            name: "memory_read".into(),
            arguments: json!({"key":"missing"}),
        };
        let (_, result) = recorded_tool(
            &log,
            "memory_read",
            Some(&model_id),
            &registry,
            &registry.definitions(),
            &call,
        )
        .await
        .expect("record tool");
        assert_eq!(result.expect_err("missing memory key").kind, "missing_key");
        let data = log.data().expect("run");
        let model_record: CallRecord =
            serde_json::from_value(data.records[0].data.clone()).expect("model record");
        let tool_record: CallRecord =
            serde_json::from_value(data.records[1].data.clone()).expect("tool record");
        assert_eq!(model_record.input["context"]["version"], 1);
        assert_eq!(model_record.input["params"]["model"], "script");
        assert_eq!(tool_record.context_id.as_deref(), Some("actual-context"));
        assert_eq!(tool_record.context_version, Some(1));
        assert_eq!(data.records[1].cause.as_deref(), Some(model_id.as_str()));
        assert_eq!(
            tool_record.error.expect("tool error")["kind"],
            "missing_key"
        );
    }
}
