use super::{TemplateRecord, TemplateSnapshot, catalog, definition, definitions};
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::supervisor::coordination::with_record_lock;
use crate::infrastructure::storage::automation::write_json;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub const MAX_TEMPLATE_BYTES: usize = 128 * 1024;

pub struct TemplateStore {
    root: Option<PathBuf>,
}

impl TemplateStore {
    pub fn new(root: Option<&Path>) -> Self {
        Self {
            root: root.map(Path::to_path_buf),
        }
    }

    pub fn read(&self, id: &str) -> RefineResult<TemplateRecord> {
        let default =
            definition(id).ok_or_else(|| RefineError::NotFound(format!("Template {id}")))?;
        let Some(root) = &self.root else {
            return Ok(TemplateRecord {
                id: id.into(),
                revision: 0,
                prompt: default.default_prompt,
            });
        };
        let path = root.join("templates").join(format!("{id}.json"));
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(TemplateRecord {
                    id: id.into(),
                    revision: 0,
                    prompt: default.default_prompt,
                });
            }
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "read template {}: {error}",
                    path.display()
                )));
            }
        };
        if bytes.len() > MAX_TEMPLATE_BYTES * 8 {
            return Err(RefineError::InvalidInput(format!(
                "Template {id} exceeds the size limit"
            )));
        }
        let record: TemplateRecord = serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!("Invalid saved template {id}: {error}"))
        })?;
        if record.id != id || record.revision == 0 || record.prompt.len() > MAX_TEMPLATE_BYTES {
            return Err(RefineError::InvalidInput(format!(
                "Invalid saved template {id}"
            )));
        }
        catalog::validate(&record.prompt).map_err(RefineError::InvalidInput)?;
        Ok(record)
    }

    pub fn show(&self, id: &str) -> RefineResult<Value> {
        let item = self.read(id)?;
        Ok(
            json!({"item":item,"name":definition(id).unwrap().name,"customized":item.revision > 0,"default_prompt":definition(id).unwrap().default_prompt,"variables":super::variables()}),
        )
    }

    pub fn list(&self) -> RefineResult<Value> {
        Ok(json!({"items": definitions().iter().map(|definition| {
            let item = self.read(&definition.id)?;
            Ok(json!({"name":definition.name,"customized":item.revision > 0,"item":item}))
        }).collect::<RefineResult<Vec<_>>>()?}))
    }

    pub fn snapshot(&self) -> RefineResult<TemplateSnapshot> {
        Ok(TemplateSnapshot {
            records: definitions()
                .iter()
                .map(|item| self.read(&item.id).map(|record| (item.id.clone(), record)))
                .collect::<RefineResult<_>>()?,
        })
    }

    pub fn save(&self, id: &str, revision: u64, prompt: &str) -> RefineResult<Value> {
        definition(id).ok_or_else(|| RefineError::NotFound(format!("Template {id}")))?;
        if prompt.len() > MAX_TEMPLATE_BYTES {
            return Err(RefineError::InvalidInput("Template exceeds 128 KiB".into()));
        }
        catalog::validate(prompt).map_err(RefineError::InvalidInput)?;
        let root = self.root.as_ref().ok_or_else(|| {
            RefineError::InvalidInput("Attach a project to edit Templates".into())
        })?;
        with_record_lock(root, "templates", || {
            if self.read(id)?.revision != revision {
                return Err(RefineError::Conflict(
                    "Template changed; reload before saving".into(),
                ));
            }
            let record = TemplateRecord {
                id: id.into(),
                revision: revision.checked_add(1).ok_or_else(|| {
                    RefineError::InvalidInput("Template revision exhausted".into())
                })?,
                prompt: prompt.into(),
            };
            let mut snapshot = self.snapshot()?;
            snapshot.records.insert(id.into(), record.clone());
            snapshot.validate_references()?;
            write_json(&root.join("templates").join(format!("{id}.json")), &record)?;
            self.show(id)
        })
    }
}
