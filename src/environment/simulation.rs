use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use crate::events::Event;

#[derive(Clone, Debug, Error, PartialEq)]
pub enum EnvironmentError {
    #[error("simulation overflow: {0}")]
    Overflow(&'static str),
    #[error("job is already running")]
    Busy,
    #[error("raw material and fuel are required")]
    InsufficientResources,
    #[error("no job exists")]
    NoJob,
    #[error("job is not complete")]
    NotComplete,
    #[error("job product was already collected")]
    AlreadyCollected,
    #[error("invalid deterministic snapshot: {0}")]
    InvalidSnapshot(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clock {
    pub now: u64,
}
impl Clock {
    pub fn advance(&mut self, ticks: u64) -> Result<(), EnvironmentError> {
        self.now = self
            .now
            .checked_add(ticks)
            .ok_or(EnvironmentError::Overflow("clock"))?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionedResource {
    pub value: u64,
    pub version: u64,
}
impl VersionedResource {
    fn changed(&self, value: u64) -> Result<Self, EnvironmentError> {
        Ok(Self {
            value,
            version: self
                .version
                .checked_add(1)
                .ok_or(EnvironmentError::Overflow("resource version"))?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Furnace {
    pub id: String,
    pub version: u64,
    pub raw: VersionedResource,
    pub fuel: VersionedResource,
    pub product: VersionedResource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Running,
    Completed,
    Collected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub started_at: u64,
    pub due_at: u64,
    pub status: JobStatus,
    pub collected: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Simulation {
    pub clock: Clock,
    pub source: String,
    pub timer: bool,
    pub duration: u64,
    pub next_job: u64,
    pub furnace: Furnace,
    pub job: Option<Job>,
}

impl Simulation {
    pub fn new(source: String, duration: u64, timer: bool) -> Self {
        Self {
            clock: Clock::default(),
            source: source.clone(),
            timer,
            duration,
            next_job: 1,
            furnace: Furnace {
                id: format!("{source}/resources"),
                version: 0,
                raw: VersionedResource {
                    value: 4,
                    version: 0,
                },
                fuel: VersionedResource {
                    value: 4,
                    version: 0,
                },
                product: VersionedResource {
                    value: 0,
                    version: 0,
                },
            },
            job: None,
        }
    }

    pub fn start(&mut self) -> Result<Value, EnvironmentError> {
        if self
            .job
            .as_ref()
            .is_some_and(|job| job.status != JobStatus::Collected)
        {
            return Err(EnvironmentError::Busy);
        }
        let due_at = self
            .clock
            .now
            .checked_add(self.duration)
            .ok_or(EnvironmentError::Overflow("job deadline"))?;
        let next_job = self
            .next_job
            .checked_add(1)
            .ok_or(EnvironmentError::Overflow("job identity"))?;
        let mut furnace = self.furnace.clone();
        if !self.timer {
            if furnace.raw.value == 0 || furnace.fuel.value == 0 {
                return Err(EnvironmentError::InsufficientResources);
            }
            furnace.raw = furnace.raw.changed(furnace.raw.value - 1)?;
            furnace.fuel = furnace.fuel.changed(furnace.fuel.value - 1)?;
            furnace.version = furnace
                .version
                .checked_add(1)
                .ok_or(EnvironmentError::Overflow("furnace version"))?;
        }
        self.job = Some(Job {
            id: format!("job-{}", self.next_job),
            started_at: self.clock.now,
            due_at,
            status: JobStatus::Running,
            collected: 0,
        });
        self.next_job = next_job;
        self.furnace = furnace;
        Ok(self.result("start"))
    }

    pub fn advance(&mut self, ticks: u64) -> Result<(), EnvironmentError> {
        self.clock.advance(ticks)
    }

    pub fn complete_due(&mut self) -> Result<Option<Event>, EnvironmentError> {
        let Some(job) = self.job.as_ref() else {
            return Ok(None);
        };
        if job.status != JobStatus::Running || self.clock.now < job.due_at {
            return Ok(None);
        }
        let mut furnace = self.furnace.clone();
        if !self.timer {
            let product = furnace
                .product
                .value
                .checked_add(1)
                .ok_or(EnvironmentError::Overflow("product"))?;
            furnace.product = furnace.product.changed(product)?;
            furnace.version = furnace
                .version
                .checked_add(1)
                .ok_or(EnvironmentError::Overflow("furnace version"))?;
        }
        self.furnace = furnace;
        if let Some(job) = self.job.as_mut() {
            job.status = JobStatus::Completed;
        }
        let job = self.job.as_ref().ok_or(EnvironmentError::NoJob)?;
        Ok(Some(Event {
            id: format!("{}/{}:completed", self.source, job.id),
            source: self.source.clone(),
            job_id: job.id.clone(),
            payload: json!({ "completed_at": self.clock.now, "resource": self.furnace, "timer": self.timer }),
        }))
    }

    pub fn mutate(&mut self, raw: u64, fuel: u64, product: u64) -> Result<(), EnvironmentError> {
        let mut furnace = self.furnace.clone();
        furnace.raw = furnace.raw.changed(
            furnace
                .raw
                .value
                .checked_add(raw)
                .ok_or(EnvironmentError::Overflow("raw"))?,
        )?;
        furnace.fuel = furnace.fuel.changed(
            furnace
                .fuel
                .value
                .checked_add(fuel)
                .ok_or(EnvironmentError::Overflow("fuel"))?,
        )?;
        furnace.product = furnace.product.changed(
            furnace
                .product
                .value
                .checked_add(product)
                .ok_or(EnvironmentError::Overflow("product"))?,
        )?;
        furnace.version = furnace
            .version
            .checked_add(1)
            .ok_or(EnvironmentError::Overflow("furnace version"))?;
        self.furnace = furnace;
        Ok(())
    }

    pub fn take(&mut self) -> Result<Value, EnvironmentError> {
        let job = self.job.as_ref().ok_or(EnvironmentError::NoJob)?;
        match job.status {
            JobStatus::Running => return Err(EnvironmentError::NotComplete),
            JobStatus::Collected => return Err(EnvironmentError::AlreadyCollected),
            JobStatus::Completed => {}
        }
        let quantity = if self.timer {
            0
        } else {
            self.furnace.product.value
        };
        let mut furnace = self.furnace.clone();
        if !self.timer {
            furnace.product = furnace.product.changed(0)?;
            furnace.version = furnace
                .version
                .checked_add(1)
                .ok_or(EnvironmentError::Overflow("furnace version"))?;
        }
        self.furnace = furnace;
        if let Some(job) = self.job.as_mut() {
            job.status = JobStatus::Collected;
            job.collected = quantity;
        }
        Ok(self.result("take"))
    }

    pub fn query(&self) -> Value {
        self.result("query")
    }

    fn result(&self, operation: &str) -> Value {
        json!({ "operation": operation, "job": self.job, "resource": self.furnace, "clock": self.clock.now, "environment": self })
    }

    /// Restore only this local simulator, never a tool or external side effect.
    /// Every mutable field is checked against the legal transition from the
    /// current snapshot, rather than trusting arbitrary recorded state.
    pub fn project_result(&mut self, result: &Value) -> Result<(), EnvironmentError> {
        let snapshot: Simulation = serde_json::from_value(
            result
                .get("environment")
                .cloned()
                .ok_or_else(|| EnvironmentError::InvalidSnapshot("missing environment".into()))?,
        )
        .map_err(|error| EnvironmentError::InvalidSnapshot(error.to_string()))?;
        let operation = result
            .get("operation")
            .and_then(Value::as_str)
            .ok_or_else(|| EnvironmentError::InvalidSnapshot("missing operation".into()))?;
        if !matches!(operation, "start" | "query" | "take") || snapshot.result(operation) != *result
        {
            return Err(EnvironmentError::InvalidSnapshot(
                "result fields disagree with environment snapshot".into(),
            ));
        }
        let mut expected = self.clone();
        match result.get("operation").and_then(Value::as_str) {
            Some("start") => {
                expected.start()?;
            }
            Some("take") => {
                expected.take()?;
            }
            Some("query") => {}
            _ => {
                return Err(EnvironmentError::InvalidSnapshot(
                    "unknown operation".into(),
                ));
            }
        }
        if expected != snapshot {
            return Err(EnvironmentError::InvalidSnapshot(
                "recorded state does not match deterministic transition".into(),
            ));
        }
        *self = snapshot;
        Ok(())
    }
}
