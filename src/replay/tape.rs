use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{ReplayError, ReplaySummary};
use crate::context::{LLMContext, ToolDefinition};
use crate::model::{GenerateParams, GenerateResponse, ModelClient, ModelError};
use crate::record::{CallRecord, RunData};
use crate::tool::{Tool, ToolError, ToolExecutor, ToolRegistry};

#[derive(Clone)]
struct Entry {
    id: String,
    call: CallRecord,
}

struct State {
    calls: VecDeque<Entry>,
    next: usize,
    model_calls: usize,
    tool_calls: usize,
    failure: Option<ReplayError>,
    stop_hooks: BTreeMap<String, Arc<AtomicBool>>,
}

/// One shared cursor orders model and tool calls together. A failed match is sticky.
#[derive(Clone)]
pub struct ReplayTape {
    state: Arc<Mutex<State>>,
    original_run_id: String,
    replay_run_id: String,
}

impl ReplayTape {
    pub fn new(data: &RunData) -> Result<Self, ReplayError> {
        Self::with_run_id(data, &data.id)
    }

    /// Only the explicit run-id prefix of context identities may differ during replay.
    /// Context versions, messages, tool definitions and generation parameters remain exact.
    pub fn with_run_id(data: &RunData, replay_run_id: &str) -> Result<Self, ReplayError> {
        let mut calls = VecDeque::new();
        let mut previous = None;
        let mut ids = BTreeSet::new();
        for record in &data.records {
            if previous.is_some_and(|sequence| record.sequence <= sequence) {
                return Err(invalid(
                    &record.id,
                    "observation sequence is not strictly increasing",
                ));
            }
            previous = Some(record.sequence);
            if !ids.insert(&record.id) {
                return Err(invalid(&record.id, "duplicate observation identity"));
            }
            if record.kind != "call" {
                continue;
            }
            let call: CallRecord = serde_json::from_value(record.data.clone())
                .map_err(|error| invalid(&record.id, &error.to_string()))?;
            if !matches!(call.call_kind.as_str(), "model" | "tool") {
                return Err(invalid(&record.id, "unsupported call kind"));
            }
            if call.output.is_some() == call.error.is_some() {
                return Err(invalid(
                    &record.id,
                    "call requires exactly one output or error",
                ));
            }
            if call.call_kind == "model" {
                let context: LLMContext = decode_input(&record.id, &call.input, "context")?;
                let _: GenerateParams = decode_input(&record.id, &call.input, "params")?;
                if call.context_id.as_deref() != Some(context.id.as_str())
                    || call.context_version != Some(context.version)
                {
                    return Err(invalid(
                        &record.id,
                        "context metadata differs from model request",
                    ));
                }
                if let Some(output) = &call.output {
                    serde_json::from_value::<GenerateResponse>(output.clone())
                        .map_err(|error| invalid(&record.id, &error.to_string()))?;
                }
                if let Some(error) = &call.error {
                    serde_json::from_value::<ModelError>(error.clone())
                        .map_err(|error| invalid(&record.id, &error.to_string()))?;
                }
            } else {
                let _: String = decode_input(&record.id, &call.input, "name")?;
                let _: Vec<ToolDefinition> = decode_input(&record.id, &call.input, "visible")?;
                if call.input.get("arguments").is_none() {
                    return Err(invalid(&record.id, "tool request lacks arguments"));
                }
                if let Some(error) = &call.error {
                    serde_json::from_value::<ToolError>(error.clone())
                        .map_err(|error| invalid(&record.id, &error.to_string()))?;
                }
            }
            calls.push_back(Entry {
                id: record.id.clone(),
                call,
            });
        }
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                calls,
                next: 0,
                model_calls: 0,
                tool_calls: 0,
                failure: None,
                stop_hooks: BTreeMap::new(),
            })),
            original_run_id: data.id.clone(),
            replay_run_id: replay_run_id.to_owned(),
        })
    }

    pub fn model(&self) -> Arc<dyn ModelClient> {
        Arc::new(ReplayModel { tape: self.clone() })
    }

    /// Reproduce a stop requested while this recorded call was in flight.
    /// The recorded outcome is returned normally; the next dispatch sees the stop signal.
    pub fn stop_after(&self, record_id: &str, signal: Arc<AtomicBool>) -> Result<(), ReplayError> {
        let mut state = self.lock()?;
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        if !state.calls.iter().any(|entry| entry.id == record_id) {
            return Err(invalid(
                record_id,
                "stop hook requires an unconsumed call record",
            ));
        }
        if state.stop_hooks.contains_key(record_id) {
            return Err(invalid(record_id, "stop hook already registered"));
        }
        state.stop_hooks.insert(record_id.to_owned(), signal);
        Ok(())
    }

    /// Keep model-visible definitions, but replace every execution (including invalid dispatches).
    pub fn wrap_tools(&self, original: &ToolRegistry) -> Result<ToolRegistry, ReplayError> {
        let mut registry = ToolRegistry::with_executor(Arc::new(self.clone()));
        let visible = Arc::new(original.definitions());
        for definition in visible.iter() {
            registry
                .register(Arc::new(ReplayTool {
                    tape: self.clone(),
                    definition: definition.clone(),
                    visible: visible.clone(),
                }))
                .map_err(|error| ReplayError::Runtime(error.to_string()))?;
        }
        Ok(registry)
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>, ReplayError> {
        self.state.lock().map_err(|_| ReplayError::Poisoned)
    }

    pub fn check(&self) -> Result<(), ReplayError> {
        let state = self.lock()?;
        match &state.failure {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn consume(&self, kind: &str, input: Value) -> Result<Entry, ReplayError> {
        let mut state = self.lock()?;
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        let Some(entry) = state.calls.front() else {
            let error = ReplayError::Divergence {
                record_id: "<end-of-tape>".into(),
                field: "call".into(),
                expected: json!("end of tape"),
                actual: json!({"call_kind":kind,"input":input}),
            };
            state.failure = Some(error.clone());
            return Err(error);
        };
        let mismatch = if entry.call.call_kind != kind {
            Some(("call_kind".into(), json!(entry.call.call_kind), json!(kind)))
        } else if kind == "model" && self.original_run_id != self.replay_run_id {
            let mut expected = entry.call.input.clone();
            let mut actual = input;
            normalize_context_id(&mut expected, &self.original_run_id);
            normalize_context_id(&mut actual, &self.replay_run_id);
            difference(&expected, &actual, "input")
        } else {
            difference(&entry.call.input, &input, "input")
        };
        if let Some((field, expected, actual)) = mismatch {
            let error = ReplayError::Divergence {
                record_id: entry.id.clone(),
                field,
                expected,
                actual,
            };
            state.failure = Some(error.clone());
            return Err(error);
        }
        state.next += 1;
        if kind == "model" {
            state.model_calls += 1;
        } else {
            state.tool_calls += 1;
        }
        let entry = state
            .calls
            .pop_front()
            .ok_or_else(|| ReplayError::Runtime("replay cursor lost its matched call".into()))?;
        if let Some(signal) = state.stop_hooks.remove(&entry.id) {
            signal.store(true, Ordering::Release);
        }
        Ok(entry)
    }

    pub fn finish(&self) -> Result<ReplaySummary, ReplayError> {
        let mut state = self.lock()?;
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        if let Some(entry) = state.calls.front() {
            let error = ReplayError::Divergence {
                record_id: entry.id.clone(),
                field: "call".into(),
                expected: json!({"call_kind":entry.call.call_kind,"input":entry.call.input}),
                actual: json!("end of execution"),
            };
            state.failure = Some(error.clone());
            return Err(error);
        }
        Ok(ReplaySummary {
            run_id: self.original_run_id.clone(),
            calls: state.next,
            model_calls: state.model_calls,
            tool_calls: state.tool_calls,
        })
    }
}

pub struct ReplayModel {
    tape: ReplayTape,
}

#[async_trait]
impl ModelClient for ReplayModel {
    async fn generate(
        &self,
        context: &LLMContext,
        params: &GenerateParams,
    ) -> Result<GenerateResponse, ModelError> {
        let entry = self
            .tape
            .consume("model", json!({"context":context,"params":params}))
            .map_err(model_failure)?;
        match (entry.call.output, entry.call.error) {
            (Some(output), None) => serde_json::from_value(output)
                .map_err(|error| model_failure(invalid(&entry.id, &error.to_string()))),
            (None, Some(error)) => Err(serde_json::from_value(error)
                .map_err(|error| model_failure(invalid(&entry.id, &error.to_string())))?),
            _ => Err(model_failure(invalid(&entry.id, "invalid model outcome"))),
        }
    }
}

/// A definition-preserving adapter. Registry-level interception supplies the actual visible set.
pub struct ReplayTool {
    tape: ReplayTape,
    definition: ToolDefinition,
    visible: Arc<Vec<ToolDefinition>>,
}

#[async_trait]
impl Tool for ReplayTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    async fn execute(&self, arguments: &Value) -> Result<Value, ToolError> {
        ToolExecutor::execute(&self.tape, &self.definition.name, &self.visible, arguments).await
    }
}

#[async_trait]
impl ToolExecutor for ReplayTape {
    async fn execute(
        &self,
        name: &str,
        visible: &[ToolDefinition],
        arguments: &Value,
    ) -> Result<Value, ToolError> {
        let entry = self
            .consume(
                "tool",
                json!({"name":name,"arguments":arguments,"visible":visible}),
            )
            .map_err(tool_failure)?;
        match (entry.call.output, entry.call.error) {
            (Some(output), None) => Ok(output),
            (None, Some(error)) => Err(serde_json::from_value(error)
                .map_err(|error| tool_failure(invalid(&entry.id, &error.to_string())))?),
            _ => Err(tool_failure(invalid(&entry.id, "invalid tool outcome"))),
        }
    }
}

fn invalid(record_id: &str, message: &str) -> ReplayError {
    ReplayError::InvalidRecord {
        record_id: record_id.into(),
        message: message.into(),
    }
}

fn decode_input<T: serde::de::DeserializeOwned>(
    record_id: &str,
    input: &Value,
    field: &str,
) -> Result<T, ReplayError> {
    let value = input
        .get(field)
        .ok_or_else(|| invalid(record_id, &format!("request lacks {field}")))?;
    serde_json::from_value(value.clone()).map_err(|error| invalid(record_id, &error.to_string()))
}

fn model_failure(error: ReplayError) -> ModelError {
    ModelError {
        kind: "replay_divergence".into(),
        message: error.to_string(),
    }
}

fn tool_failure(error: ReplayError) -> ToolError {
    ToolError {
        kind: "replay_divergence".into(),
        message: error.to_string(),
    }
}

fn normalize_context_id(input: &mut Value, run_id: &str) {
    if let Some(id) = input.pointer_mut("/context/id") {
        if let Some(text) = id.as_str() {
            if text == run_id {
                *id = json!("$run");
            } else if let Some(suffix) = text
                .strip_prefix(run_id)
                .filter(|suffix| suffix.starts_with('/'))
            {
                *id = json!(format!("$run{suffix}"));
            }
        }
    }
}

pub(super) fn difference(
    expected: &Value,
    actual: &Value,
    field: &str,
) -> Option<(String, Value, Value)> {
    if expected == actual {
        return None;
    }
    match (expected, actual) {
        (Value::Object(left), Value::Object(right)) => {
            let keys: BTreeSet<_> = left.keys().chain(right.keys()).collect();
            for key in keys {
                let path = format!("{field}.{key}");
                match (left.get(key), right.get(key)) {
                    (Some(left), Some(right)) => {
                        if let Some(result) = difference(left, right, &path) {
                            return Some(result);
                        }
                    }
                    (left, right) => {
                        return Some((
                            path,
                            left.cloned().unwrap_or_else(missing),
                            right.cloned().unwrap_or_else(missing),
                        ));
                    }
                }
            }
            None
        }
        (Value::Array(left), Value::Array(right)) => {
            for index in 0..left.len().max(right.len()) {
                let path = format!("{field}[{index}]");
                match (left.get(index), right.get(index)) {
                    (Some(left), Some(right)) => {
                        if let Some(result) = difference(left, right, &path) {
                            return Some(result);
                        }
                    }
                    (left, right) => {
                        return Some((
                            path,
                            left.cloned().unwrap_or_else(missing),
                            right.cloned().unwrap_or_else(missing),
                        ));
                    }
                }
            }
            None
        }
        _ => Some((field.into(), expected.clone(), actual.clone())),
    }
}

fn missing() -> Value {
    json!({"$replay":"missing field"})
}
