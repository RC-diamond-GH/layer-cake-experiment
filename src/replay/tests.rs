use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{ReplayError, ReplayTape};
use crate::context::{ChatMessage, LLMContext, ToolDefinition};
use crate::model::{FinishReason, GenerateParams, GenerateResponse, ModelError};
use crate::record::{CallRecord, Observation, RunData};
use crate::tool::{Tool, ToolError, ToolExecutor, ToolRegistry};

fn context() -> LLMContext {
    let mut context = LLMContext::new("run/context", "Keep every input.");
    context.append(ChatMessage::User {
        content: "original request".into(),
    });
    context
}

fn definition() -> ToolDefinition {
    ToolDefinition {
        name: "effect".into(),
        description: "Never execute on replay".into(),
        parameters: json!({"type":"object"}),
    }
}

fn response() -> GenerateResponse {
    GenerateResponse {
        message: ChatMessage::Assistant {
            content: Some("original answer".into()),
            tool_calls: Vec::new(),
        },
        finish_reason: FinishReason::Stop,
        usage: None,
    }
}

fn model_call() -> CallRecord {
    let context = context();
    CallRecord {
        call_kind: "model".into(),
        input: json!({"context":context,"params":GenerateParams::default()}),
        output: Some(json!(response())),
        error: None,
        elapsed_ms: 8,
        context_id: Some(context.id),
        context_version: Some(context.version),
        tool_call_id: None,
    }
}

fn tool_call() -> CallRecord {
    CallRecord {
        call_kind: "tool".into(),
        input: json!({"name":"effect","arguments":{"key":"original"},"visible":[definition()]}),
        output: Some(json!({"value":7})),
        error: None,
        elapsed_ms: 4,
        context_id: None,
        context_version: None,
        tool_call_id: Some("tool-1".into()),
    }
}

fn data(calls: Vec<CallRecord>) -> RunData {
    RunData {
        id: "run".into(),
        experiment: "test".into(),
        config: json!({}),
        objects: Vec::new(),
        records: calls
            .into_iter()
            .enumerate()
            .map(|(index, call)| Observation {
                id: format!("record-{index}"),
                sequence: index as u64 + 1,
                run_id: "run".into(),
                object_id: call.call_kind.clone(),
                cause: None,
                timestamp_ms: 0,
                kind: "call".into(),
                data: json!(call),
            })
            .collect(),
        snapshots: vec![context()],
        summary: json!({}),
    }
}

#[tokio::test]
async fn context_version_mismatch_is_first_sticky_divergence()
-> Result<(), Box<dyn std::error::Error>> {
    let tape = ReplayTape::new(&data(vec![model_call(), tool_call()]))?;
    let mut changed = context();
    changed.version += 1;
    assert!(
        tape.model()
            .generate(&changed, &GenerateParams::default())
            .await
            .is_err()
    );
    let first = tape.check().expect_err("changed version must diverge");
    assert!(
        matches!(&first, ReplayError::Divergence { record_id, field, expected, actual }
        if record_id == "record-0" && field == "input.context.version" && expected == &json!(1) && actual == &json!(2))
    );
    assert!(
        ToolExecutor::execute(&tape, "effect", &[definition()], &json!({"key":"original"}))
            .await
            .is_err()
    );
    assert_eq!(tape.finish().expect_err("failure is sticky"), first);
    Ok(())
}

#[tokio::test]
async fn messages_and_generation_parameters_are_not_dropped()
-> Result<(), Box<dyn std::error::Error>> {
    let tape = ReplayTape::new(&data(vec![model_call()]))?;
    let mut changed = context();
    changed.messages[0] = ChatMessage::User {
        content: "changed request".into(),
    };
    assert!(
        tape.model()
            .generate(&changed, &GenerateParams::default())
            .await
            .is_err()
    );
    assert!(
        matches!(tape.check(), Err(ReplayError::Divergence { field, .. }) if field == "input.context.messages[0].content")
    );
    let tape = ReplayTape::new(&data(vec![model_call()]))?;
    let mut params = GenerateParams::default();
    params.max_tokens = Some(19);
    assert!(tape.model().generate(&context(), &params).await.is_err());
    assert!(
        matches!(tape.check(), Err(ReplayError::Divergence { field, .. }) if field == "input.params.max_tokens")
    );
    Ok(())
}

#[tokio::test]
async fn model_and_tool_calls_share_global_order() -> Result<(), Box<dyn std::error::Error>> {
    let tape = ReplayTape::new(&data(vec![model_call(), tool_call()]))?;
    assert!(
        ToolExecutor::execute(&tape, "effect", &[definition()], &json!({"key":"original"}))
            .await
            .is_err()
    );
    assert!(
        matches!(tape.check(), Err(ReplayError::Divergence { record_id, field, .. }) if record_id == "record-0" && field == "call_kind")
    );
    let tape = ReplayTape::new(&data(vec![model_call(), tool_call()]))?;
    assert_eq!(
        tape.model()
            .generate(&context(), &GenerateParams::default())
            .await?,
        response()
    );
    assert_eq!(
        ToolExecutor::execute(&tape, "effect", &[definition()], &json!({"key":"original"})).await?,
        json!({"value":7})
    );
    let summary = tape.finish()?;
    assert_eq!(
        (summary.calls, summary.model_calls, summary.tool_calls),
        (2, 1, 1)
    );
    Ok(())
}

#[tokio::test]
async fn tool_visibility_and_arguments_are_exact() -> Result<(), Box<dyn std::error::Error>> {
    let tape = ReplayTape::new(&data(vec![tool_call()]))?;
    assert!(
        ToolExecutor::execute(&tape, "effect", &[], &json!({"key":"original"}))
            .await
            .is_err()
    );
    assert!(
        matches!(tape.check(), Err(ReplayError::Divergence { field, .. }) if field == "input.visible[0]")
    );
    let tape = ReplayTape::new(&data(vec![tool_call()]))?;
    assert!(
        ToolExecutor::execute(&tape, "effect", &[definition()], &json!({"key":"changed"}))
            .await
            .is_err()
    );
    assert!(
        matches!(tape.check(), Err(ReplayError::Divergence { field, .. }) if field == "input.arguments.key")
    );
    Ok(())
}

#[tokio::test]
async fn exhaustion_detects_both_extra_and_unconsumed_calls()
-> Result<(), Box<dyn std::error::Error>> {
    let tape = ReplayTape::new(&data(vec![model_call()]))?;
    assert!(
        matches!(tape.finish(), Err(ReplayError::Divergence { record_id, field, .. }) if record_id == "record-0" && field == "call")
    );
    let tape = ReplayTape::new(&data(Vec::new()))?;
    assert!(
        tape.model()
            .generate(&context(), &GenerateParams::default())
            .await
            .is_err()
    );
    assert!(
        matches!(tape.check(), Err(ReplayError::Divergence { record_id, .. }) if record_id == "<end-of-tape>")
    );
    Ok(())
}

#[tokio::test]
async fn original_errors_are_returned_and_consumed() -> Result<(), Box<dyn std::error::Error>> {
    let model_error = ModelError {
        kind: "provider".into(),
        message: "original model failure".into(),
    };
    let tool_error = ToolError {
        kind: "permission".into(),
        message: "original tool failure".into(),
    };
    let mut model = model_call();
    model.output = None;
    model.error = Some(json!(model_error));
    let mut tool = tool_call();
    tool.output = None;
    tool.error = Some(json!(tool_error));
    let tape = ReplayTape::new(&data(vec![model, tool]))?;
    assert_eq!(
        tape.model()
            .generate(&context(), &GenerateParams::default())
            .await,
        Err(model_error)
    );
    assert_eq!(
        ToolExecutor::execute(&tape, "effect", &[definition()], &json!({"key":"original"})).await,
        Err(tool_error)
    );
    assert_eq!(tape.finish()?.calls, 2);
    Ok(())
}

struct Effect(Arc<AtomicUsize>);
#[async_trait]
impl Tool for Effect {
    fn definition(&self) -> ToolDefinition {
        definition()
    }
    async fn execute(&self, _: &Value) -> Result<Value, ToolError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"real_effect":true}))
    }
}

#[tokio::test]
async fn registry_replay_never_executes_original_effects() -> Result<(), Box<dyn std::error::Error>>
{
    let effects = Arc::new(AtomicUsize::new(0));
    let mut original = ToolRegistry::new();
    original.register(Arc::new(Effect(effects.clone())))?;
    let tape = ReplayTape::new(&data(vec![tool_call()]))?;
    let replay = tape.wrap_tools(&original)?;
    assert_eq!(
        replay
            .execute("effect", &[definition()], &json!({"key":"original"}))
            .await?,
        json!({"value":7})
    );
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    tape.finish()?;
    Ok(())
}

#[tokio::test]
async fn successful_null_tool_result_roundtrips() -> Result<(), Box<dyn std::error::Error>> {
    let mut call = tool_call();
    call.output = Some(Value::Null);
    let serialized = serde_json::to_string(&data(vec![call]))?;
    let loaded = serde_json::from_str(&serialized)?;
    let tape = ReplayTape::new(&loaded)?;
    assert_eq!(
        ToolExecutor::execute(&tape, "effect", &[definition()], &json!({"key":"original"})).await?,
        Value::Null
    );
    tape.finish()?;
    Ok(())
}

#[tokio::test]
async fn only_explicit_context_run_identity_is_normalized() -> Result<(), Box<dyn std::error::Error>>
{
    let tape = ReplayTape::with_run_id(&data(vec![model_call()]), "replayed")?;
    let mut changed = context();
    changed.id = "replayed/context".into();
    assert_eq!(
        tape.model()
            .generate(&changed, &GenerateParams::default())
            .await?,
        response()
    );
    tape.finish()?;
    let tape = ReplayTape::with_run_id(&data(vec![model_call()]), "replayed")?;
    changed.id = "replayed/other-context".into();
    assert!(
        tape.model()
            .generate(&changed, &GenerateParams::default())
            .await
            .is_err()
    );
    assert!(
        matches!(tape.check(), Err(ReplayError::Divergence { field, .. }) if field == "input.context.id")
    );
    Ok(())
}

#[tokio::test]
async fn stop_hook_latches_only_after_matching_call_and_preserves_outcome()
-> Result<(), Box<dyn std::error::Error>> {
    let tape = ReplayTape::new(&data(vec![model_call(), tool_call()]))?;
    let stop = Arc::new(AtomicBool::new(false));
    tape.stop_after("record-1", stop.clone())?;
    assert!(!stop.load(Ordering::Acquire));
    assert_eq!(
        tape.model()
            .generate(&context(), &GenerateParams::default())
            .await?,
        response()
    );
    assert!(!stop.load(Ordering::Acquire));
    assert_eq!(
        ToolExecutor::execute(&tape, "effect", &[definition()], &json!({"key":"original"})).await?,
        json!({"value":7})
    );
    assert!(stop.load(Ordering::Acquire));
    tape.finish()?;
    assert!(tape.stop_after("record-1", stop).is_err());
    Ok(())
}

#[tokio::test]
async fn stop_hook_preserves_original_error_and_ignores_divergent_calls()
-> Result<(), Box<dyn std::error::Error>> {
    let expected = ModelError {
        kind: "provider".into(),
        message: "original failure".into(),
    };
    let mut call = model_call();
    call.output = None;
    call.error = Some(json!(expected));
    let tape = ReplayTape::new(&data(vec![call]))?;
    let stop = Arc::new(AtomicBool::new(false));
    tape.stop_after("record-0", stop.clone())?;
    assert_eq!(
        tape.model()
            .generate(&context(), &GenerateParams::default())
            .await,
        Err(expected)
    );
    assert!(stop.load(Ordering::Acquire));
    tape.finish()?;

    let tape = ReplayTape::new(&data(vec![model_call()]))?;
    let stop = Arc::new(AtomicBool::new(false));
    assert!(tape.stop_after("missing", stop.clone()).is_err());
    tape.stop_after("record-0", stop.clone())?;
    assert!(tape.stop_after("record-0", stop.clone()).is_err());
    let mut changed = context();
    changed.version += 1;
    assert!(
        tape.model()
            .generate(&changed, &GenerateParams::default())
            .await
            .is_err()
    );
    assert!(!stop.load(Ordering::Acquire));
    assert!(tape.finish().is_err());
    Ok(())
}
