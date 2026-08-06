use super::*;

impl QualityService for FileQualityService {
    fn run_checks(&self, request: QualityCheckRequest) -> RefineResult<QualityCheckResult> {
        let candidate_root = PathBuf::from(&request.cwd);
        verify_candidate(&candidate_root, &request.candidate_commit, "before")?;
        let settings = self.load_settings()?;
        let definitions = quality_test_definitions(&settings);
        if definitions.is_empty() {
            verify_candidate(&candidate_root, &request.candidate_commit, "after")?;
            return Ok(QualityCheckResult {
                owner_id: request.owner_id,
                ok: true,
                summary: "No Quality tests configured.".to_string(),
                results: Vec::new(),
                diagnostics: vec![
                    "No Quality tests or migrated commands are configured; evaluation was a no-op."
                        .to_string(),
                ],
                candidate_commit: request.candidate_commit,
            });
        }
        let test_names = definitions
            .iter()
            .map(|definition| definition.test.clone())
            .collect::<Vec<_>>();
        let tests_json = serde_json::to_string_pretty(&test_names).map_err(|error| {
            RefineError::Serialization(format!("failed to encode Quality tests: {error}"))
        })?;
        let prompt = render(
            PromptTemplate::PostImplementationQuality,
            &[
                ("owner_id", &request.owner_id),
                ("candidate_cwd", &request.cwd),
                ("business_requirements", &settings.business_requirements),
                ("instructions", &settings.instructions),
                ("tests_json", &tests_json),
            ],
        );
        let runtime_root = self.runtime_root.clone().ok_or_else(|| {
            RefineError::Degraded("runtime root is required for Quality".to_string())
        })?;
        self.ensure_operation_active(&request, "provider launch")?;
        let output = HostAgentProviderService::with_runtime_root(&runtime_root).invoke(
            ProviderInvocation {
                provider: request.provider.clone(),
                prompt,
                session_id: None,
                cwd: Some(request.cwd.clone()),
                process_metadata: request.process_metadata.clone(),
            },
        )?;
        let plan = parse_quality_provider_output(&request.owner_id, &test_names, &output)?;
        let mut results = Vec::with_capacity(definitions.len());
        let mut diagnostics = plan.diagnostics;
        for (definition, planned) in definitions.iter().zip(plan.results) {
            let mut result = planned;
            if let Some(required) = definition.required_command.as_deref()
                && result.command != required
            {
                result.status = "failed".to_string();
                result.evidence = format!(
                    "Migrated Quality command must remain {required:?}; the agent proposed {:?}.",
                    result.command
                );
                diagnostics.push(result.evidence.clone());
                results.push(result);
                continue;
            }
            if result.command.trim().is_empty() {
                result.status = "failed".to_string();
                result.evidence =
                    "Pass claim rejected because no supervised command execution was requested."
                        .to_string();
                diagnostics.push(result.evidence.clone());
                results.push(result);
                continue;
            }
            let mut metadata = request.process_metadata.clone();
            metadata.insert("quality_test".to_string(), json!(&result.test));
            metadata.insert("quality_command".to_string(), json!(&result.command));
            self.ensure_operation_active(&request, "the next test command")?;
            let observed = self.run_observed_command(&result.command, &candidate_root, metadata)?;
            if observed.shell_parser_aborted() {
                verify_candidate(&candidate_root, &request.candidate_commit, "after")?;
                return Err(quality_command_harness_fault(&result.command, &observed));
            }
            let observed_ok = observed.exit_code == Some(0);
            result.process_id = Some(observed.process_id.clone());
            result.exit_code = observed.exit_code;
            let observed_evidence = observed.evidence();
            if result.status != "passed" || !observed_ok || result.evidence.trim().is_empty() {
                result.status = "failed".to_string();
            }
            result.evidence = format!("{} Agent report: {}", observed_evidence, result.evidence);
            diagnostics.push(observed_evidence);
            results.push(result);
        }
        verify_candidate(&candidate_root, &request.candidate_commit, "after")?;
        let ok = results.iter().all(|result| result.status == "passed");
        let mut result = QualityCheckResult {
            owner_id: request.owner_id,
            ok,
            summary: if ok {
                "All Quality tests passed with observed supervised evidence.".to_string()
            } else {
                String::new()
            },
            results,
            diagnostics,
            candidate_commit: request.candidate_commit,
        };
        if !result.ok {
            result.summary = quality_failure_summary(&result);
        }
        Ok(result)
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
        })
    }

    fn gate(&self, owner_id: &str) -> RefineResult<QualityCheckResult> {
        let settings = self.load_settings()?;
        Ok(QualityCheckResult {
            owner_id: owner_id.to_string(),
            ok: true,
            summary: "Quality evaluates every Goal candidate.".to_string(),
            results: Vec::new(),
            diagnostics: vec![format!(
                "Quality is active with {} plain-text test(s) and {} migrated command(s).",
                settings.tests.len(),
                settings.legacy_commands.len()
            )],
            candidate_commit: String::new(),
        })
    }
}
