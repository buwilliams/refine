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
                checked_at: None,
                provider_attempts: Vec::new(),
            });
        }
        let test_names = definitions
            .iter()
            .map(|definition| definition.test.clone())
            .collect::<Vec<_>>();
        let tests_json = serde_json::to_string_pretty(&test_names).map_err(|error| {
            RefineError::Serialization(format!("failed to encode Quality tests: {error}"))
        })?;
        let quality_contract = QualityEvaluationWire::contract_json();
        let prompt = render(
            PromptTemplate::PostImplementationQuality,
            &[
                ("owner_id", &request.owner_id),
                ("candidate_cwd", &request.cwd),
                ("business_requirements", &settings.business_requirements),
                ("instructions", &settings.instructions),
                ("tests_json", &tests_json),
                ("quality_contract", &quality_contract),
            ],
        );
        let runtime_root = self.runtime_root.clone().ok_or_else(|| {
            RefineError::Degraded("runtime root is required for Quality".to_string())
        })?;
        let provider = HostAgentProviderService::with_runtime_root(&runtime_root);
        let repair_session_id = std::cell::RefCell::new(None::<String>);
        let provider_attempts = std::cell::RefCell::new(Vec::new());
        let (_, mut plan) = crate::application::agent_io::structured_output::run_with_repair(
            &crate::application::agent_io::structured_output::RepairPolicy::default(),
            |directive| {
                let (launch_prompt, launch_phase, settle_phase, metadata) = match directive {
                    None => (
                        prompt.clone(),
                        "provider launch",
                        "provider response settlement",
                        request.process_metadata.clone(),
                    ),
                    Some(directive) => {
                        let mut metadata = request.process_metadata.clone();
                        metadata.insert(
                            "structured_output_repair_attempt".to_string(),
                            json!(directive.attempt),
                        );
                        (
                            crate::application::agent_io::prompts::structured_output::repair_prompt(
                                &prompt,
                                "Quality evaluation",
                                &quality_contract,
                                directive,
                            ),
                            "structured output repair",
                            "repaired provider response settlement",
                            metadata,
                        )
                    }
                };
                self.ensure_operation_active(&request, launch_phase)?;
                let invocation = provider.invoke_detailed(ProviderInvocation {
                    provider: request.provider.clone(),
                    prompt: launch_prompt,
                    session_id: repair_session_id.borrow().clone(),
                    cwd: Some(request.cwd.clone()),
                    stall_timeout_seconds: request.stall_timeout_seconds,
                    process_metadata: metadata,
                })?;
                self.ensure_operation_active(&request, settle_phase)?;
                Ok(invocation)
            },
            |invocation| invocation.output.as_str(),
            |output| parse_quality_provider_output(&request.owner_id, &test_names, output),
            |invocation, outcome| {
                let attempt = QualityProviderAttempt {
                    attempt: outcome.attempt,
                    process_id: invocation.process_id.clone(),
                    provider_session_id: invocation.provider_session_id.clone(),
                    raw_output: invocation.output.clone(),
                    diagnostics: outcome.diagnostics.map(str::to_string),
                    accepted: outcome.diagnostics.is_none(),
                };
                record_quality_provider_attempt(&request, &attempt)?;
                *repair_session_id.borrow_mut() = invocation.provider_session_id.clone();
                provider_attempts.borrow_mut().push(attempt);
                Ok(())
            },
        )?;
        plan.provider_attempts = provider_attempts.into_inner();
        let mut results = Vec::with_capacity(definitions.len());
        let mut diagnostics = plan.diagnostics;
        let provider_attempts = plan.provider_attempts;
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
            checked_at: None,
            provider_attempts,
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
            checked_at: None,
            provider_attempts: Vec::new(),
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
            checked_at: None,
            provider_attempts: Vec::new(),
        })
    }
}
