use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::context::LLMContext;
use crate::record::{Observation, RunData};

/// Compare saved facts. Byte counts are serialized UTF-8 sizes, never estimated tokens.
pub fn compare(left: &RunData, right: &RunData) -> Value {
    let left_calls: Vec<_> = left
        .records
        .iter()
        .filter(|record| record.kind == "call")
        .collect();
    let right_calls: Vec<_> = right
        .records
        .iter()
        .filter(|record| record.kind == "call")
        .collect();
    let mut calls = Vec::new();
    for index in 0..left_calls.len().max(right_calls.len()) {
        let before = left_calls.get(index).copied();
        let after = right_calls.get(index).copied();
        let mut differences = Vec::new();
        match (before, after) {
            (Some(before), Some(after)) => collect_differences(&before.data, &after.data, "call", &mut differences),
            (before, after) => differences.push(json!({"field":"call","left":before.map(|record|&record.data),"right":after.map(|record|&record.data),"left_present":before.is_some(),"right_present":after.is_some()})),
        }
        calls.push(json!({
            "index":index,
            "left":before.map(call_detail),
            "right":after.map(call_detail),
            "equal":differences.is_empty(),
            "differences":differences,
        }));
    }
    let mut config_diff = Vec::new();
    collect_differences(&left.config, &right.config, "config", &mut config_diff);
    let mut summary_diff = Vec::new();
    collect_differences(&left.summary, &right.summary, "summary", &mut summary_diff);
    json!({
        "left_run":left.id,
        "right_run":right.id,
        "experiments":{"left":left.experiment,"right":right.experiment},
        "config":{"left":left.config,"right":right.config,"differences":config_diff},
        "calls":{"left":call_counts(&left_calls),"right":call_counts(&right_calls),"details":calls},
        "context_size":{"unit":"serialized UTF-8 JSON bytes; not tokens","left":context_sizes(left),"right":context_sizes(right)},
        "summary":{"left":left.summary,"right":right.summary,"differences":summary_diff},
    })
}

fn call_counts(calls: &[&Observation]) -> Value {
    json!({
        "total":calls.len(),
        "model":calls.iter().filter(|call|call.data.get("call_kind").and_then(Value::as_str)==Some("model")).count(),
        "tool":calls.iter().filter(|call|call.data.get("call_kind").and_then(Value::as_str)==Some("tool")).count(),
        "other_or_invalid":calls.iter().filter(|call|!matches!(call.data.get("call_kind").and_then(Value::as_str),Some("model"|"tool"))).count(),
        "errors":calls.iter().filter(|call|call.data.get("error").is_some_and(|error|!error.is_null())).count(),
    })
}

fn call_detail(record: &Observation) -> Value {
    json!({
        "record_id":record.id,
        "sequence":record.sequence,
        "object_id":record.object_id,
        "cause":record.cause,
        "timestamp_ms":record.timestamp_ms,
        "call":record.data,
        "model_input_context_size":record.data.pointer("/input/context").map(value_size),
    })
}

fn value_size(value: &Value) -> Value {
    match serde_json::to_vec(value) {
        Ok(bytes) => json!({"json_bytes":bytes.len()}),
        Err(error) => json!({"serialization_error":error.to_string()}),
    }
}

fn snapshot_size(context: &LLMContext) -> Value {
    let bytes = match serde_json::to_vec(context) {
        Ok(bytes) => json!(bytes.len()),
        Err(error) => {
            return json!({"context_id":context.id,"version":context.version,"serialization_error":error.to_string()});
        }
    };
    json!({
        "context_id":context.id,
        "version":context.version,
        "json_bytes":bytes,
        "messages":context.messages.len(),
        "tools":context.tools.len(),
        "global_prompts":context.global_prompt.len(),
        "system_prompt_bytes":context.system_prompt.len(),
        "global_prompt_content_bytes":context.global_prompt.iter().map(|prompt|prompt.content.len()).sum::<usize>(),
    })
}

fn context_sizes(data: &RunData) -> Value {
    let snapshots: Vec<_> = data.snapshots.iter().map(snapshot_size).collect();
    let sizes: Vec<_> = snapshots
        .iter()
        .filter_map(|snapshot| snapshot.get("json_bytes").and_then(Value::as_u64))
        .collect();
    let model_contexts: Vec<_> = data.records.iter().filter(|record|record.kind=="call" && record.data.get("call_kind").and_then(Value::as_str)==Some("model"))
        .map(|record|json!({"record_id":record.id,"context_id":record.data.get("context_id"),"context_version":record.data.get("context_version"),"size":record.data.pointer("/input/context").map(value_size),"recorded_usage":record.data.pointer("/output/usage")})).collect();
    json!({
        "snapshot_count":snapshots.len(),
        "total_snapshot_json_bytes":sizes.iter().sum::<u64>(),
        "max_snapshot_json_bytes":sizes.iter().max(),
        "snapshots":snapshots,
        "model_inputs":model_contexts,
    })
}

fn collect_differences(left: &Value, right: &Value, field: &str, out: &mut Vec<Value>) {
    if left == right {
        return;
    }
    match (left, right) {
        (Value::Object(left), Value::Object(right)) => {
            let keys: BTreeSet<_> = left.keys().chain(right.keys()).collect();
            for key in keys {
                let path = format!("{field}.{key}");
                match (left.get(key), right.get(key)) {
                    (Some(left), Some(right)) => collect_differences(left, right, &path, out),
                    (left, right) => out.push(json!({"field":path,"left":left,"right":right,"left_present":left.is_some(),"right_present":right.is_some()})),
                }
            }
        }
        (Value::Array(left), Value::Array(right)) => {
            for index in 0..left.len().max(right.len()) {
                let path = format!("{field}[{index}]");
                match (left.get(index), right.get(index)) {
                    (Some(left), Some(right)) => collect_differences(left, right, &path, out),
                    (left, right) => out.push(json!({"field":path,"left":left,"right":right,"left_present":left.is_some(),"right_present":right.is_some()})),
                }
            }
        }
        _ => out.push(json!({"field":field,"left":left,"right":right,"left_present":true,"right_present":true})),
    }
}
