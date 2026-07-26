use super::*;

#[derive(Clone, Debug)]
pub struct FileReporterService {
    pub refine_dir: PathBuf,
}

impl FileReporterService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
        }
    }

    pub fn list(&self) -> RefineResult<Value> {
        Ok(json!({"reporters": self.load_reporters()?}))
    }

    pub fn create(&self, name: &str) -> RefineResult<Value> {
        let clean = normalize_reporter_name(name)?;
        let mut reporters = self.load_reporters()?;
        if let Some(existing) = reporters.iter().find(|reporter| {
            reporter.get("name").and_then(|value| value.as_str()) == Some(clean.as_str())
        }) {
            return Ok(json!({"reporter": existing}));
        }
        let next_id = reporters
            .iter()
            .filter_map(|reporter| reporter.get("id").and_then(|value| value.as_u64()))
            .max()
            .unwrap_or(0)
            + 1;
        let reporter = json!({"id": next_id, "name": clean, "created": now_timestamp()});
        reporters.push(reporter.clone());
        self.save_reporters(&reporters)?;
        Ok(json!({"reporter": reporter}))
    }

    pub fn rename(&self, id: u64, name: &str) -> RefineResult<Value> {
        let clean = normalize_reporter_name(name)?;
        let mut reporters = self.load_reporters()?;
        let Some(reporter) = reporters
            .iter_mut()
            .find(|reporter| reporter.get("id").and_then(|value| value.as_u64()) == Some(id))
        else {
            return Err(RefineError::NotFound(format!(
                "Reporter {id} was not found"
            )));
        };
        let old = reporter
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        reporter["name"] = Value::String(clean.clone());
        self.save_reporters(&reporters)?;
        if old != clean {
            rewrite_reporter_references(&self.refine_dir, &old, &clean)?;
        }
        Ok(json!({"ok": true, "old": old, "new": clean}))
    }

    pub fn delete(&self, id: u64) -> RefineResult<Value> {
        let mut reporters = self.load_reporters()?;
        let len = reporters.len();
        reporters
            .retain(|reporter| reporter.get("id").and_then(|value| value.as_u64()) != Some(id));
        if reporters.len() == len {
            return Err(RefineError::NotFound(format!(
                "Reporter {id} was not found"
            )));
        }
        self.save_reporters(&reporters)?;
        Ok(json!({"ok": true}))
    }

    pub fn merge(&self, id: u64, target_id: u64) -> RefineResult<Value> {
        if id == target_id {
            return Err(RefineError::InvalidInput(
                "cannot merge a reporter into itself".to_string(),
            ));
        }
        let reporters = self.load_reporters()?;
        let old = reporters
            .iter()
            .find(|reporter| reporter.get("id").and_then(|value| value.as_u64()) == Some(id))
            .and_then(|reporter| reporter.get("name").and_then(|value| value.as_str()))
            .unwrap_or("")
            .to_string();
        let new = reporters
            .iter()
            .find(|reporter| reporter.get("id").and_then(|value| value.as_u64()) == Some(target_id))
            .and_then(|reporter| reporter.get("name").and_then(|value| value.as_str()))
            .unwrap_or("")
            .to_string();
        if old.is_empty() || new.is_empty() {
            return Err(RefineError::NotFound("Reporter was not found".to_string()));
        }
        self.delete(id)?;
        rewrite_reporter_references(&self.refine_dir, &old, &new)?;
        Ok(json!({"ok": true, "old": old, "new": new}))
    }

    fn load_reporters(&self) -> RefineResult<Vec<Value>> {
        let value = read_json_or_default(self.refine_dir.join(REPORTERS_FILE), json!([]))?;
        let reporters = normalize_reporters(&value);
        if reporters.is_empty() {
            let seeded = self.seed_reporters_from_goal_rounds()?;
            if !seeded.is_empty() {
                self.save_reporters(&seeded)?;
                return Ok(seeded);
            }
        }
        Ok(reporters)
    }

    fn save_reporters(&self, reporters: &[Value]) -> RefineResult<()> {
        write_json(
            self.refine_dir.join(REPORTERS_FILE),
            &Value::Array(reporters.to_vec()),
        )
    }

    fn seed_reporters_from_goal_rounds(&self) -> RefineResult<Vec<Value>> {
        let mut names = BTreeSet::new();
        collect_reporter_names(&self.refine_dir.join("goals"), "goal.json", &mut names)?;
        collect_reporter_names(
            &self.refine_dir.join("features"),
            "feature.json",
            &mut names,
        )?;
        let now = now_timestamp();
        Ok(names
            .into_iter()
            .enumerate()
            .map(|(idx, name)| json!({"id": idx + 1, "name": name, "created": now}))
            .collect())
    }
}
