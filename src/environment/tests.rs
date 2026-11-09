use serde_json::json;

use super::{Clock, EnvironmentError, JobStatus, Simulation};

#[test]
fn explicit_clock_boundaries_and_real_resource_collection() -> Result<(), EnvironmentError> {
    let mut simulation = Simulation::new("furnace".into(), 3, false);
    simulation.start()?;
    assert_eq!(simulation.furnace.raw.value, 3);
    assert_eq!(simulation.furnace.fuel.value, 3);
    assert_eq!(simulation.furnace.version, 1);
    assert!(simulation.complete_due()?.is_none());
    simulation.advance(2)?;
    assert!(simulation.complete_due()?.is_none());
    assert_eq!(simulation.take(), Err(EnvironmentError::NotComplete));
    simulation.advance(1)?;
    let event = simulation.complete_due()?.ok_or(EnvironmentError::NoJob)?;
    assert_eq!(event.payload["resource"]["product"]["value"], 1);
    assert!(simulation.complete_due()?.is_none());
    simulation.mutate(1, 1, 2)?;
    assert_eq!(event.payload["resource"]["product"]["value"], 1);
    assert_eq!(simulation.query()["resource"]["product"]["value"], 3);
    assert_eq!(simulation.furnace.version, 3);
    let result = simulation.take()?;
    assert_eq!(result["job"]["collected"], 3);
    assert_eq!(result["resource"]["product"]["value"], 0);
    assert_eq!(simulation.furnace.version, 4);
    assert_eq!(simulation.take(), Err(EnvironmentError::AlreadyCollected));
    assert_eq!(
        simulation.job.as_ref().map(|job| &job.status),
        Some(&JobStatus::Collected)
    );
    Ok(())
}

#[test]
fn simulator_results_restore_state_without_executing_tools() -> Result<(), EnvironmentError> {
    let mut live = Simulation::new("furnace".into(), 2, false);
    let mut replay = live.clone();
    let start_result = live.start()?;
    replay.project_result(&start_result)?;
    assert_eq!(live, replay);
    live.advance(2)?;
    replay.advance(2)?;
    assert_eq!(live.complete_due()?, replay.complete_due()?);
    live.mutate(0, 0, 2)?;
    replay.mutate(0, 0, 2)?;
    replay.project_result(&live.query())?;
    let collected = live.take()?;
    replay.project_result(&collected)?;
    assert_eq!(live, replay);
    assert_eq!(replay.job.as_ref().map(|job| job.collected), Some(3));
    assert!(matches!(
        replay.project_result(&collected),
        Err(EnvironmentError::AlreadyCollected)
    ));
    Ok(())
}

#[test]
fn projected_snapshots_and_result_fields_are_validated() -> Result<(), EnvironmentError> {
    let mut live = Simulation::new("furnace".into(), 2, false);
    let mut replay = live.clone();
    let result = live.start()?;
    let mut changed = result.clone();
    changed["resource"]["product"]["value"] = json!(999);
    assert!(matches!(
        replay.project_result(&changed),
        Err(EnvironmentError::InvalidSnapshot(_))
    ));
    let mut changed = result.clone();
    changed["environment"]["duration"] = json!(9);
    assert!(matches!(
        replay.project_result(&changed),
        Err(EnvironmentError::InvalidSnapshot(_))
    ));
    assert!(replay.job.is_none());
    replay.project_result(&result)?;
    assert_eq!(live, replay);
    Ok(())
}

#[test]
fn timer_uses_same_event_shape_without_consuming_furnace_resources() -> Result<(), EnvironmentError>
{
    let mut timer = Simulation::new("timer".into(), 0, true);
    timer.start()?;
    let event = timer.complete_due()?.ok_or(EnvironmentError::NoJob)?;
    assert_eq!(event.source, "timer");
    assert_eq!(event.job_id, "job-1");
    assert_eq!(event.payload["timer"], true);
    assert_eq!(timer.take()?["job"]["collected"], 0);
    assert_eq!(timer.furnace.version, 0);
    assert_eq!(timer.furnace.raw.value, 4);
    Ok(())
}

#[test]
fn overflow_fails_without_mutating_state() -> Result<(), EnvironmentError> {
    let mut clock = Clock { now: u64::MAX };
    assert!(matches!(
        clock.advance(1),
        Err(EnvironmentError::Overflow(_))
    ));
    assert_eq!(clock.now, u64::MAX);
    let mut simulation = Simulation::new("furnace".into(), 1, false);
    simulation.clock = clock;
    let before = simulation.clone();
    assert!(matches!(
        simulation.start(),
        Err(EnvironmentError::Overflow(_))
    ));
    assert_eq!(simulation, before);
    assert!(matches!(
        simulation.mutate(u64::MAX, 0, 0),
        Err(EnvironmentError::Overflow(_))
    ));
    assert_eq!(simulation, before);
    Ok(())
}
