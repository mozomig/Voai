mod catalog;
mod connected;
mod processing;
mod store;

pub use catalog::{CATALOG, ModelFile, ModelKind, ModelSpec, find};
pub use connected::{ConnectedModel, local_model_size_limit, validate_text_model};
pub use processing::server::{
    has_server_key, normalize_server_url, process_server, server_models, set_server_key,
};
pub use processing::{
    AgentCli, AgentConfig, AgentProfiles, MAX_PROMPT_BYTES, Provider, default_processing_prompt,
    has_api_key, process_agent, process_cloud, process_local, set_api_key,
    validate_processing_prompt,
};
pub use store::ModelStore;

use std::sync::atomic::{AtomicBool, Ordering};

fn check_cancelled(cancel: &AtomicBool) -> anyhow::Result<()> {
    anyhow::ensure!(
        !cancel.load(Ordering::Acquire),
        voai_l10n::tr("operation.cancelled")
    );
    Ok(())
}
