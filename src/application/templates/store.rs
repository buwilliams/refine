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

    fn consistent<T>(&self, action: impl FnOnce() -> RefineResult<T>) -> RefineResult<T> {
        if let Some(root) = &self.root {
            with_record_lock(root, "templates", || {
                self.finish_reset()?;
                action()
            })
        } else {
            action()
        }
    }

    // A durable reset journal lets readers finish an interrupted batch before observing it.
    fn finish_reset(&self) -> RefineResult<()> {
        let Some(root) = &self.root else {
            return Ok(());
        };
        let path = root.join("templates/reset-pending.json");
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(RefineError::Io(error.to_string())),
        };
        let records: Vec<TemplateRecord> = serde_json::from_slice(&bytes)
            .map_err(|error| RefineError::Serialization(error.to_string()))?;
        let mut ids = std::collections::BTreeSet::new();
        for record in &records {
            if definition(&record.id).is_none() || record.revision == 0 || !ids.insert(&record.id) {
                return Err(RefineError::InvalidInput(
                    "Invalid template reset journal".into(),
                ));
            }
            super::validate_prompt(&record.prompt)?;
        }
        for record in records {
            write_json(
                &root.join("templates").join(format!("{}.json", record.id)),
                &record,
            )?;
        }
        std::fs::remove_file(path).map_err(|error| RefineError::Io(error.to_string()))?;
        Ok(())
    }

    pub fn read(&self, id: &str) -> RefineResult<TemplateRecord> {
        self.consistent(|| self.read_record(id))
    }

    fn read_record(&self, id: &str) -> RefineResult<TemplateRecord> {
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
            json!({"item":item,"name":definition(id).unwrap().name,"customized":item.prompt != definition(id).unwrap().default_prompt,"default_prompt":definition(id).unwrap().default_prompt,"usage":self.usage(id),"variables":super::variables()}),
        )
    }

    fn usage(&self, id: &str) -> super::usage::TemplateUsage {
        let template = crate::application::agent_io::prompts::PromptTemplate::ALL
            .iter()
            .find(|template| template.id() == id)
            .expect("catalog entry");
        super::usage::usage(*template)
    }

    pub fn list(&self) -> RefineResult<Value> {
        self.consistent(|| self.list_records())
    }

    fn list_records(&self) -> RefineResult<Value> {
        Ok(json!({"items": definitions().iter().map(|definition| {
            let item = self.read_record(&definition.id)?;
            Ok(json!({"name":definition.name,"customized":item.prompt != definition.default_prompt,"default_prompt":definition.default_prompt,"usage":self.usage(&definition.id),"item":item}))
        }).collect::<RefineResult<Vec<_>>>()?}))
    }

    pub fn snapshot(&self) -> RefineResult<TemplateSnapshot> {
        self.consistent(|| self.snapshot_records())
    }

    fn snapshot_records(&self) -> RefineResult<TemplateSnapshot> {
        Ok(TemplateSnapshot {
            records: definitions()
                .iter()
                .map(|item| {
                    self.read_record(&item.id)
                        .map(|record| (item.id.clone(), record))
                })
                .collect::<RefineResult<_>>()?,
        })
    }

    pub fn reset(
        &self,
        revisions: &std::collections::BTreeMap<String, u64>,
    ) -> RefineResult<Value> {
        let root = self.root.as_ref().ok_or_else(|| {
            RefineError::InvalidInput("Attach a project to reset Templates".into())
        })?;
        if revisions.is_empty() {
            return Err(RefineError::InvalidInput(
                "Choose templates to reset".into(),
            ));
        }
        self.consistent(|| {
            let mut snapshot = self.snapshot_records()?;
            let mut records = Vec::new();
            for (id, revision) in revisions {
                let default = definition(id)
                    .ok_or_else(|| RefineError::NotFound(format!("Template {id}")))?;
                let current = &snapshot.records[id];
                if current.revision != *revision {
                    return Err(RefineError::Conflict(
                        "Templates changed; reopen the reset preview".into(),
                    ));
                }
                if current.prompt == default.default_prompt {
                    continue;
                }
                let record = TemplateRecord {
                    id: id.clone(),
                    revision: revision.checked_add(1).ok_or_else(|| {
                        RefineError::InvalidInput("Template revision exhausted".into())
                    })?,
                    prompt: default.default_prompt,
                };
                snapshot.records.insert(id.clone(), record.clone());
                records.push(record);
            }
            snapshot.validate_references()?;
            if !records.is_empty() {
                write_json(&root.join("templates/reset-pending.json"), &records)?;
                self.finish_reset()?;
            }
            Ok(json!({"reset":records.iter().map(|record| &record.id).collect::<Vec<_>>()}))
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
        self.consistent(|| {
            if self.read_record(id)?.revision != revision {
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
            let mut snapshot = self.snapshot_records()?;
            snapshot.records.insert(id.into(), record.clone());
            snapshot.validate_references()?;
            write_json(&root.join("templates").join(format!("{id}.json")), &record)?;
            self.show(id)
        })
    }
}
