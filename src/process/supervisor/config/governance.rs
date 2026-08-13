use super::*;

#[derive(Clone, Debug)]
pub struct FileGovernanceService {
    pub refine_dir: PathBuf,
}

impl FileGovernanceService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
        }
    }

    pub fn load(&self) -> RefineResult<Value> {
        self.with_locked_governance(|value| Ok(value.clone()))
    }

    pub fn save(&self, body: &Value) -> RefineResult<Value> {
        let updates = body.as_object().ok_or_else(|| {
            RefineError::InvalidInput("Governance patch must be a JSON object".to_string())
        })?;
        if updates.is_empty() {
            return Err(RefineError::InvalidInput(
                "Governance patch requires at least one value".to_string(),
            ));
        }
        for key in updates.keys() {
            if !matches!(
                key.as_str(),
                "product"
                    | "constitution"
                    | "rules"
                    | "rules_revision"
                    | "max_automatic_round_retries"
            ) {
                return Err(RefineError::InvalidInput(format!(
                    "unknown Governance field: {key}"
                )));
            }
        }
        self.with_locked_governance(|current| {
            if let Some(product) = body.get("product") {
                let product = product.as_str().ok_or_else(|| {
                    RefineError::InvalidInput("product must be a string".to_string())
                })?;
                current["product"] = Value::String(product.trim().to_string());
            }
            if let Some(constitution) = body.get("constitution") {
                let constitution = constitution.as_str().ok_or_else(|| {
                    RefineError::InvalidInput("constitution must be a string".to_string())
                })?;
                current["constitution"] = Value::String(constitution.trim().to_string());
            }
            if let Some(max_retries) = body.get("max_automatic_round_retries") {
                let max_retries = max_retries.as_u64().ok_or_else(|| {
                    RefineError::InvalidInput(
                        "max_automatic_round_retries must be a nonnegative integer".to_string(),
                    )
                })?;
                let max_retries = u32::try_from(max_retries).map_err(|_| {
                    RefineError::InvalidInput(
                        "max_automatic_round_retries must fit in a 32-bit integer".to_string(),
                    )
                })?;
                current["max_automatic_round_retries"] = json!(max_retries);
            }
            if let Some(rules) = body.get("rules") {
                if !rules.is_array() {
                    return Err(RefineError::InvalidInput("rules must be a list".to_string()));
                }
                let revision = current["rules_revision"].as_u64().unwrap_or(0);
                let expected = body
                    .get("rules_revision")
                    .and_then(Value::as_u64)
                    .or((revision == 0).then_some(0))
                    .ok_or_else(|| {
                        RefineError::Conflict(format!(
                            "Governance rules_revision is required; current revision is {revision}"
                        ))
                    })?;
                if expected != revision {
                    return Err(RefineError::Conflict(format!(
                        "Governance rules changed after they were read (expected revision {expected}, current revision {revision})"
                    )));
                }
                current["rules"] = normalize_rules(rules);
                current["rules_revision"] = json!(revision.saturating_add(1));
            }
            normalize_governance(current);
            self.write_governance(current)?;
            Ok(current.clone())
        })
    }

    pub fn generate_rules(&self, body: &Value) -> RefineResult<Value> {
        let product = body
            .get("product")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let constitution = body
            .get("constitution")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if product.is_empty() || constitution.is_empty() {
            return Err(RefineError::InvalidInput(
                "product and constitution are required".to_string(),
            ));
        }
        Ok(json!({
            "ok": true,
            "rules": [
                governance_rule("Keep implementation aligned with the documented product intent.", "generated"),
                governance_rule("Respect the project constitution before adding new behavior.", "generated")
            ],
            "raw": ""
        }))
    }

    fn with_locked_governance<T>(
        &self,
        action: impl FnOnce(&mut Value) -> RefineResult<T>,
    ) -> RefineResult<T> {
        let path = self.refine_dir.join(GOVERNANCE_FILE);
        let key = record_lock_key(&path);
        with_record_lock(&self.refine_dir, &key, || {
            let existed = path.exists();
            let mut value = read_json_or_default(
                path,
                json!({"product": "", "constitution": "", "rules": [], "max_automatic_round_retries": 5}),
            )?;
            let before = value.clone();
            normalize_governance(&mut value);
            if existed && value != before {
                self.write_governance(&value)?;
            }
            action(&mut value)
        })
    }

    fn write_governance(&self, value: &Value) -> RefineResult<()> {
        write_json(self.refine_dir.join(GOVERNANCE_FILE), value)
    }
}
