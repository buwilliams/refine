use super::*;
use crate::model::automation::*;
use serde_json::json;
use std::collections::BTreeMap;

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("refine-events-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn service(&self) -> FileEventService {
        FileEventService::with_runtime_root(self.0.join("state"), self.0.join("runtime"))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn parameter() -> Parameter {
    Parameter {
        name: "subject".into(),
        required: true,
        default: Some(json!("default")),
        ..Default::default()
    }
}

#[test]
fn migration_preserves_content_is_idempotent_and_stale_writes_cannot_erase_edits() {
    let fixture = Fixture::new();
    let service = fixture.service();
    std::fs::create_dir_all(&service.refine_dir).unwrap();
    std::fs::write(service.refine_dir.join("guidance.json"), r#"[{"id":"context","name":"Accessibility","rule":"For interfaces","instructions":"Support keyboard navigation","enabled":false}]"#).unwrap();
    let config = service.config().unwrap();
    assert_eq!(config.events.len(), 21);
    assert!(!system_catalog().iter().any(|s| s.contains("sync")));
    assert!(
        config.skills["guidance-context"]
            .prompt
            .contains("keyboard navigation")
    );
    assert!(!config.skills["guidance-context"].enabled);
    assert!(
        service
            .refine_dir
            .join("automation/migration-v1.json")
            .exists()
    );
    let mut skill = config.skills["default-plan"].clone();
    skill.prompt = "A new planning method".into();
    service
        .save(
            "skills",
            &skill.id,
            json!({"revision": config.revision, "item": skill}),
        )
        .unwrap();
    assert!(
        service
            .save(
                "skills",
                &skill.id,
                json!({"revision": config.revision, "item": skill})
            )
            .unwrap_err()
            .to_string()
            .contains("Refresh")
    );
    std::fs::write(service.refine_dir.join("guidance.json"), "[]").unwrap();
    let current = service.config().unwrap();
    assert_eq!(
        current.skills["default-plan"].prompt,
        "A new planning method"
    );
    assert!(current.skills.contains_key("guidance-context"));
}

#[test]
fn disabled_node_override_masks_only_its_project_binding() {
    let fixture = Fixture::new();
    let mut config = (*fixture.service().config().unwrap()).clone();
    let event = config.events.get_mut("workflow.plan.enter").unwrap();
    let mut override_binding = event.bindings[0].clone();
    override_binding.id = "node-plan".into();
    override_binding.scope.node_id = Some("node-a".into());
    override_binding.overrides = Some("default-plan".into());
    override_binding.enabled = false;
    event.bindings.push(override_binding);
    config.validate().unwrap();
    let event = &config.events["workflow.plan.enter"];
    assert!(config.bindings(event, "node-a").is_empty());
    assert_eq!(config.bindings(event, "node-b").len(), 1);
}

#[test]
fn typed_parameters_resolve_explicit_then_context_then_default_and_fail_on_missing() {
    let explicit = BTreeMap::from([("subject".into(), json!("manual"))]);
    let mapped = BTreeMap::from([("subject".into(), json!("goal"))]);
    assert_eq!(
        execution::resolve_parameters(&[parameter()], &explicit, &mapped).unwrap()["subject"],
        "manual"
    );
    assert_eq!(
        execution::resolve_parameters(&[parameter()], &BTreeMap::new(), &mapped).unwrap()["subject"],
        "goal"
    );
    assert_eq!(
        execution::resolve_parameters(&[parameter()], &BTreeMap::new(), &BTreeMap::new()).unwrap()
            ["subject"],
        "default"
    );
    let mut required = parameter();
    required.default = None;
    assert!(
        execution::resolve_parameters(&[required], &BTreeMap::new(), &BTreeMap::new())
            .unwrap_err()
            .to_string()
            .contains("subject")
    );
    assert!(
        execution::resolve_parameters(
            &[parameter()],
            &BTreeMap::from([("unknown".into(), json!(1))]),
            &BTreeMap::new()
        )
        .is_err()
    );
}

#[test]
fn callback_cannot_spoof_another_invocation_role_or_binding() {
    let result = SkillResult {
        invocation_id: "one".into(),
        binding_id: "first".into(),
        role: "task".into(),
        outcome: "success".into(),
        summary: "Done".into(),
        evidence: Vec::new(),
        artifacts: json!({}),
    };
    assert!(result.validate("two", "first", "task").is_err());
    assert!(result.validate("one", "second", "task").is_err());
    assert!(result.validate("one", "first", "governance").is_err());
    result.validate("one", "first", "task").unwrap();
}

#[cfg(unix)]
#[test]
fn ordered_independent_runs_collect_failure_preserve_evidence_and_do_not_replay() {
    use std::os::unix::fs::PermissionsExt;
    let _env = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap();
    let fixture = Fixture::new();
    let service = fixture.service();
    let provider = fixture.0.join("smoke-ai");
    std::fs::write(&provider, r#"#!/usr/bin/env python3
import json, sys, pathlib
prompt = sys.argv[1]
contract = json.loads(prompt.split('Refine completion contract (supplied by the system):\n', 1)[1].split('\nReturn one JSON', 1)[0])
contract['outcome'] = 'failure' if prompt.startswith('FAIL') else 'success'
contract['summary'] = 'Observed ' + contract['outcome']
path = pathlib.Path('launches.txt')
with path.open('a') as f: f.write(contract['binding_id'] + '\n')
print(json.dumps(contract))
"#).unwrap();
    std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o755)).unwrap();
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }
    let mut config = (*service.config().unwrap()).clone();
    for (id, prompt) in [
        ("first", "FAIL: inspect first"),
        ("second", "Inspect independently"),
    ] {
        config.skills.insert(
            id.into(),
            Skill {
                id: id.into(),
                name: id.into(),
                prompt: prompt.into(),
                role: "task".into(),
                enabled: true,
                scope: Scope::default(),
                parameters: Vec::new(),
                provenance: None,
            },
        );
    }
    let event = EventDefinition {
        id: "manual".into(),
        name: "Manual test".into(),
        kind: EventKind::Custom,
        source: None,
        enabled: true,
        scope: Scope::default(),
        parameters: Vec::new(),
        on_success: None,
        bindings: ["first", "second"]
            .iter()
            .enumerate()
            .map(|(order, id)| Binding {
                id: (*id).into(),
                skill_id: (*id).into(),
                enabled: true,
                mode: BindingMode::Blocking,
                order: order as i32,
                scope: Scope::default(),
                overrides: None,
                inputs: BTreeMap::new(),
            })
            .collect(),
    };
    let context = InvocationContext {
        node_id: "default".into(),
        target_root: fixture.0.clone(),
        cwd: fixture.0.clone(),
        provider: "smoke-ai".into(),
        goal_id: None,
        round_idx: None,
        workflow_revision: None,
        candidate_commit: None,
        data: json!({}),
        metadata: Default::default(),
    };
    let invocation = service
        .prepare_pinned(&config, &event, context, BTreeMap::new(), "occurrence")
        .unwrap();
    let result = service.execute(&invocation.id, || Ok(()));
    unsafe {
        match previous {
            Some(value) => std::env::set_var("REFINE_SMOKE_AI_PATH", value),
            None => std::env::remove_var("REFINE_SMOKE_AI_PATH"),
        }
    }
    let result = result.unwrap();
    assert_eq!(result.state, InvocationState::Failed, "{result:?}");
    assert_eq!(result.results.len(), 2);
    assert_eq!(
        std::fs::read_to_string(fixture.0.join("launches.txt")).unwrap(),
        "first\nsecond\n"
    );
    assert_eq!(service.execute(&invocation.id, || Ok(())).unwrap(), result);
    assert_eq!(
        std::fs::read_to_string(fixture.0.join("launches.txt")).unwrap(),
        "first\nsecond\n"
    );
    assert_eq!(result.attempts.len(), 2);
}

#[cfg(unix)]
#[test]
fn default_skills_complete_a_real_workflow_with_observed_quality_and_candidate_evidence() {
    use crate::application::work_items::FileWorkItemService;
    use crate::application::workflow::WorkflowEngine;
    use crate::infrastructure::process::supervisor::config::FileSettingsService;
    use crate::model::workflow::GoalStatus;
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let target = fixture.0.join("app");
    std::fs::create_dir_all(&target).unwrap();
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
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "events@example.invalid"]);
    git(&["config", "user.name", "Events test"]);
    std::fs::write(target.join("app.txt"), "before\n").unwrap();
    git(&["add", "app.txt"]);
    git(&["commit", "-qm", "initial"]);
    let provider = fixture.0.join("provider");
    std::fs::write(&provider, r##"#!/usr/bin/env python3
import sys,json,pathlib
prompt=' '.join(sys.argv[1:])
decode=json.JSONDecoder().raw_decode
result=decode(prompt.split('Refine completion contract (supplied by the system):\n',1)[1])[0]
context=decode(prompt.split('Pinned context:\n',1)[1])[0]
role=result['role']
result['evidence']=['Inspected the candidate and observed the requested behavior.']
if role=='implement':
 pathlib.Path('app.txt').write_text('after\n')
 checklist=context['goal']['rounds'][-1]['implementation_plan']['final_plan']['result']['checklist']
 result['artifacts']={'implementation_evidence':{'checklist':[{'id':i['id'],'outcome':'completed','evidence':'Changed app.txt and verified contents'} for i in checklist], 'verification':['app.txt contains after']}}
if role=='quality':
 result['artifacts']={'tests':[{'test':'Requested output','command':"test \"$(cat app.txt)\" = after",'status':'passed','evidence':'The supervised command checks the actual file'}]}
print(json.dumps(result))
"##).unwrap();
    std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _environment = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            unsafe {
                if let Some(v) = &self.0 {
                    std::env::set_var("REFINE_SMOKE_AI_PATH", v)
                } else {
                    std::env::remove_var("REFINE_SMOKE_AI_PATH")
                }
            }
        }
    }
    let _restore = Restore(std::env::var_os("REFINE_SMOKE_AI_PATH"));
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }
    let root = crate::infrastructure::storage::project_layout::refine_dir_for_target_root(&target)
        .unwrap();
    FileSettingsService::new(&root)
        .update(&json!({"agent_cli":"smoke-ai"}))
        .unwrap();
    let service = FileEventService::with_runtime_root(&root, fixture.0.join("runtime"));
    service.config().unwrap();
    let work = FileWorkItemService::new(&root);
    work.create_goal_summary("Change the output", Some("EVENTGOAL"))
        .unwrap();
    work.append_goal_round_summary(
        "EVENTGOAL",
        "Reporter",
        "Change app.txt to after and verify it",
    )
    .unwrap();
    work.transition_goal_status("EVENTGOAL", GoalStatus::Todo)
        .unwrap();
    let result =
        WorkflowEngine::with_target_root(fixture.0.join("runtime"), &target).evaluate_workflow();
    assert!(
        result.is_ok(),
        "{result:?}\n{}",
        work.show_goal_detail("EVENTGOAL").unwrap()
    );
    let goal = work.show_goal_detail("EVENTGOAL").unwrap();
    assert_eq!(goal["status"], "review", "{goal}");
    assert_eq!(
        std::fs::read_to_string(target.join("app.txt")).unwrap(),
        "after\n"
    );
    assert!(
        goal["rounds"][0]["event_results"]
            .as_object()
            .unwrap()
            .len()
            >= 4
    );
    let runs = service.invocations(0, 100).unwrap();
    assert!(
        runs["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|run| run["event"]["source"] == "workflow.quality.enter"
                && run["state"] == "succeeded")
    );
}

fn add_gate(service: &FileEventService, source: &str, missing_input: bool) {
    let mut config = (*service.config().unwrap()).clone();
    let mut skill = config.skills["default-plan"].clone();
    skill.id = "gate".into();
    skill.role = "task".into();
    if missing_input {
        let mut input = parameter();
        input.default = None;
        skill.parameters.push(input);
    }
    config.skills.insert(skill.id.clone(), skill);
    let binding = Binding {
        id: "gate".into(),
        skill_id: "gate".into(),
        enabled: true,
        mode: BindingMode::Blocking,
        order: 0,
        scope: Scope::default(),
        overrides: None,
        inputs: BTreeMap::new(),
    };
    config
        .events
        .get_mut(source)
        .unwrap()
        .bindings
        .push(binding);
    crate::infrastructure::storage::automation::AutomationStore::new(&service.refine_dir)
        .update(config.revision, |stored| {
            *stored = config;
            Ok(())
        })
        .unwrap();
}

#[test]
fn manual_transition_waits_for_exit_evidence_and_cancellation_can_supersede_it() {
    use crate::application::work_items::FileWorkItemService;
    use crate::model::workflow::GoalStatus;
    let fixture = Fixture::new();
    let service = fixture.service();
    add_gate(&service, "workflow.backlog.exit", false);
    let work = FileWorkItemService::new(&service.refine_dir);
    for id in ["GATE1", "GATE2"] {
        work.create_goal_summary("Gated transition", Some(id))
            .unwrap();
        work.append_goal_round_summary(id, "Reporter", "Do work")
            .unwrap();
        let error = work
            .transition_goal_status(id, GoalStatus::Todo)
            .unwrap_err();
        assert!(error.to_string().contains(transitions::PENDING));
        assert_eq!(work.show_goal_detail(id).unwrap()["status"], "backlog");
    }
    service.dispatch_goal_events(&fixture.0).unwrap();
    let runs = service.invocations(0, 100).unwrap();
    for run in runs["items"].as_array().unwrap() {
        let id = run["id"].as_str().unwrap();
        let mut invocation = service.invocation(id).unwrap();
        invocation.state = InvocationState::Succeeded;
        service.save_invocation(&invocation).unwrap();
    }
    work.cancel_goal_summary("GATE2").unwrap();
    service.dispatch_goal_events(&fixture.0).unwrap();
    let first = work.show_goal_detail("GATE1").unwrap();
    assert_eq!(first["status"], "todo");
    assert!(first.get("pending_event_transition").is_none());
    assert_eq!(
        work.show_goal_detail("GATE2").unwrap()["status"],
        "cancelled"
    );
    let generation = first["event_generation"].clone();
    service.dispatch_goal_events(&fixture.0).unwrap();
    assert_eq!(
        work.show_goal_detail("GATE1").unwrap()["event_generation"],
        generation
    );
}

#[test]
fn missing_automatic_input_settles_as_visible_error_instead_of_waiting_forever() {
    use crate::application::work_items::FileWorkItemService;
    use crate::model::workflow::GoalStatus;
    let fixture = Fixture::new();
    let service = fixture.service();
    add_gate(&service, "workflow.backlog.exit", true);
    let work = FileWorkItemService::new(&service.refine_dir);
    work.create_goal_summary("Missing parameter", Some("INPUT1"))
        .unwrap();
    work.append_goal_round_summary("INPUT1", "Reporter", "Do work")
        .unwrap();
    assert!(
        work.transition_goal_status("INPUT1", GoalStatus::Todo)
            .is_err()
    );
    service.dispatch_goal_events(&fixture.0).unwrap();
    let goal = work.show_goal_detail("INPUT1").unwrap();
    assert_eq!(goal["status"], "backlog");
    assert_eq!(goal["pending_event_transition"]["state"], "failed");
    let runs = service.invocations(0, 100).unwrap();
    assert_eq!(runs["items"][0]["state"], "error");
    assert!(
        runs["items"][0]["error"]
            .as_str()
            .unwrap()
            .contains("subject")
    );
}

#[test]
fn configuration_sync_merges_disjoint_edits_fences_both_editors_and_rejects_broken_references() {
    let fixture = Fixture::new();
    let base = (*fixture.service().config().unwrap()).clone();
    let mut left = base.clone();
    let mut right = base.clone();
    left.revision += 1;
    right.revision += 1;
    left.skills.get_mut("default-plan").unwrap().prompt = "Left plan".into();
    right.skills.get_mut("default-quality").unwrap().prompt = "Right quality".into();
    let merge = |a: &AutomationConfig, b: &AutomationConfig| {
        crate::application::persistence_sync::state_merge::merge_state_file(
            std::path::Path::new("automation/config.json"),
            &serde_json::to_vec(&base).unwrap(),
            &serde_json::to_vec(a).unwrap(),
            &serde_json::to_vec(b).unwrap(),
        )
    };
    let merged: AutomationConfig = serde_json::from_slice(&merge(&left, &right).unwrap()).unwrap();
    assert_eq!(merged.revision, 3);
    assert_eq!(merged.skills["default-plan"].prompt, "Left plan");
    assert_eq!(merged.skills["default-quality"].prompt, "Right quality");
    right.skills.get_mut("default-plan").unwrap().prompt = "Conflicting plan".into();
    assert!(merge(&left, &right).is_none());
    right = base.clone();
    right.skills.remove("default-plan");
    assert!(merge(&left, &right).is_none());
}

#[test]
fn cancelled_invocations_cannot_be_overwritten_or_relaunched_by_late_workers() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let config = service.config().unwrap();
    let mut event = config.events["workflow.plan.enter"].clone();
    event.bindings.clear();
    let context = service.manual_context(&fixture.0, &json!({})).unwrap();
    let invocation = service
        .prepare_pinned(&config, &event, context, BTreeMap::new(), "cancel-test")
        .unwrap();
    service.cancel_invocation(&invocation.id).unwrap();
    let mut late = invocation.clone();
    late.state = InvocationState::Succeeded;
    assert!(service.save_invocation(&late).is_err());
    assert_eq!(
        service
            .execute(&invocation.id, || panic!(
                "cancelled invocation must not launch"
            ))
            .unwrap()
            .state,
        InvocationState::Cancelled
    );
}

#[test]
fn startup_occurrences_are_idempotent_and_new_boots_get_new_invocations() {
    let fixture = Fixture::new();
    let service = fixture.service();
    add_gate(&service, "node.startup.ready", false);
    service.startup_ready(&fixture.0, "boot-one").unwrap();
    service.startup_ready(&fixture.0, "boot-one").unwrap();
    assert_eq!(service.invocations(0, 100).unwrap()["total"], 1);
    service.startup_ready(&fixture.0, "boot-two").unwrap();
    assert_eq!(service.invocations(0, 100).unwrap()["total"], 2);
}

#[test]
fn manual_request_identity_and_success_action_receipts_survive_replay() {
    use crate::application::work_items::FileWorkItemService;
    let fixture = Fixture::new();
    let service = fixture.service();
    let config = service.config().unwrap();
    service.save("events", "retry", json!({"revision": config.revision, "item": {"name":"Retry", "on_success":"retry", "parameters":[{"name":"reason", "default":"requested"}]}})).unwrap();
    assert!(
        service
            .trigger("retry", &fixture.0, &json!({}))
            .unwrap_err()
            .to_string()
            .contains("Select a Goal")
    );
    let work = FileWorkItemService::new(&service.refine_dir);
    work.create_goal_summary("Retry", Some("REPLAY1")).unwrap();
    work.append_goal_round_summary("REPLAY1", "Reporter", "Do work")
        .unwrap();
    work.cancel_goal_summary("REPLAY1").unwrap();
    let body = json!({"goal_id":"REPLAY1", "request_id":"same-request"});
    let mut invocation = service.trigger("retry", &fixture.0, &body).unwrap();
    assert_eq!(
        service.trigger("retry", &fixture.0, &body).unwrap().id,
        invocation.id
    );
    assert!(service.trigger("retry", &fixture.0, &json!({"goal_id":"REPLAY1","request_id":"same-request","parameters":{"reason":"different"}})).is_err());
    invocation.state = InvocationState::Succeeded;
    service.save_invocation(&invocation).unwrap();
    service.apply_success_action(&mut invocation).unwrap();
    assert_eq!(
        service.goal_invocations("REPLAY1", 0, 30).unwrap()["total"],
        1
    );
    assert_eq!(
        service.goal_invocations("OTHER", 0, 30).unwrap()["total"],
        0
    );
    let settled = work.show_goal_detail("REPLAY1").unwrap();
    assert_eq!(settled["status"], "todo");
    assert!(settled["event_actions"].get(&invocation.id).is_some());
    // Simulate a restart after the Goal receipt but before invocation settlement.
    invocation.action_applied = false;
    service.save_invocation(&invocation).unwrap();
    service.apply_success_action(&mut invocation).unwrap();
    assert!(invocation.action_applied);
    assert_eq!(
        work.show_goal_detail("REPLAY1").unwrap()["workflow_revision"],
        settled["workflow_revision"]
    );
}

#[test]
fn goal_and_system_paths_resolve_arrays_and_missing_shared_parameters_keep_requiredness() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let mut config = (*service.config().unwrap()).clone();
    let skill = config.skills.get_mut("default-plan").unwrap();
    skill.parameters = vec![
        Parameter {
            name: "request".into(),
            required: true,
            ..Default::default()
        },
        Parameter {
            name: "node".into(),
            required: true,
            ..Default::default()
        },
    ];
    let mut event = config.events["workflow.plan.enter"].clone();
    event.bindings[0].inputs = BTreeMap::from([
        ("request".into(), "goal.rounds.0.prompt".into()),
        ("node".into(), "system.node_id".into()),
    ]);
    let mut context = service.manual_context(&fixture.0, &json!({})).unwrap();
    context.data["goal"] = json!({"rounds":[{"prompt":"Current request"}]});
    assert!(
        service
            .launch_parameters(&config, &event, &context)
            .unwrap()
            .is_empty()
    );
    let invocation = service
        .prepare_pinned(&config, &event, context.clone(), BTreeMap::new(), "paths")
        .unwrap();
    assert_eq!(
        invocation.bindings[0].parameters["request"],
        "Current request"
    );
    assert_eq!(invocation.bindings[0].parameters["node"], context.node_id);
    event.bindings[0].inputs.clear();
    event.parameters.push(Parameter {
        name: "request".into(),
        required: false,
        ..Default::default()
    });
    assert!(
        service
            .launch_parameters(&config, &event, &context)
            .unwrap()
            .iter()
            .find(|p| p.name == "request")
            .unwrap()
            .required
    );
}

#[cfg(unix)]
#[test]
fn cancelling_a_running_event_terminates_its_managed_child_and_rejects_late_results() {
    use std::os::unix::fs::PermissionsExt;
    let _env = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new();
    let service = fixture.service();
    let provider = fixture.0.join("cancellable-provider");
    std::fs::write(&provider, r#"#!/usr/bin/env python3
import json,sys,pathlib,time
prompt=' '.join(sys.argv[1:])
result=json.JSONDecoder().raw_decode(prompt.split('Refine completion contract (supplied by the system):\n',1)[1])[0]
pathlib.Path('ready').write_text('ready')
time.sleep(3)
pathlib.Path('late-mutation').write_text('must not happen')
print(json.dumps(result))
"#).unwrap();
    std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o755)).unwrap();
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            unsafe {
                if let Some(v) = &self.0 {
                    std::env::set_var("REFINE_SMOKE_AI_PATH", v)
                } else {
                    std::env::remove_var("REFINE_SMOKE_AI_PATH")
                }
            }
        }
    }
    let _restore = Restore(std::env::var_os("REFINE_SMOKE_AI_PATH"));
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }
    let mut config = (*service.config().unwrap()).clone();
    config.skills.get_mut("default-plan").unwrap().role = "task".into();
    let event = config.events["workflow.plan.enter"].clone();
    let mut context = service.manual_context(&fixture.0, &json!({})).unwrap();
    context.provider = "smoke-ai".into();
    let invocation = service
        .prepare_pinned(&config, &event, context, BTreeMap::new(), "running-cancel")
        .unwrap();
    let worker_service = service.clone();
    let id = invocation.id.clone();
    let worker = std::thread::spawn(move || worker_service.execute(&id, || Ok(())));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !fixture.0.join("ready").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "provider did not launch"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    service.cancel_invocation(&invocation.id).unwrap();
    let _ = worker.join().unwrap();
    assert_eq!(
        service.invocation(&invocation.id).unwrap().state,
        InvocationState::Cancelled
    );
    assert!(!fixture.0.join("late-mutation").exists());
    let supervisor = crate::infrastructure::process::subprocess::FileProcessSupervisor::new(
        service.runtime().unwrap(),
    );
    assert!(supervisor.list().unwrap().iter().all(|p| {
        !crate::infrastructure::process::subprocess::FileProcessSupervisor::process_is_alive(p)
            .unwrap()
    }));
}

#[test]
fn interrupted_index_updates_recover_without_replaying_completed_work() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let config = service.config().unwrap();
    let mut event = config.events["node.startup.ready"].clone();
    event.bindings.clear();
    let context = service.manual_context(&fixture.0, &json!({})).unwrap();
    let mut invocation = service
        .prepare_pinned(&config, &event, context, BTreeMap::new(), "index-recovery")
        .unwrap();
    let journal = service
        .refine_dir
        .join("automation/index-updates/default")
        .join(format!("{}.json", invocation.id));
    let pending = service
        .refine_dir
        .join("automation/pending/default")
        .join(format!("{}.json", invocation.id));
    std::fs::remove_dir_all(service.refine_dir.join("automation/history")).unwrap();
    std::fs::remove_file(&pending).unwrap();
    crate::infrastructure::storage::automation::write_json(&journal, &json!({"id":invocation.id}))
        .unwrap();
    assert_eq!(service.invocations(0, 30).unwrap()["total"], 1);
    assert!(pending.exists());
    assert!(!journal.exists());
    // The canonical record is already terminal while its old pending index remains.
    invocation.state = InvocationState::Succeeded;
    crate::infrastructure::storage::automation::write_json(
        &service.invocation_path(&invocation.id).unwrap(),
        &invocation,
    )
    .unwrap();
    crate::infrastructure::storage::automation::write_json(&journal, &json!({"id":invocation.id}))
        .unwrap();
    service.repair_invocation_indexes(Some("default")).unwrap();
    assert!(!pending.exists());
    assert_eq!(
        service.invocations(0, 30).unwrap()["items"][0]["state"],
        "succeeded"
    );
}

#[test]
fn oversized_configuration_is_rejected_before_replacing_the_readable_document() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let original = service.config().unwrap();
    let error =
        crate::infrastructure::storage::automation::AutomationStore::new(&service.refine_dir)
            .update(original.revision, |config| {
                let mut skill = config.skills["default-plan"].clone();
                skill.prompt = "x".repeat(131_072);
                for index in 0..130 {
                    skill.id = format!("large-{index}");
                    config.skills.insert(skill.id.clone(), skill.clone());
                }
                Ok(())
            })
            .unwrap_err();
    assert!(error.to_string().contains("16 MiB"));
    assert_eq!(service.config().unwrap().as_ref(), original.as_ref());
}
