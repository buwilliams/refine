use super::*;
use crate::application::agent_io::contracts::skill_result::result_contract;
use crate::application::events::execution::PinnedBinding;
use crate::application::workflow::gates::GateAssessment;
use crate::infrastructure::git::worktrees::FileGitWorktreeService;

fn prepared(fixture: &Fixture, source: &str) -> EventInvocation {
    let service = fixture.service();
    let context = InvocationContext {
        node_id: "default".into(),
        target_root: fixture.0.clone(),
        cwd: fixture.0.clone(),
        workspace: None,
        lifecycle: None,
        provider: "smoke-ai".into(),
        goal_id: None,
        round_idx: None,
        workflow_revision: None,
        candidate_commit: None,
        data: json!({}),
        metadata: Default::default(),
    };
    service
        .prepare(source, context, BTreeMap::new(), "repair-regression")
        .unwrap()
}
fn result(invocation: &EventInvocation, binding: &PinnedBinding, outcome: &str) -> SkillResult {
    let mut result: SkillResult = serde_json::from_value(result_contract(
        &invocation.id,
        &binding.binding.id,
        &binding.skill.role,
    ))
    .unwrap();
    result.outcome = outcome.into();
    result.summary = outcome.into();
    result
}

#[test]
fn execution_outcomes_and_gate_permission_are_independent() {
    let fixture = Fixture::new();
    let mut invocation = prepared(&fixture, "workflow.quality.enter");
    let required = invocation.bindings[0].clone();
    let mut background = required.clone();
    background.binding.id = "background".into();
    background.binding.mode = BindingMode::Background;
    invocation.bindings.push(background.clone());
    invocation.results.insert(
        required.binding.id.clone(),
        result(&invocation, &required, "success"),
    );
    invocation.results.insert(
        background.binding.id.clone(),
        result(&invocation, &background, "error"),
    );
    invocation.state = execution::aggregate_state(&invocation);
    assert_eq!(invocation.state, InvocationState::Error);
    assert_eq!(invocation.gate_assessment(), GateAssessment::Satisfied);
    invocation.event.on_success = Some("retry".into());
    assert!(invocation.success_action_ready());
    invocation
        .results
        .get_mut(&required.binding.id)
        .unwrap()
        .outcome = "failure".into();
    assert_eq!(invocation.gate_assessment(), GateAssessment::Finding);
    assert!(!invocation.success_action_ready());
    invocation.results.remove(&required.binding.id);
    assert_eq!(invocation.gate_assessment(), GateAssessment::Fault);
    invocation.state = InvocationState::Running;
    assert_eq!(invocation.gate_assessment(), GateAssessment::Missing);
    invocation.bindings.remove(0);
    invocation.state = InvocationState::Error;
    assert_eq!(invocation.gate_assessment(), GateAssessment::Satisfied);
    assert!(!invocation.success_action_ready()); // A standalone failed task cannot run a success action.
    invocation.state = InvocationState::Succeeded; // Old producer's recorded state remains visible.
    assert_eq!(invocation.execution_state(), InvocationState::Error);
    assert!(!invocation.success_action_ready());
    assert_eq!(invocation.state, InvocationState::Succeeded);
    invocation.state = InvocationState::Cancelled;
    assert_eq!(invocation.gate_assessment(), GateAssessment::Fault);
}

#[cfg(unix)]
struct ProviderEnv(Option<std::ffi::OsString>);

#[test]
fn blocking_gate_checks_identity_but_does_not_grade_artifacts() {
    let fixture = Fixture::new();
    let mut invocation = prepared(&fixture, "workflow.plan.enter");
    let binding = invocation.bindings[0].clone();
    let mut accepted: SkillResult = serde_json::from_value(result_contract(
        &invocation.id,
        &binding.binding.id,
        &binding.skill.role,
    ))
    .unwrap();
    invocation.state = InvocationState::Succeeded;
    invocation
        .results
        .insert(binding.binding.id.clone(), accepted.clone());
    assert_eq!(invocation.gate_assessment(), GateAssessment::Satisfied);
    accepted.invocation_id = "different-invocation".into();
    invocation
        .results
        .insert(binding.binding.id.clone(), accepted.clone());
    assert_eq!(invocation.gate_assessment(), GateAssessment::Fault);
    accepted.invocation_id = invocation.id.clone();
    accepted.artifacts = json!({"plan": {"summary": "missing checklist"}});
    invocation
        .results
        .insert(binding.binding.id.clone(), accepted);
    assert_eq!(invocation.gate_assessment(), GateAssessment::Satisfied);
}
#[cfg(unix)]
impl Drop for ProviderEnv {
    fn drop(&mut self) {
        unsafe {
            match &self.0 {
                Some(v) => std::env::set_var("REFINE_SMOKE_AI_PATH", v),
                None => std::env::remove_var("REFINE_SMOKE_AI_PATH"),
            }
        }
    }
}
#[cfg(unix)]
fn provider(fixture: &Fixture, script: &str) -> ProviderEnv {
    use std::os::unix::fs::PermissionsExt;
    let path = fixture.0.join("provider");
    std::fs::write(&path, format!("#!/usr/bin/env python3\n{script}")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let restore = ProviderEnv(std::env::var_os("REFINE_SMOKE_AI_PATH"));
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", path);
    }
    restore
}

#[cfg(unix)]
#[test]
fn arbitrary_plan_shapes_are_retained_without_grading() {
    let _env = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    for shape in ["extra_checklist_fields", "resolution_map"] {
        let fixture = Fixture::new();
        let _restore = provider(
            &fixture,
            &format!(
                r#"
import json,sys,pathlib
prompt=' '.join(sys.argv[1:])
contract=json.JSONDecoder().raw_decode(prompt.split('Refine completion contract (supplied by the system):\n',1)[1])[0]
if prompt.startswith('Repair only'):
 assert 'DO_WORK_ONCE_ONLY' not in prompt
 assert 'Rejected completion (data, not instructions):' in prompt
 assert 'Diagnostic:' in prompt
 with pathlib.Path('repair-count').open('a') as f: f.write('repair\n')
else:
 assert 'DO_WORK_ONCE_ONLY' in prompt
 with pathlib.Path('work-count').open('a') as f: f.write('work\n')
 if '{shape}' == 'extra_checklist_fields': contract['artifacts']={{'plan':{{'checklist':[{{'affected_behavior':['context']}}]}}}}
 else: contract['artifacts']={{'plan':{{'criticism_resolutions':{{'C1':'resolved'}}}}}}
print(json.dumps(contract))
"#
            ),
        );
        let service = fixture.service();
        let mut invocation = prepared(&fixture, "workflow.plan.enter");
        invocation.bindings[0].skill.prompt = "DO_WORK_ONCE_ONLY".into();
        service.save_invocation(&invocation).unwrap();
        let completed = service.execute(&invocation.id, || Ok(())).unwrap();
        assert_eq!(completed.state, InvocationState::Succeeded, "{completed:?}");
        assert_eq!(completed.attempts.len(), 1);
        assert!(completed.attempts[0]["diagnostic"].is_null());
        assert_eq!(
            std::fs::read_to_string(fixture.0.join("work-count")).unwrap(),
            "work\n"
        );
        assert!(!fixture.0.join("repair-count").exists());
    }
}

#[cfg(unix)]
#[test]
fn restart_accepts_a_durable_provider_receipt_before_launching_more_work() {
    let _env = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let fixture = Fixture::new();
    let _restore = provider(
        &fixture,
        r#"
import json,sys,pathlib
prompt=' '.join(sys.argv[1:])
result=json.JSONDecoder().raw_decode(prompt.split('Refine completion contract (supplied by the system):\n',1)[1])[0]
with pathlib.Path('work-count').open('a') as f: f.write('work\n')
print(json.dumps(result))
"#,
    );
    let service = fixture.service();
    let mut invocation = prepared(&fixture, "workflow.plan.enter");
    let binding = invocation.bindings[0].clone();
    let contract = crate::application::agent_io::contracts::skill_result::report_contract();
    let prompt = format!("Refine completion contract (supplied by the system):\n{contract}");
    let calls = std::cell::Cell::new(0);
    let interrupted = super::super::completion::run(
        &service,
        &mut invocation,
        &binding,
        &prompt,
        &contract,
        &Default::default(),
        Some(10),
        true,
        &|| {
            calls.set(calls.get() + 1);
            if calls.get() > 1 {
                Err(crate::error::RefineError::Conflict(
                    "simulated shutdown before acceptance".into(),
                ))
            } else {
                Ok(())
            }
        },
    );
    assert!(interrupted.is_err());
    let retained = service.invocation(&invocation.id).unwrap();
    assert_eq!(retained.attempts.len(), 1);
    assert!(retained.results.is_empty());
    let resumed = fixture
        .service()
        .execute(&invocation.id, || Ok(()))
        .unwrap();
    assert_eq!(resumed.state, InvocationState::Succeeded, "{resumed:?}");
    assert_eq!(resumed.attempts.len(), 1);
    assert_eq!(
        std::fs::read_to_string(fixture.0.join("work-count")).unwrap(),
        "work\n"
    );
}

#[test]
fn content_observation_detects_edits_to_already_dirty_files_without_staging_them() {
    let fixture = Fixture::new();
    let git = |args: &[&str]| {
        let result = std::process::Command::new("git")
            .args(args)
            .current_dir(&fixture.0)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        result.stdout
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "Test"]);
    git(&["config", "user.email", "test@example.invalid"]);
    std::fs::write(fixture.0.join("file"), "original").unwrap();
    git(&["add", "file"]);
    git(&["commit", "-qm", "initial"]);
    std::fs::write(fixture.0.join("file"), "staged").unwrap();
    git(&["add", "file"]);
    std::fs::write(fixture.0.join("file"), "dirty one").unwrap();
    let index_before = git(&["write-tree"]);
    let service = FileGitWorktreeService::new(&fixture.0);
    let before = service.observed_worktree_tree().unwrap();
    let status_before = git(&["status", "--porcelain"]);
    std::fs::write(fixture.0.join("file"), "dirty two").unwrap();
    let after = service.observed_worktree_tree().unwrap();
    assert_ne!(before, after);
    assert_eq!(status_before, git(&["status", "--porcelain"]));
    assert_eq!(index_before, git(&["write-tree"]));
    assert!(
        !std::fs::read_dir(fixture.0.join(".git")).unwrap().any(|p| p
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("refine-observation-"))
    );
}

#[test]
fn gate_configuration_is_pinned_per_occurrence_and_new_rounds_see_edits() {
    use crate::application::work_items::FileWorkItemService;
    let fixture = Fixture::new();
    let service = fixture.service();
    let work = FileWorkItemService::new(&service.refine_dir);
    work.create_goal_summary("Pinned gate", Some("PINNED"))
        .unwrap();
    work.append_goal_round_summary("PINNED", "Reporter", "Review the candidate")
        .unwrap();
    let first = service
        .gate_configuration("PINNED", 0, "default", "workflow.quality.enter", || Ok(()))
        .unwrap();
    assert!(!first.skills.contains_key("default-governance"));
    assert_eq!(first.events.len(), 1);
    let mut skill = first.skills["default-quality"].clone();
    skill.prompt = "Changed after admission".into();
    service
        .save(
            "skills",
            &skill.id,
            json!({"revision":first.revision,"item":skill}),
        )
        .unwrap();
    let pinned = service
        .gate_configuration("PINNED", 0, "default", "workflow.quality.enter", || Ok(()))
        .unwrap();
    assert_eq!(
        pinned.skills["default-quality"].prompt,
        first.skills["default-quality"].prompt
    );
    work.append_goal_round_summary("PINNED", "Reporter", "Fresh work")
        .unwrap();
    let next = service
        .gate_configuration("PINNED", 1, "default", "workflow.quality.enter", || Ok(()))
        .unwrap();
    assert_eq!(
        next.skills["default-quality"].prompt,
        "Changed after admission"
    );
}

#[test]
fn paused_and_capacity_waits_are_local_and_transition_bounded() {
    use crate::application::workflow::WorkflowEngine;
    use crate::application::workflow::engine::admission::{ExecutionReservation, reserve};
    use crate::infrastructure::process::supervisor::config::FileSettingsService;
    let fixture = Fixture::new();
    let service = fixture.service();
    FileSettingsService::new(&service.refine_dir)
        .update(&json!({"agent_cli":"smoke-ai","parallel_run_cap":1}))
        .unwrap();
    let invocation = prepared(&fixture, "workflow.quality.enter");
    let engine = WorkflowEngine::with_target_root(fixture.0.join("runtime"), &fixture.0);
    engine.set_workflow_paused(true).unwrap();
    assert_eq!(service.dispatch_pending(&fixture.0).unwrap(), 0);
    let first = service.invocation_view(&invocation.id).unwrap();
    assert_eq!(first["waiting"]["reason"], "paused");
    service.dispatch_pending(&fixture.0).unwrap();
    assert_eq!(
        first["waiting"],
        service.invocation_view(&invocation.id).unwrap()["waiting"]
    );
    engine.set_workflow_paused(false).unwrap();
    let policy = engine.policy_for_refine_dir(&service.refine_dir).unwrap();
    let lease = reserve(
        &engine,
        &policy,
        format!("test:{}", fixture.0.display()),
        ExecutionReservation {
            runtime: fixture.0.join("runtime"),
            invocation_id: None,
            goal_id: Some("HELD".into()),
            node: "default".into(),
            provider: "smoke-ai".into(),
            target: fixture.0.display().to_string(),
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(service.dispatch_pending(&fixture.0).unwrap(), 0);
    assert_eq!(
        service.invocation_view(&invocation.id).unwrap()["waiting"]["reason"],
        "capacity"
    );
    assert!(
        service
            .invocation(&invocation.id)
            .unwrap()
            .attempts
            .is_empty()
    );
    drop(lease);
    service.cancel_invocation(&invocation.id).unwrap();
    assert!(
        service
            .invocation_view(&invocation.id)
            .unwrap()
            .get("waiting")
            .is_none()
    );
}

#[cfg(unix)]
#[test]
fn malformed_report_retains_dirty_candidate_without_a_repair_launch() {
    let _env = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let fixture = Fixture::new();
    let target = fixture.0.join("app");
    std::fs::create_dir(&target).unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "Test"]);
    git(&["config", "user.email", "test@example.invalid"]);
    std::fs::write(target.join("file"), "original").unwrap();
    git(&["add", "file"]);
    git(&["commit", "-qm", "initial"]);
    let index_before = git(&["write-tree"]);
    let _restore = provider(
        &fixture,
        r#"
import json,sys,pathlib
prompt=' '.join(sys.argv[1:])
result=json.JSONDecoder().raw_decode(prompt.split('Refine completion contract (supplied by the system):\n',1)[1])[0]
if prompt.startswith('Repair only'):
 pathlib.Path('file').write_text('unauthorized repair edit')
else:
 pathlib.Path('file').write_text('legitimate quality correction')
 result.pop('outcome')
print(json.dumps(result))
"#,
    );
    let service = fixture.service();
    let mut invocation = prepared(&fixture, "workflow.quality.enter");
    invocation.context.cwd = target.clone();
    invocation.context.target_root = target.clone();
    service.save_invocation(&invocation).unwrap();
    let completed = service.execute(&invocation.id, || Ok(())).unwrap();
    assert_eq!(completed.state, InvocationState::Error);
    assert_eq!(completed.attempts.len(), 1);
    assert_eq!(
        std::fs::read_to_string(target.join("file")).unwrap(),
        "legitimate quality correction"
    );
    assert_eq!(index_before, git(&["write-tree"]));
}

#[test]
fn interrupted_launch_without_receipt_does_not_invoke_provider_again() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let mut invocation = prepared(&fixture, "workflow.plan.enter");
    let binding = invocation.bindings[0].clone();
    invocation.context.metadata.insert(
        "started_bindings".into(),
        json!({binding.binding.id.clone(): "2026-09-11T00:00:00Z"}),
    );
    service.save_invocation(&invocation).unwrap();
    let contract = result_contract(&invocation.id, &binding.binding.id, &binding.skill.role);
    let error = super::super::completion::run(
        &service,
        &mut invocation,
        &binding,
        "must not launch",
        &contract,
        &Default::default(),
        Some(10),
        true,
        &|| Ok(()),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("interrupted without a completion receipt"),
        "{error}"
    );
    assert!(
        service
            .invocation(&invocation.id)
            .unwrap()
            .attempts
            .is_empty()
    );
}

#[test]
fn every_workflow_role_uses_the_agent_decision_without_required_supporting_fields() {
    for role in ["plan", "implement", "quality", "governance"] {
        for outcome in ["success", "failure", "error"] {
            let fixture = Fixture::new();
            let mut invocation = prepared(&fixture, &format!("workflow.{role}.enter"));
            let binding = invocation.bindings[0].clone();
            let result: SkillResult = serde_json::from_value(json!({
                "invocation_id":invocation.id, "binding_id":binding.binding.id,
                "role":role, "outcome":outcome
            }))
            .unwrap();
            invocation.results.insert(binding.binding.id, result);
            invocation.state = execution::aggregate_state(&invocation);
            assert_eq!(
                invocation.gate_assessment(),
                match outcome {
                    "success" => GateAssessment::Satisfied,
                    "failure" => GateAssessment::Finding,
                    _ => GateAssessment::Fault,
                },
                "{role}: {outcome}"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn plan_and_governance_skills_may_edit_when_their_instructions_authorize_it() {
    let _env = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    for role in ["plan", "governance"] {
        let fixture = Fixture::new();
        fixture.repository();
        let _restore = provider(
            &fixture,
            r#"
import json,sys,pathlib
prompt=' '.join(sys.argv[1:])
assert 'Do not change Goal state, merge or push.' not in prompt
assert 'This invocation is observational' not in prompt
assert 'Use supported Refine commands for Goal changes.' in prompt
result=json.JSONDecoder().raw_decode(prompt.split('Refine completion contract (supplied by the system):\n',1)[1])[0]
pathlib.Path('authorized-edit.txt').write_text('Skill-authorized change\n')
result['outcome']='success'
print(json.dumps(result))
"#,
        );
        let service = fixture.service();
        let work = crate::application::work_items::FileWorkItemService::new(&service.refine_dir);
        work.create_goal_summary("Skill-authorized work", Some("GOAL1"))
            .unwrap();
        work.append_goal_round_summary("GOAL1", "test", "Edit the requested file")
            .unwrap();
        let git = FileGitWorktreeService::new(&fixture.0);
        let base = git.resolve_commit("HEAD").unwrap();
        let branch = "refine/GOAL1/round-1";
        let workspace = git.managed_worktree_path(branch).unwrap();
        git.ensure_worktree_from_base(branch, &workspace, &base)
            .unwrap();
        work.update_goal_git_refs("GOAL1", branch, "main", &base, Some(&base))
            .unwrap();
        let mut invocation = prepared(&fixture, &format!("workflow.{role}.enter"));
        invocation.context.cwd = workspace.clone();
        invocation.context.workspace = Some(
            crate::infrastructure::git::worktrees::ManagedWorktree {
                repository: fixture.0.clone(),
                path: workspace.clone(),
                branch: branch.into(),
                commit: None,
                allow_rebase: false,
                registration: None,
            }
            .pin()
            .unwrap(),
        );
        invocation.context.candidate_commit = Some(base);
        invocation.context.goal_id = Some("GOAL1".into());
        invocation.context.round_idx = Some(0);
        invocation.bindings[0].skill.prompt =
            "Edit authorized-edit.txt as needed, then report your decision.".into();
        service.save_invocation(&invocation).unwrap();
        let completed = service.execute(&invocation.id, || Ok(())).unwrap();
        assert_eq!(completed.state, InvocationState::Succeeded, "{completed:?}");
        assert_eq!(
            std::fs::read_to_string(workspace.join("authorized-edit.txt")).unwrap(),
            "Skill-authorized change\n"
        );
        assert_eq!(completed.attempts[0]["observational_violation"], false);
        assert!(
            super::super::completion::accepted_tree(&completed, &completed.bindings[0].binding.id)
                .is_some()
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn admitted_workflow_continues_interrupted_work_once_in_the_same_round() {
    let _env = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let fixture = Fixture::new();
    fixture.repository();
    let _restore = provider(
        &fixture,
        r#"
import json,sys,pathlib
prompt=' '.join(sys.argv[1:])
assert 'Previous execution was interrupted' in prompt
result=json.JSONDecoder().raw_decode(prompt.split('Refine completion contract (supplied by the system):\n',1)[1])[0]
with pathlib.Path('work-count').open('a') as f: f.write('work\n')
print(json.dumps(result))
"#,
    );
    let service = fixture.service();
    let items = crate::application::work_items::FileWorkItemService::new(&service.refine_dir);
    items
        .create_goal_summary("Continue", Some("GOAL1"))
        .unwrap();
    items
        .append_goal_round_summary("GOAL1", "Human", "Retain existing work")
        .unwrap();
    items
        .set_goal_status_unchecked("GOAL1", &crate::model::workflow::GoalStatus::Plan)
        .unwrap();
    let git = FileGitWorktreeService::new(&fixture.0);
    let branch = "refine/GOAL1/round-1";
    let base = git.resolve_commit("main").unwrap();
    let cwd = git
        .ensure_worktree_from_base(branch, &git.managed_worktree_path(branch).unwrap(), &base)
        .unwrap();
    items
        .update_goal_git_refs("GOAL1", branch, "main", &base, None)
        .unwrap();
    let goal = items.show_goal_detail("GOAL1").unwrap();
    let mut invocation = service
        .prepare(
            "workflow.plan.enter",
            InvocationContext {
                node_id: "default".into(),
                target_root: fixture.0.clone(),
                cwd: cwd.clone().into(),
                workspace: None,
                lifecycle: None,
                provider: "smoke-ai".into(),
                goal_id: Some("GOAL1".into()),
                round_idx: Some(0),
                workflow_revision: goal["workflow_revision"].as_u64(),
                candidate_commit: None,
                data: json!({"goal":goal}),
                metadata: Default::default(),
            },
            Default::default(),
            "continue-plan",
        )
        .unwrap();
    invocation.state = InvocationState::Running;
    invocation.context.metadata.insert(
        "started_bindings".into(),
        json!({invocation.bindings[0].binding.id.clone():"interrupted"}),
    );
    service.save_invocation(&invocation).unwrap();
    use crate::infrastructure::process::subprocess::{
        FileProcessSupervisor, ManagedProcessSpec, ProcessOwner, ProcessSupervisor,
    };
    let old_owner = FileProcessSupervisor::new(fixture.0.join("runtime/agents"));
    let old_process = old_owner
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Runner,
            command: "/bin/sleep".into(),
            args: vec!["60".into()],
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: serde_json::from_value(json!({
                "event_invocation_id": invocation.id, "goal_id":"GOAL1",
                "isolated_process_group":true
            }))
            .unwrap(),
        })
        .unwrap();
    let completed = service
        .execute_with_metadata(&invocation.id, Some(&Default::default()), || Ok(()))
        .unwrap();
    assert!(!old_owner.group_pending(&old_process).unwrap());
    assert_eq!(completed.state, InvocationState::Succeeded, "{completed:?}");
    assert_eq!(
        completed.context.metadata["interrupted_launches"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(completed.context.round_idx, Some(0));
    let repeated = service
        .execute_with_metadata(&invocation.id, Some(&Default::default()), || Ok(()))
        .unwrap();
    assert_eq!(repeated, completed);
    assert_eq!(
        std::fs::read_to_string(std::path::Path::new(&cwd).join("work-count")).unwrap(),
        "work\n"
    );
    assert_eq!(
        items.show_goal_detail("GOAL1").unwrap()["rounds"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
