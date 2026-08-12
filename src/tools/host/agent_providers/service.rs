use super::*;

struct ProviderCommandExecution<'a> {
    args: &'a [String],
    stdin: Option<String>,
    cwd: Option<&'a Path>,
    launch_environment: &'a EffectiveLaunchEnvironment,
    environment_overrides: &'a [(String, String)],
    output_format: &'a str,
    process_metadata: Map<String, Value>,
    authorization_command: Option<String>,
}

struct ProviderLaunchRequest<'a> {
    prompt: &'a str,
    session_id: Option<&'a str>,
    cwd: Option<&'a Path>,
    interactive: bool,
    environment: EffectiveLaunchEnvironment,
}

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

    pub fn reap_orphan_prompt_artifacts(&self) -> RefineResult<usize> {
        reap_orphan_prompt_artifacts(&self.prompt_runtime_root()?)
    }

    fn spec(provider: &str) -> Option<ProviderSpec> {
        let provider = provider.trim();
        if provider.is_empty() || provider.as_bytes().contains(&0) {
            return None;
        }
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
            _ => Some(ProviderSpec::new(
                provider, provider, provider, "plain", false, false,
            )),
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
            prompt_transport: spec.noninteractive_prompt_capability(),
        }
    }

    fn smoke_ai_binary(&self, spec: &ProviderSpec) -> Option<String> {
        if self
            .path_override
            .as_deref()
            .and_then(|path| find_executable(&spec.binary, Some(path)))
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
        self.interactive_command_with_environment(provider, prompt, &[])
    }

    pub fn interactive_command_with_environment(
        &self,
        provider: &str,
        prompt: &str,
        environment_overrides: &[(String, String)],
    ) -> RefineResult<InteractiveProviderCommand> {
        let _ = self.reap_orphan_prompt_artifacts()?;
        let (spec, binary) = self.resolve_binary_for_provider(provider)?;
        let environment =
            EffectiveLaunchEnvironment::assemble(&ProcessOwner::Agent, environment_overrides)?;
        self.prepare_provider_launch(
            &spec,
            binary,
            ProviderLaunchRequest {
                prompt,
                session_id: None,
                cwd: None,
                interactive: true,
                environment,
            },
        )
    }

    fn prepare_provider_launch(
        &self,
        spec: &ProviderSpec,
        binary: String,
        request: ProviderLaunchRequest<'_>,
    ) -> RefineResult<PreparedProviderLaunch> {
        let capability = if request.interactive {
            spec.interactive_prompt_capability()
        } else {
            spec.noninteractive_prompt_capability()
        };
        let prepared = prepare_prompt_with_environment(
            &self.prompt_runtime_root()?,
            capability,
            request.prompt,
            &request.environment,
            |delivered| {
                if request.interactive {
                    std::iter::once(binary.clone())
                        .chain(spec.interactive_args(delivered))
                        .collect()
                } else {
                    spec.chat_args(&binary, delivered, request.session_id, request.cwd)
                }
            },
        )?;
        let args = if request.interactive {
            spec.interactive_args(&prepared.delivered_prompt)
        } else {
            spec.chat_args(
                &binary,
                &prepared.delivered_prompt,
                request.session_id,
                request.cwd,
            )
            .into_iter()
            .skip(1)
            .collect()
        };
        request.environment.validate_launch(&binary, &args)?;
        let authorization_command =
            safe_authorization_command(&binary, request.interactive, &prepared.metadata);
        Ok(PreparedProviderLaunch {
            provider: spec.name.clone(),
            display_name: spec.display_name.to_string(),
            args,
            binary,
            stdin: prepared.stdin,
            prompt_transport: prepared.metadata.clone(),
            prompt_artifact: prepared.artifact,
            authorization_command,
            launch_environment: request.environment,
        })
    }

    pub fn invoke_detailed(
        &self,
        invocation: ProviderInvocation,
    ) -> RefineResult<ProviderInvocationResult> {
        self.invoke_detailed_with_output(invocation, |_| {})
    }

    /// Launch an installed provider as an independently supervised Agent.
    ///
    /// Maintenance capabilities use this form when the provider must survive a
    /// daemon restart and drive several typed Refine commands from durable
    /// context. The returned managed-process record is the durable launch
    /// receipt; callers correlate it with an operation through standard
    /// `operation_id` metadata.
    pub(crate) fn launch_managed(
        &self,
        invocation: ProviderInvocation,
    ) -> RefineResult<crate::process::subprocess::ManagedProcess> {
        let _ = self.reap_orphan_prompt_artifacts()?;
        let (spec, binary) = self.resolve_binary_for_provider(&invocation.provider)?;
        if invocation
            .cwd
            .as_deref()
            .is_some_and(|cwd| cwd.as_bytes().contains(&0))
        {
            return Err(RefineError::InvalidInput(
                "provider cwd contains a NUL byte and cannot be launched".to_string(),
            ));
        }
        let cwd = invocation.cwd.as_deref().map(Path::new);
        let launch_environment = EffectiveLaunchEnvironment::assemble(&ProcessOwner::Agent, &[])?;
        let prepared = self.prepare_provider_launch(
            &spec,
            binary,
            ProviderLaunchRequest {
                prompt: &invocation.prompt,
                session_id: invocation.session_id.as_deref(),
                cwd,
                interactive: false,
                environment: launch_environment,
            },
        )?;
        prepared.validate_prompt_artifact()?;
        let mut metadata = invocation.process_metadata;
        metadata.insert(
            "prompt_transport".to_string(),
            serde_json::to_value(&prepared.prompt_transport).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to encode provider prompt transport metadata: {error}"
                ))
            })?,
        );
        let supervisor = FileProcessSupervisor::new(self.prompt_runtime_root()?);
        let process = supervisor.launch(ManagedProcessSpec {
            owner: ProcessOwner::Agent,
            command: prepared.binary,
            args: prepared.args,
            cwd: cwd.map(|path| path.display().to_string()),
            env: Vec::new(),
            stdin: prepared.stdin,
            limits: Some(ProcessResourceLimits {
                kill_on_parent_exit: false,
                ..Default::default()
            }),
            authorization_command: Some(prepared.authorization_command),
            sensitive: false,
            metadata,
        })?;

        // Keep a prompt-file lease alive until the provider exits. If the
        // daemon terminates first, the process metadata keeps the artifact
        // discoverable by the normal orphan reaper after the provider settles.
        if let Some(artifact) = prepared.prompt_artifact {
            let observer = supervisor.clone();
            let process_id = process.id.clone();
            std::thread::spawn(move || {
                loop {
                    match observer.wait(&process_id) {
                        Ok(current) if current.state == "running" => {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                        _ => break,
                    }
                }
                drop(artifact);
            });
        }
        Ok(process)
    }

    pub fn invoke_detailed_with_output<F>(
        &self,
        invocation: ProviderInvocation,
        on_output: F,
    ) -> RefineResult<ProviderInvocationResult>
    where
        F: FnMut(String),
    {
        self.invoke_detailed_with_environment_and_output(invocation, &[], on_output)
    }

    pub(crate) fn invoke_detailed_with_environment_and_output<F>(
        &self,
        invocation: ProviderInvocation,
        environment_overrides: &[(String, String)],
        on_output: F,
    ) -> RefineResult<ProviderInvocationResult>
    where
        F: FnMut(String),
    {
        let _ = self.reap_orphan_prompt_artifacts()?;
        let (spec, binary) = self.resolve_binary_for_provider(&invocation.provider)?;
        if invocation
            .cwd
            .as_deref()
            .is_some_and(|cwd| cwd.as_bytes().contains(&0))
        {
            return Err(RefineError::InvalidInput(
                "provider cwd contains a NUL byte and cannot be launched".to_string(),
            ));
        }
        let cwd = invocation.cwd.as_deref().map(Path::new);
        let launch_environment =
            EffectiveLaunchEnvironment::assemble(&ProcessOwner::Agent, environment_overrides)?;
        let prepared = self.prepare_provider_launch(
            &spec,
            binary,
            ProviderLaunchRequest {
                prompt: &invocation.prompt,
                session_id: invocation.session_id.as_deref(),
                cwd,
                interactive: false,
                environment: launch_environment,
            },
        )?;
        prepared.validate_prompt_artifact()?;
        let mut process_metadata = invocation.process_metadata;
        process_metadata.insert(
            "prompt_transport".to_string(),
            serde_json::to_value(&prepared.prompt_transport).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to encode provider prompt transport metadata: {error}"
                ))
            })?,
        );
        let args = std::iter::once(prepared.binary.clone())
            .chain(prepared.args.clone())
            .collect::<Vec<_>>();
        self.run_provider_command_result_with_output(
            ProviderCommandExecution {
                args: &args,
                stdin: prepared.stdin,
                cwd,
                launch_environment: &prepared.launch_environment,
                environment_overrides,
                output_format: &spec.output_format,
                process_metadata,
                authorization_command: Some(prepared.authorization_command.clone()),
            },
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
        let launch_environment = EffectiveLaunchEnvironment::assemble(&ProcessOwner::Agent, &[])?;
        let Some((launch_binary, launch_args)) = args.split_first() else {
            return Err(RefineError::InvalidInput(
                "provider command cannot be empty".to_string(),
            ));
        };
        launch_environment.validate_launch(launch_binary, launch_args)?;
        self.run_provider_command_result_with_output(
            ProviderCommandExecution {
                args: &args,
                stdin: None,
                cwd: None,
                launch_environment: &launch_environment,
                environment_overrides: &[],
                output_format: &spec.output_format,
                process_metadata,
                authorization_command: None,
            },
            on_output,
        )
    }

    fn run_provider_command_result_with_output<F>(
        &self,
        launch: ProviderCommandExecution<'_>,
        mut on_output: F,
    ) -> RefineResult<ProviderInvocationResult>
    where
        F: FnMut(String),
    {
        let Some((binary, rest)) = launch.args.split_first() else {
            return Err(RefineError::InvalidInput(
                "provider command cannot be empty".to_string(),
            ));
        };
        let runtime_root = self.prompt_runtime_root()?;
        let mut formatter = ProviderActivityFormatter::new(launch.output_format);
        let output = FileProcessSupervisor::new(runtime_root)
            .run_to_completion_with_prepared_environment(
                ManagedProcessSpec {
                    owner: ProcessOwner::Agent,
                    command: binary.to_string(),
                    args: rest.to_vec(),
                    cwd: launch.cwd.map(|path| path.display().to_string()),
                    env: launch.environment_overrides.to_vec(),
                    stdin: launch.stdin,
                    limits: Some(ProcessResourceLimits {
                        kill_on_parent_exit: true,
                        ..Default::default()
                    }),
                    authorization_command: launch
                        .authorization_command
                        .or_else(|| Some(launch.args.join(" "))),
                    sensitive: false,
                    metadata: launch.process_metadata,
                },
                launch.launch_environment,
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
        let process_id = output.process.id.clone();
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
        let final_text = extract_final_text(&stdout, launch.output_format);
        let provider_session_id = extract_provider_session_id(&stdout);
        if final_text.trim().is_empty() {
            Ok(ProviderInvocationResult {
                output: stdout.clone(),
                provider_session_id,
                raw_output: stdout,
                process_id,
            })
        } else {
            Ok(ProviderInvocationResult {
                output: final_text,
                provider_session_id,
                raw_output: stdout,
                process_id,
            })
        }
    }

    fn prompt_runtime_root(&self) -> RefineResult<PathBuf> {
        self.runtime_root.clone().ok_or_else(|| {
            RefineError::Conflict(
                "stateful provider launch requires the daemon or caller to supply the owning checkout's port-scoped runtime; Refine will not fall back to HOME, XDG, OS app-support state, caller CWD, or an unscoped base run directory"
                    .to_string(),
            )
        })
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
