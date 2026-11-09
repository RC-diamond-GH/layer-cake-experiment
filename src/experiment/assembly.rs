use super::ExperimentConfig;
use super::ModelConfig;
use crate::context::{ChatMessage, LLMContext, ToolCall};
use crate::model::{
    FinishReason, GenerateResponse, ModelClient, OpenAiClient, ScriptModel, ScriptStep,
};
use crate::react::{ReAct, ReactState, StepOutcome, StopReason};
use crate::reactive::{ReactiveExperiment, assemble_with};
use crate::record::RunLog;
use crate::tool::{ToolRegistry, memory_tools};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, thiserror::Error)]
pub enum ExperimentError {
    #[error("configuration: {0}")]
    Config(String),
    #[error(transparent)]
    Record(#[from] crate::record::RecordError),
    #[error(transparent)]
    React(#[from] crate::react::ReactError),
    #[error(transparent)]
    Reactive(#[from] crate::reactive::ReactiveError),
    #[error(transparent)]
    Tool(#[from] crate::tool::ToolError),
    #[error(transparent)]
    Model(#[from] crate::model::ModelError),
}

pub fn definitions() -> Value {
    json!([
        {"name":"react-memory","description":"逐轮读取、修改内存并回答；支持脚本或 OpenAI-compatible 模型","controls":["continue","step","pause","stop","input"],"config":["model","generation","system_prompt","user_message","max_rounds"]},
        {"name":"furnace-polling","description":"模拟时钟下主动轮询熔炉作业","controls":["continue","step","pause","stop","input"],"config":["scenario"]},
        {"name":"furnace-reactive","description":"完成事件经订阅、队列唤醒 Agent","controls":["continue","step","pause","stop","input"],"config":["scenario"]},
        {"name":"timer-reactive","description":"复用订阅机制的计时器事件源","controls":["continue","step","pause","stop","input"],"config":["scenario"]}
    ])
}

enum Engine {
    React(ReAct),
    Reactive(ReactiveExperiment),
}

pub struct Experiment {
    engine: Engine,
    pub log: RunLog,
    pub boundaries: u64,
    pub max_boundaries: u64,
    pub finished: bool,
    pub failed: bool,
}

impl Experiment {
    pub async fn step(&mut self) -> Result<bool, ExperimentError> {
        if self.finished {
            return Ok(true);
        }
        if self.boundaries >= self.max_boundaries {
            self.stop()?;
            self.failed = true;
            self.update_summary()?;
            return Err(ExperimentError::Config(
                "execution boundary budget exhausted".into(),
            ));
        }
        self.boundaries += 1;
        let result: Result<bool, ExperimentError> = match &mut self.engine {
            Engine::React(agent) => agent
                .step(None)
                .await
                .map(|s| s == StepOutcome::Finished)
                .map_err(Into::into),
            Engine::Reactive(agent) => agent.step().await.map_err(Into::into),
        };
        match result {
            Ok(done) => {
                self.finished = done;
                if done && !self.normal_finish() && !self.was_stopped() {
                    self.failed = true;
                }
                self.update_summary()?;
                Ok(done)
            }
            Err(error) => {
                self.finished = true;
                self.failed = true;
                self.update_summary()?;
                Err(error)
            }
        }
    }

    pub fn input(&mut self, value: Value, cause: Option<&str>) -> Result<(), ExperimentError> {
        if self.finished {
            return Err(ExperimentError::Config(
                "run is terminal; input cannot be applied".into(),
            ));
        }
        match &mut self.engine {
            Engine::React(agent) => {
                let text = value
                    .as_str()
                    .or_else(|| value.get("content").and_then(Value::as_str))
                    .ok_or_else(|| {
                        ExperimentError::Config(
                            "memory input expects a string or {content:string}".into(),
                        )
                    })?;
                agent.append_user(text.to_owned(), cause)?;
            }
            Engine::Reactive(agent) => {
                agent.input(value)?;
                self.finished =
                    agent.state().get("finished").and_then(Value::as_bool) == Some(true);
            }
        }
        self.update_summary()
    }

    pub fn stop(&mut self) -> Result<(), ExperimentError> {
        match &mut self.engine {
            Engine::React(agent) => agent.stop()?,
            Engine::Reactive(agent) => agent.stop()?,
        }
        self.finished = true;
        self.update_summary()
    }

    pub fn stop_signal(&self) -> Arc<std::sync::atomic::AtomicBool> {
        match &self.engine {
            Engine::React(agent) => agent.stop_signal(),
            Engine::Reactive(agent) => agent.stop_signal(),
        }
    }

    fn was_stopped(&self) -> bool {
        match &self.engine {
            Engine::React(agent) => matches!(
                agent.state(),
                ReactState::Terminal {
                    reason: StopReason::Stopped
                }
            ),
            Engine::Reactive(agent) => {
                agent.state().get("boundary").and_then(Value::as_str) == Some("stopped")
            }
        }
    }

    pub fn context(&self) -> &LLMContext {
        match &self.engine {
            Engine::React(agent) => &agent.context,
            Engine::Reactive(agent) => agent.context(),
        }
    }

    pub fn state(&self) -> Value {
        let engine = match &self.engine {
            Engine::React(agent) => json!({"agent":agent.state(),"rounds":agent.rounds}),
            Engine::Reactive(agent) => agent.state(),
        };
        let status = if !self.finished {
            "active"
        } else if self.failed {
            "failed"
        } else if self.normal_finish() {
            "finished"
        } else {
            "stopped"
        };
        json!({"status":status,"finished":self.finished,"failed":self.failed,"boundary":self.boundaries,"engine":engine})
    }

    pub fn update_summary(&self) -> Result<(), ExperimentError> {
        self.log.set_summary(self.state())?;
        Ok(())
    }

    pub fn normal_finish(&self) -> bool {
        if self.failed || !self.finished {
            return false;
        }
        match &self.engine {
            Engine::React(agent) => matches!(
                agent.state(),
                ReactState::Terminal {
                    reason: StopReason::Finished
                }
            ),
            Engine::Reactive(agent) => {
                agent.state().get("boundary").and_then(Value::as_str) == Some("finished")
            }
        }
    }
}

fn response(content: Option<&str>, calls: Vec<ToolCall>) -> GenerateResponse {
    let finish_reason = if calls.is_empty() {
        FinishReason::Stop
    } else {
        FinishReason::ToolCalls
    };
    GenerateResponse {
        message: ChatMessage::Assistant {
            content: content.map(str::to_owned),
            tool_calls: calls,
        },
        finish_reason,
        usage: None,
    }
}

fn memory_script(user: &str) -> ScriptModel {
    ScriptModel::new(vec![
        ScriptStep {
            expected_last_user: Some(user.into()),
            response: Ok(response(
                None,
                vec![ToolCall {
                    id: "read-counter".into(),
                    name: "memory_read".into(),
                    arguments: json!({"key":"counter"}),
                }],
            )),
        },
        ScriptStep {
            expected_last_user: None,
            response: Ok(response(
                None,
                vec![ToolCall {
                    id: "write-counter".into(),
                    name: "memory_write".into(),
                    arguments: json!({"key":"counter","value":1}),
                }],
            )),
        },
        ScriptStep {
            expected_last_user: None,
            response: Ok(response(
                Some("counter 已通过 memory_write 设置为 1。"),
                vec![],
            )),
        },
    ])
}

pub fn assemble(config: ExperimentConfig, run_id: String) -> Result<Experiment, ExperimentError> {
    assemble_internal(config, run_id, None)
}

pub(super) fn assemble_internal(
    mut config: ExperimentConfig,
    run_id: String,
    tape: Option<&crate::replay::ReplayTape>,
) -> Result<Experiment, ExperimentError> {
    if config.max_rounds == 0 || config.max_boundaries == 0 {
        return Err(ExperimentError::Config(
            "max_rounds and max_boundaries must be positive".into(),
        ));
    }
    if tape.is_none() {
        config.model.resolve_environment()?;
    }
    let serialized =
        serde_json::to_value(&config).map_err(|e| ExperimentError::Config(e.to_string()))?;
    let log = RunLog::new(run_id.clone(), config.experiment.clone(), serialized);
    log.register(&run_id, "run", &config.experiment)?;
    let mut params = config.generation.clone();
    let injected_model = tape.map(|t| t.model());
    let model: Option<Arc<dyn ModelClient>> = match (&config.model, injected_model) {
        (_, Some(model)) => Some(model),
        (ModelConfig::Script, _) => None,
        (
            ModelConfig::Openai {
                endpoint,
                api_key_env,
                model,
            },
            _,
        ) => {
            params.model = model.clone();
            let key = std::env::var(api_key_env).map_err(|_| {
                ExperimentError::Config(format!(
                    "missing API key environment variable {api_key_env}"
                ))
            })?;
            Some(Arc::new(OpenAiClient::new(endpoint.clone(), key)?))
        }
    };
    // Replay uses the same configured model name without reading API credentials.
    if let ModelConfig::Openai { model, .. } = &config.model {
        params.model = model.clone();
    }
    let engine = match config.experiment.as_str() {
        "react-memory" => {
            let memory = Arc::new(Mutex::new(BTreeMap::from([("counter".into(), json!(0))])));
            let mut registry = ToolRegistry::new();
            for tool in memory_tools(memory) {
                registry.register(tool)?;
            }
            let registry = if let Some(tape) = tape {
                tape.wrap_tools(&registry)
                    .map_err(|e| ExperimentError::Config(e.to_string()))?
            } else {
                registry
            };
            let mut context = LLMContext::new(format!("{run_id}/context"), config.system_prompt);
            context.append(ChatMessage::User {
                content: config.user_message.clone(),
            });
            context.set_tools(registry.definitions());
            let model = model.unwrap_or_else(|| Arc::new(memory_script(&config.user_message)));
            Engine::React(ReAct::new(
                format!("{run_id}/agent"),
                context,
                model,
                Arc::new(registry),
                params,
                config.max_rounds,
                log.clone(),
            )?)
        }
        name @ ("furnace-reactive" | "furnace-polling" | "timer-reactive") => {
            if !matches!(config.model, ModelConfig::Script) {
                return Err(ExperimentError::Config("reactive examples use deterministic script policies; OpenAI model is available in react-memory".into()));
            }
            let wrapper: Option<crate::reactive::RegistryWrapper> = tape.map(|t| {
                let tape = t.clone();
                Arc::new(move |registry: ToolRegistry| {
                    tape.wrap_tools(&registry)
                        .map_err(|e| crate::tool::ToolError::new("replay", e.to_string()))
                }) as crate::reactive::RegistryWrapper
            });
            Engine::Reactive(assemble_with(
                run_id.clone(),
                config.scenario,
                log.clone(),
                name == "furnace-polling",
                name == "timer-reactive",
                model,
                wrapper,
            )?)
        }
        other => {
            return Err(ExperimentError::Config(format!(
                "unknown experiment {other}; use list"
            )));
        }
    };
    let experiment = Experiment {
        engine,
        log,
        boundaries: 0,
        max_boundaries: config.max_boundaries,
        finished: false,
        failed: false,
    };
    experiment.update_summary()?;
    Ok(experiment)
}
