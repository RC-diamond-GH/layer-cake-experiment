use std::error::Error;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::context::ChatMessage;
use crate::model::{FinishReason, GenerateResponse, ScriptModel, ScriptStep};
use crate::record::{RunData, RunLog};
use crate::replay::ReplayTape;
use crate::tool::ToolError;

use super::{ReactiveExperiment, RegistryWrapper, ScenarioConfig, assemble, assemble_with};

async fn finish(experiment: &mut ReactiveExperiment) -> Result<(), Box<dyn Error>> {
    for _ in 0..300 {
        if experiment.step().await? {
            return Ok(());
        }
    }
    Err("scenario failed to terminate within its deterministic test budget".into())
}

fn log(id: &str, config: &ScenarioConfig) -> RunLog {
    RunLog::new(id.into(), "furnace-reactive".into(), json!(config))
}

fn observation<'a>(
    data: &'a RunData,
    kind: &str,
) -> Result<&'a crate::record::Observation, Box<dyn Error>> {
    data.records
        .iter()
        .find(|record| record.kind == kind)
        .ok_or_else(|| format!("missing {kind} observation").into())
}

#[tokio::test]
async fn default_furnace_reactive_and_polling_finish_from_actual_tools()
-> Result<(), Box<dyn Error>> {
    for polling in [false, true] {
        let config = ScenarioConfig::default();
        let run_id = if polling { "polling" } else { "reactive" };
        let log = log(run_id, &config);
        let mut experiment = assemble(run_id.into(), config, log.clone(), polling, false)?;
        finish(&mut experiment).await?;
        let data = log.data()?;
        assert_eq!(data.summary["finished"], true);
        assert_eq!(data.summary["collected"], 1);
        assert_eq!(data.summary["resource"]["raw"]["value"], 3);
        assert_eq!(data.summary["resource"]["fuel"]["value"], 3);
        assert_eq!(data.summary["resource"]["product"]["value"], 0);
        assert_eq!(data.summary["resource"]["version"], 3);
        assert_eq!(data.summary["clock"], 3);
        assert_eq!(data.summary["rounds"], if polling { 6 } else { 4 });
        assert_eq!(data.summary["deliveries"], if polling { 0 } else { 1 });
        let calls = data
            .records
            .iter()
            .filter(|record| record.kind == "call" && record.data["call_kind"] == "tool")
            .collect::<Vec<_>>();
        assert_eq!(
            calls.first().map(|record| &record.data["input"]["name"]),
            Some(&json!("furnace_start"))
        );
        assert_eq!(
            calls.last().map(|record| &record.data["input"]["name"]),
            Some(&json!("furnace_take"))
        );
        assert!(experiment.context().messages.iter().any(|message| matches!(message, ChatMessage::Tool { content, .. } if content.contains("\"collected\":1"))));
        let before = log.data()?.records.len();
        assert!(experiment.step().await?);
        assert_eq!(log.data()?.records.len(), before);
    }
    Ok(())
}

#[tokio::test]
async fn all_flags_and_timer_source_reuse_same_delivery_mechanism() -> Result<(), Box<dyn Error>> {
    for timer in [false, true] {
        for polling in [false, true] {
            for flags in 0..8 {
                let config = ScenarioConfig {
                    duration: 2,
                    completion_before_subscribe: flags & 1 != 0,
                    duplicate_notification: flags & 2 != 0,
                    mutate_while_waiting: flags & 4 != 0,
                };
                let run_id = format!("matrix-{timer}-{polling}-{flags}");
                let log = log(&run_id, &config);
                let mut experiment = assemble(run_id, config.clone(), log.clone(), polling, timer)?;
                finish(&mut experiment).await?;
                let data = log.data()?;
                assert_eq!(data.summary["finished"], true);
                assert_eq!(
                    data.summary["collected"],
                    if timer {
                        0
                    } else if config.mutate_while_waiting {
                        3
                    } else {
                        1
                    }
                );
                assert_eq!(
                    data.summary["duplicates_suppressed"],
                    if config.duplicate_notification { 1 } else { 0 }
                );
                assert_eq!(data.summary["events"], 1);
                assert_eq!(data.summary["deliveries"], if polling { 0 } else { 1 });
                assert_eq!(
                    data.summary["mutated_while_waiting"],
                    config.mutate_while_waiting
                );
                assert_eq!(
                    data.summary["rounds"],
                    if polling && !config.completion_before_subscribe {
                        5
                    } else {
                        4
                    }
                );
                if config.completion_before_subscribe && !polling {
                    assert!(
                        observation(&data, "event_published")?.sequence
                            < observation(&data, "subscribe")?.sequence
                    );
                }
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn causal_chain_links_event_subscription_queue_refresh_and_model()
-> Result<(), Box<dyn Error>> {
    let config = ScenarioConfig {
        completion_before_subscribe: true,
        duplicate_notification: true,
        mutate_while_waiting: true,
        ..ScenarioConfig::default()
    };
    let log = log("causal", &config);
    let mut experiment = assemble("causal".into(), config, log.clone(), false, false)?;
    finish(&mut experiment).await?;
    let data = log.data()?;
    let published = observation(&data, "event_published")?;
    let subscribe = observation(&data, "subscribe")?;
    let matching = observation(&data, "event_match")?;
    let queued = observation(&data, "event_queued")?;
    let delivered = observation(&data, "event_delivered")?;
    let decision = observation(&data, "wake_decision")?;
    let refreshed = observation(&data, "context_refreshed")?;
    assert_eq!(matching.cause.as_deref(), Some(published.id.as_str()));
    assert_eq!(matching.data["subscription_record"], subscribe.id);
    assert_eq!(queued.cause.as_deref(), Some(matching.id.as_str()));
    assert_eq!(delivered.cause.as_deref(), Some(queued.id.as_str()));
    assert_eq!(decision.cause.as_deref(), Some(delivered.id.as_str()));
    assert_eq!(refreshed.cause.as_deref(), Some(decision.id.as_str()));
    let generation = data
        .records
        .iter()
        .find(|record| {
            record.kind == "call"
                && record.data["call_kind"] == "model"
                && record.cause.as_deref() == Some(refreshed.id.as_str())
        })
        .ok_or("missing event-triggered model generation")?;
    assert_eq!(
        generation.data["context_version"],
        refreshed.data["context_version"]
    );
    assert_eq!(published.data["event"]["payload"]["resource"]["version"], 2);
    assert_eq!(refreshed.data["resource_version"], 3);
    assert!(
        data.snapshots
            .iter()
            .any(|context| context.id == experiment.context().id
                && json!(context.version) == generation.data["context_version"])
    );
    assert_eq!(data.summary["collected"], 3);
    for record in &data.records {
        assert!(
            data.objects
                .iter()
                .any(|object| object.id == record.object_id)
        );
        if let Some(cause) = &record.cause {
            assert!(data.records.iter().any(|candidate| candidate.id == *cause && candidate.sequence < record.sequence));
        }
    }
    Ok(())
}

#[tokio::test]
async fn deterministic_inputs_control_clock_resources_and_cancellation()
-> Result<(), Box<dyn Error>> {
    let config = ScenarioConfig {
        duration: 20,
        ..ScenarioConfig::default()
    };
    let log = log("controlled", &config);
    let mut experiment = assemble("controlled".into(), config, log.clone(), false, false)?;
    assert!(!experiment.step().await?);
    assert_eq!(experiment.state()["clock"], 0);
    assert_eq!(experiment.state()["boundary"], "subscribe");
    assert!(!experiment.step().await?);
    assert_eq!(experiment.state()["clock"], 0);
    experiment.input(json!({"kind":"tick","ticks":19}))?;
    experiment.input(json!({"kind":"mutate","product":5}))?;
    assert_eq!(experiment.state()["clock"], 19);
    finish(&mut experiment).await?;
    assert_eq!(log.data()?.summary["collected"], 6);
    assert_eq!(log.data()?.summary["clock"], 20);

    let config = ScenarioConfig::default();
    let cancelled_log = self::log("cancelled", &config);
    let mut cancelled = assemble(
        "cancelled".into(),
        config,
        cancelled_log.clone(),
        false,
        false,
    )?;
    cancelled.step().await?;
    cancelled.step().await?;
    cancelled.input(json!({"kind":"cancel"}))?;
    assert!(cancelled.step().await?);
    assert_eq!(cancelled.state()["boundary"], "stopped");
    assert_eq!(cancelled.state()["queue_len"], 0);
    assert_eq!(cancelled_log.data()?.summary["status"], "cancelled");
    assert!(cancelled.input(Value::String("resume".into())).is_err());
    Ok(())
}

#[tokio::test]
async fn strict_replay_restores_deterministic_environment_from_tool_results()
-> Result<(), Box<dyn Error>> {
    for polling in [false, true] {
        for timer in [false, true] {
            let config = ScenarioConfig {
                completion_before_subscribe: true,
                duplicate_notification: true,
                mutate_while_waiting: true,
                ..ScenarioConfig::default()
            };
            let run_id = format!("replay-{polling}-{timer}");
            let original_log = log(&run_id, &config);
            let mut original = assemble(
                run_id.clone(),
                config.clone(),
                original_log.clone(),
                polling,
                timer,
            )?;
            finish(&mut original).await?;
            let data = original_log.data()?;
            let tape = ReplayTape::new(&data)?;
            let tool_tape = tape.clone();
            let wrapper: RegistryWrapper = Arc::new(move |registry| {
                tool_tape
                    .wrap_tools(&registry)
                    .map_err(|error| ToolError::new("replay", error.to_string()))
            });
            let replay_log = log(&run_id, &config);
            let mut replay = assemble_with(
                run_id,
                config,
                replay_log.clone(),
                polling,
                timer,
                Some(tape.model()),
                Some(wrapper),
            )?;
            finish(&mut replay).await?;
            assert!(tape.finish()?.calls > 0);
            assert_eq!(replay_log.data()?.summary, data.summary);
            assert_eq!(replay.context(), original.context());
        }
    }
    Ok(())
}

#[tokio::test]
async fn truncation_is_a_terminal_failure_not_a_successful_scenario() -> Result<(), Box<dyn Error>>
{
    let config = ScenarioConfig::default();
    let log = log("truncated", &config);
    let model = ScriptModel::new(vec![ScriptStep {
        expected_last_user: None,
        response: Ok(GenerateResponse {
            message: ChatMessage::Assistant {
                content: Some("unfinished".into()),
                tool_calls: Vec::new(),
            },
            finish_reason: FinishReason::Length,
            usage: None,
        }),
    }]);
    let mut experiment = assemble_with(
        "truncated".into(),
        config,
        log.clone(),
        false,
        false,
        Some(Arc::new(model)),
        None,
    )?;
    assert!(experiment.step().await.is_err());
    assert_eq!(experiment.state()["boundary"], "failed");
    assert_eq!(log.data()?.summary["status"], "failed");
    let calls = log
        .data()?
        .records
        .iter()
        .filter(|record| record.kind == "call")
        .count();
    assert!(experiment.step().await.is_err());
    assert_eq!(
        log.data()?
            .records
            .iter()
            .filter(|record| record.kind == "call")
            .count(),
        calls
    );
    Ok(())
}

#[tokio::test]
async fn stop_signal_prevents_environment_boundary_dispatch() -> Result<(), Box<dyn Error>> {
    let config = ScenarioConfig::default();
    let log = log("signal", &config);
    let mut experiment = assemble("signal".into(), config, log.clone(), false, false)?;
    experiment.step().await?;
    experiment.step().await?;
    assert_eq!(experiment.state()["boundary"], "wait");
    let clock = experiment.state()["clock"].clone();
    experiment
        .stop_signal()
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(experiment.step().await?);
    assert_eq!(experiment.state()["boundary"], "stopped");
    assert_eq!(experiment.state()["clock"], clock);
    assert!(!observation(&log.data()?, "cancel")?.data["purged"].is_null());
    Ok(())
}

#[tokio::test]
async fn stop_during_generation_retains_model_result_and_does_not_start_job()
-> Result<(), Box<dyn Error>> {
    struct WaitingModel {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }
    #[async_trait::async_trait]
    impl crate::model::ModelClient for WaitingModel {
        async fn generate(
            &self,
            _: &crate::context::LLMContext,
            _: &crate::model::GenerateParams,
        ) -> Result<GenerateResponse, crate::model::ModelError> {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(GenerateResponse {
                message: ChatMessage::Assistant {
                    content: None,
                    tool_calls: vec![crate::context::ToolCall {
                        id: "start".into(),
                        name: "furnace_start".into(),
                        arguments: json!({}),
                    }],
                },
                finish_reason: FinishReason::ToolCalls,
                usage: None,
            })
        }
    }
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let config = ScenarioConfig::default();
    let log = log("inflight-stop", &config);
    let mut experiment = assemble_with(
        "inflight-stop".into(),
        config,
        log.clone(),
        false,
        false,
        Some(Arc::new(WaitingModel {
            entered: entered.clone(),
            release: release.clone(),
        })),
        None,
    )?;
    let signal = experiment.stop_signal();
    let future = tokio::spawn(async move {
        let result = experiment.step().await;
        (experiment, result)
    });
    entered.notified().await;
    signal.store(true, std::sync::atomic::Ordering::SeqCst);
    release.notify_one();
    let (experiment, result) = future.await?;
    assert!(result?);
    assert_eq!(experiment.state()["boundary"], "stopped");
    assert!(experiment.state()["environment"]["job"].is_null());
    let data = log.data()?;
    let calls: Vec<_> = data
        .records
        .iter()
        .filter(|record| record.kind == "call")
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].data["call_kind"], "model");
    assert!(!calls[0].data["output"].is_null());
    Ok(())
}
