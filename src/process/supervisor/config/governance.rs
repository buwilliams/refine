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
        let mut value = read_json_or_default(
            self.refine_dir.join(GOVERNANCE_FILE),
            json!({"product": "", "constitution": "", "rules": [], "max_automatic_round_retries": 5}),
        )?;
        normalize_governance(&mut value);
        Ok(value)
    }

    pub fn save(&self, body: &Value) -> RefineResult<Value> {
        let mut current = self.load()?;
        if let Some(product) = body.get("product").and_then(|value| value.as_str()) {
            current["product"] = Value::String(product.trim().to_string());
        }
        if let Some(constitution) = body.get("constitution").and_then(|value| value.as_str()) {
            current["constitution"] = Value::String(constitution.trim().to_string());
        }
        if let Some(rules) = body.get("rules") {
            if !rules.is_array() {
                return Err(RefineError::InvalidInput(
                    "rules must be a list".to_string(),
                ));
            }
            current["rules"] = normalize_rules(rules);
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
        normalize_governance(&mut current);
        write_json(self.refine_dir.join(GOVERNANCE_FILE), &current)?;
        Ok(current)
    }

    pub fn generate_rules(&self, body: &Value) -> RefineResult<Value> {
        let product = body
            .get("product")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        let constitution = body
            .get("constitution")
            .and_then(|value| value.as_str())
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
}
