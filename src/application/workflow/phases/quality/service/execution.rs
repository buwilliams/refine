use super::*;

impl QualityService for FileQualityService {
    fn run_checks(&self, request: QualityCheckRequest) -> RefineResult<QualityCheckResult> {
        self.ensure_operation_active(&request, "before Quality checks")?;
        if self.refine_dir.join("automation/config.json").exists() {
            return self.run_skill_checks(request);
        }
        // No Skill configuration means there is no Quality work to dispatch.
        // Legacy settings remain migration input and never launch a second evaluator.
        verify_candidate(
            Path::new(&request.cwd),
            &request.candidate_commit,
            "without a Quality Skill",
        )?;
        self.ensure_operation_active(&request, "optional Quality settlement")?;
        Ok(QualityCheckResult {
            owner_id: request.owner_id,
            ok: true,
            summary: "No Quality Skill applies; passed without agent checks.".into(),
            results: Vec::new(),
            diagnostics: Vec::new(),
            candidate_commit: request.candidate_commit,
            checked_at: None,
            provider_attempts: Vec::new(),
            skill_evidence: None,
        })
    }

    fn screenshots(&self, _owner_id: &str) -> RefineResult<Vec<String>> {
        Ok(Vec::new())
    }

    fn compare(&self, baseline: &str, candidate: &str) -> RefineResult<QualityCheckResult> {
        let baseline_bytes = fs::read(baseline)
            .map_err(|error| RefineError::Io(format!("failed to read {baseline}: {error}")))?;
        let candidate_bytes = fs::read(candidate)
            .map_err(|error| RefineError::Io(format!("failed to read {candidate}: {error}")))?;
        let ok = baseline_bytes == candidate_bytes;
        Ok(QualityCheckResult {
            owner_id: format!("{baseline}:{candidate}"),
            ok,
            summary: if ok {
                "Artifacts match exactly.".to_string()
            } else {
                "Artifacts differ.".to_string()
            },
            results: Vec::new(),
            diagnostics: vec![if ok {
                "artifacts match exactly".to_string()
            } else {
                "artifacts differ".to_string()
            }],
            candidate_commit: String::new(),
            checked_at: None,
            provider_attempts: Vec::new(),
            skill_evidence: None,
        })
    }

    fn gate(&self, owner_id: &str) -> RefineResult<QualityCheckResult> {
        let config =
            crate::application::events::FileEventService::new(&self.refine_dir).config()?;
        Ok(QualityCheckResult {
            owner_id: owner_id.to_string(),
            ok: true,
            summary: "Quality evaluates every Goal candidate.".to_string(),
            results: Vec::new(),
            diagnostics: vec![format!(
                "Quality is managed through Skills (configuration revision {}).",
                config.revision
            )],
            candidate_commit: String::new(),
            checked_at: None,
            provider_attempts: Vec::new(),
            skill_evidence: None,
        })
    }
}
