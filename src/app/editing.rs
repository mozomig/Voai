//! A service selection captures the destination and model when a job starts.
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use voai_models::{AgentCli, AgentProfiles, Provider};

#[derive(Clone, Copy)]
pub enum EditorSource {
    Server,
    Api(Provider),
    Cli(AgentCli),
}

impl EditorSource {
    pub const CLOUD: [Self; 4] = [
        Self::Api(Provider::OpenAI),
        Self::Api(Provider::Anthropic),
        Self::Cli(AgentCli::Codex),
        Self::Cli(AgentCli::Claude),
    ];
    pub fn services() -> impl Iterator<Item = Self> {
        [Self::Server].into_iter().chain(Self::CLOUD)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Server => voai_l10n::tr("ui.mode.server"),
            Self::Api(provider) => provider.name(),
            Self::Cli(agent) => agent.name(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ServiceModel {
    Server { endpoint: String, model: String },
    Api { provider: Provider, model: String },
    Cli { agent: AgentCli, model: String },
}

impl ServiceModel {
    pub fn name(&self) -> &str {
        match self {
            Self::Server { model, .. } | Self::Api { model, .. } => model,
            Self::Cli { agent, model } if model.is_empty() => agent.name(),
            Self::Cli { model, .. } => model,
        }
    }

    pub fn source_name(&self) -> &'static str {
        match self {
            Self::Server { .. } => voai_l10n::tr("ui.mode.server"),
            Self::Api { provider, .. } => provider.name(),
            Self::Cli { agent, .. } => agent.name(),
        }
    }

    pub(super) fn edit(
        &self,
        profiles: &AgentProfiles,
        prompt: &str,
        text: &str,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        match self {
            Self::Server { endpoint, model } => {
                voai_models::process_server(endpoint, model, prompt, text, cancel)
            }
            Self::Api { provider, model } => {
                voai_models::process_cloud(*provider, model, prompt, text, cancel)
            }
            Self::Cli { agent, model } => {
                let mut config = profiles.get(*agent).clone();
                config.model.clone_from(model);
                voai_models::process_agent(*agent, &config, prompt, text, cancel)
            }
        }
        .map_err(|e| e.to_string())
    }
}
