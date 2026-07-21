use std::path::{Path, PathBuf};

use crate::process::supervisor::config::FileSettingsService;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::runtime::RuntimeRoot;
use crate::tools::host::agent_providers::{AgentProviderService, HostAgentProviderService};
use crate::tools::host::project_layout::prepare_refine_dir;
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::project_registry::FileProjectRegistryService;

/// Inputs for `refine node init`. Flags win over the environment; the
/// environment is how a provisioned worker receives its identity
/// (`REFINE_NODE_ID`), its work source (`REFINE_TARGET_REPO_URL`), and its
/// agent posture (`REFINE_AGENT_PROVIDERS`) — injected as provider secrets at
/// provision time.
#[derive(Clone, Debug, Default)]
pub struct WorkerInitOptions {
    pub node_id: Option<String>,
    pub repo_url: Option<String>,
    pub target_path: Option<PathBuf>,
    pub agent_providers: Option<String>,
    pub runtime_root: PathBuf,
    pub port: u16,
}

/// Turns this machine into a working fleet node: clone/refresh the target
/// repo, attach it, take on the node identity so this daemon can claim the
/// Goals distribute assigns to it, select the agent provider, and verify the
/// provider binary is present. Idempotent — safe to run on every boot.
pub fn initialize_worker(options: WorkerInitOptions) -> RefineResult<serde_json::Value> {
    let node_id = resolve(options.node_id, "REFINE_NODE_ID").unwrap_or_else(|| "default".into());
    let repo_url = resolve(options.repo_url, "REFINE_TARGET_REPO_URL");
    let agent_providers = resolve(options.agent_providers, "REFINE_AGENT_PROVIDERS");
    let target_path = options
        .target_path
        .or_else(|| std::env::var_os("REFINE_TARGET_PATH").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("target-app"));
    let port_root = RuntimeRoot {
        root: options.runtime_root.clone(),
    }
    .port_root(options.port);

    let mut steps = Vec::new();
    let mut ok = true;

    let repo_ready = ensure_target_repo(&target_path, repo_url.as_deref(), &mut steps);
    if !repo_ready {
        // Without a target repo there is nothing to attach or own; report
        // what happened instead of half-initializing.
        return Ok(summary(false, &node_id, &target_path, steps));
    }

    match FileProjectRegistryService::new(&options.runtime_root, None)
        .attach_with_migration(&target_path.display().to_string())
    {
        Ok(_) => record(&mut steps, "attach_project", true, "attached"),
        Err(error) => {
            ok = false;
            record(&mut steps, "attach_project", false, &error.to_string());
        }
    }

    let refine_dir = prepare_refine_dir(&target_path)?;
    let nodes = FileNodeRegistryService::new(&refine_dir);
    match nodes.create(&node_id) {
        Ok(_) => record(&mut steps, "ensure_node", true, "created"),
        Err(RefineError::Conflict(_)) => record(&mut steps, "ensure_node", true, "exists"),
        Err(error) => {
            ok = false;
            record(&mut steps, "ensure_node", false, &error.to_string());
        }
    }
    match FileNodeRegistryService::with_active_root(&refine_dir, &port_root).activate(&node_id) {
        Ok(_) => record(&mut steps, "activate_node", true, "active"),
        Err(error) => {
            ok = false;
            record(&mut steps, "activate_node", false, &error.to_string());
        }
    }

    if let Some(providers) = &agent_providers {
        let primary = providers
            .split(',')
            .map(str::trim)
            .find(|value| !value.is_empty());
        if let Some(provider) = primary {
            match FileSettingsService::with_active_root(&refine_dir, &port_root)
                .update(&serde_json::json!({ "agent_cli": provider }))
            {
                Ok(_) => record(&mut steps, "select_agent_provider", true, provider),
                Err(error) => {
                    ok = false;
                    record(
                        &mut steps,
                        "select_agent_provider",
                        false,
                        &error.to_string(),
                    );
                }
            }
            match HostAgentProviderService::new().detect() {
                Ok(capabilities) => {
                    let installed = capabilities
                        .iter()
                        .any(|capability| capability.name == provider && capability.installed);
                    record(
                        &mut steps,
                        "verify_agent_binary",
                        installed,
                        if installed {
                            "installed"
                        } else {
                            "provider binary not found on PATH"
                        },
                    );
                    ok &= installed;
                }
                Err(error) => {
                    ok = false;
                    record(&mut steps, "verify_agent_binary", false, &error.to_string());
                }
            }
        }
    } else {
        record(
            &mut steps,
            "select_agent_provider",
            true,
            "skipped: REFINE_AGENT_PROVIDERS not set",
        );
    }

    Ok(summary(ok, &node_id, &target_path, steps))
}

fn ensure_target_repo(
    target_path: &Path,
    repo_url: Option<&str>,
    steps: &mut Vec<serde_json::Value>,
) -> bool {
    if target_path.join(".git").exists() {
        let pulled = run_git(&[
            "-C",
            &target_path.display().to_string(),
            "pull",
            "--ff-only",
        ]);
        match pulled {
            Ok(output) => record(steps, "refresh_target_repo", true, output.trim()),
            // A failed refresh is not fatal: the checkout still exists and the
            // daemon can sync later through /project/sync.
            Err(error) => record(
                steps,
                "refresh_target_repo",
                true,
                &format!("skipped: {error}"),
            ),
        }
        return true;
    }
    let Some(repo_url) = repo_url.filter(|url| !url.trim().is_empty()) else {
        record(
            steps,
            "clone_target_repo",
            false,
            "no repository at target path and REFINE_TARGET_REPO_URL is not set",
        );
        return false;
    };
    match run_git(&["clone", repo_url, &target_path.display().to_string()]) {
        Ok(_) => {
            record(steps, "clone_target_repo", true, repo_url);
            true
        }
        Err(error) => {
            record(steps, "clone_target_repo", false, &error.to_string());
            false
        }
    }
}

fn run_git(args: &[&str]) -> RefineResult<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| RefineError::Io(format!("failed to run git: {error}")))?;
    if !output.status.success() {
        return Err(RefineError::Io(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn resolve(flag: Option<String>, env_name: &str) -> Option<String> {
    flag.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::env::var(env_name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
}

fn record(steps: &mut Vec<serde_json::Value>, step: &str, ok: bool, detail: &str) {
    steps.push(serde_json::json!({
        "step": step,
        "ok": ok,
        "detail": detail
    }));
}

fn summary(
    ok: bool,
    node_id: &str,
    target_path: &Path,
    steps: Vec<serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "ok": ok,
        "node_id": node_id,
        "target_path": target_path.display().to_string(),
        "steps": steps
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }

    fn init_git_repo(path: &Path) {
        fs::create_dir_all(path).unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        ] {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(&args)
                .status()
                .unwrap();
            assert!(status.success());
        }
    }

    #[test]
    fn worker_init_attaches_existing_repo_and_activates_node_identity() {
        let temp_root = unique_temp_dir("worker-init");
        let target = temp_root.join("target-app");
        init_git_repo(&target);
        let runtime_root = temp_root.join("run");

        let report = initialize_worker(WorkerInitOptions {
            node_id: Some("fly-worker-1".to_string()),
            repo_url: None,
            target_path: Some(target.clone()),
            agent_providers: None,
            runtime_root: runtime_root.clone(),
            port: 8080,
        })
        .unwrap();

        assert_eq!(report["ok"], true, "report: {report}");
        let refine_dir =
            crate::tools::host::project_layout::refine_dir_for_target_root(&target).unwrap();
        let nodes = fs::read_to_string(refine_dir.join("nodes.json")).unwrap();
        assert!(nodes.contains("fly-worker-1"));
        let active =
            fs::read_to_string(runtime_root.join("8080").join("active-node.json")).unwrap();
        assert!(active.contains("fly-worker-1"));
        // idempotent second run
        let second = initialize_worker(WorkerInitOptions {
            node_id: Some("fly-worker-1".to_string()),
            repo_url: None,
            target_path: Some(target.clone()),
            agent_providers: None,
            runtime_root: runtime_root.clone(),
            port: 8080,
        })
        .unwrap();
        assert_eq!(second["ok"], true, "second report: {second}");
        assert!(!target.join(".refine").exists());
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn worker_init_without_repo_or_url_reports_failure() {
        let temp_root = unique_temp_dir("worker-init-norepo");
        let report = initialize_worker(WorkerInitOptions {
            node_id: Some("w".to_string()),
            repo_url: None,
            target_path: Some(temp_root.join("missing")),
            agent_providers: None,
            runtime_root: temp_root.join("run"),
            port: 8080,
        })
        .unwrap();
        assert_eq!(report["ok"], false);
        assert_eq!(report["steps"][0]["step"], "clone_target_repo");
        fs::remove_dir_all(&temp_root).ok();
    }
}
