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
        let launch_environment = EffectiveLaunchEnvironment::assemble(&ProcessOwner::Agent, &[])?;
        let prepared = self.prepare_provider_launch(
            &spec,
            binary,
            ProviderLaunchRequest {
                prompt: "",
                session_id: Some(session_id),
                interactive_session: None,
                cwd,
                interactive: false,
                environment: launch_environment,
            },
        )?;
        let args = std::iter::once(prepared.binary.clone())
            .chain(prepared.args.clone())
            .collect::<Vec<_>>();
        self.run_provider_command_result_with_output(
            ProviderCommandExecution {
                args: &args,
                stdin: prepared.stdin,
                cwd,
                launch_environment: &prepared.launch_environment,
                environment_overrides: &[],
                output_format: &spec.output_format,
                process_metadata,
                authorization_command: Some(prepared.authorization_command),
                stall_timeout_seconds: None,
            },
            on_output,
        )
    }
}
