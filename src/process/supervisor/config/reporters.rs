use super::*;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ReporterCascade {
    phase: String,
    operation: String,
    source_id: u64,
    old: String,
    new: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    staged_reporter_id: Option<u64>,
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ReporterCascadeTestStage {
    BeforeRecord(PathBuf),
    RecordLocked(PathBuf),
    AfterRecord(PathBuf),
}

#[derive(Clone)]
pub struct FileReporterService {
    pub refine_dir: PathBuf,
    #[cfg(test)]
    cascade_hook: Option<Arc<dyn Fn(ReporterCascadeTestStage) + Send + Sync>>,
}

impl std::fmt::Debug for FileReporterService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileReporterService")
            .field("refine_dir", &self.refine_dir)
            .finish_non_exhaustive()
    }
}

impl FileReporterService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            #[cfg(test)]
            cascade_hook: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_cascade_hook(
        refine_dir: impl Into<PathBuf>,
        hook: Arc<dyn Fn(ReporterCascadeTestStage) + Send + Sync>,
    ) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            cascade_hook: Some(hook),
        }
    }

    pub fn list(&self) -> RefineResult<Value> {
        self.with_registry_lock(|| Ok(json!({"reporters": self.load_reporters()?})))
    }

    pub fn create(&self, name: &str) -> RefineResult<Value> {
        self.with_registry_lock(|| self.create_locked(name))
    }

    /// Keep registration and its dependent durable mutation in one ordered
    /// critical section. Reporter-aware Goal writers take the registry first
    /// and the canonical Goal record lock second; cascades use the same order.
    pub(crate) fn with_registered<T>(
        &self,
        name: &str,
        action: impl FnOnce() -> RefineResult<T>,
    ) -> RefineResult<T> {
        let name = name.trim();
        if name.is_empty() {
            return action();
        }
        self.with_registry_lock(|| {
            self.create_locked(name)?;
            action()
        })
    }

    pub fn rename(&self, id: u64, name: &str) -> RefineResult<Value> {
        self.with_registry_lock(|| {
            let clean = normalize_reporter_name(name)?;
            let reporters = self.load_reporters()?;
            let old = reporter_name(&reporters, id)?;
            if old == clean {
                return Ok(json!({"ok": true, "old": old, "new": clean}));
            }
            let staged_reporter_id = if reporters.iter().any(|reporter| {
                reporter.get("name").and_then(Value::as_str) == Some(clean.as_str())
            }) {
                None
            } else {
                Some(next_reporter_id(&reporters))
            };
            let cascade = ReporterCascade {
                phase: "prepared".to_string(),
                operation: "rename".to_string(),
                source_id: id,
                old,
                new: clean,
                staged_reporter_id,
            };
            self.save_cascade(&cascade)?;
            self.resume_cascade_locked(&cascade)
        })
    }

    pub fn delete(&self, id: u64) -> RefineResult<Value> {
        self.with_registry_lock(|| {
            let mut reporters = self.load_reporters()?;
            let len = reporters.len();
            reporters.retain(|reporter| reporter.get("id").and_then(Value::as_u64) != Some(id));
            if reporters.len() == len {
                return Err(RefineError::NotFound(format!(
                    "Reporter {id} was not found"
                )));
            }
            self.save_reporters(&reporters)?;
            Ok(json!({"ok": true}))
        })
    }

    pub fn merge(&self, id: u64, target_id: u64) -> RefineResult<Value> {
        self.with_registry_lock(|| {
            if id == target_id {
                return Err(RefineError::InvalidInput(
                    "cannot merge a reporter into itself".to_string(),
                ));
            }
            let reporters = self.load_reporters()?;
            let cascade = ReporterCascade {
                phase: "prepared".to_string(),
                operation: "merge".to_string(),
                source_id: id,
                old: reporter_name(&reporters, id)?,
                new: reporter_name(&reporters, target_id)?,
                staged_reporter_id: None,
            };
            self.save_cascade(&cascade)?;
            self.resume_cascade_locked(&cascade)
        })
    }

    fn create_locked(&self, name: &str) -> RefineResult<Value> {
        let clean = normalize_reporter_name(name)?;
        let mut reporters = self.load_reporters()?;
        if let Some(existing) = reporters
            .iter()
            .find(|reporter| reporter.get("name").and_then(Value::as_str) == Some(clean.as_str()))
        {
            return Ok(json!({"reporter": existing}));
        }
        let reporter = new_reporter(next_reporter_id(&reporters), &clean);
        reporters.push(reporter.clone());
        self.save_reporters(&reporters)?;
        Ok(json!({"reporter": reporter}))
    }

    fn with_registry_lock<T>(&self, action: impl FnOnce() -> RefineResult<T>) -> RefineResult<T> {
        with_record_lock(&self.refine_dir, REPORTERS_FILE, || {
            self.recover_pending_cascade_locked()?;
            action()
        })
    }

    fn recover_pending_cascade_locked(&self) -> RefineResult<()> {
        let path = self.cascade_path();
        if !path.exists() {
            return Ok(());
        }
        let value = read_json_or_default(path.clone(), json!({}))?;
        let cascade: ReporterCascade = serde_json::from_value(value).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse Reporter cascade {}: {error}",
                path.display()
            ))
        })?;
        self.resume_cascade_locked(&cascade).map(|_| ())
    }

    fn resume_cascade_locked(&self, cascade: &ReporterCascade) -> RefineResult<Value> {
        let mut cascade = cascade.clone();
        match cascade.phase.as_str() {
            "prepared" => {
                self.ensure_cascade_names_registered(&cascade)?;
                rewrite_reporter_references(
                    &self.refine_dir,
                    &cascade.old,
                    &cascade.new,
                    |stage| self.observe_cascade_stage(stage),
                )?;
                cascade.phase = "cascaded".to_string();
                self.save_cascade(&cascade)?;
            }
            "cascaded" => {}
            phase => {
                return Err(RefineError::Serialization(format!(
                    "unsupported Reporter cascade phase {phase}"
                )));
            }
        }
        let mut reporters = self.load_reporters()?;
        match cascade.operation.as_str() {
            "rename" => {
                let reporter = reporters
                    .iter_mut()
                    .find(|reporter| {
                        reporter.get("id").and_then(Value::as_u64) == Some(cascade.source_id)
                    })
                    .ok_or_else(|| {
                        RefineError::NotFound(format!(
                            "Reporter {} was not found",
                            cascade.source_id
                        ))
                    })?;
                reporter["name"] = Value::String(cascade.new.clone());
                if let Some(staged_id) = cascade.staged_reporter_id {
                    reporters.retain(|reporter| {
                        reporter.get("id").and_then(Value::as_u64) != Some(staged_id)
                    });
                }
            }
            "merge" => reporters.retain(|reporter| {
                reporter.get("id").and_then(Value::as_u64) != Some(cascade.source_id)
            }),
            operation => {
                return Err(RefineError::Serialization(format!(
                    "unsupported Reporter cascade operation {operation}"
                )));
            }
        }
        self.save_reporters(&reporters)?;
        self.remove_cascade()?;
        Ok(json!({"ok": true, "old": cascade.old, "new": cascade.new}))
    }

    fn ensure_cascade_names_registered(&self, cascade: &ReporterCascade) -> RefineResult<()> {
        let mut reporters = self.load_reporters()?;
        let mut changed = false;
        if !reporters.iter().any(|reporter| {
            reporter.get("name").and_then(Value::as_str) == Some(cascade.old.as_str())
        }) {
            reporters.push(new_reporter(cascade.source_id, &cascade.old));
            changed = true;
        }
        if !reporters.iter().any(|reporter| {
            reporter.get("name").and_then(Value::as_str) == Some(cascade.new.as_str())
        }) {
            let id = cascade
                .staged_reporter_id
                .unwrap_or_else(|| next_reporter_id(&reporters));
            reporters.push(new_reporter(id, &cascade.new));
            changed = true;
        }
        if changed {
            self.save_reporters(&reporters)?;
        }
        Ok(())
    }

    fn observe_cascade_stage(&self, stage: ReporterCascadeStage<'_>) {
        #[cfg(test)]
        if let Some(hook) = &self.cascade_hook {
            hook(match stage {
                ReporterCascadeStage::BeforeRecord(path) => {
                    ReporterCascadeTestStage::BeforeRecord(path.to_path_buf())
                }
                ReporterCascadeStage::RecordLocked(path) => {
                    ReporterCascadeTestStage::RecordLocked(path.to_path_buf())
                }
                ReporterCascadeStage::AfterRecord(path) => {
                    ReporterCascadeTestStage::AfterRecord(path.to_path_buf())
                }
            });
        }
        #[cfg(not(test))]
        match stage {
            ReporterCascadeStage::BeforeRecord(path)
            | ReporterCascadeStage::RecordLocked(path)
            | ReporterCascadeStage::AfterRecord(path) => {
                let _ = path;
            }
        }
    }

    fn load_reporters(&self) -> RefineResult<Vec<Value>> {
        let path = self.refine_dir.join(REPORTERS_FILE);
        if path.exists() {
            return read_json_or_default(path, json!([])).map(|value| normalize_reporters(&value));
        }
        let seeded = self.seed_reporters_from_goal_rounds()?;
        if !seeded.is_empty() {
            self.save_reporters(&seeded)?;
        }
        Ok(seeded)
    }

    fn save_reporters(&self, reporters: &[Value]) -> RefineResult<()> {
        write_value_durably(
            &self.refine_dir.join(REPORTERS_FILE),
            &Value::Array(reporters.to_vec()),
        )
    }

    fn save_cascade(&self, cascade: &ReporterCascade) -> RefineResult<()> {
        let value = serde_json::to_value(cascade).map_err(|error| {
            RefineError::Serialization(format!("failed to encode Reporter cascade: {error}"))
        })?;
        write_value_durably(&self.cascade_path(), &value)
    }

    fn remove_cascade(&self) -> RefineResult<()> {
        let path = self.cascade_path();
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to remove Reporter cascade {}: {error}",
                    path.display()
                )));
            }
        }
        fs::File::open(&self.refine_dir)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to sync Reporter cascade directory {}: {error}",
                    self.refine_dir.display()
                ))
            })
    }

    fn cascade_path(&self) -> PathBuf {
        self.refine_dir.join(REPORTER_CASCADE_FILE)
    }

    fn seed_reporters_from_goal_rounds(&self) -> RefineResult<Vec<Value>> {
        let mut names = BTreeSet::new();
        collect_reporter_names(&self.refine_dir.join("goals"), "goal.json", &mut names)?;
        collect_reporter_names(
            &self.refine_dir.join("features"),
            "feature.json",
            &mut names,
        )?;
        Ok(names
            .into_iter()
            .enumerate()
            .map(|(index, name)| new_reporter(index as u64 + 1, &name))
            .collect())
    }
}

fn reporter_name(reporters: &[Value], id: u64) -> RefineResult<String> {
    reporters
        .iter()
        .find(|reporter| reporter.get("id").and_then(Value::as_u64) == Some(id))
        .and_then(|reporter| reporter.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .ok_or_else(|| RefineError::NotFound(format!("Reporter {id} was not found")))
}

fn next_reporter_id(reporters: &[Value]) -> u64 {
    reporters
        .iter()
        .filter_map(|reporter| reporter.get("id").and_then(Value::as_u64))
        .max()
        .unwrap_or(0)
        + 1
}

fn new_reporter(id: u64, name: &str) -> Value {
    json!({"id": id, "name": name, "created": now_timestamp()})
}

fn write_value_durably(path: &Path, value: &Value) -> RefineResult<()> {
    let mut encoded = serde_json::to_vec_pretty(value)
        .map_err(|error| RefineError::Serialization(format!("failed to encode JSON: {error}")))?;
    encoded.push(b'\n');
    replace_file_durably(path, &encoded)
}
