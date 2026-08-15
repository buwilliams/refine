use std::path::{Path, PathBuf};

use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::project_layout::refine_dir_for_target_root;
use crate::tools::product::project_registry::FileProjectRegistryService;

use super::{AgentProviderService, HostAgentProviderService};

/// Pick the provider that performs delegated maintenance work: an explicit
/// `--provider` wins, then the active target app's configured provider, then
/// the first installed provider CLI in alphabetical order.
pub fn resolve_agent_provider(
    runtime_root: &Path,
    explicit: Option<String>,
) -> RefineResult<String> {
    if let Some(provider) = explicit
        .map(|provider| provider.trim().to_string())
        .filter(|provider| !provider.is_empty())
    {
        return Ok(provider);
    }
    if let Some(provider) = target_app_configured_provider(runtime_root) {
        return Ok(provider);
    }
    first_installed_provider()
}

fn target_app_configured_provider(runtime_root: &Path) -> Option<String> {
    let registry = FileProjectRegistryService::new(runtime_root, None)
        .load()
        .ok()?;
    let target_root = PathBuf::from(registry.active_app?);
    let refine_dir = refine_dir_for_target_root(&target_root).ok()?;
    let settings = FileSettingsService::with_active_root(refine_dir, target_root)
        .load()
        .ok()?;
    settings
        .get("agent_cli")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .map(str::to_string)
}

fn first_installed_provider() -> RefineResult<String> {
    let mut installed = HostAgentProviderService::new()
        .detect()?
        .into_iter()
        .filter(|capability| capability.installed && capability.name != "smoke-ai")
        .map(|capability| capability.name)
        .collect::<Vec<_>>();
    installed.sort();
    installed.into_iter().next().ok_or_else(|| {
        RefineError::NotFound(
            "no agent provider CLI found on PATH; install one (e.g. claude, codex, copilot, gemini) or pass --provider".to_string(),
        )
    })
}
