use super::*;
use std::os::unix::fs::PermissionsExt;

const CHILD_TEST: &str = "infrastructure::agents::invocation::service::detached_credentials_tests::detached_launch_child";

// Invoked in a separate libtest process so its exit genuinely removes the launcher.
#[test]
fn detached_launch_child() {
    let Some(root) = std::env::var_os("REFINE_PROVIDER_DETACHED_TEST_ROOT").map(PathBuf::from)
    else {
        return;
    };
    let service = HostAgentProviderService::with_runtime_root(root.join("run/8082"))
        .with_refine_dir(root.join("state"));
    let process = service
        .launch_managed(ProviderInvocation {
            provider: "detached-test".into(),
            prompt: String::new(),
            session_id: None,
            cwd: Some(root.display().to_string()),
            stall_timeout_seconds: None,
            process_metadata: Default::default(),
        })
        .unwrap();
    std::fs::write(
        root.join("receipt.json"),
        serde_json::to_vec(&process).unwrap(),
    )
    .unwrap();
}

#[test]
fn detached_credentials_remain_redacted_after_the_launching_process_exits() {
    let root = std::env::temp_dir().join(format!(
        "refine-detached-credentials-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let executable = root.join("provider");
    std::fs::write(&executable, "#!/usr/bin/python3\nimport os,time\ntime.sleep(.3)\nprint(os.environ['OPENAI_API_KEY'],flush=True)\nprint('finished after launcher exit',flush=True)\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut catalog = crate::model::providers::defaults();
    let mut provider = crate::model::providers::ProviderDefinition::generic("detached-test");
    provider.executable = executable.display().to_string();
    provider
        .credentials
        .insert("OPENAI_API_KEY".into(), "DETACHED_TEST_CREDENTIAL".into());
    catalog.providers.push(provider);
    crate::application::agents::providers::save(&root.join("state"), &serde_json::json!(catalog))
        .unwrap();
    let secret = "detached-credential-never-persist";
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env("REFINE_PROVIDER_DETACHED_TEST_ROOT", &root)
        .env("DETACHED_TEST_CREDENTIAL", secret)
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}",
        String::from_utf8_lossy(&child.stderr)
    );
    let process: crate::infrastructure::process::subprocess::ManagedProcess =
        serde_json::from_slice(&std::fs::read(root.join("receipt.json")).unwrap()).unwrap();
    let path = process.stdout_path.as_ref().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let output = loop {
        let output = std::fs::read_to_string(path).unwrap_or_default();
        assert!(!output.contains(secret));
        if output.contains("finished after launcher exit") {
            break output;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "detached provider did not finish: {output}"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert!(output.contains("[REDACTED]"));
    let supervisor = FileProcessSupervisor::new(root.join("run/8082"));
    supervisor.wait(&process.id).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
