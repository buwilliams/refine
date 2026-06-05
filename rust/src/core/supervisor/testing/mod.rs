use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::core::host::agent_providers::{ProviderInvocation, ProviderInvocationResult};
use crate::core::host::process_supervision::{
    ManagedProcess, ManagedProcessSpec, ProcessOwner, ProcessResourceLimits,
};
use crate::core::supervisor::errors::{RefineError, RefineResult};

#[derive(Clone, Debug)]
pub struct TestRuntimeFixture {
    pub root: PathBuf,
    pub runtime_root: PathBuf,
    pub durable_root: PathBuf,
}

impl TestRuntimeFixture {
    pub fn new(name: &str) -> RefineResult<Self> {
        let root = unique_temp_dir(name);
        let runtime_root = root.join("run").join("8080");
        let durable_root = root.join("app").join(".refine");
        fs::create_dir_all(&runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create test runtime root {}: {error}",
                runtime_root.display()
            ))
        })?;
        fs::create_dir_all(&durable_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create test durable root {}: {error}",
                durable_root.display()
            ))
        })?;
        Ok(Self {
            root,
            runtime_root,
            durable_root,
        })
    }

    pub fn cleanup(&self) -> RefineResult<()> {
        if self.root.exists() {
            fs::remove_dir_all(&self.root).map_err(|error| {
                RefineError::Io(format!(
                    "failed to remove test fixture {}: {error}",
                    self.root.display()
                ))
            })?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct FakeProcessSupervisor {
    launched: Vec<ManagedProcessSpec>,
    signalled: Vec<(String, String)>,
}

impl FakeProcessSupervisor {
    pub fn launch(&mut self, spec: ManagedProcessSpec) -> ManagedProcess {
        let id = format!("fake-process-{}", self.launched.len() + 1);
        let process = ManagedProcess {
            id,
            owner: spec.owner.clone(),
            pid: Some(10_000 + self.launched.len() as u32),
            state: "running".to_string(),
            label: Some(spec.command.clone()),
            details: Some(spec.args.join(" ")),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: spec.limits.clone(),
            started_at: "2026-01-01T00:00:00Z".to_string(),
            exit_code: None,
        };
        self.launched.push(spec);
        process
    }

    pub fn signal(&mut self, process_id: &str, signal: &str) {
        self.signalled
            .push((process_id.to_string(), signal.to_string()));
    }

    pub fn launched(&self) -> &[ManagedProcessSpec] {
        &self.launched
    }

    pub fn signalled(&self) -> &[(String, String)] {
        &self.signalled
    }
}

#[derive(Clone, Debug)]
pub struct FakeProvider {
    pub provider: String,
    pub outputs: Vec<String>,
    pub exit_code: i32,
}

impl FakeProvider {
    pub fn new(provider: &str) -> Self {
        Self {
            provider: provider.to_string(),
            outputs: Vec::new(),
            exit_code: 0,
        }
    }

    pub fn with_output(mut self, text: &str) -> Self {
        self.outputs.push(text.to_string());
        self
    }

    pub fn invoke(&self, invocation: ProviderInvocation) -> ProviderInvocationResult {
        let output = self.outputs.join("\n");
        ProviderInvocationResult {
            output,
            provider_session_id: invocation.session_id,
            raw_output: self.outputs.join("\n"),
        }
    }
}

pub fn assert_json_contract(value: &Value, required_fields: &[&str]) -> RefineResult<()> {
    let Some(object) = value.as_object() else {
        return Err(RefineError::Serialization(
            "contract value is not a JSON object".to_string(),
        ));
    };
    for field in required_fields {
        if !object.contains_key(*field) {
            return Err(RefineError::Serialization(format!(
                "contract value is missing required field {field}"
            )));
        }
    }
    Ok(())
}

pub fn process_spec(command: &str, args: &[&str]) -> ManagedProcessSpec {
    ManagedProcessSpec {
        owner: ProcessOwner::UserHelper,
        command: command.to_string(),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        cwd: None,
        env: Vec::new(),
        stdin: None,
        limits: Some(ProcessResourceLimits::default()),
    }
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "refine-native-test-{prefix}-{}-{nanos}",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn testing_fixture_creates_isolated_roots_and_contract_helpers() {
        let fixture = TestRuntimeFixture::new("testing-fixture").unwrap();
        assert!(fixture.runtime_root.exists());
        assert!(fixture.durable_root.exists());
        assert_json_contract(&serde_json::json!({"ok": true, "id": "one"}), &["ok", "id"]).unwrap();
        assert!(assert_json_contract(&serde_json::json!({"ok": true}), &["id"]).is_err());
        fixture.cleanup().unwrap();
    }

    #[test]
    fn fake_process_supervisor_and_provider_record_behavior() {
        let mut supervisor = FakeProcessSupervisor::default();
        let process = supervisor.launch(process_spec("printf", &["hello"]));
        supervisor.signal(&process.id, "terminate");
        assert_eq!(supervisor.launched().len(), 1);
        assert_eq!(supervisor.signalled()[0].1, "terminate");

        let provider = FakeProvider::new("fake-ai").with_output("done");
        let result = provider.invoke(ProviderInvocation {
            provider: "fake-ai".to_string(),
            prompt: "ship it".to_string(),
            cwd: None,
            session_id: None,
        });
        assert_eq!(result.output, "done");
    }
}
