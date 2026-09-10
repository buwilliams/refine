//! Workflow adapters bind generic Skill results to the existing semantic gates.
use super::execution::BlockingInvocation;
use super::{FileEventService, InvocationContext, InvocationState};
use crate::application::workflow::engine::context::WorkflowContext;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::supervisor::coordination::with_record_lock;
use crate::model::automation::{BindingMode, SkillResult};
use crate::model::workflow::GoalStatus;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

pub fn run(
    ctx: &WorkflowContext<'_>,
    status: GoalStatus,
    edge: &str,
    cwd: &Path,
    data: Value,
    variant: &str,
) -> RefineResult<Vec<SkillResult>> {
    Ok(run_checked(ctx, status, edge, cwd, data, variant)?.results)
}

#[derive(Default)]
struct WorkflowResults {
    results: Vec<SkillResult>,
    required: Vec<BlockingInvocation>,
}

fn run_checked(
    ctx: &WorkflowContext<'_>,
    status: GoalStatus,
    edge: &str,
    cwd: &Path,
    data: Value,
    variant: &str,
) -> RefineResult<WorkflowResults> {
    ctx.revalidate_authority(status.clone())?;
    let source = format!("workflow.{}.{}", status.as_str(), edge);
    let config = FileEventService::new(ctx.refine_dir()).gate_configuration(
        &ctx.goal_id,
        ctx.round_idx,
        &ctx.node_id,
        &source,
        || ctx.revalidate_authority(status.clone()),
    )?;
    let service = FileEventService::with_runtime_root(ctx.refine_dir(), ctx.runtime_root);
    let events = config
        .events
        .values()
        .filter(|e| {
            e.source.as_deref() == Some(&source) && e.enabled && e.scope.applies(&ctx.node_id)
        })
        .collect::<Vec<_>>();
    let requires_role = edge == "enter" && ["plan", "implement"].contains(&status.as_str());
    if requires_role
        && !events.iter().any(|e| {
            config
                .bindings(e, &ctx.node_id)
                .iter()
                .any(|(b, _)| b.mode == BindingMode::Blocking)
        })
    {
        return Err(RefineError::InvalidInput(format!(
            "{} requires an enabled blocking {} Skill",
            source,
            status.as_str()
        )));
    }
    let goal = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
    if events.iter().all(|event| {
        config
            .bindings(event, &ctx.node_id)
            .iter()
            .all(|(binding, _)| binding.mode == BindingMode::Context)
    }) {
        return Ok(WorkflowResults::default());
    }
    let preplan = status == GoalStatus::Todo;
    let mut context = InvocationContext {
        node_id: ctx.node_id.clone(),
        target_root: ctx.target_root.into(),
        cwd: cwd.into(),
        workspace: if preplan {
            None
        } else {
            Some(ctx.managed_worktree()?)
        },
        lifecycle: None,
        provider: ctx.provider.clone(),
        goal_id: Some(ctx.goal_id.clone()),
        round_idx: Some(ctx.round_idx),
        workflow_revision: Some(ctx.attempt_authority.workflow_revision),
        candidate_commit: ctx.commit.clone(),
        data: json!({"goal": super::execution::goal_context(&goal), "system": {"node_id": ctx.node_id, "project_root": ctx.target_root, "workspace": cwd, "workflow_step": status.as_str(), "candidate_commit": ctx.commit}, "context": data}),
        metadata: ctx.workflow_process_metadata(status.as_str(), "EventSkill"),
    };
    // Todo has no implementation checkout yet. Entry reuses the durable occurrence
    // dispatched by the lifecycle worker; otherwise the current workflow claim
    // authorizes a separate invocation-owned checkout for the Todo-to-Plan edge.
    if preplan
        && edge == "enter"
        && let Some(occurrence) = goal["workflow_events"].as_array().and_then(|items| {
            items
                .iter()
                .find(|item| item["generation"] == goal["event_generation"] && item["to"] == "todo")
        })
    {
        context.data["occurrence"] = occurrence.clone();
    }
    let mut checked = WorkflowResults::default();
    for event in events {
        if config
            .bindings(event, &ctx.node_id)
            .iter()
            .all(|(b, _)| b.mode == BindingMode::Context)
        {
            continue;
        }
        let generation = goal
            .get("event_generation")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let key = format!(
            "{}:{}:{generation}:{}:{source}:{variant}",
            ctx.goal_id, ctx.round_idx, ctx.node_id
        );
        let invocation =
            service.prepare_pinned(&config, event, context.clone(), BTreeMap::new(), &key)?;
        if let Some(required) = BlockingInvocation::pin(&invocation) {
            checked.required.push(required);
        }
        let mut invocation =
            service.execute_with_metadata(&invocation.id, Some(&context.metadata), || {
                ctx.revalidate_authority(status.clone())
            })?;
        if invocation.success_action_ready() {
            service.apply_success_action(&mut invocation)?;
            ctx.revalidate_authority(status.clone())?;
        }
        with_record_lock(&ctx.refine_dir(), &ctx.goal_id, || {
            ctx.revalidate_authority(status.clone())?;
            let detail = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
            let mut evidence = detail
                .get("rounds")
                .and_then(Value::as_array)
                .and_then(|rs| rs.get(ctx.round_idx))
                .and_then(|r| r.get("event_results"))
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            evidence.insert(invocation.id.clone(), json!({"source": source, "state": invocation.state, "results": invocation.results, "candidate_commit": ctx.commit}));
            ctx.work_items.update_goal_round_evaluation_summary(
                &ctx.goal_id,
                ctx.round_idx,
                &json!({"event_results": evidence}),
            )?;
            Ok(())
        })?;
        let blocking = config
            .bindings(event, &ctx.node_id)
            .iter()
            .any(|(binding, _)| binding.mode == BindingMode::Blocking);
        if blocking && invocation.bindings.is_empty() && invocation.state == InvocationState::Error
        {
            return Err(invocation.execution_error());
        }
        checked.results.extend(
            invocation
                .blocking_results()?
                .into_iter()
                .map(|mut result| {
                    result.binding_id = format!("{}:{}", event.id, result.binding_id);
                    result
                }),
        );
    }
    // A later binding/event may have invalidated an earlier completion. Preserve
    // failed findings as results, but never return incomplete or replaced evidence.
    with_record_lock(&ctx.refine_dir(), &ctx.goal_id, || {
        ctx.revalidate_authority(status)?;
        for required in &checked.required {
            required.validate_completion(&service)?;
        }
        Ok(())
    })?;
    Ok(checked)
}

pub fn require_success(results: &[SkillResult]) -> RefineResult<()> {
    if let Some(result) = results.iter().find(|r| r.outcome != "success") {
        return Err(RefineError::Degraded(format!(
            "Skill {} failed: {}",
            result.binding_id, result.summary
        )));
    }
    Ok(())
}

pub fn exit(ctx: &WorkflowContext<'_>, from: GoalStatus, to: GoalStatus) -> RefineResult<()> {
    let cwd = ctx
        .worktree_path
        .as_deref()
        .map(Path::new)
        .unwrap_or(ctx.target_root);
    let mut checked = WorkflowResults::default();
    if !["plan", "implement", "quality", "governance"].contains(&from.as_str()) {
        checked = run_checked(
            ctx,
            from.clone(),
            "enter",
            cwd,
            json!({"destination":to.as_str()}),
            "",
        )?;
        require_success(&checked.results)?;
    }
    let results = run_checked(
        ctx,
        from.clone(),
        "exit",
        cwd,
        json!({"destination": to.as_str()}),
        &format!("to-{}-{}", to.as_str(), ctx.commit.as_deref().unwrap_or("")),
    )?;
    require_success(&results.results)?;
    checked.required.extend(results.required);
    // Later Skills may run after an entry verdict was accepted. Recheck every
    // blocking invocation at settlement before authorizing the status change.
    with_record_lock(&ctx.refine_dir(), &ctx.goal_id, || {
        ctx.revalidate_authority(from)?;
        let service = FileEventService::with_runtime_root(ctx.refine_dir(), ctx.runtime_root);
        service.settle_blocking(&checked.required, |validation| {
            validation?;
            let goal = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
            super::transitions::approve_exit(&ctx.refine_dir(), &goal, to.as_str())
        })
    })
}
