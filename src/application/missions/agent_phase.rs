//! Mission agent-phase runner.
//!
//! Long agent calls (investigation, reduction, criticism, synthesis,
//! holistic quality, governance) run as one-shot durable operations with
//! stable ownership `mission:<id>:round:<n>:<stage>`, so a competing worker
//! or restart cannot run the same phase concurrently. Output is decoded
//! through the shared structured-output transport with bounded repair; every
//! rejected raw attempt is preserved as durable phase evidence. The runner
//! never decides anything: the engine applies typed results
//! deterministically.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::application::agent_io::prompts::structured_output::repair_prompt;
use crate::application::agent_io::structured_output::contract::Contract;
use crate::application::agent_io::structured_output::{RepairPolicy, run_with_repair};
use crate::error::RefineResult;
use crate::infrastructure::agents::invocation::{AgentProviderService, ProviderInvocation};
use crate::infrastructure::process::supervisor::operations::{
    FileOperationRegistry, OperationState,
};

/// The stall cap for unattended Mission phase agents: a hung CLI must not
/// hold the workflow runner forever.
pub const MISSION_AGENT_STALL_TIMEOUT_SECONDS: u64 = 900;

/// One executed Mission agent phase: the durable operation receipt plus the
/// decoded typed output and every raw provider attempt.
pub struct PhaseRun<T> {
    pub operation_id: String,
    pub output: T,
    pub attempts: Vec<PhaseAttempt>,
}

/// One raw provider attempt, accepted or rejected with its diagnostic.
/// Rejected attempts are durable evidence, never silently discarded.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PhaseAttempt {
    pub attempt: usize,
    pub raw_output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<String>,
    pub accepted: bool,
}

/// The stage identity of one Mission agent phase.
pub fn stage_owner(mission_id: &str, round: usize, stage: &str) -> String {
    format!("mission:{mission_id}:round:{round}:{stage}")
}

/// Run one agent phase under an exclusive durable operation, with bounded
/// structured-output repair.
#[allow(clippy::too_many_arguments)]
pub fn run_agent_phase<T: Contract>(
    provider: &dyn AgentProviderService,
    runtime_root: &Path,
    mission_id: &str,
    round: usize,
    stage: &str,
    provider_name: &str,
    prompt: &str,
    cwd: Option<&Path>,
) -> RefineResult<PhaseRun<T>> {
    let owner = stage_owner(mission_id, round, stage);
    let registry = FileOperationRegistry::new(runtime_root);
    let operation = registry.register_exclusive_with_request(
        &owner,
        json!({
            "mission_id": mission_id,
            "mission_round": round,
            "stage": stage,
            "kind": "mission_agent_phase",
        }),
    )?;

    let contract_json = T::contract_json();
    let mut metadata = serde_json::Map::new();
    metadata.insert("kind".to_string(), json!("mission_stage"));
    metadata.insert("mission_id".to_string(), json!(mission_id));
    metadata.insert("mission_round".to_string(), json!(round));
    metadata.insert("stage".to_string(), json!(stage));
    metadata.insert("owner".to_string(), json!(owner));
    let attempts: std::cell::RefCell<Vec<PhaseAttempt>> = std::cell::RefCell::default();

    let result = run_with_repair(
        &RepairPolicy::default(),
        |directive| {
            let launch_prompt = match directive {
                None => prompt.to_string(),
                Some(directive) => repair_prompt(prompt, T::LABEL, &contract_json, directive),
            };
            let invocation = ProviderInvocation {
                provider: provider_name.to_string(),
                prompt: launch_prompt,
                session_id: None,
                cwd: cwd.map(|path| path.display().to_string()),
                stall_timeout_seconds: Some(MISSION_AGENT_STALL_TIMEOUT_SECONDS),
                process_metadata: metadata.clone(),
            };
            provider.invoke(invocation)
        },
        |output: &String| output.as_str(),
        |raw: &str| Ok(T::decode(raw)?),
        |_, outcome| {
            attempts.borrow_mut().push(PhaseAttempt {
                attempt: outcome.attempt,
                raw_output: outcome.raw_output.to_string(),
                diagnostics: outcome.diagnostics.map(str::to_string),
                accepted: outcome.diagnostics.is_none(),
            });
            Ok(())
        },
    );

    match result {
        Ok((raw_output, decoded)) => {
            let attempts = attempts.into_inner();
            let _ = raw_output;
            let operation = registry.finish_with_result(
                &operation.id,
                OperationState::Succeeded,
                json!({"stage": stage, "provider": provider_name, "attempts": attempts.len()}),
            )?;
            Ok(PhaseRun {
                operation_id: operation.id,
                output: decoded,
                attempts,
            })
        }
        Err(error) => {
            let attempts = attempts.into_inner();
            let _ = registry.fail_with_error(
                &operation.id,
                json!({
                    "stage": stage,
                    "code": "mission_agent_phase_failed",
                    "message": error.to_string(),
                    "attempts": attempts,
                }),
            );
            Err(error)
        }
    }
}

/// Record one phase's evidence into the Mission round's phase evidence map.
pub fn phase_evidence_entry(
    stage: &str,
    operation_id: &str,
    attempts: &[PhaseAttempt],
    summary: Value,
) -> Value {
    let now = super::service::FileMissionService::now_timestamp();
    json!({
        "operation_id": operation_id,
        "stage": stage,
        "attempts": attempts,
        "summary": summary,
        "recorded": now,
    })
}

/// The provider a Mission phase should use: an explicit choice, else the
/// configured target-app agent CLI, else the first installed agent.
pub fn resolve_mission_provider(
    runtime_root: &Path,
    explicit: Option<&str>,
) -> RefineResult<String> {
    crate::application::agents::provider_selection::resolve_agent_provider(
        runtime_root,
        explicit.map(str::to_string),
    )
}
