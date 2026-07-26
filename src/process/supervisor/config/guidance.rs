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
        let value = read_json_or_default(self.refine_dir.join(GUIDANCE_FILE), json!([]))?;
        Ok(json!({"guidance": normalize_guidance_list(&value)}))
    }

    pub fn update(&self, body: &Value) -> RefineResult<Value> {
        let Some(items) = body.get("guidance") else {
            return Err(RefineError::InvalidInput(
                "guidance must be a list".to_string(),
            ));
        };
        if !items.is_array() {
            return Err(RefineError::InvalidInput(
                "guidance must be a list".to_string(),
            ));
        }
        let guidance = normalize_guidance_list(items);
        write_json(self.refine_dir.join(GUIDANCE_FILE), &guidance)?;
        Ok(json!({"guidance": guidance}))
    }
}
