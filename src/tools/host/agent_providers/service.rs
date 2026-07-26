use super::*;

#[derive(Clone, Debug, Default)]
pub struct HostAgentProviderService {
    pub path_override: Option<String>,
    pub runtime_root: Option<PathBuf>,
}

impl HostAgentProviderService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_runtime_root(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: Some(runtime_root.into()),
            ..Self::default()
        }
    }

    fn spec(provider: &str) -> Option<ProviderSpec> {
        match provider {
            "claude" => Some(ProviderSpec::new(
                "claude",
                "Claude Code",
                "claude",
                "claude_json",
                true,
                false,
            )),
            "codex" => Some(ProviderSpec::new(
                "codex",
                "OpenAI Codex",
                "codex",
                "codex_json",
                true,
                false,
            )),
            "gemini" => Some(ProviderSpec::new(
                "gemini", "Gemini", "gemini", "plain", false, false,
            )),
            "copilot" => Some(ProviderSpec::new(
                "copilot",
                "GitHub Copilot",
                "copilot",
                "copilot_json",
                false,
                false,
            )),
            "smoke-ai" => Some(ProviderSpec::new(
                "smoke-ai", "Smoke AI", "smoke-ai", "plain", false, false,
            )),
            _ => None,
        }
    }

    fn specs() -> Vec<ProviderSpec> {
        ["claude", "codex", "gemini", "copilot", "smoke-ai"]
            .into_iter()
            .filter_map(Self::spec)
            .collect()
    }

    fn detect_spec(&self, spec: ProviderSpec) -> ProviderCapability {
        let smoke_ai_binary = (spec.name == "smoke-ai")
            .then(|| self.smoke_ai_binary(&spec))
            .flatten();
        let binary = smoke_ai_binary
            .clone()
            .unwrap_or_else(|| spec.binary.to_string());
        let path = if spec.name == "smoke-ai" && smoke_ai_binary.is_none() {
            None
        } else {
            find_executable(&binary, self.path_override.as_deref())
        };
        ProviderCapability {
            name: spec.name.to_string(),
            display_name: spec.display_name.to_string(),
            binary,
            installed: path.is_some(),
            path: path.map(|path| path.display().to_string()),
            supports_resume: spec.supports_resume,
            supports_direct_api: spec.supports_direct_api,
            supports_cli: true,
            output_format: spec.output_format.to_string(),
        }
    }

    fn smoke_ai_binary(&self, spec: &ProviderSpec) -> Option<String> {
        if self
            .path_override
            .as_deref()
            .and_then(|path| find_executable(spec.binary, Some(path)))
            .is_some()
        {
            return Some(spec.binary.to_string());
        }
        env::var("REFINE_SMOKE_AI_PATH")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    fn resolve_binary_for_provider(&self, provider: &str) -> RefineResult<(ProviderSpec, String)> {
        let spec = Self::spec(provider)
            .ok_or_else(|| RefineError::InvalidInput(format!("unknown provider {provider}")))?;
        let capability = self.detect_spec(spec.clone());
        let Some(path) = capability.path.or_else(|| {
            if capability.installed {
                Some(capability.binary.clone())
            } else {
                None
            }
        }) else {
            return Err(RefineError::Degraded(format!(
                "{} CLI was not found on PATH",
                capability.display_name
            )));
        };
        Ok((spec, path))
    }

    /// Resolve the configured provider into its native interactive CLI invocation.
    ///
    /// Interactive toolbar sessions deliberately do not use the JSON/print modes used by
    /// workflow automation. The provider owns its conversation UX while Refine owns the PTY,
    /// working directory, process lifecycle, and initial orchestration context.
    pub fn interactive_command(
        &self,
        provider: &str,
        prompt: &str,
    ) -> RefineResult<InteractiveProviderCommand> {
        let (spec, binary) = self.resolve_binary_for_provider(provider)?;
        Ok(InteractiveProviderCommand {
            provider: provider.to_string(),
            display_name: spec.display_name.to_string(),
            args: spec.interactive_args(prompt),
            binary,
        })
    }

    pub fn invoke_detailed(
        &self,
        invocation: ProviderInvocation,
    ) -> RefineResult<ProviderInvocationResult> {
        self.invoke_detailed_with_output(invocation, |_| {})
    }

    pub fn invoke_detailed_with_output<F>(
        &self,
        invocation: ProviderInvocation,
        on_output: F,
    ) -> RefineResult<ProviderInvocationResult>
    where
        F: FnMut(String),
    {
        let (spec, binary) = self.resolve_binary_for_provider(&invocation.provider)?;
        let cwd = invocation.cwd.as_deref().map(Path::new);
        let args = spec.chat_args(
            &binary,
            &invocation.prompt,
            invocation.session_id.as_deref(),
            cwd,
        );
        let stdin = spec.prompt_stdin(&invocation.prompt);
        self.run_provider_command_result_with_output(
            &args,
            stdin,
            cwd,
            spec.output_format,
            invocation.process_metadata,
            on_output,
        )
    }

    pub fn resume_detailed(
        &self,
        provider: &str,
        session_id: &str,
    ) -> RefineResult<ProviderInvocationResult> {
        self.resume_detailed_with_output(provider, session_id, |_| {})
    }

    pub fn resume_detailed_with_output<F>(
        &self,
        provider: &str,
        session_id: &str,
        on_output: F,
    ) -> RefineResult<ProviderInvocationResult>
    where
        F: FnMut(String),
    {
        self.resume_detailed_with_output_and_metadata(
            provider,
            session_id,
            Default::default(),
            on_output,
        )
    }

    pub fn resume_detailed_with_output_and_metadata<F>(
        &self,
        provider: &str,
        session_id: &str,
        process_metadata: Map<String, Value>,
        on_output: F,
    ) -> RefineResult<ProviderInvocationResult>
    where
        F: FnMut(String),
    {
        let (spec, binary) = self.resolve_binary_for_provider(provider)?;
        if !spec.supports_resume {
            return Err(RefineError::InvalidInput(format!(
                "{} does not support provider-session resume",
                spec.display_name
            )));
        }
        let args = spec.chat_args(&binary, "", Some(session_id), None);
        self.run_provider_command_result_with_output(
            &args,
            None,
            None,
            spec.output_format,
            process_metadata,
            on_output,
        )
    }

    fn run_provider_command_result_with_output<F>(
        &self,
        args: &[String],
        stdin: Option<String>,
        cwd: Option<&Path>,
        output_format: &str,
        process_metadata: Map<String, Value>,
        mut on_output: F,
    ) -> RefineResult<ProviderInvocationResult>
    where
        F: FnMut(String),
    {
        let Some((binary, rest)) = args.split_first() else {
            return Err(RefineError::InvalidInput(
                "provider command cannot be empty".to_string(),
            ));
        };
        let runtime_root = self
            .runtime_root
            .clone()
            .unwrap_or_else(|| PathBuf::from("run/agent-processes"));
        let mut formatter = ProviderActivityFormatter::new(output_format);
        let output = FileProcessSupervisor::new(runtime_root).run_to_completion_with_output(
            ManagedProcessSpec {
                owner: ProcessOwner::Agent,
                command: binary.to_string(),
                args: rest.to_vec(),
                cwd: cwd.map(|path| path.display().to_string()),
                env: Vec::new(),
                stdin,
                limits: Some(ProcessResourceLimits {
                    kill_on_parent_exit: true,
                    ..Default::default()
                }),
                authorization_command: Some(args.join(" ")),
                sensitive: false,
                metadata: process_metadata,
            },
            |stream, bytes| {
                for line in formatter.push(stream, bytes) {
                    on_output(line);
                }
            },
        )?;
        for line in formatter.finish() {
            on_output(line);
        }
        let success = output.success();
        let exit_code = output.process.exit_code;
        let stdout = output.stdout;
        let stderr = output.stderr;
        if !success {
            let message = provider_error_message(&stdout, &stderr)
                .or_else(|| last_non_empty_line(&stderr))
                .or_else(|| last_non_empty_line(&stdout))
                .unwrap_or_else(|| {
                    let exit = exit_code
                        .map(|code| code.to_string())
                        .unwrap_or_else(|| "unknown".to_string());
                    format!("provider command exited {exit}")
                });
            return Err(RefineError::Degraded(message));
        }
        if let Some(message) = provider_error_message(&stdout, &stderr) {
            return Err(RefineError::Degraded(message));
        }
        let final_text = extract_final_text(&stdout, output_format);
        let provider_session_id = extract_provider_session_id(&stdout);
        if final_text.trim().is_empty() {
            Ok(ProviderInvocationResult {
                output: stdout.clone(),
                provider_session_id,
                raw_output: stdout,
            })
        } else {
            Ok(ProviderInvocationResult {
                output: final_text,
                provider_session_id,
                raw_output: stdout,
            })
        }
    }
}

impl AgentProviderService for HostAgentProviderService {
    fn detect(&self) -> RefineResult<Vec<ProviderCapability>> {
        Ok(Self::specs()
            .into_iter()
            .map(|spec| self.detect_spec(spec))
            .collect())
    }

    fn configure(&self, provider: &str) -> RefineResult<()> {
        Self::spec(provider)
            .map(|_| ())
            .ok_or_else(|| RefineError::InvalidInput(format!("unknown provider {provider}")))
    }

    fn authenticate(&self, provider: &str) -> RefineResult<()> {
        let capability = self
            .detect_spec(Self::spec(provider).ok_or_else(|| {
                RefineError::InvalidInput(format!("unknown provider {provider}"))
            })?);
        if capability.installed {
            Ok(())
        } else {
            Err(RefineError::Degraded(format!(
                "{} CLI was not found on PATH",
                capability.display_name
            )))
        }
    }

    fn invoke(&self, invocation: ProviderInvocation) -> RefineResult<String> {
        self.invoke_detailed(invocation).map(|result| result.output)
    }

    fn resume(&self, provider: &str, session_id: &str) -> RefineResult<String> {
        self.resume_detailed(provider, session_id)
            .map(|result| result.output)
    }

    fn diagnose(&self, provider: &str) -> RefineResult<Vec<String>> {
        let capability = self
            .detect_spec(Self::spec(provider).ok_or_else(|| {
                RefineError::InvalidInput(format!("unknown provider {provider}"))
            })?);
        if capability.installed {
            Ok(vec![format!(
                "{} CLI found at {}",
                capability.display_name,
                capability.path.unwrap_or_default()
            )])
        } else {
            Ok(vec![format!(
                "{} CLI not found; install it and run its login command on the host",
                capability.display_name
            )])
        }
    }
}
