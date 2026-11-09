use async_trait::async_trait;
use serde_json::{Value, json};

use crate::context::{ChatMessage, LLMContext, ToolCall};
use crate::model::{FinishReason, GenerateParams, GenerateResponse, ModelClient, ModelError};

/// A deterministic policy reading actual tool observations, not a fixed final
/// response or a preselected number of polling rounds.
pub struct ScenarioModel {
    timer: bool,
}
impl ScenarioModel {
    pub fn new(timer: bool) -> Self {
        Self { timer }
    }
}

#[async_trait]
impl ModelClient for ScenarioModel {
    async fn generate(
        &self,
        context: &LLMContext,
        _params: &GenerateParams,
    ) -> Result<GenerateResponse, ModelError> {
        let last_result = context.messages.iter().rev().find_map(|message| {
            if let ChatMessage::Tool { content, .. } = message {
                Some(content)
            } else {
                None
            }
        });
        let mut content = None;
        let operation = if let Some(result) = last_result {
            let value: Value = serde_json::from_str(result).map_err(|error| ModelError {
                kind: "scenario".into(),
                message: error.to_string(),
            })?;
            match value.get("operation").and_then(Value::as_str) {
                Some("start") => Some("query"),
                Some("query") => match value.pointer("/job/status").and_then(Value::as_str) {
                    Some("running") => Some("query"),
                    Some("completed") => Some("take"),
                    status => {
                        return Err(ModelError {
                            kind: "scenario".into(),
                            message: format!("unexpected queried job status: {status:?}"),
                        });
                    }
                },
                Some("take") => {
                    let job_id = value
                        .pointer("/job/id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| ModelError {
                            kind: "scenario".into(),
                            message: "missing job identity".into(),
                        })?;
                    let quantity = value
                        .pointer("/job/collected")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| ModelError {
                            kind: "scenario".into(),
                            message: "missing collected quantity".into(),
                        })?;
                    let version = value
                        .pointer("/resource/version")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| ModelError {
                            kind: "scenario".into(),
                            message: "missing resource version".into(),
                        })?;
                    content = Some(format!(
                        "Completed job {job_id}. Collected {quantity} from resource version {version}."
                    ));
                    None
                }
                operation => {
                    return Err(ModelError {
                        kind: "scenario".into(),
                        message: format!("unexpected tool operation: {operation:?}"),
                    });
                }
            }
        } else {
            Some("start")
        };
        let tool_calls = operation
            .map(|operation| ToolCall {
                id: format!(
                    "scenario-call-{}",
                    context
                        .messages
                        .iter()
                        .filter(|message| matches!(message, ChatMessage::Assistant { .. }))
                        .count()
                        + 1
                ),
                name: format!(
                    "{}_{}",
                    if self.timer { "timer" } else { "furnace" },
                    operation
                ),
                arguments: json!({}),
            })
            .into_iter()
            .collect();
        Ok(GenerateResponse {
            message: ChatMessage::Assistant {
                content,
                tool_calls,
            },
            finish_reason: if operation.is_some() {
                FinishReason::ToolCalls
            } else {
                FinishReason::Stop
            },
            usage: None,
        })
    }
}
