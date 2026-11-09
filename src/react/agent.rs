use super::{ReAct, ReactError, ReactState, StepOutcome, StopReason};
use crate::{
    context::{ChatMessage, LLMContext},
    model::{FinishReason, GenerateParams, ModelClient},
    record::{RunLog, recorded_model, recorded_tool},
    tool::ToolRegistry,
};
use serde_json::json;
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

impl ReAct {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        context: LLMContext,
        model: Arc<dyn ModelClient>,
        tools: Arc<ToolRegistry>,
        params: GenerateParams,
        max_rounds: u64,
        log: RunLog,
    ) -> Result<Self, ReactError> {
        let model_id = format!("{id}/model");
        log.register(&id, "agent", &id)?;
        log.register(&context.id, "context", &context.id)?;
        log.register(&model_id, "model", &model_id)?;
        let definitions = tools.definitions();
        for tool in &definitions {
            log.register(&tool.name, "tool", &tool.name)?;
        }
        let mut names = BTreeSet::new();
        for visible in &context.tools {
            if !names.insert(visible.name.as_str()) || !definitions.contains(visible) {
                return Err(ReactError::State(format!(
                    "visible tool {} must have a unique registered definition",
                    visible.name
                )));
            }
        }
        log.snapshot(&context)?;
        let context_cause = Some(log.observe(
            &context.id,
            "context",
            None,
            json!({"context_id":context.id,"version":context.version,"operation":"initial"}),
        )?);
        let agent = Self {
            context,
            rounds: 0,
            log,
            id,
            model_id,
            model,
            tools,
            params,
            max_rounds,
            state: ReactState::Ready,
            context_cause,
            stop_signal: Arc::new(AtomicBool::new(false)),
        };
        agent.log.observe(
            &agent.id,
            "state",
            agent.context_cause.as_deref(),
            json!({"state":agent.state,"rounds":0}),
        )?;
        Ok(agent)
    }

    pub fn state(&self) -> &ReactState {
        &self.state
    }

    /// A latched stop request; callers set it without cancelling the current call.
    pub fn stop_signal(&self) -> Arc<AtomicBool> {
        self.stop_signal.clone()
    }

    fn stop_requested(&self) -> bool {
        self.stop_signal.load(Ordering::Acquire)
    }

    fn finish(
        &mut self,
        reason: StopReason,
        cause: Option<&str>,
    ) -> Result<StepOutcome, ReactError> {
        self.state = ReactState::Terminal { reason };
        self.log.observe(
            &self.id,
            "state",
            cause,
            json!({"state":self.state,"rounds":self.rounds}),
        )?;
        Ok(StepOutcome::Finished)
    }

    fn append(
        &mut self,
        message: ChatMessage,
        cause: Option<&str>,
        operation: &str,
    ) -> Result<(), ReactError> {
        self.context.append(message);
        self.log.snapshot(&self.context)?;
        self.context_cause = Some(self.log.observe(&self.context.id, "context", cause, json!({"context_id":self.context.id,"version":self.context.version,"operation":operation}))?);
        Ok(())
    }

    pub fn append_user(&mut self, content: String, cause: Option<&str>) -> Result<(), ReactError> {
        // Reactive drivers may append fresh input before explicitly reopening a normal finish.
        if self.stop_requested() {
            return Err(ReactError::State("stop has been requested".into()));
        }
        if matches!(&self.state, ReactState::Terminal { reason } if *reason != StopReason::Finished)
        {
            return Err(ReactError::State(
                "cannot append input after abnormal termination or stop".into(),
            ));
        }
        let result = (|| {
            let input = self.log.observe(
                &self.id,
                "input",
                cause,
                json!({"content":content,"round":self.rounds}),
            )?;
            self.append(ChatMessage::User { content }, Some(&input), "user_input")
        })();
        if matches!(&result, Err(ReactError::Record(_))) {
            self.state = ReactState::Terminal {
                reason: StopReason::RecordError,
            };
        }
        result
    }

    pub fn reopen(&mut self) -> Result<(), ReactError> {
        if self.stop_requested() {
            return Err(ReactError::State(
                "a stopped agent cannot be reopened".into(),
            ));
        }
        if self.state
            != (ReactState::Terminal {
                reason: StopReason::Finished,
            })
        {
            return Err(ReactError::State(
                "only a normally finished agent can be reopened".into(),
            ));
        }
        let result = (|| -> Result<(), ReactError> {
            let control = self.log.observe(
                &self.id,
                "control",
                self.context_cause.as_deref(),
                json!({"operation":"reopen","rounds":self.rounds}),
            )?;
            self.state = ReactState::Ready;
            self.log.observe(
                &self.id,
                "state",
                Some(&control),
                json!({"state":self.state,"rounds":self.rounds}),
            )?;
            self.context_cause = Some(control);
            Ok(())
        })();
        if matches!(&result, Err(ReactError::Record(_))) {
            self.state = ReactState::Terminal {
                reason: StopReason::RecordError,
            };
        }
        result
    }

    pub fn stop(&mut self) -> Result<(), ReactError> {
        self.stop_signal.store(true, Ordering::Release);
        if matches!(&self.state, ReactState::Terminal { reason } if *reason != StopReason::Finished)
        {
            return Ok(());
        }
        self.state = ReactState::Terminal {
            reason: StopReason::Stopped,
        };
        let control = self.log.observe(
            &self.id,
            "control",
            None,
            json!({"operation":"stop","rounds":self.rounds}),
        )?;
        self.finish(StopReason::Stopped, Some(&control))?;
        Ok(())
    }

    pub async fn step(&mut self, cause: Option<&str>) -> Result<StepOutcome, ReactError> {
        if self.stop_requested()
            && matches!(
                &self.state,
                ReactState::Ready
                    | ReactState::Terminal {
                        reason: StopReason::Finished
                    }
            )
        {
            return self.finish(StopReason::Stopped, cause);
        }
        if matches!(&self.state, ReactState::Terminal { .. }) {
            return Ok(StepOutcome::Finished);
        }
        let result = self.step_inner(cause).await;
        if matches!(&result, Err(ReactError::Record(_))) {
            self.state = ReactState::Terminal {
                reason: StopReason::RecordError,
            };
        }
        result
    }

    async fn step_inner(&mut self, cause: Option<&str>) -> Result<StepOutcome, ReactError> {
        if self.stop_requested() {
            return self.finish(StopReason::Stopped, cause);
        }
        if self.rounds >= self.max_rounds {
            return self.finish(StopReason::Exhausted, cause);
        }
        self.rounds += 1;
        let effective_cause = cause
            .map(str::to_owned)
            .or_else(|| self.context_cause.clone());
        let (model_record, result) = recorded_model(
            &self.log,
            &self.model_id,
            effective_cause.as_deref(),
            self.model.as_ref(),
            &self.context,
            &self.params,
        )
        .await?;
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                if self.stop_requested() {
                    return self.finish(StopReason::Stopped, Some(&model_record));
                }
                self.finish(StopReason::ModelError, Some(&model_record))?;
                return Err(error.into());
            }
        };
        let calls = match &response.message {
            ChatMessage::Assistant { tool_calls, .. } => tool_calls.clone(),
            _ => {
                self.finish(StopReason::InvalidResponse, Some(&model_record))?;
                return Err(ReactError::InvalidResponse(
                    "model returned a non-assistant message".into(),
                ));
            }
        };
        self.append(response.message, Some(&model_record), "model_response")?;
        if self.stop_requested() {
            return self.finish(StopReason::Stopped, Some(&model_record));
        }
        match response.finish_reason {
            FinishReason::Length => return self.finish(StopReason::Truncated, Some(&model_record)),
            FinishReason::ContentFilter => {
                return self.finish(StopReason::ContentFiltered, Some(&model_record));
            }
            FinishReason::Error => {
                return self.finish(StopReason::ModelFinishError, Some(&model_record));
            }
            FinishReason::Unknown(reason) => {
                return self.finish(StopReason::UnknownFinish(reason), Some(&model_record));
            }
            FinishReason::Stop | FinishReason::ToolCalls => {}
        }
        if calls.is_empty() {
            if response.finish_reason == FinishReason::ToolCalls {
                self.finish(StopReason::InvalidResponse, Some(&model_record))?;
                return Err(ReactError::InvalidResponse(
                    "tool_calls finish reason without calls".into(),
                ));
            }
            return self.finish(StopReason::Finished, Some(&model_record));
        }
        let mut ids = BTreeSet::new();
        if calls
            .iter()
            .any(|call| call.id.is_empty() || call.name.is_empty() || !ids.insert(call.id.as_str()))
        {
            self.finish(StopReason::InvalidResponse, Some(&model_record))?;
            return Err(ReactError::InvalidResponse(
                "tool call ids and names must be nonempty, with unique ids".into(),
            ));
        }
        // Calls share their proposing model observation, and dispatch is strictly sequential.
        for call in calls {
            if self.stop_requested() {
                return self.finish(StopReason::Stopped, Some(&model_record));
            }
            self.log.register(&call.name, "tool", &call.name)?;
            let (tool_record, result) = recorded_tool(
                &self.log,
                &call.name,
                Some(&model_record),
                self.tools.as_ref(),
                &self.context.tools,
                &call,
            )
            .await?;
            let (content, error) = match result {
                Ok(value) => (value.to_string(), None),
                Err(error) => (json!({"error":error}).to_string(), Some(error)),
            };
            self.append(
                ChatMessage::Tool {
                    tool_call_id: call.id,
                    content,
                },
                Some(&tool_record),
                "tool_result",
            )?;
            if self.stop_requested() {
                return self.finish(StopReason::Stopped, Some(&tool_record));
            }
            if let Some(error) = error {
                self.finish(StopReason::ToolError, Some(&tool_record))?;
                return Err(error.into());
            }
        }
        if self.rounds >= self.max_rounds {
            return self.finish(StopReason::Exhausted, Some(&model_record));
        }
        self.log.observe(
            &self.id,
            "round",
            Some(&model_record),
            json!({"rounds":self.rounds,"state":self.state}),
        )?;
        Ok(StepOutcome::Continue)
    }
}
