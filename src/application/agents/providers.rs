//! One catalog and selection contract for surfaces and launch consumers.
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::storage::providers::ProviderStore;
use crate::model::providers::{ProviderCatalog, ProviderDefinition};
use serde_json::{Value, json};
use std::path::Path;

pub fn resolve(
    catalog: &ProviderCatalog,
    explicit: Option<&str>,
    node: Option<&str>,
) -> RefineResult<ProviderDefinition> {
    catalog
        .provider(selection_id(catalog, explicit, node))
        .cloned()
}

/// Reads may expose an invalid retained selection so users can repair it; launches
/// use resolve above to require a matching definition before spawning.
pub fn selection_id<'a>(
    catalog: &'a ProviderCatalog,
    explicit: Option<&'a str>,
    node: Option<&'a str>,
) -> &'a str {
    explicit
        .filter(|s| !s.is_empty())
        .or(node.filter(|s| !s.is_empty()))
        .unwrap_or(&catalog.default_provider)
}
pub fn response(root: &Path, node_override: Option<&str>) -> RefineResult<Value> {
    let catalog = ProviderStore::new(root).load()?;
    let effective = selection_id(&catalog, None, node_override).to_string();
    let selection_error = catalog.provider(&effective).err().map(|e| e.to_string());
    Ok(
        json!({"catalog":catalog, "node_override":node_override, "effective_provider":effective, "selection_error":selection_error,
        "selection_source": if node_override.is_some() { "node" } else { "system" }}),
    )
}
pub fn save(root: &Path, body: &Value) -> RefineResult<ProviderCatalog> {
    let catalog = serde_json::from_value(body.clone())
        .map_err(|e| RefineError::InvalidInput(format!("invalid AI provider catalog: {e}")))?;
    ProviderStore::new(root).save(catalog)
}

#[cfg(test)]
mod tests;
