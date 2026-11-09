use super::*;
use crate::{
    context::{ChatMessage, ToolCall},
    model::{FinishReason, GenerateResponse, ScriptModel, ScriptStep},
    record::CallRecord,
    tool::{Memory, memory_tools},
};
use serde_json::{Value, json};

fn response(calls: Vec<ToolCall>, finish_reason: FinishReason) -> ScriptStep {
    ScriptStep {
        expected_last_user: None,
        response: Ok(GenerateResponse {
            message: ChatMessage::Assistant {
                content: Some("response".into()),
                tool_calls: calls,
            },
            finish_reason,
            usage: None,
        }),
    }
}
fn call(id: &str, name: &str, arguments: Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments,
    }
}
fn agent(steps: Vec<ScriptStep>, max_rounds: u64) -> (ReAct, Memory) {
    let memory = Memory::default();
    let mut tools = ToolRegistry::new();
    for tool in memory_tools(memory.clone()) {
        tools.register(tool).expect("register");
    }
    let mut context = LLMContext::new("context", "system");
    context.set_tools(tools.definitions());
    let agent = ReAct::new(
        "agent".into(),
        context,
        Arc::new(ScriptModel::new(steps)),
        Arc::new(tools),
        GenerateParams::default(),
        max_rounds,
        RunLog::new("run".into(), "test".into(), Value::Null),
    )
    .expect("agent");
    (agent, memory)
}
fn calls(agent: &ReAct) -> Vec<CallRecord> {
    agent
        .log
        .data()
        .expect("run")
        .records
        .into_iter()
        .filter(|record| record.kind == "call")
        .map(|record| serde_json::from_value(record.data).expect("call record"))
        .collect()
}

#[tokio::test]
async fn all_ordered_tool_results_are_part_of_one_round() {
    let (mut agent, memory) = agent(
        vec![
            response(
                vec![
                    call("write", "memory_write", json!({"key":"a","value":42})),
                    call("read", "memory_read", json!({"key":"a"})),
                ],
                FinishReason::ToolCalls,
            ),
            response(vec![], FinishReason::Stop),
        ],
        4,
    );
    assert_eq!(
        agent.step(None).await.expect("first round"),
        StepOutcome::Continue
    );
    assert_eq!(agent.rounds, 1);
    assert_eq!(memory.lock().expect("memory").get("a"), Some(&json!(42)));
    assert_eq!(agent.context.messages.len(), 3);
    assert!(
        matches!(&agent.context.messages[1], ChatMessage::Tool { tool_call_id, .. } if tool_call_id == "write")
    );
    assert!(
        matches!(&agent.context.messages[2], ChatMessage::Tool { tool_call_id, content } if tool_call_id == "read" && content == "42")
    );
    assert_eq!(
        agent.step(None).await.expect("final round"),
        StepOutcome::Finished
    );
    let count = calls(&agent).len();
    assert_eq!(count, 4);
    assert_eq!(
        agent.step(None).await.expect("terminal no-op"),
        StepOutcome::Finished
    );
    assert_eq!(calls(&agent).len(), count);
    assert_eq!(agent.rounds, 2);
}

#[tokio::test]
async fn tool_error_is_written_and_prevents_remaining_dispatch() {
    let (mut agent, memory) = agent(
        vec![response(
            vec![
                call("missing", "memory_read", json!({"key":"absent"})),
                call("forbidden", "memory_write", json!({"key":"a","value":1})),
            ],
            FinishReason::ToolCalls,
        )],
        4,
    );
    assert!(matches!(agent.step(None).await, Err(ReactError::Tool(_))));
    assert_eq!(
        agent.state(),
        &ReactState::Terminal {
            reason: StopReason::ToolError
        }
    );
    assert!(memory.lock().expect("memory").is_empty());
    assert_eq!(calls(&agent).len(), 2);
    assert!(
        matches!(agent.context.messages.last(), Some(ChatMessage::Tool { tool_call_id, content }) if tool_call_id == "missing" && content.contains("missing_key"))
    );
    assert_eq!(
        agent.step(None).await.expect("terminal"),
        StepOutcome::Finished
    );
    assert_eq!(calls(&agent).len(), 2);
    assert!(agent.reopen().is_err());
}

#[tokio::test]
async fn truncation_filter_and_unknown_never_dispatch_tools() {
    for (finish, reason) in [
        (FinishReason::Length, StopReason::Truncated),
        (FinishReason::ContentFilter, StopReason::ContentFiltered),
        (FinishReason::Error, StopReason::ModelFinishError),
        (
            FinishReason::Unknown("future".into()),
            StopReason::UnknownFinish("future".into()),
        ),
    ] {
        let (mut agent, memory) = agent(
            vec![response(
                vec![call("write", "memory_write", json!({"key":"a","value":1}))],
                finish,
            )],
            4,
        );
        assert_eq!(
            agent.step(None).await.expect("terminal reason"),
            StepOutcome::Finished
        );
        assert_eq!(agent.state(), &ReactState::Terminal { reason });
        assert!(memory.lock().expect("memory").is_empty());
        assert_eq!(calls(&agent).len(), 1);
        assert_eq!(
            agent.step(None).await.expect("terminal no-op"),
            StepOutcome::Finished
        );
        assert_eq!(calls(&agent).len(), 1);
    }
}

#[tokio::test]
async fn zero_budget_and_stop_prevent_model_dispatch() {
    let (mut exhausted, _) = agent(vec![], 0);
    assert_eq!(
        exhausted.step(None).await.expect("zero rounds"),
        StepOutcome::Finished
    );
    assert_eq!(
        exhausted.state(),
        &ReactState::Terminal {
            reason: StopReason::Exhausted
        }
    );
    assert!(calls(&exhausted).is_empty());
    let (mut stopped, _) = agent(vec![], 5);
    stopped.stop().expect("stop");
    assert_eq!(
        stopped.step(None).await.expect("stopped"),
        StepOutcome::Finished
    );
    assert_eq!(
        stopped.state(),
        &ReactState::Terminal {
            reason: StopReason::Stopped
        }
    );
    assert!(calls(&stopped).is_empty());
    assert!(stopped.reopen().is_err());
}

#[tokio::test]
async fn exhausted_after_tool_round_is_distinct_from_final_reply() {
    let (mut agent, _) = agent(
        vec![response(
            vec![call("write", "memory_write", json!({"key":"a","value":1}))],
            FinishReason::ToolCalls,
        )],
        1,
    );
    assert_eq!(
        agent.step(None).await.expect("complete bounded round"),
        StepOutcome::Finished
    );
    assert_eq!(
        agent.state(),
        &ReactState::Terminal {
            reason: StopReason::Exhausted
        }
    );
    assert_eq!(agent.context.messages.len(), 2);
    assert_eq!(calls(&agent).len(), 2);
}

#[tokio::test]
async fn normal_finish_can_reopen_without_resetting_budget() {
    let (mut agent, _) = agent(
        vec![
            response(vec![], FinishReason::Stop),
            response(vec![], FinishReason::Stop),
        ],
        2,
    );
    agent.step(None).await.expect("finish");
    agent
        .append_user("new event".into(), None)
        .expect("input at boundary");
    agent.reopen().expect("normal reopen");
    assert_eq!(agent.rounds, 1);
    agent.step(None).await.expect("second finish");
    agent.reopen().expect("reopen retains budget");
    agent.step(None).await.expect("exhausted");
    assert_eq!(
        agent.state(),
        &ReactState::Terminal {
            reason: StopReason::Exhausted
        }
    );
    assert_eq!(calls(&agent).len(), 2);
}

#[tokio::test]
async fn model_failure_preserves_error_and_never_retries() {
    let (mut agent, _) = agent(
        vec![ScriptStep {
            expected_last_user: None,
            response: Err(ModelError::new("network", "actual error")),
        }],
        4,
    );
    let error = agent.step(None).await.expect_err("model error");
    assert!(matches!(error, ReactError::Model(ModelError { ref kind, .. }) if kind == "network"));
    assert_eq!(
        agent.state(),
        &ReactState::Terminal {
            reason: StopReason::ModelError
        }
    );
    agent.step(None).await.expect("no retry");
    assert_eq!(calls(&agent).len(), 1);
    assert!(agent.context.messages.is_empty());
}

#[tokio::test]
async fn duplicate_call_ids_fail_before_any_tool_effect() {
    let (mut agent, memory) = agent(
        vec![response(
            vec![
                call("duplicate", "memory_write", json!({"key":"a","value":1})),
                call("duplicate", "memory_write", json!({"key":"b","value":2})),
            ],
            FinishReason::ToolCalls,
        )],
        4,
    );
    assert!(matches!(
        agent.step(None).await,
        Err(ReactError::InvalidResponse(_))
    ));
    assert!(memory.lock().expect("memory").is_empty());
    assert_eq!(calls(&agent).len(), 1);
}

#[tokio::test]
async fn stop_after_normal_finish_blocks_controlled_reopen() {
    let (mut agent, _) = agent(vec![response(vec![], FinishReason::Stop)], 4);
    agent.step(None).await.expect("normal finish");
    agent.stop().expect("stop finished agent");
    assert_eq!(
        agent.state(),
        &ReactState::Terminal {
            reason: StopReason::Stopped
        }
    );
    assert!(agent.reopen().is_err());
    agent.step(None).await.expect("no dispatch");
    assert_eq!(calls(&agent).len(), 1);
}

struct AwaitingModel {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl ModelClient for AwaitingModel {
    async fn generate(
        &self,
        _: &LLMContext,
        _: &GenerateParams,
    ) -> Result<GenerateResponse, ModelError> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(GenerateResponse {
            message: ChatMessage::Assistant {
                content: Some("actual model return".into()),
                tool_calls: vec![call(
                    "must-not-run",
                    "memory_write",
                    json!({"key":"effect","value":1}),
                )],
            },
            finish_reason: FinishReason::ToolCalls,
            usage: None,
        })
    }
}

struct AwaitingTool {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl crate::tool::Tool for AwaitingTool {
    fn definition(&self) -> crate::context::ToolDefinition {
        crate::context::ToolDefinition {
            name: "awaiting_tool".into(),
            description: "Controlled asynchronous test tool".into(),
            parameters: json!({"type":"object"}),
        }
    }
    async fn execute(&self, _: &Value) -> Result<Value, ToolError> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(json!({"actual":"tool return"}))
    }
}

#[tokio::test]
async fn stop_during_model_await_preserves_return_and_prevents_tool_dispatch() {
    use std::sync::atomic::Ordering;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let model = Arc::new(AwaitingModel {
        entered: entered.clone(),
        release: release.clone(),
    });
    let memory = Memory::default();
    let mut registry = ToolRegistry::new();
    for tool in memory_tools(memory.clone()) {
        registry.register(tool).expect("register");
    }
    let mut context = LLMContext::new("context", "system");
    context.set_tools(registry.definitions());
    let mut agent = ReAct::new(
        "agent".into(),
        context,
        model,
        Arc::new(registry),
        GenerateParams::default(),
        4,
        RunLog::new("run".into(), "test".into(), Value::Null),
    )
    .expect("agent");
    let stop = agent.stop_signal();
    let stopper = async {
        entered.notified().await;
        stop.store(true, Ordering::Release);
        release.notify_one();
    };
    let (result, ()) = tokio::join!(agent.step(None), stopper);
    assert_eq!(result.expect("cooperative stop"), StepOutcome::Finished);
    assert_eq!(
        agent.state(),
        &ReactState::Terminal {
            reason: StopReason::Stopped
        }
    );
    assert!(memory.lock().expect("memory").is_empty());
    let recorded = calls(&agent);
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].output.as_ref().expect("actual model response")["message"]["content"],
        "actual model return"
    );
    assert!(
        matches!(agent.context.messages.last(), Some(ChatMessage::Assistant { content: Some(content), .. }) if content == "actual model return")
    );
    assert!(agent.reopen().is_err());
    agent.step(None).await.expect("stopped remains terminal");
    assert_eq!(calls(&agent).len(), 1);
}

#[tokio::test]
async fn stop_during_tool_await_preserves_return_and_prevents_next_tool() {
    use std::sync::atomic::Ordering;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let memory = Memory::default();
    let mut registry = ToolRegistry::new();
    for tool in memory_tools(memory.clone()) {
        registry.register(tool).expect("register");
    }
    registry
        .register(Arc::new(AwaitingTool {
            entered: entered.clone(),
            release: release.clone(),
        }))
        .expect("awaiting tool");
    let mut context = LLMContext::new("context", "system");
    context.set_tools(registry.definitions());
    let model = Arc::new(ScriptModel::new(vec![response(
        vec![
            call("in-flight", "awaiting_tool", json!({})),
            call(
                "must-not-run",
                "memory_write",
                json!({"key":"effect","value":1}),
            ),
        ],
        FinishReason::ToolCalls,
    )]));
    let mut agent = ReAct::new(
        "agent".into(),
        context,
        model,
        Arc::new(registry),
        GenerateParams::default(),
        4,
        RunLog::new("run".into(), "test".into(), Value::Null),
    )
    .expect("agent");
    let stop = agent.stop_signal();
    let stopper = async {
        entered.notified().await;
        stop.store(true, Ordering::Release);
        release.notify_one();
    };
    let (result, ()) = tokio::join!(agent.step(None), stopper);
    assert_eq!(result.expect("cooperative stop"), StepOutcome::Finished);
    assert_eq!(
        agent.state(),
        &ReactState::Terminal {
            reason: StopReason::Stopped
        }
    );
    assert!(memory.lock().expect("memory").is_empty());
    let recorded = calls(&agent);
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[1].output, Some(json!({"actual":"tool return"})));
    assert!(
        matches!(agent.context.messages.last(), Some(ChatMessage::Tool { tool_call_id, content }) if tool_call_id == "in-flight" && content == "{\"actual\":\"tool return\"}")
    );
    assert!(agent.reopen().is_err());
    agent.step(None).await.expect("stopped remains terminal");
    assert_eq!(calls(&agent).len(), 2);
}

#[tokio::test]
async fn external_stop_signal_prevents_initial_dispatch_and_reopen() {
    use std::sync::atomic::Ordering;
    let (mut ready, _) = agent(vec![], 4);
    ready.stop_signal().store(true, Ordering::Release);
    assert_eq!(
        ready.step(None).await.expect("stop before generation"),
        StepOutcome::Finished
    );
    assert_eq!(ready.rounds, 0);
    assert!(calls(&ready).is_empty());
    assert!(ready.reopen().is_err());
    let (mut finished, _) = agent(vec![response(vec![], FinishReason::Stop)], 4);
    finished.step(None).await.expect("normal finish");
    finished.stop_signal().store(true, Ordering::Release);
    assert!(finished.reopen().is_err());
    assert!(finished.append_user("late input".into(), None).is_err());
    finished.step(None).await.expect("latched stop");
    assert_eq!(
        finished.state(),
        &ReactState::Terminal {
            reason: StopReason::Stopped
        }
    );
    assert_eq!(calls(&finished).len(), 1);
}
