use super::{GenerateParams, GenerateResponse, ModelClient, ModelError, ScriptStep};
use crate::context::{ChatMessage, LLMContext};
use async_trait::async_trait;
use std::{collections::VecDeque, sync::Mutex};

/// Each attempted generation consumes exactly one script entry, including mismatches.
pub struct ScriptModel {
    steps: Mutex<VecDeque<ScriptStep>>,
}
impl ScriptModel {
    pub fn new(steps: Vec<ScriptStep>) -> Self {
        Self {
            steps: Mutex::new(steps.into()),
        }
    }
}
#[async_trait]
impl ModelClient for ScriptModel {
    async fn generate(
        &self,
        context: &LLMContext,
        _: &GenerateParams,
    ) -> Result<GenerateResponse, ModelError> {
        let step = self
            .steps
            .lock()
            .map_err(|_| ModelError::new("script_lock", "script state lock poisoned"))?
            .pop_front()
            .ok_or_else(|| ModelError::new("script_exhausted", "no scripted response remains"))?;
        if let Some(expected) = step.expected_last_user {
            let actual = context
                .messages
                .iter()
                .rev()
                .find_map(|message| match message {
                    ChatMessage::User { content } => Some(content.as_str()),
                    _ => None,
                });
            if actual != Some(expected.as_str()) {
                return Err(ModelError::new(
                    "script_condition",
                    format!("last user message: expected {expected:?}, found {actual:?}"),
                ));
            }
        }
        step.response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn mismatch_consumes_step_and_exhaustion_is_explicit() {
        let model = ScriptModel::new(vec![ScriptStep {
            expected_last_user: Some("expected".into()),
            response: Err(ModelError::new("unused", "unused")),
        }]);
        let context = LLMContext::new("context", "system");
        assert_eq!(
            model
                .generate(&context, &GenerateParams::default())
                .await
                .expect_err("condition mismatch")
                .kind,
            "script_condition"
        );
        assert_eq!(
            model
                .generate(&context, &GenerateParams::default())
                .await
                .expect_err("exhaustion")
                .kind,
            "script_exhausted"
        );
    }
}
