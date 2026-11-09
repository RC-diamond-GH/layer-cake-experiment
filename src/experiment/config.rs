use crate::{
    model::{GenerateParams, ModelError},
    reactive::ScenarioConfig,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelConfig {
    Script,
    Openai {
        #[serde(default)]
        endpoint: String,
        #[serde(default = "openai_key_env")]
        api_key_env: String,
        #[serde(default)]
        model: String,
    },
}

fn openai_key_env() -> String {
    "OPENAI_API_KEY".into()
}

fn required_env(name: &str) -> Result<String, ModelError> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ModelError::new(
                "configuration",
                format!("missing or empty environment variable {name}"),
            )
        })
}

impl ModelConfig {
    pub(super) fn resolve_environment(&mut self) -> Result<(), ModelError> {
        if let Self::Openai {
            endpoint, model, ..
        } = self
        {
            if endpoint.is_empty() {
                let base = required_env("OPENAI_BASE_URL")?;
                let mut url = reqwest::Url::parse(&base).map_err(|_| {
                    ModelError::new(
                        "configuration",
                        "OPENAI_BASE_URL must be an HTTP(S) API base URL",
                    )
                })?;
                if !matches!(url.scheme(), "http" | "https")
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.query().is_some()
                    || url.fragment().is_some()
                {
                    return Err(ModelError::new(
                        "configuration",
                        "OPENAI_BASE_URL must be an HTTP(S) API base URL without credentials, query or fragment",
                    ));
                }
                url.set_path(&format!(
                    "{}/chat/completions",
                    url.path().trim_end_matches('/')
                ));
                *endpoint = url.into();
            }
            if model.is_empty() {
                *model = required_env("OPENAI_MODEL")?;
            }
        }
        Ok(())
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self::Script
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExperimentConfig {
    pub experiment: String,
    pub model: ModelConfig,
    pub generation: GenerateParams,
    pub system_prompt: String,
    pub user_message: String,
    pub max_rounds: u64,
    pub max_boundaries: u64,
    pub scenario: ScenarioConfig,
}

impl Default for ExperimentConfig {
    fn default() -> Self {
        Self {
            experiment: "react-memory".into(),
            model: ModelConfig::Script,
            generation: GenerateParams::default(),
            system_prompt:
                "Use the provided tools. Read counter, write counter = 1, then explain the result."
                    .into(),
            user_message: "Set counter to 1 using memory tools.".into(),
            max_rounds: 16,
            max_boundaries: 128,
            scenario: ScenarioConfig::default(),
        }
    }
}
