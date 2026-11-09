use super::{ControlCommand, ExperimentConfig, assembly::assemble_internal};
use crate::record::RunData;
use crate::replay::{ReplayError, ReplaySummary, ReplayTape};

/// Reconstruct the original deterministic controller, replacing every model/tool call.
/// Applied interactive inputs and stops are reintroduced at their recorded boundaries.
pub async fn replay(data: &RunData) -> Result<ReplaySummary, ReplayError> {
    let config: ExperimentConfig = serde_json::from_value(data.config.clone())
        .map_err(|e| ReplayError::Runtime(format!("saved experiment config: {e}")))?;
    let tape = ReplayTape::new(data)?;
    let mut experiment = assemble_internal(config, data.id.clone(), Some(&tape))
        .map_err(|e| ReplayError::Runtime(e.to_string()))?;
    let target = data
        .summary
        .get("boundary")
        .and_then(|n| n.as_u64())
        .ok_or_else(|| ReplayError::Runtime("saved summary has no boundary cutoff".into()))?;
    let signal = experiment.stop_signal();
    let mut stop_before = std::collections::BTreeSet::new();
    let mut hooked_calls = std::collections::BTreeSet::new();
    for request in data.records.iter().filter(|r| {
        r.object_id == data.id
            && r.kind == "control_requested"
            && r.data.get("effective").and_then(|v| v.as_str()) == Some("before_next_dispatch")
    }) {
        if let Some(call) = data
            .records
            .iter()
            .find(|r| r.sequence > request.sequence && r.kind == "call")
        {
            if hooked_calls.insert(call.id.as_str()) {
                tape.stop_after(&call.id, signal.clone())?;
            }
        } else {
            let boundary = data
                .records
                .iter()
                .find(|r| r.kind == "control" && r.cause.as_deref() == Some(request.id.as_str()))
                .and_then(|r| r.data.get("boundary"))
                .and_then(|v| v.as_u64())
                .ok_or_else(|| {
                    ReplayError::Runtime(format!(
                        "stop request {} has no acknowledged boundary",
                        request.id
                    ))
                })?;
            stop_before.insert(boundary);
        }
    }
    let mut controls = data
        .records
        .iter()
        .filter(|r| r.kind == "control" && r.object_id == data.id)
        .peekable();
    loop {
        while let Some(record) = controls.peek() {
            let boundary = record
                .data
                .get("boundary")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| {
                    ReplayError::Runtime(format!("control {} has no boundary", record.id))
                })?;
            if boundary > experiment.boundaries {
                break;
            }
            let Some(record) = controls.next() else {
                return Err(ReplayError::Runtime(
                    "control cursor lost its boundary record".into(),
                ));
            };
            if record.data.get("accepted").and_then(|v| v.as_bool()) != Some(true) {
                continue;
            }
            let command: ControlCommand = serde_json::from_value(record.data["command"].clone())
                .map_err(|e| ReplayError::Runtime(format!("control {}: {e}", record.id)))?;
            match command {
                ControlCommand::Input(value) => {
                    let applied = experiment
                        .log
                        .observe(
                            &data.id,
                            "replay_control",
                            None,
                            serde_json::json!({"source_record":record.id,"boundary":boundary}),
                        )
                        .map_err(|e| ReplayError::Runtime(e.to_string()))?;
                    experiment
                        .input(value, Some(&applied))
                        .map_err(|e| ReplayError::Runtime(format!("control {}: {e}", record.id)))?;
                }
                ControlCommand::Stop | ControlCommand::Shutdown => {
                    if !experiment.finished {
                        experiment
                            .stop()
                            .map_err(|e| ReplayError::Runtime(e.to_string()))?;
                    }
                }
                _ => {}
            }
        }
        if experiment.boundaries >= target || experiment.finished {
            break;
        }
        if stop_before.contains(&(experiment.boundaries + 1)) {
            signal.store(true, std::sync::atomic::Ordering::Release);
        }
        let result = experiment.step().await;
        tape.check()?;
        // A recorded model/tool failure is replayed as that same terminal failure.
        // Request mismatches are sticky in the tape and checked before accepting it.
        if result.is_err() {
            break;
        }
    }
    tape.check()?;
    if experiment.boundaries != target {
        return Err(ReplayError::Runtime(format!(
            "boundary divergence: expected {target}, actual {}",
            experiment.boundaries
        )));
    }
    tape.finish()
}
