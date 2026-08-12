use super::*;
use std::collections::BTreeSet;
use std::fs;

#[derive(Default)]
struct FakeUpdateHost {
    running_ports: Vec<u16>,
    stopped_ports: BTreeSet<u16>,
    restarted_ports: Vec<u16>,
    agent_seen_all_stopped: bool,
    fail_agent: bool,
    agent_succeeded: bool,
    agent_output: String,
    fail_verify: bool,
    fail_restart: bool,
    invocations: Vec<UpdateAgentInvocation>,
    verify_stopped: bool,
    events: Vec<String>,
}

impl DeployedUpdateHost for FakeUpdateHost {
    fn running_ports(&mut self) -> RefineResult<Vec<u16>> {
        Ok(self.running_ports.clone())
    }

    fn stop_port(&mut self, port: u16) -> RefineResult<()> {
        self.events.push(format!("stop:{port}"));
        self.stopped_ports.insert(port);
        Ok(())
    }

    fn port_stopped(&mut self, port: u16) -> RefineResult<bool> {
        Ok(self.verify_stopped && self.stopped_ports.contains(&port))
    }

    fn run_update_agent(
        &mut self,
        invocation: &UpdateAgentInvocation,
    ) -> RefineResult<UpdateAgentOutcome> {
        self.agent_seen_all_stopped = self
            .running_ports
            .iter()
            .all(|port| self.stopped_ports.contains(port));
        self.invocations.push(invocation.clone());
        self.events.push("update_agent".to_string());
        if self.fail_agent {
            return Err(RefineError::Conflict("update agent failed".to_string()));
        }
        Ok(UpdateAgentOutcome {
            succeeded: self.agent_succeeded,
            target_version: Some("9.8.7".to_string()),
            binary_path: Some(invocation.cwd.join("bin/refine")),
            output: self.agent_output.clone(),
        })
    }

    fn verify_binary_mode(&mut self, _checkout: &Path, _binary_path: &Path) -> RefineResult<()> {
        if self.fail_verify {
            Err(RefineError::Conflict(
                "binary mode check failed".to_string(),
            ))
        } else {
            Ok(())
        }
    }

    fn restart_port(&mut self, port: u16) -> RefineResult<DaemonStatus> {
        if self.fail_restart {
            return Err(RefineError::Conflict("restart failed".to_string()));
        }
        self.events.push(format!("restart:{port}"));
        self.restarted_ports.push(port);
        Ok(DaemonStatus {
            port,
            daemon_healthy: true,
            web_available: true,
            worker_state: "idle".to_string(),
            target_app_state: "unknown".to_string(),
            launch_mode: "binary".to_string(),
            executable_path: Some("/tmp/refine/bin/refine".to_string()),
            active_operations: Vec::new(),
            degraded_integrations: Vec::new(),
            lifecycle_evidence: None,
        })
    }
}

#[test]
fn deployed_update_stops_ports_before_invoking_agent_and_restarts_them() {
    let mut host = FakeUpdateHost {
        running_ports: vec![8080, 9090],
        verify_stopped: true,
        agent_succeeded: true,
        ..Default::default()
    };
    let summary = run_deployed_update(
        &mut host,
        DeployedUpdateOptions::new("/tmp/refine", "/tmp/refine/run").with_assume_yes(true),
    );

    assert!(summary.ok);
    assert!(host.agent_seen_all_stopped);
    assert_eq!(summary.stopped_ports, vec![8080, 9090]);
    assert_eq!(summary.restarted_ports, vec![8080, 9090]);
    assert_eq!(
        host.events,
        vec![
            "stop:8080",
            "stop:9090",
            "update_agent",
            "restart:8080",
            "restart:9090"
        ]
    );
    assert_eq!(summary.target_version.as_deref(), Some("9.8.7"));
    assert_eq!(host.invocations.len(), 1);
    assert_eq!(host.invocations[0].provider, "claude");
    assert!(host.invocations[0].prompt.contains(UPDATE_RUNBOOK_PATH));
    assert!(
        host.invocations[0]
            .prompt
            .contains("Proceed with the documented defaults")
    );
    let agent = summary.update_agent.as_ref().unwrap();
    assert_eq!(agent.provider, "claude");
    assert!(agent.prompt.contains("/tmp/refine"));
}

#[test]
fn deployed_update_prompt_asks_questions_without_assume_yes() {
    let mut host = FakeUpdateHost {
        running_ports: Vec::new(),
        verify_stopped: true,
        agent_succeeded: true,
        ..Default::default()
    };
    let summary = run_deployed_update(
        &mut host,
        DeployedUpdateOptions::new("/tmp/refine", "/tmp/refine/run").with_provider("codex"),
    );

    assert!(summary.ok);
    assert_eq!(host.invocations.len(), 1);
    assert_eq!(host.invocations[0].provider, "codex");
    assert!(
        !host.invocations[0]
            .prompt
            .contains("Proceed with the documented defaults")
    );
    assert_eq!(summary.update_agent.as_ref().unwrap().provider, "codex");
}

#[test]
fn deployed_update_does_not_run_agent_until_ports_verify_stopped() {
    let mut host = FakeUpdateHost {
        running_ports: vec![8080],
        verify_stopped: false,
        agent_succeeded: true,
        ..Default::default()
    };
    let summary = run_deployed_update(
        &mut host,
        DeployedUpdateOptions::new("/tmp/refine", "/tmp/refine/run"),
    );

    assert!(!summary.ok);
    assert!(host.invocations.is_empty());
    assert_eq!(summary.failures[0].stage, "verify_stopped");
}

#[test]
fn deployed_update_does_not_restart_or_report_success_when_agent_fails() {
    let mut host = FakeUpdateHost {
        running_ports: vec![8080],
        fail_agent: true,
        verify_stopped: true,
        agent_succeeded: true,
        ..Default::default()
    };
    let summary = run_deployed_update(
        &mut host,
        DeployedUpdateOptions::new("/tmp/refine", "/tmp/refine/run"),
    );

    assert!(!summary.ok);
    assert_eq!(summary.stopped_ports, vec![8080]);
    assert!(summary.restarted_ports.is_empty());
    assert_eq!(summary.failures[0].stage, "update_agent");
    let recovery = summary.manual_recovery_command.as_deref().unwrap();
    assert!(recovery.contains("./r agent invoke"));
    assert!(recovery.contains("--provider claude"));
}

#[test]
fn deployed_update_reports_failed_agent_output() {
    let mut host = FakeUpdateHost {
        running_ports: vec![8080],
        verify_stopped: true,
        agent_succeeded: false,
        agent_output: "build failed".to_string(),
        ..Default::default()
    };
    let summary = run_deployed_update(
        &mut host,
        DeployedUpdateOptions::new("/tmp/refine", "/tmp/refine/run"),
    );

    assert!(!summary.ok);
    assert_eq!(summary.failures[0].stage, "update_agent");
    assert!(summary.failures[0].message.contains("build failed"));
    assert_eq!(
        summary.update_agent.as_ref().unwrap().output,
        "build failed"
    );
    assert!(summary.restarted_ports.is_empty());
}

#[test]
fn deployed_update_reports_after_agent_failures_without_false_success() {
    let mut host = FakeUpdateHost {
        running_ports: vec![8080],
        fail_verify: true,
        verify_stopped: true,
        agent_succeeded: true,
        ..Default::default()
    };
    let summary = run_deployed_update(
        &mut host,
        DeployedUpdateOptions::new("/tmp/refine", "/tmp/refine/run"),
    );

    assert!(!summary.ok);
    assert_eq!(summary.failures[0].stage, "verify_binary_mode");
    assert!(summary.rollback_possible);
    assert!(summary.restarted_ports.is_empty());
}

#[test]
fn deployed_update_reports_restart_failures_after_binary_replacement() {
    let mut host = FakeUpdateHost {
        running_ports: vec![8080],
        fail_restart: true,
        verify_stopped: true,
        agent_succeeded: true,
        ..Default::default()
    };
    let summary = run_deployed_update(
        &mut host,
        DeployedUpdateOptions::new("/tmp/refine", "/tmp/refine/run"),
    );

    assert!(!summary.ok);
    assert_eq!(summary.failures[0].stage, "restart_ports");
    assert!(summary.rollback_possible);
    assert!(summary.restarted_ports.is_empty());
}

#[test]
fn refine_checkout_detection_requires_the_source_entrypoints() {
    let root = std::env::temp_dir().join(format!(
        "refine-checkout-detection-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("docs/runbooks")).unwrap();
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"refine\"\n").unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(root.join("docs/runbooks/install.md"), "# Install Refine\n").unwrap();

    assert!(!is_refine_checkout(&root));
    fs::write(root.join("r"), "#!/bin/sh\n").unwrap();
    assert!(is_refine_checkout(&root));

    fs::remove_dir_all(root).unwrap();
}
