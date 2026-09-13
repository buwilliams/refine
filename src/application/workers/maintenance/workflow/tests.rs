use super::*;
use crate::application::work_items::WorkflowControl;
use crate::model::workflow::GoalStatus;

struct Fixture {
    root: PathBuf,
    runtime: PathBuf,
    items: FileWorkItemService,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("refine-workflow-cleanup-{}", uuid::Uuid::new_v4()));
        let runtime = root.join("runtime");
        let items = FileWorkItemService::new(root.join(".refine"));
        items
            .create_goal_summary("Cleanup", Some("CLEANUP"))
            .unwrap();
        items
            .append_goal_round_summary("CLEANUP", "operator", "Implement the request")
            .unwrap();
        items.start_goal_workflow("CLEANUP").unwrap();
        Self {
            root,
            runtime,
            items,
        }
    }

    fn launch(&self, agents: bool, extra: Value) -> ManagedProcess {
        let goal = self.items.show_goal_detail("CLEANUP").unwrap();
        let mut metadata = json!({"goal_id":"CLEANUP", "target_app_id":self.root,
            "workflow_step_generation":goal["event_generation"], "workflow_revision":goal["workflow_revision"],
            "isolated_process_group":true,"agent_hard_cap_millis":600_000});
        metadata
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        FileProcessSupervisor::new(if agents {
            self.runtime.join("agents")
        } else {
            self.runtime.clone()
        })
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Agent,
            command: "/bin/sleep".into(),
            args: vec!["60".into()],
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: serde_json::from_value(metadata).unwrap(),
        })
        .unwrap()
    }

    fn assign(&self, to: GoalStatus) {
        self.items
            .control_workflow(
                "CLEANUP",
                &WorkflowControl {
                    to,
                    reason: "Select workflow step".into(),
                    context: String::new(),
                    expected_revision:
                        self.items.show_goal_detail("CLEANUP").unwrap()["workflow_revision"]
                            .as_u64()
                            .unwrap(),
                    request_id: uuid::Uuid::new_v4().to_string(),
                    actor: "operator".into(),
                    force: true,
                    invocation_id: None,
                },
            )
            .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for root in [self.runtime.clone(), self.runtime.join("agents")] {
            let supervisor = FileProcessSupervisor::new(root);
            for process in supervisor.list().unwrap_or_default() {
                let _ = supervisor.terminate_owned_and_confirm_exit(
                    &process,
                    "kill",
                    Duration::from_secs(2),
                );
            }
            for group in supervisor.owned_groups().unwrap_or_default() {
                let _ = supervisor.stop_owned_group(&group, Duration::from_secs(2));
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn durable_decision_precedes_restart_cleanup_and_preserves_replacement_agents() {
    let f = Fixture::new();
    let old = f.launch(true, json!({"toolbar_timeout_protected":true}));
    let submitting_skill = f.launch(
        false,
        json!({"event_invocation_id":"submitting-skill", "agent_hard_cap_millis":null}),
    );
    let before = f.items.show_goal_detail("CLEANUP").unwrap();
    f.assign(GoalStatus::Todo);
    assert_eq!(f.items.show_goal_detail("CLEANUP").unwrap(), before);
    assert_eq!(maintain_daemon(&f.runtime).superseded_groups, 0);
    assert!(FileProcessSupervisor::process_is_alive(&old).unwrap());

    f.assign(GoalStatus::Cancelled);
    let recorded = f.items.show_goal_detail("CLEANUP").unwrap();
    assert_eq!(recorded["status"], "cancelled");
    assert!(FileProcessSupervisor::process_is_alive(&old).unwrap());
    assert!(FileProcessSupervisor::process_is_alive(&submitting_skill).unwrap());
    // A fresh maintenance instance only needs durable Goal and process evidence;
    // no continuation from the request thread or active workflow runner exists.
    let replacement = f.launch(true, json!({"toolbar_timeout_protected":true}));
    let health = maintain_daemon(&f.runtime);
    assert!(health.failures.is_empty(), "{:?}", health.failures);
    assert_eq!(health.superseded_groups, 2);
    assert!(!FileProcessSupervisor::process_is_alive(&old).unwrap());
    assert!(!FileProcessSupervisor::process_is_alive(&submitting_skill).unwrap());
    assert!(FileProcessSupervisor::process_is_alive(&replacement).unwrap());
    assert_eq!(f.items.show_goal_detail("CLEANUP").unwrap(), recorded);
    assert_eq!(maintain_daemon(&f.runtime).superseded_groups, 0);
}

#[test]
fn normal_phase_advances_do_not_stop_the_enclosing_execution() {
    let f = Fixture::new();
    let process = f.launch(false, json!({}));
    f.items
        .advance_automated_goal_status("CLEANUP", GoalStatus::Plan)
        .unwrap();
    f.items
        .advance_automated_goal_status("CLEANUP", GoalStatus::Implement)
        .unwrap();
    let health = maintain_daemon(&f.runtime);
    assert!(health.failures.is_empty(), "{:?}", health.failures);
    assert_eq!(health.superseded_groups, 0);
    assert!(FileProcessSupervisor::process_is_alive(&process).unwrap());
    f.items.cancel_goal_summary("CLEANUP").unwrap();
    let health = maintain_daemon(&f.runtime);
    assert!(health.failures.is_empty(), "{:?}", health.failures);
    assert_eq!(health.superseded_groups, 1);
}

#[test]
fn supersession_stops_preparation_but_a_started_publication_finishes() {
    let f = Fixture::new();
    let preparing = f.launch(false, json!({"kind":"git", "git_command":"fetch"}));
    let publishing = f.launch(
        false,
        json!({"kind":"git", "git_command":"push", "side_effect_committed":true}),
    );
    f.assign(GoalStatus::Cancelled);
    let health = maintain_daemon(&f.runtime);
    assert!(health.failures.is_empty(), "{:?}", health.failures);
    assert_eq!(health.superseded_groups, 1);
    assert!(!FileProcessSupervisor::process_is_alive(&preparing).unwrap());
    assert!(FileProcessSupervisor::process_is_alive(&publishing).unwrap());
}

#[test]
fn interrupted_cleanup_retries_and_a_newer_decision_does_not_stop_new_execution() {
    let f = Fixture::new();
    let old = f.launch(false, json!({}));
    f.items.cancel_goal_summary("CLEANUP").unwrap();
    let group_path = f
        .runtime
        .join("owned-groups")
        .join(format!("{}.json", old.id));
    let group = std::fs::read(&group_path).unwrap();
    std::fs::write(&group_path, "{interrupted").unwrap();
    let health = maintain_daemon(&f.runtime);
    assert!(!health.failures.is_empty());
    assert_eq!(
        f.items.show_goal_detail("CLEANUP").unwrap()["status"],
        "cancelled"
    );
    assert!(FileProcessSupervisor::process_is_alive(&old).unwrap());
    f.items.undo_goal_summary("CLEANUP").unwrap();
    let current = f.launch(false, json!({}));
    std::fs::write(&group_path, group).unwrap();
    let health = maintain_daemon(&f.runtime);
    assert!(health.failures.is_empty(), "{:?}", health.failures);
    assert_eq!(health.superseded_groups, 1);
    assert!(!FileProcessSupervisor::process_is_alive(&old).unwrap());
    assert!(FileProcessSupervisor::process_is_alive(&current).unwrap());
}
