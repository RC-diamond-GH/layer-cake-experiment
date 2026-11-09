use layer_cake::experiment::{ControlCommand, ExperimentConfig, assemble};
use serde_json::json;

#[tokio::test]
async fn replay_preserves_boundary_input_and_stop_without_extra_calls() {
    let id = "controlled-run".to_owned();
    let mut run = assemble(ExperimentConfig::default(), id.clone()).expect("assemble");
    assert!(!run.step().await.expect("read round"));
    let input = json!("Keep the requested counter value.");
    let request = run
        .log
        .observe(
            &id,
            "control_requested",
            None,
            json!({"command":ControlCommand::Input(input.clone())}),
        )
        .expect("request");
    run.input(input.clone(), Some(&request)).expect("input");
    run.log
        .observe(
            &id,
            "control",
            Some(&request),
            json!({"command":ControlCommand::Input(input),"boundary":1,"accepted":true}),
        )
        .expect("input ack");
    assert!(!run.step().await.expect("write round"));
    run.stop().expect("stop");
    run.log
        .observe(
            &id,
            "control",
            None,
            json!({"command":ControlCommand::Stop,"boundary":2,"accepted":true}),
        )
        .expect("stop ack");
    let saved = run.log.data().expect("data");
    assert_eq!(saved.records.iter().filter(|r| r.kind == "call").count(), 4);
    assert!(run.step().await.expect("terminal step"));
    assert_eq!(run.boundaries, 2);
    let summary = layer_cake::replay::verify(&saved)
        .await
        .expect("exact controlled replay");
    assert_eq!(summary.model_calls, 2);
    assert_eq!(summary.tool_calls, 2);
}

#[tokio::test]
async fn exhausted_round_budget_replays_completed_tool_results() {
    let mut config = ExperimentConfig::default();
    config.max_rounds = 1;
    let mut run = assemble(config, "limited-run".into()).expect("assemble");
    assert!(run.step().await.expect("one round exhausts budget"));
    assert!(!run.normal_finish());
    let data = run.log.data().expect("data");
    match data
        .snapshots
        .last()
        .expect("final context")
        .messages
        .last()
    {
        Some(layer_cake::context::ChatMessage::Tool {
            tool_call_id,
            content,
        }) => {
            assert_eq!(tool_call_id, "read-counter");
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(content).expect("tool JSON"),
                json!(0)
            );
        }
        other => panic!("exhaustion must preserve the completed read result, got {other:?}"),
    }
    let summary = layer_cake::replay::verify(&data)
        .await
        .expect("exhausted replay");
    assert_eq!(summary.model_calls, 1);
    assert_eq!(summary.tool_calls, 1);
}
