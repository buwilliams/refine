use crate::application::projects::registry::FileProjectRegistryService;
use crate::error::RefineResult;
use crate::infrastructure::process::supervisor::config::FileSettingsService;
use crate::infrastructure::storage::providers::ProviderStore;
use std::path::Path;

/// Explicit selection, node override, system default. Installation never changes selection.
pub fn resolve_agent_provider(
    runtime_root: &Path,
    explicit: Option<String>,
) -> RefineResult<String> {
    let status = FileProjectRegistryService::new(runtime_root, None).status()?;
    let (catalog, node) = if let Some(root) = status.refine_dir {
        let root = std::path::PathBuf::from(root);
        (
            ProviderStore::new(&root).load()?,
            FileSettingsService::with_active_root(&root, runtime_root).provider_override()?,
        )
    } else {
        (crate::model::providers::defaults(), None)
    };
    super::providers::resolve(&catalog, explicit.as_deref(), node.as_deref()).map(|p| p.id)
}
