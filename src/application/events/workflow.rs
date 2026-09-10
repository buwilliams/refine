//! Workflow adapters bind generic Skill results to the existing semantic gates.
use super::{FileEventService, InvocationContext};
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
    let context = InvocationContext {
        node_id: ctx.node_id.clone(),
        target_root: ctx.target_root.into(),
        cwd: cwd.into(),
        provider: ctx.provider.clone(),
        goal_id: Some(ctx.goal_id.clone()),
        round_idx: Some(ctx.round_idx),
        workflow_revision: Some(ctx.attempt_authority.workflow_revision),
        candidate_commit: ctx.commit.clone(),
        data: json!({"goal": super::execution::goal_context(&goal), "system": {"node_id": ctx.node_id, "project_root": ctx.target_root, "workspace": cwd, "workflow_step": status.as_str(), "candidate_commit": ctx.commit}, "context": data}),
        metadata: ctx.workflow_process_metadata(status.as_str(), "EventSkill"),
    };
    let mut results = Vec::new();
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
        if invocation.gate_assessment()
            == crate::application::workflow::gates::GateAssessment::Fault
        {
            return Err(invocation.execution_error());
        }
        results.extend(
            invocation
                .bindings
                .iter()
                .filter(|b| b.binding.mode == BindingMode::Blocking)
                .filter_map(|b| invocation.results.get(&b.binding.id))
                .cloned()
                .map(|mut result| {
                    result.binding_id = format!("{}:{}", event.id, result.binding_id);
                    result
                }),
        );
    }
    Ok(results)
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
    if !["plan", "implement", "quality", "governance"].contains(&from.as_str()) {
        require_success(&run(ctx, from.clone(), "enter", cwd, json!({}), "")?)?;
    }
    let results = run(
        ctx,
        from.clone(),
        "exit",
        cwd,
        json!({"destination": to.as_str()}),
        &format!("to-{}-{}", to.as_str(), ctx.commit.as_deref().unwrap_or("")),
    )?;
    require_success(&results)?;
    ctx.revalidate_authority(from)?;
    let goal = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
    super::transitions::approve_exit(&ctx.refine_dir(), &goal, to.as_str())
}
