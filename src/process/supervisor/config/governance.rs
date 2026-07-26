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
            json!({"product": "", "constitution": "", "rules": []}),
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
