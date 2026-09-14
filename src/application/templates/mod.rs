//! Editable, project-owned sources for every Refine-authored agent prompt.
mod catalog;
mod rendering;
mod store;

pub use catalog::{definition, definitions, variables};
pub use rendering::{TemplateScope, TemplateSnapshot, TemplateValue, Variables};
pub use store::TemplateStore;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TemplateRecord {
    pub id: String,
    pub revision: u64,
    pub prompt: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct TemplateDefinition {
    pub id: String,
    pub name: String,
    pub default_prompt: String,
}

#[cfg(test)]
mod tests;

/// Validate a draft using the same rules as persisted Templates.
pub fn validate_prompt(prompt: &str) -> crate::error::RefineResult<()> {
    if prompt.len() > store::MAX_TEMPLATE_BYTES {
        return Err(crate::error::RefineError::InvalidInput(
            "Template exceeds 128 KiB".into(),
        ));
    }
    catalog::validate(prompt).map_err(crate::error::RefineError::InvalidInput)
}
