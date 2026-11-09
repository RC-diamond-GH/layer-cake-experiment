use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use serde::Serialize;
use serde_json::{Value, json};

use crate::context::{ChatMessage, LLMContext};
use crate::environment::{JobStatus, Simulation};
use crate::events::{Delivery, Event, EventHub, Subscription};
use crate::model::{GenerateParams, ModelClient};
use crate::react::{ReAct, ReactState, StepOutcome, StopReason};
use crate::record::RunLog;
use crate::tool::ToolRegistry;

use super::{ReactiveError, RegistryWrapper, ScenarioConfig, ScenarioModel};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    Model,
    EarlyCompletion,
    Subscribe,
    Mutate,
    Wait,
    Publish,
    Duplicate,
    Deliver,
    Refresh,
    Finished,
    Stopped,
    Failed,
}

pub struct ReactiveExperiment {
    agent: ReAct,
    simulation: Arc<Mutex<Simulation>>,
    hub: EventHub,
    log: RunLog,
    config: ScenarioConfig,
    polling: bool,
    timer: bool,
    driver_id: String,
    source_id: String,
    receiver_id: String,
    queue_id: String,
    subscription_id: String,
    boundary: Boundary,
    steps: u64,
    max_steps: u64,
    subscribed: bool,
    mutated: bool,
    pending_event: Option<Event>,
    last_event: Option<Event>,
    event_records: BTreeMap<String, String>,
    queue_records: BTreeMap<(String, String), String>,
    subscription_record: Option<String>,
    cause: Option<String>,
    delivered: u64,
    duplicate_count: u64,
    observed_tool_messages: usize,
}

pub fn assemble(
    run_id: String,
    config: ScenarioConfig,
    log: RunLog,
    polling: bool,
    timer: bool,
) -> Result<ReactiveExperiment, ReactiveError> {
    assemble_with(run_id, config, log, polling, timer, None, None)
}

pub fn assemble_with(
    run_id: String,
    config: ScenarioConfig,
    log: RunLog,
    polling: bool,
    timer: bool,
    model: Option<Arc<dyn ModelClient>>,
    registry_wrapper: Option<RegistryWrapper>,
) -> Result<ReactiveExperiment, ReactiveError> {
    let max_steps = config
        .duration
        .checked_mul(if polling { 5 } else { 1 })
        .and_then(|duration| duration.checked_add(256))
        .ok_or_else(|| {
            ReactiveError::Scenario("duration exceeds deterministic boundary budget".into())
        })?;
    let max_rounds = if polling {
        config
            .duration
            .checked_add(32)
            .ok_or_else(|| ReactiveError::Scenario("duration exceeds model budget".into()))?
    } else {
        32
    };
    let source_id = format!("{run_id}/{}", if timer { "timer" } else { "furnace" });
    let driver_id = format!("{run_id}/driver");
    let receiver_id = format!("{run_id}/receiver");
    let queue_id = format!("{receiver_id}/queue");
    let subscription_id = format!("{run_id}/completion-subscription");
    for (id, kind, label) in [
        (&source_id, "event_source", "deterministic event source"),
        (
            &driver_id,
            "reactive_driver",
            "deterministic boundary controller",
        ),
        (&receiver_id, "receiver", "agent receiver"),
        (&queue_id, "event_queue", "receiver FIFO"),
        (
            &subscription_id,
            "subscription",
            "job completion subscription",
        ),
    ] {
        log.register(id, kind, label)?;
    }
    let simulation = Arc::new(Mutex::new(Simulation::new(
        source_id.clone(),
        config.duration,
        timer,
    )));
    let resource_id = format!("{source_id}/resources");
    log.register(&resource_id, "resource", "versioned raw, fuel and product")?;
    for name in ["raw", "fuel", "product"] {
        log.register(&format!("{resource_id}/{name}"), "resource", name)?;
    }
    let mut registry = ToolRegistry::new();
    for tool in crate::environment::simulation_tools(simulation.clone(), timer) {
        registry.register(tool)?;
    }
    if let Some(wrapper) = registry_wrapper {
        registry = wrapper(registry)?;
    }
    let mut context = LLMContext::new(
        format!("{run_id}/context"),
        "Start one job, wait for completion, query current resources and collect its output. Never use a stale completion payload as current resource state.",
    );
    context.append(ChatMessage::User {
        content: if timer {
            "Start a timer and confirm completion."
        } else {
            "Fire the furnace and collect the completed product."
        }
        .into(),
    });
    context.set_tools(registry.definitions());
    let agent = ReAct::new(
        format!("{run_id}/agent"),
        context,
        model.unwrap_or_else(|| Arc::new(ScenarioModel::new(timer))),
        Arc::new(registry),
        GenerateParams::default(),
        max_rounds,
        log.clone(),
    )?;
    let result = ReactiveExperiment {
        agent,
        simulation,
        hub: EventHub::new(),
        log,
        config,
        polling,
        timer,
        driver_id,
        source_id,
        receiver_id,
        queue_id,
        subscription_id,
        boundary: Boundary::Model,
        steps: 0,
        max_steps,
        subscribed: false,
        mutated: false,
        pending_event: None,
        last_event: None,
        event_records: BTreeMap::new(),
        queue_records: BTreeMap::new(),
        subscription_record: None,
        cause: None,
        delivered: 0,
        duplicate_count: 0,
        observed_tool_messages: 0,
    };
    result.record_resource(None)?;
    Ok(result)
}

impl ReactiveExperiment {
    fn simulation(&self) -> Result<Simulation, ReactiveError> {
        Ok(self
            .simulation
            .lock()
            .map_err(|error| ReactiveError::Lock(error.to_string()))?
            .clone())
    }

    fn observe(
        &self,
        kind: &str,
        cause: Option<&str>,
        data: Value,
    ) -> Result<String, ReactiveError> {
        Ok(self.log.observe(&self.driver_id, kind, cause, data)?)
    }

    fn record_resource(&self, cause: Option<&str>) -> Result<(), ReactiveError> {
        let simulation = self.simulation()?;
        self.log.observe(&simulation.furnace.id, "resource_version", cause, json!({"clock":simulation.clock.now,"resource":simulation.furnace,"job":simulation.job}))?;
        for (name, resource) in [
            ("raw", &simulation.furnace.raw),
            ("fuel", &simulation.furnace.fuel),
            ("product", &simulation.furnace.product),
        ] {
            self.log.observe(&format!("{}/{name}", simulation.furnace.id), "resource_version", cause, json!({"clock":simulation.clock.now,"value":resource.value,"version":resource.version}))?;
        }
        if let Some(job) = simulation.job {
            let id = format!("{}/{}", simulation.source, job.id);
            self.log.register(&id, "job", &job.id)?;
            self.log.observe(
                &id,
                "job_state",
                cause,
                json!({"clock":simulation.clock.now,"job":job}),
            )?;
        }
        Ok(())
    }

    fn next_wait(&self) -> Boundary {
        if self.config.mutate_while_waiting && !self.mutated {
            Boundary::Mutate
        } else if !self.polling && self.hub.pending(&self.receiver_id) > 0 {
            Boundary::Deliver
        } else if self.polling && self.last_event.is_some() {
            Boundary::Refresh
        } else {
            Boundary::Wait
        }
    }

    fn queue_observations(&mut self, deliveries: Vec<Delivery>) -> Result<(), ReactiveError> {
        for delivery in deliveries {
            let event_cause = self
                .event_records
                .get(&delivery.event.id)
                .map(String::as_str);
            let matching = self.log.observe(&delivery.subscription_id, "event_match", event_cause, json!({"event_id":delivery.event.id,"source":delivery.event.source,"job_id":delivery.event.job_id,"receiver":delivery.receiver,"subscription_record":self.subscription_record}))?;
            let queued = self.log.observe(&self.queue_id, "event_queued", Some(&matching), json!({"event_id":delivery.event.id,"subscription_id":delivery.subscription_id,"receiver":delivery.receiver}))?;
            self.queue_records
                .insert((delivery.subscription_id, delivery.event.id), queued);
        }
        Ok(())
    }

    fn project_tool_results(&mut self, mut projected: Simulation) -> Result<(), ReactiveError> {
        let messages: Vec<&str> = self
            .agent
            .context
            .messages
            .iter()
            .filter_map(|message| {
                if let ChatMessage::Tool { content, .. } = message {
                    Some(content.as_str())
                } else {
                    None
                }
            })
            .collect();
        for content in messages.iter().skip(self.observed_tool_messages) {
            let value: Value = serde_json::from_str(content).map_err(|error| {
                ReactiveError::Scenario(format!("invalid simulator tool result: {error}"))
            })?;
            projected.project_result(&value)?;
        }
        *self
            .simulation
            .lock()
            .map_err(|error| ReactiveError::Lock(error.to_string()))? = projected;
        self.observed_tool_messages = messages.len();
        Ok(())
    }

    pub async fn step(&mut self) -> Result<bool, ReactiveError> {
        if matches!(self.boundary, Boundary::Finished | Boundary::Stopped) {
            return Ok(true);
        }
        if self.boundary == Boundary::Failed {
            return Err(ReactiveError::Scenario(
                "driver is terminal after an earlier failure".into(),
            ));
        }
        if self.stop_signal().load(Ordering::SeqCst) {
            self.stop()?;
            return Ok(true);
        }
        let result = self.step_boundary().await;
        if let Err(error) = &result {
            self.boundary = Boundary::Failed;
            self.observe(
                "driver_failure",
                self.cause.as_deref(),
                json!({"error":error.to_string(),"step":self.steps}),
            )?;
            self.log.set_summary(json!({"finished":false,"status":"failed","error":error.to_string(),"state":self.state()}))?;
        }
        result
    }

    async fn step_boundary(&mut self) -> Result<bool, ReactiveError> {
        self.steps = self
            .steps
            .checked_add(1)
            .ok_or_else(|| ReactiveError::Scenario("boundary count overflow".into()))?;
        if self.steps > self.max_steps {
            return Err(ReactiveError::Scenario(
                "deterministic boundary budget exhausted".into(),
            ));
        }
        let boundary_record = self.observe("boundary", self.cause.as_deref(), json!({"boundary":self.boundary,"clock":self.simulation()?.clock.now,"step":self.steps}))?;
        match self.boundary {
            Boundary::Model => {
                let before_round = self.simulation()?;
                let had_job = before_round.job.is_some();
                let outcome = self
                    .agent
                    .step(self.cause.as_deref().or(Some(&boundary_record)))
                    .await?;
                if outcome == StepOutcome::Finished
                    && !matches!(
                        self.agent.state(),
                        ReactState::Terminal {
                            reason: StopReason::Finished | StopReason::Stopped
                        }
                    )
                {
                    return Err(ReactiveError::Scenario(format!(
                        "agent terminated abnormally: {:?}",
                        self.agent.state()
                    )));
                }
                self.project_tool_results(before_round)?;
                let data = self.log.data()?;
                self.cause = data
                    .records
                    .iter()
                    .rev()
                    .find(|record| record.kind == "call")
                    .map(|record| record.id.clone())
                    .or(Some(boundary_record));
                self.record_resource(self.cause.as_deref())?;
                if matches!(
                    self.agent.state(),
                    ReactState::Terminal {
                        reason: StopReason::Stopped
                    }
                ) {
                    self.stop()?;
                    return Ok(true);
                }
                let simulation = self.simulation()?;
                if outcome == StepOutcome::Finished {
                    if simulation
                        .job
                        .as_ref()
                        .is_some_and(|job| job.status == JobStatus::Collected)
                    {
                        self.finish()?;
                    } else if simulation
                        .job
                        .as_ref()
                        .is_some_and(|job| job.status == JobStatus::Completed)
                    {
                        return Err(ReactiveError::Scenario(
                            "model finished without collecting the completed job".into(),
                        ));
                    } else if simulation.job.is_some() {
                        self.boundary = if !had_job && self.config.completion_before_subscribe {
                            Boundary::EarlyCompletion
                        } else if !self.subscribed && !self.polling {
                            Boundary::Subscribe
                        } else {
                            self.next_wait()
                        };
                    } else {
                        return Err(ReactiveError::Scenario(
                            "model finished without starting a job".into(),
                        ));
                    }
                } else if !had_job && simulation.job.is_some() {
                    self.boundary = if self.config.completion_before_subscribe {
                        Boundary::EarlyCompletion
                    } else if self.polling {
                        self.next_wait()
                    } else {
                        Boundary::Subscribe
                    };
                } else if simulation
                    .job
                    .as_ref()
                    .is_some_and(|job| job.status == JobStatus::Running)
                {
                    self.boundary = self.next_wait();
                }
            }
            Boundary::EarlyCompletion => {
                let simulation = self.simulation()?;
                let due = simulation
                    .job
                    .as_ref()
                    .ok_or_else(|| {
                        ReactiveError::Scenario("missing job before early completion".into())
                    })?
                    .due_at;
                let mut state = self
                    .simulation
                    .lock()
                    .map_err(|error| ReactiveError::Lock(error.to_string()))?;
                let ticks = due.saturating_sub(state.clock.now);
                state.advance(ticks)?;
                self.pending_event = state.complete_due()?;
                drop(state);
                self.record_resource(Some(&boundary_record))?;
                self.boundary = Boundary::Publish;
            }
            Boundary::Subscribe => {
                let simulation = self.simulation()?;
                let job = simulation.job.ok_or_else(|| {
                    ReactiveError::Scenario("cannot subscribe before job starts".into())
                })?;
                let subscription = Subscription {
                    id: self.subscription_id.clone(),
                    source: self.source_id.clone(),
                    job_id: job.id,
                    receiver: self.receiver_id.clone(),
                    active: true,
                };
                self.subscription_record = Some(self.log.observe(
                    &self.subscription_id,
                    "subscribe",
                    self.cause.as_deref(),
                    json!({"subscription":subscription,"clock":simulation.clock.now}),
                )?);
                let deliveries = self.hub.subscribe(subscription)?;
                self.subscribed = true;
                self.queue_observations(deliveries)?;
                self.boundary = self.next_wait();
            }
            Boundary::Mutate => {
                self.simulation
                    .lock()
                    .map_err(|error| ReactiveError::Lock(error.to_string()))?
                    .mutate(1, 1, 2)?;
                self.mutated = true;
                self.cause = Some(self.observe("resource_mutated_while_waiting", Some(&boundary_record), json!({"clock":self.simulation()?.clock.now,"resource":self.simulation()?.furnace}))?);
                self.record_resource(self.cause.as_deref())?;
                self.boundary = self.next_wait();
            }
            Boundary::Wait => {
                let mut state = self
                    .simulation
                    .lock()
                    .map_err(|error| ReactiveError::Lock(error.to_string()))?;
                let ticks = if state.job.as_ref().is_some_and(|job| {
                    job.status == JobStatus::Running && state.clock.now < job.due_at
                }) {
                    1
                } else {
                    0
                };
                state.advance(ticks)?;
                self.pending_event = state.complete_due()?;
                drop(state);
                self.cause = Some(self.observe("clock_advanced", Some(&boundary_record), json!({"clock":self.simulation()?.clock.now,"waiting_for":self.simulation()?.job}))?);
                if self.pending_event.is_some() {
                    self.record_resource(self.cause.as_deref())?;
                    self.boundary = Boundary::Publish;
                } else if self.polling {
                    self.boundary = Boundary::Refresh;
                }
            }
            Boundary::Publish => {
                let event = self.pending_event.take().ok_or_else(|| {
                    ReactiveError::Scenario("missing completion at publish boundary".into())
                })?;
                self.log
                    .register(&event.id, "event", "retained job completion")?;
                let event_record = self.log.observe(
                    &event.id,
                    "event_published",
                    self.cause.as_deref(),
                    json!({"event":event,"clock":self.simulation()?.clock.now}),
                )?;
                self.event_records
                    .insert(event.id.clone(), event_record.clone());
                self.cause = Some(event_record);
                let publication = self.hub.publish(event.clone())?;
                self.queue_observations(publication.deliveries)?;
                self.last_event = Some(event);
                self.boundary = if self.config.duplicate_notification {
                    Boundary::Duplicate
                } else {
                    self.after_publication()
                };
            }
            Boundary::Duplicate => {
                let event = self
                    .last_event
                    .clone()
                    .ok_or_else(|| ReactiveError::Scenario("missing duplicate event".into()))?;
                let publication = self.hub.publish(event.clone())?;
                if !publication.duplicate || !publication.deliveries.is_empty() {
                    return Err(ReactiveError::Scenario(
                        "duplicate suppression invariant failed".into(),
                    ));
                }
                self.duplicate_count += 1;
                self.log.observe(
                    &event.id,
                    "event_duplicate_suppressed",
                    self.event_records.get(&event.id).map(String::as_str),
                    json!({"event_id":event.id,"queued":0}),
                )?;
                self.boundary = self.after_publication();
            }
            Boundary::Deliver => {
                let delivery = self.hub.pop(&self.receiver_id).ok_or_else(|| {
                    ReactiveError::Scenario("wake boundary has no queued event".into())
                })?;
                let queued = self
                    .queue_records
                    .get(&(delivery.subscription_id.clone(), delivery.event.id.clone()))
                    .map(String::as_str);
                self.cause = Some(self.log.observe(
                    &self.receiver_id,
                    "event_delivered",
                    queued,
                    json!({"delivery":delivery,"clock":self.simulation()?.clock.now}),
                )?);
                self.delivered += 1;
                self.boundary = Boundary::Refresh;
            }
            Boundary::Refresh => {
                let simulation = self.simulation()?;
                let decision = self.observe(if self.polling { "poll_decision" } else { "wake_decision" }, self.cause.as_deref(), json!({"clock":simulation.clock.now,"resource_version":simulation.furnace.version,"event_id":self.last_event.as_ref().map(|event| &event.id),"subscription_id":if self.subscribed {Some(&self.subscription_id)} else {None}}))?;
                self.agent.append_user(
                    format!("Current environment snapshot: {}", simulation.query()),
                    Some(&decision),
                )?;
                let refreshed = self.log.observe(&self.agent.context.id, "context_refreshed", Some(&decision), json!({"context_id":self.agent.context.id,"context_version":self.agent.context.version,"resource_version":simulation.furnace.version,"clock":simulation.clock.now}))?;
                self.cause = Some(refreshed);
                if matches!(
                    self.agent.state(),
                    ReactState::Terminal {
                        reason: StopReason::Finished
                    }
                ) {
                    self.agent.reopen()?;
                }
                self.boundary = Boundary::Model;
            }
            Boundary::Finished | Boundary::Stopped | Boundary::Failed => {}
        }
        Ok(matches!(
            self.boundary,
            Boundary::Finished | Boundary::Stopped
        ))
    }

    fn after_publication(&self) -> Boundary {
        if !self.polling && !self.subscribed {
            Boundary::Subscribe
        } else {
            self.next_wait()
        }
    }

    fn finish(&mut self) -> Result<(), ReactiveError> {
        let simulation = self.simulation()?;
        let job = simulation
            .job
            .as_ref()
            .ok_or_else(|| ReactiveError::Scenario("missing completed job".into()))?;
        if job.status != JobStatus::Collected {
            return Err(ReactiveError::Scenario(
                "finished job was not collected".into(),
            ));
        }
        if !self.timer && simulation.furnace.product.value != 0 {
            return Err(ReactiveError::Scenario(
                "finished furnace has uncollected product".into(),
            ));
        }
        if self.config.duplicate_notification && self.duplicate_count != 1 {
            return Err(ReactiveError::Scenario(
                "duplicate scenario did not publish exactly one duplicate".into(),
            ));
        }
        if self.config.mutate_while_waiting && !self.mutated {
            return Err(ReactiveError::Scenario(
                "resource mutation scenario was not exercised".into(),
            ));
        }
        if !self.polling && self.delivered != 1 {
            return Err(ReactiveError::Scenario(
                "reactive completion must deliver exactly once".into(),
            ));
        }
        self.boundary = Boundary::Finished;
        let summary = json!({"finished":true,"status":"finished","polling":self.polling,"timer":self.timer,"clock":simulation.clock.now,"rounds":self.agent.rounds,"steps":self.steps,"collected":job.collected,"resource":simulation.furnace,"job":job,"events":self.hub.retained().len(),"deliveries":self.delivered,"duplicates_suppressed":self.duplicate_count,"mutated_while_waiting":self.mutated,"context_id":self.agent.context.id,"context_version":self.agent.context.version});
        self.observe("driver_finished", self.cause.as_deref(), summary.clone())?;
        self.log.set_summary(summary)?;
        Ok(())
    }

    /// JSON strings and `{kind:"user",content:...}` append user input.
    /// `{kind:"tick",ticks:n}` advances the deterministic clock; completion is
    /// processed at the next wait boundary. `{kind:"mutate",raw:n,fuel:n,product:n}`
    /// adds versioned resources. `{kind:"cancel"}` cancels the active subscription.
    pub fn input(&mut self, value: Value) -> Result<(), ReactiveError> {
        if matches!(
            self.boundary,
            Boundary::Finished | Boundary::Stopped | Boundary::Failed
        ) {
            return Err(ReactiveError::Input("run is terminal".into()));
        }
        let record = self.observe("input", self.cause.as_deref(), value.clone())?;
        if let Some(content) = value.as_str() {
            self.agent.append_user(content.into(), Some(&record))?;
        } else {
            match value.get("kind").and_then(Value::as_str) {
                Some("user") => self.agent.append_user(
                    value
                        .get("content")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            ReactiveError::Input("user content must be a string".into())
                        })?
                        .into(),
                    Some(&record),
                )?,
                Some("tick") => {
                    let ticks = value.get("ticks").and_then(Value::as_u64).ok_or_else(|| {
                        ReactiveError::Input("ticks must be a nonnegative integer".into())
                    })?;
                    self.simulation
                        .lock()
                        .map_err(|error| ReactiveError::Lock(error.to_string()))?
                        .advance(ticks)?;
                }
                Some("mutate") => {
                    let amount = |key: &str| -> Result<u64, ReactiveError> {
                        match value.get(key) {
                            None => Ok(0),
                            Some(value) => value.as_u64().ok_or_else(|| {
                                ReactiveError::Input(format!("{key} must be a nonnegative integer"))
                            }),
                        }
                    };
                    self.simulation
                        .lock()
                        .map_err(|error| ReactiveError::Lock(error.to_string()))?
                        .mutate(amount("raw")?, amount("fuel")?, amount("product")?)?;
                    self.record_resource(Some(&record))?;
                }
                Some("cancel") => {
                    let purged = self.hub.cancel(&self.subscription_id)?;
                    self.log.observe(
                        &self.subscription_id,
                        "cancel",
                        Some(&record),
                        json!({"purged":purged}),
                    )?;
                    self.agent.stop()?;
                    self.boundary = Boundary::Stopped;
                    self.log.set_summary(
                        json!({"finished":false,"status":"cancelled","state":self.state()}),
                    )?;
                }
                kind => {
                    return Err(ReactiveError::Input(format!(
                        "unknown input kind: {kind:?}"
                    )));
                }
            }
        }
        self.cause = Some(record);
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), ReactiveError> {
        if matches!(
            self.boundary,
            Boundary::Finished | Boundary::Stopped | Boundary::Failed
        ) {
            return Ok(());
        }
        self.agent.stop()?;
        if self.subscribed {
            let purged = self.hub.cancel(&self.subscription_id)?;
            self.log.observe(
                &self.subscription_id,
                "cancel",
                self.cause.as_deref(),
                json!({"purged":purged}),
            )?;
        }
        self.boundary = Boundary::Stopped;
        self.observe(
            "driver_stopped",
            self.cause.as_deref(),
            json!({"clock":self.simulation()?.clock.now}),
        )?;
        self.log
            .set_summary(json!({"finished":false,"status":"stopped","state":self.state()}))?;
        Ok(())
    }

    pub fn stop_signal(&self) -> Arc<AtomicBool> {
        self.agent.stop_signal()
    }

    pub fn state(&self) -> Value {
        match self.simulation() {
            Ok(simulation) => {
                json!({"finished":matches!(self.boundary,Boundary::Finished|Boundary::Stopped|Boundary::Failed),"boundary":self.boundary,"clock":simulation.clock.now,"waiting_for":if matches!(self.boundary,Boundary::Wait|Boundary::Subscribe|Boundary::Deliver) {simulation.job.as_ref().map(|job| &job.id)} else {None},"rounds":self.agent.rounds,"steps":self.steps,"queue_len":self.hub.pending(&self.receiver_id),"environment":simulation,"agent":self.agent.state()})
            }
            Err(error) => json!({"finished":true,"error":error.to_string()}),
        }
    }

    pub fn context(&self) -> &LLMContext {
        &self.agent.context
    }
}
