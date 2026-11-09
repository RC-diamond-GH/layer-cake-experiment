use serde_json::{Value, json};

use super::{compare, html};
use crate::context::{ChatMessage, LLMContext};
use crate::record::{CallRecord, ObjectRecord, Observation, RunData};

fn fixture() -> RunData {
    let mut context = LLMContext::new("context", "Complete system prompt");
    context.append(ChatMessage::User {
        content: "Complete user message".into(),
    });
    let call = CallRecord {
        call_kind: "model".into(),
        input: json!({"context":context,"params":{"model":"script","max_tokens":123}}),
        output: Some(
            json!({"message":{"role":"assistant","content":"Complete output","tool_calls":[]},"finish_reason":"Stop","usage":null}),
        ),
        error: None,
        elapsed_ms: 21,
        context_id: Some(context.id.clone()),
        context_version: Some(context.version),
        tool_call_id: None,
    };
    RunData {
        id: "run-1".into(),
        experiment: "example".into(),
        config: json!({"rounds":4}),
        objects: vec![ObjectRecord {
            id: "model".into(),
            kind: "model".into(),
            label: "Script model".into(),
        }],
        records: vec![Observation {
            id: "record-1".into(),
            sequence: 42,
            run_id: "run-1".into(),
            object_id: "model".into(),
            cause: Some("trigger-1".into()),
            timestamp_ms: 57,
            kind: "call".into(),
            data: json!(call),
        }],
        snapshots: vec![context],
        summary: json!({"status":"running"}),
    }
}

#[test]
fn report_contains_full_details_and_cutoff() -> Result<(), Box<dyn std::error::Error>> {
    let document = html(&fixture())?;
    for expected in [
        "sequence 42",
        "timestamp 57 ms",
        "running",
        "object-filter",
        "Complete system prompt",
        "Complete user message",
        "Complete output",
        "max_tokens",
        "123",
        "context-0",
        "version 1",
        "trigger-1",
        "Elapsed 21 ms",
        "Run metadata",
        "Registered objects",
    ] {
        assert!(document.contains(expected), "report missing {expected}");
    }
    assert!(!document.contains("<script src="));
    assert!(!document.contains("<link "));
    Ok(())
}

#[test]
fn every_run_controlled_surface_is_html_escaped() -> Result<(), Box<dyn std::error::Error>> {
    let attack = "</script><img src=x onerror=alert(1)>\"'&<script>alert(2)</script>";
    let mut data = fixture();
    data.id = attack.into();
    data.experiment = attack.into();
    data.config = json!({"payload":attack});
    data.summary = json!({"status":attack});
    data.objects[0].id = attack.into();
    data.objects[0].label = attack.into();
    data.records[0].id = attack.into();
    data.records[0].object_id = attack.into();
    data.records[0].cause = Some(attack.into());
    data.records[0].data["input"]["context"]["system_prompt"] = json!(attack);
    data.records[0].data["output"]["message"]["content"] = json!(attack);
    data.snapshots[0].system_prompt = attack.into();
    let document = html(&data)?;
    assert!(!document.contains(attack));
    assert!(!document.contains("<img"));
    assert!(!document.contains("<script>alert"));
    assert!(document.contains("&lt;/script&gt;&lt;img"));
    assert!(document.contains("&quot;&#39;&amp;"));
    assert_eq!(document.matches("<script>").count(), 1);
    assert_eq!(document.matches("</script>").count(), 1);
    assert!(!document.contains("innerHTML"));
    Ok(())
}

#[test]
fn comparison_reports_config_calls_and_real_context_sizes() -> Result<(), Box<dyn std::error::Error>>
{
    let left = fixture();
    let mut right = fixture();
    right.id = "run-2".into();
    right.config["rounds"] = json!(8);
    right.records[0].data["input"]["params"]["max_tokens"] = json!(456);
    right.records[0].data["error"] = json!({"kind":"test","message":"failure"});
    right.records[0]
        .data
        .as_object_mut()
        .ok_or("call must be object")?
        .remove("output");
    right.snapshots[0].append(ChatMessage::User {
        content: "additional material".into(),
    });
    let result = compare(&left, &right);
    assert_eq!(result["config"]["differences"][0]["field"], "config.rounds");
    assert_eq!(result["calls"]["left"]["model"], 1);
    assert_eq!(result["calls"]["right"]["errors"], 1);
    assert_eq!(result["calls"]["details"][0]["equal"], false);
    let differences = result["calls"]["details"][0]["differences"]
        .as_array()
        .ok_or("differences must be array")?;
    assert!(
        differences
            .iter()
            .any(|difference| difference["field"] == "call.input.params.max_tokens")
    );
    assert_eq!(
        result["context_size"]["left"]["snapshots"][0]["json_bytes"],
        json!(serde_json::to_vec(&left.snapshots[0])?.len())
    );
    assert!(
        result["context_size"]["right"]["total_snapshot_json_bytes"].as_u64()
            > result["context_size"]["left"]["total_snapshot_json_bytes"].as_u64()
    );
    assert_eq!(
        result["context_size"]["left"]["model_inputs"][0]["recorded_usage"],
        Value::Null
    );
    assert!(!result.to_string().contains("estimated_tokens"));
    Ok(())
}

#[test]
fn comparison_distinguishes_missing_values_from_null() {
    let mut left = fixture();
    let mut right = fixture();
    left.config = json!({"setting":null});
    right.config = json!({});
    let result = compare(&left, &right);
    assert_eq!(result["config"]["differences"][0]["left_present"], true);
    assert_eq!(result["config"]["differences"][0]["right_present"], false);
    right.records.clear();
    let result = compare(&left, &right);
    assert_eq!(
        result["calls"]["details"][0]["differences"][0]["right_present"],
        false
    );
}
