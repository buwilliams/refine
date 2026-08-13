use super::*;

#[derive(Clone, Debug)]
pub struct FileGuidanceService {
    pub refine_dir: PathBuf,
}

impl FileGuidanceService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
        }
    }

    pub fn list(&self) -> RefineResult<Value> {
        self.with_locked_document(|document| Ok(document.clone()))
    }

    pub fn update(&self, body: &Value) -> RefineResult<Value> {
        validate_guidance_document_patch(body)?;
        let items = body
            .get("guidance")
            .ok_or_else(|| RefineError::InvalidInput("guidance must be a list".to_string()))?;
        let guidance = normalize_guidance_input(items)?;
        self.mutate(body, move |document| {
            document["guidance"] = guidance;
            Ok(())
        })
    }

    pub fn add(&self, body: &Value) -> RefineResult<Value> {
        validate_guidance_fields(body, true)?;
        self.mutate(body, |document| {
            let items = document["guidance"]
                .as_array_mut()
                .expect("normalized list");
            let mut item = body.clone();
            item.as_object_mut()
                .expect("validated object")
                .remove("revision");
            item["id"] = Value::String(new_guidance_id(items));
            let normalized = normalize_guidance_input(&json!([item]))?;
            items.push(normalized[0].clone());
            Ok(())
        })
    }

    pub fn edit(&self, id: &str, body: &Value) -> RefineResult<Value> {
        validate_guidance_fields(body, false)?;
        self.mutate(body, |document| {
            let items = document["guidance"]
                .as_array_mut()
                .expect("normalized list");
            let item = items
                .iter_mut()
                .find(|item| item["id"] == id)
                .ok_or_else(|| {
                    RefineError::NotFound(format!("Guidance entry {id} was not found"))
                })?;
            for key in ["name", "rule", "instructions", "enabled"] {
                if let Some(value) = body.get(key) {
                    item[key] = value.clone();
                }
            }
            validate_guidance_fields(item, true)?;
            Ok(())
        })
    }

    pub fn remove(&self, id: &str, body: &Value) -> RefineResult<Value> {
        validate_guidance_remove(body)?;
        self.mutate(body, |document| {
            let items = document["guidance"]
                .as_array_mut()
                .expect("normalized list");
            let before = items.len();
            items.retain(|item| item["id"] != id);
            if items.len() == before {
                return Err(RefineError::NotFound(format!(
                    "Guidance entry {id} was not found"
                )));
            }
            Ok(())
        })
    }

    fn mutate(
        &self,
        body: &Value,
        mutation: impl FnOnce(&mut Value) -> RefineResult<()>,
    ) -> RefineResult<Value> {
        let expected = guidance_revision(body)?;
        self.with_locked_document(|document| {
            let current = document["revision"].as_u64().unwrap_or(0);
            let expected = expected.or((current == 0).then_some(0)).ok_or_else(|| {
                RefineError::Conflict(format!(
                    "Guidance revision is required; current revision is {current}"
                ))
            })?;
            if expected != current {
                return Err(RefineError::Conflict(format!(
                    "Guidance changed after it was read (expected revision {expected}, current revision {current})"
                )));
            }
            mutation(document)?;
            document["revision"] = json!(current.saturating_add(1));
            self.write_document(document)?;
            Ok(document.clone())
        })
    }

    pub fn validate_update(body: &Value) -> RefineResult<()> {
        validate_guidance_document_patch(body)
    }

    pub fn validate_add(body: &Value) -> RefineResult<()> {
        validate_guidance_fields(body, true)
    }

    pub fn validate_edit(body: &Value) -> RefineResult<()> {
        validate_guidance_fields(body, false)
    }

    pub fn validate_remove(body: &Value) -> RefineResult<()> {
        validate_guidance_remove(body)
    }

    fn with_locked_document<T>(
        &self,
        action: impl FnOnce(&mut Value) -> RefineResult<T>,
    ) -> RefineResult<T> {
        let path = self.refine_dir.join(GUIDANCE_FILE);
        let key = record_lock_key(&path);
        with_record_lock(&self.refine_dir, &key, || {
            let existed = path.exists();
            let raw = read_json_or_default(path, json!([]))?;
            if raw.is_object() && raw.get("guidance").is_some_and(|value| !value.is_array()) {
                return Err(RefineError::Serialization(
                    "guidance.json guidance must be a list".to_string(),
                ));
            }
            let mut document = normalize_guidance_document(&raw);
            if existed && document != raw {
                self.write_document(&document)?;
            }
            action(&mut document)
        })
    }

    fn write_document(&self, document: &Value) -> RefineResult<()> {
        write_json(self.refine_dir.join(GUIDANCE_FILE), document)
    }
}
