//! Shared provider catalog persistence. The node lock also fences reference validation.
use super::automation::write_json;
use crate::application::fleet::nodes::FileNodeRegistryService;
use crate::error::{RefineError, RefineResult};
use crate::model::providers::{ProviderCatalog, ProviderDefinition, defaults};
use std::path::{Path, PathBuf};

pub struct ProviderStore {
    root: PathBuf,
}
impl ProviderStore {
    pub fn new(root: &Path) -> Self {
        Self { root: root.into() }
    }
    pub fn load(&self) -> RefineResult<ProviderCatalog> {
        FileNodeRegistryService::new(&self.root).with_registry_lock(|| self.load_locked())
    }
    fn load_locked(&self) -> RefineResult<ProviderCatalog> {
        let path = self.root.join("providers.json");
        let catalog = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| RefineError::Serialization(format!("{}: {e}", path.display())))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Virtual defaults do not compete with a catalog adopted from another node.
                // Existing generic executable selections are persisted during migration.
                let mut catalog = defaults();
                let registry = FileNodeRegistryService::new(&self.root).load_registry()?;
                for node in registry.nodes {
                    if let Some(id) = node
                        .settings
                        .get("agent_cli")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        if catalog.provider(id).is_err() {
                            catalog.providers.push(ProviderDefinition::generic(id));
                        }
                    }
                }
                if catalog.providers.len() > defaults().providers.len() {
                    catalog.validate()?;
                    catalog.revision = 1;
                    write_json(&path, &catalog)?;
                }
                catalog
            }
            Err(e) => return Err(RefineError::Io(format!("{}: {e}", path.display()))),
        };
        catalog.validate()?;
        Ok(catalog)
    }
    pub fn save(&self, mut catalog: ProviderCatalog) -> RefineResult<ProviderCatalog> {
        catalog.validate()?;
        FileNodeRegistryService::new(&self.root).with_registry_lock(|| {
            let current = self.load_locked()?;
            if current.revision != catalog.revision {
                return Err(RefineError::Conflict("AI provider catalog changed; reload and reapply your edits".into()));
            }
            for node in FileNodeRegistryService::new(&self.root).load_registry()?.nodes {
                if let Some(id) = node.settings.get("agent_cli").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
                    if catalog.provider(id).is_err() {
                        return Err(RefineError::Conflict(format!("AI provider {id:?} is selected by node {}; change or clear that node selection before deleting it", node.id)));
                    }
                }
            }
            catalog.revision = catalog.revision.checked_add(1).ok_or_else(|| RefineError::InvalidInput("provider revision exhausted".into()))?;
            write_json(&self.root.join("providers.json"), &catalog)?;
            Ok(catalog)
        })
    }
}
