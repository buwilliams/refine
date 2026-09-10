//! Explicit-cwd provider session resumption through the shared supervisor.
use super::*;

impl HostAgentProviderService {
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
        self.resume_detailed_at_cwd_with_output_and_metadata(
            provider,
            session_id,
            None,
            process_metadata,
            on_output,
        )
    }

    pub fn resume_detailed_at_cwd_with_output_and_metadata<F>(
        &self,
        provider: &str,
        session_id: &str,
        cwd: Option<&Path>,
        process_metadata: Map<String, Value>,
        on_output: F,
    ) -> RefineResult<ProviderInvocationResult>
    where
        F: FnMut(String),
    {
        crate::infrastructure::git::worktrees::validate_workspace_launch(&process_metadata, cwd)?;
        let (spec, binary) = self.resolve_binary_for_provider(provider)?;
        if !spec.supports_resume {
            return Err(RefineError::InvalidInput(format!(
                "{} does not support provider-session resume",
                spec.display_name
            )));
        }
        let args = spec.chat_args(&binary, "", Some(session_id), cwd);
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
                cwd,
                launch_environment: &launch_environment,
                environment_overrides: &[],
                output_format: &spec.output_format,
                process_metadata,
                authorization_command: None,
                stall_timeout_seconds: None,
            },
            on_output,
        )
    }
}
