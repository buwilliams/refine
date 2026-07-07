use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use crate::model::JsonObject;
use crate::model::fleet::{
    CURRENT_FLEET_SCHEMA_VERSION, FleetCommandStep, FleetConfig, FleetOperation,
    FleetProviderConfig, FleetStepResult, render_argv, render_template, valid_provider_name,
};
use crate::model::node::{Node, NodeHealth};
use crate::process::subprocess::{FileProcessSupervisor, ManagedProcessSpec, ProcessOwner};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::security::FileSecurityService;
use crate::tools::host::deployed_update::discover_refine_checkout;
use crate::tools::product::nodes::FileNodeRegistryService;

pub const FLEET_CONFIG_FILE: &str = "fleet.json";

/// Provisions and deprovisions fleet nodes on cloud providers. Behavior is
/// data-driven: providers are argv command templates resolved from
/// `.refine/fleet.json` layered over built-in defaults, so a released binary
/// can provision workers built from a newer Refine ref without code changes.
#[derive(Clone, Debug)]
pub struct FileFleetService {
    pub refine_dir: PathBuf,
    pub runtime_root: Option<PathBuf>,
}

impl FileFleetService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: None,
        }
    }

    pub fn with_runtime_root(
        refine_dir: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: Some(runtime_root.into()),
        }
    }

    pub fn config_path(&self) -> PathBuf {
        self.refine_dir.join(FLEET_CONFIG_FILE)
    }

    pub fn load_config(&self) -> RefineResult<FleetConfig> {
        let mut config = self.load_configured()?.unwrap_or_default();
        for (name, provider) in builtin_providers() {
            config.providers.entry(name).or_insert(provider);
        }
        for (name, provider) in &config.providers {
            if !valid_provider_name(name) {
                return Err(RefineError::InvalidInput(format!(
                    "fleet provider name {name} must be lowercase alphanumeric, underscore, or hyphen"
                )));
            }
            if provider.binary.trim().is_empty() {
                return Err(RefineError::InvalidInput(format!(
                    "fleet provider {name} must declare a binary"
                )));
            }
        }
        Ok(config)
    }

    fn load_configured(&self) -> RefineResult<Option<FleetConfig>> {
        let path = self.config_path();
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read fleet config {}: {error}",
                path.display()
            ))
        })?;
        let config = serde_json::from_slice::<FleetConfig>(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse fleet config {}: {error}",
                path.display()
            ))
        })?;
        if config.schema_version > CURRENT_FLEET_SCHEMA_VERSION {
            return Err(RefineError::Conflict(format!(
                "fleet config {} uses schema_version {} but this Refine build supports up to {}; update Refine on this node before provisioning",
                path.display(),
                config.schema_version,
                CURRENT_FLEET_SCHEMA_VERSION
            )));
        }
        Ok(Some(config))
    }

    pub fn providers_response(&self) -> RefineResult<serde_json::Value> {
        let configured = self.load_configured()?;
        let configured_names: Vec<String> = configured
            .as_ref()
            .map(|config| config.providers.keys().cloned().collect())
            .unwrap_or_default();
        let config = self.load_config()?;
        let providers: Vec<serde_json::Value> = config
            .providers
            .iter()
            .map(|(name, provider)| {
                serde_json::json!({
                    "name": name,
                    "display_name": if provider.display_name.is_empty() {
                        name.clone()
                    } else {
                        provider.display_name.clone()
                    },
                    "binary": provider.binary,
                    "credential_env": provider.credential_env,
                    "require_credentials": provider.require_credentials,
                    "defaults": provider.defaults,
                    "source": if configured_names.contains(name) { "config" } else { "builtin" }
                })
            })
            .collect();
        Ok(serde_json::json!({
            "default_provider": config.default_provider,
            "config_path": self.config_path().display().to_string(),
            "config_present": configured.is_some(),
            "providers": providers
        }))
    }

    pub fn provision_response(
        &self,
        node_id: &str,
        provider_override: Option<&str>,
        dry_run: bool,
    ) -> RefineResult<serde_json::Value> {
        self.run_operation(
            FleetOperation::Provision,
            node_id,
            provider_override,
            dry_run,
        )
    }

    pub fn deprovision_response(
        &self,
        node_id: &str,
        dry_run: bool,
    ) -> RefineResult<serde_json::Value> {
        self.run_operation(FleetOperation::Deprovision, node_id, None, dry_run)
    }

    pub fn provision_status_response(&self, node_id: &str) -> RefineResult<serde_json::Value> {
        self.run_operation(FleetOperation::Status, node_id, None, false)
    }

    fn run_operation(
        &self,
        operation: FleetOperation,
        node_id: &str,
        provider_override: Option<&str>,
        dry_run: bool,
    ) -> RefineResult<serde_json::Value> {
        let nodes = FileNodeRegistryService::new(&self.refine_dir);
        let mut registry = nodes.load_registry()?;
        let Some(index) = registry
            .nodes
            .iter()
            .position(|node| node.id == node_id && !node.archived)
        else {
            return Err(RefineError::NotFound(format!(
                "node {node_id} was not found"
            )));
        };
        let config = self.load_config()?;
        let provider_name =
            resolve_provider_name(&config, &registry.nodes[index], provider_override)?;
        let Some(provider) = config.providers.get(&provider_name) else {
            return Err(RefineError::NotFound(format!(
                "fleet provider {provider_name} is not defined; add it to {}",
                self.config_path().display()
            )));
        };
        let steps = operation_steps(provider, operation);
        if steps.is_empty() {
            return Err(RefineError::InvalidInput(format!(
                "fleet provider {provider_name} does not define a {} command",
                operation.as_str()
            )));
        }
        let placeholders = placeholder_values(&provider_name, provider, &registry.nodes[index])?;
        let credential_env = credential_environment(provider, &registry.nodes[index], dry_run)?;
        let results = self.run_steps(steps, &placeholders, &credential_env, dry_run)?;
        let ok = results
            .iter()
            .all(|result| result.ok || result.allow_failure);

        if !dry_run {
            apply_operation_outcome(
                &mut registry.nodes[index],
                operation,
                &provider_name,
                ok,
                &results,
            );
            nodes.save_registry(&registry)?;
        }
        Ok(serde_json::json!({
            "ok": ok,
            "node_id": node_id,
            "provider": provider_name,
            "operation": operation.as_str(),
            "dry_run": dry_run,
            "steps": results,
            "node": registry.nodes[index]
        }))
    }

    fn run_steps(
        &self,
        steps: &[FleetCommandStep],
        placeholders: &BTreeMap<String, String>,
        credential_env: &[(String, String)],
        dry_run: bool,
    ) -> RefineResult<Vec<FleetStepResult>> {
        let security = self.security()?;
        let mut results = Vec::new();
        let mut failed = false;
        for step in steps {
            let argv = render_argv(&step.argv, placeholders).map_err(RefineError::InvalidInput)?;
            if argv.is_empty() {
                return Err(RefineError::InvalidInput(
                    "fleet command step has an empty argv".to_string(),
                ));
            }
            let display = argv.join(" ");
            if dry_run || failed {
                results.push(FleetStepResult {
                    command: display,
                    exit_code: None,
                    stdout: String::new(),
                    stderr: String::new(),
                    ok: dry_run,
                    allow_failure: step.allow_failure,
                    executed: false,
                });
                continue;
            }
            security.authorize_host_command("fleet", &display)?;
            let output = FileProcessSupervisor::with_allowed_commands(
                security.runtime_root.clone(),
                security.allowed_commands.iter().cloned(),
            )
            .run_to_completion(ManagedProcessSpec {
                owner: ProcessOwner::Maintenance,
                command: argv[0].clone(),
                args: argv[1..].to_vec(),
                cwd: None,
                env: credential_env.to_vec(),
                stdin: None,
                limits: None,
                authorization_command: Some(display.clone()),
                sensitive: false,
                metadata: Default::default(),
            })?;
            let ok = output.success();
            results.push(FleetStepResult {
                command: display,
                exit_code: output.process.exit_code,
                stdout: output.stdout.trim().to_string(),
                stderr: output.stderr.trim().to_string(),
                ok,
                allow_failure: step.allow_failure,
                executed: true,
            });
            if !ok && !step.allow_failure {
                failed = true;
            }
        }
        Ok(results)
    }

    fn security(&self) -> RefineResult<FileSecurityService> {
        let runtime_root = self
            .runtime_root
            .clone()
            .unwrap_or_else(|| self.refine_dir.join("runtime"));
        FileSecurityService::from_project_settings(runtime_root, &self.refine_dir)
    }
}

fn resolve_provider_name(
    config: &FleetConfig,
    node: &Node,
    provider_override: Option<&str>,
) -> RefineResult<String> {
    let name = provider_override
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| {
            let provider = node.provider.trim();
            (!provider.is_empty()).then(|| provider.to_string())
        })
        .unwrap_or_else(|| config.default_provider.clone());
    if !valid_provider_name(&name) {
        return Err(RefineError::InvalidInput(format!(
            "invalid fleet provider name {name}"
        )));
    }
    Ok(name)
}

fn operation_steps(
    provider: &FleetProviderConfig,
    operation: FleetOperation,
) -> &[FleetCommandStep] {
    match operation {
        FleetOperation::Provision => &provider.provision,
        FleetOperation::Deprovision => &provider.deprovision,
        FleetOperation::Status => &provider.status,
    }
}

/// Builds the placeholder map: computed values (node identity, checkout
/// assets), then provider defaults, then the node's provisioning overrides.
/// Default and override values may themselves reference earlier placeholders.
fn placeholder_values(
    provider_name: &str,
    provider: &FleetProviderConfig,
    node: &Node,
) -> RefineResult<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();
    values.insert("node_id".to_string(), node.id.clone());
    values.insert("refine_port".to_string(), node.refine_port.to_string());
    values.insert("binary".to_string(), provider.binary.clone());
    if let Ok(checkout) = discover_refine_checkout() {
        values.insert(
            "fleet_dir".to_string(),
            checkout
                .join("scripts/fleet")
                .join(provider_name)
                .display()
                .to_string(),
        );
    }
    merge_placeholder_object(&mut values, &provider.defaults)?;
    merge_placeholder_object(&mut values, &node.provisioning)?;
    if !values.contains_key("app_name") {
        values.insert("app_name".to_string(), format!("refine-{}", node.id));
    }
    Ok(values)
}

fn merge_placeholder_object(
    values: &mut BTreeMap<String, String>,
    object: &JsonObject,
) -> RefineResult<()> {
    for (key, value) in object {
        let text = match value {
            serde_json::Value::String(text) => text.clone(),
            serde_json::Value::Number(number) => number.to_string(),
            serde_json::Value::Bool(flag) => flag.to_string(),
            _ => continue,
        };
        let rendered = render_template(&text, values).map_err(RefineError::InvalidInput)?;
        values.insert(key.clone(), rendered);
    }
    Ok(())
}

/// Credential posture per the fleet intent: secrets come from the invoking
/// environment (a credential source the fleet trusts) and are passed through
/// to the provider process without ever being written to shared state.
fn credential_environment(
    provider: &FleetProviderConfig,
    node: &Node,
    dry_run: bool,
) -> RefineResult<Vec<(String, String)>> {
    let mut env = Vec::new();
    for name in &provider.credential_env {
        if let Ok(value) = std::env::var(name) {
            if !value.is_empty() {
                env.push((name.clone(), value));
            }
        }
    }
    let require = node
        .provisioning
        .get("require_credentials")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(provider.require_credentials);
    if require && env.is_empty() && !dry_run && !provider.credential_env.is_empty() {
        return Err(RefineError::InvalidInput(format!(
            "provider requires credentials; set one of: {}",
            provider.credential_env.join(", ")
        )));
    }
    Ok(env)
}

fn apply_operation_outcome(
    node: &mut Node,
    operation: FleetOperation,
    provider_name: &str,
    ok: bool,
    results: &[FleetStepResult],
) {
    let mut details = serde_json::Map::new();
    details.insert(
        "fleet".to_string(),
        serde_json::json!({
            "operation": operation.as_str(),
            "provider": provider_name,
            "steps": results
        }),
    );
    let status = match (operation, ok) {
        (FleetOperation::Provision, true) => "ready",
        (FleetOperation::Deprovision, true) => "deprovisioned",
        (FleetOperation::Status, true) => "ready",
        (_, false) => "failed",
    };
    if operation == FleetOperation::Provision {
        node.provider = provider_name.to_string();
    }
    if operation == FleetOperation::Deprovision && ok {
        node.enabled = false;
    }
    node.health = Some(NodeHealth {
        status: status.to_string(),
        checked_at: now_timestamp(),
        details: Some(details),
    });
    node.updated_at = now_timestamp();
}

/// Built-in providers used when `.refine/fleet.json` is absent or does not
/// redefine them. A fleet.json entry with the same name fully replaces the
/// built-in, which is how newer Refine versions (or operators) retune
/// provisioning without a new control binary.
pub fn builtin_providers() -> Vec<(String, FleetProviderConfig)> {
    vec![("fly".to_string(), builtin_fly_provider())]
}

fn builtin_fly_provider() -> FleetProviderConfig {
    let mut defaults = JsonObject::new();
    defaults.insert("org".to_string(), serde_json::json!("personal"));
    defaults.insert("region".to_string(), serde_json::json!("iad"));
    defaults.insert("vm_size".to_string(), serde_json::json!("shared-cpu-2x"));
    defaults.insert("vm_memory".to_string(), serde_json::json!("2048"));
    defaults.insert(
        "repo_url".to_string(),
        serde_json::json!("https://github.com/buwilliams/refine.git"),
    );
    defaults.insert("refine_ref".to_string(), serde_json::json!("main"));
    FleetProviderConfig {
        display_name: "Fly.io".to_string(),
        binary: "fly".to_string(),
        credential_env: vec!["FLY_API_TOKEN".to_string()],
        require_credentials: false,
        defaults,
        provision: vec![
            FleetCommandStep {
                argv: string_vec(&[
                    "{binary}",
                    "apps",
                    "create",
                    "{app_name}",
                    "--org",
                    "{org}",
                    "--yes",
                ]),
                allow_failure: true,
            },
            FleetCommandStep {
                argv: string_vec(&[
                    "{binary}",
                    "deploy",
                    "--app",
                    "{app_name}",
                    "--config",
                    "{fleet_dir}/fly.worker.toml",
                    "--dockerfile",
                    "{fleet_dir}/Dockerfile",
                    "--build-arg",
                    "REFINE_REF={refine_ref}",
                    "--build-arg",
                    "REFINE_REPO_URL={repo_url}",
                    "--regions",
                    "{region}",
                    "--vm-size",
                    "{vm_size}",
                    "--vm-memory",
                    "{vm_memory}",
                    "--remote-only",
                    "--yes",
                ]),
                allow_failure: false,
            },
        ],
        deprovision: vec![FleetCommandStep {
            argv: string_vec(&["{binary}", "apps", "destroy", "{app_name}", "--yes"]),
            allow_failure: false,
        }],
        status: vec![FleetCommandStep {
            argv: string_vec(&["{binary}", "status", "--app", "{app_name}", "--json"]),
            allow_failure: false,
        }],
    }
}

fn string_vec(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| part.to_string()).collect()
}

fn now_timestamp() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::host::cluster::{FileClusterService, NodeRemoteUpdate};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }

    fn service_with_node(prefix: &str, node_id: &str) -> (PathBuf, FileFleetService) {
        let temp_root = unique_temp_dir(prefix);
        let refine_dir = temp_root.join(".refine");
        FileClusterService::new(&refine_dir)
            .upsert_node(node_id, NodeRemoteUpdate::default())
            .unwrap();
        let fleet = FileFleetService::new(&refine_dir);
        (temp_root, fleet)
    }

    #[test]
    fn builtin_fly_provider_is_available_without_config() {
        let (temp_root, fleet) = service_with_node("fleet-builtin", "worker-1");
        let providers = fleet.providers_response().unwrap();
        assert_eq!(providers["default_provider"], "fly");
        assert_eq!(providers["config_present"], false);
        let fly = providers["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|provider| provider["name"] == "fly")
            .unwrap();
        assert_eq!(fly["source"], "builtin");
        assert_eq!(fly["binary"], "fly");
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn provision_dry_run_renders_fly_commands_and_persists_nothing() {
        let (temp_root, fleet) = service_with_node("fleet-dry-run", "worker-1");
        let response = fleet.provision_response("worker-1", None, true).unwrap();
        assert_eq!(response["ok"], true);
        assert_eq!(response["dry_run"], true);
        assert_eq!(response["provider"], "fly");
        let steps = response["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2);
        let create = steps[0]["command"].as_str().unwrap();
        assert!(create.starts_with("fly apps create refine-worker-1"));
        assert_eq!(steps[0]["executed"], false);
        let deploy = steps[1]["command"].as_str().unwrap();
        assert!(deploy.contains("fly deploy --app refine-worker-1"));
        assert!(deploy.contains("--build-arg REFINE_REF=main"));
        assert!(deploy.contains("--regions iad"));
        // dry run must not stamp provider/health into the registry
        let node = FileClusterService::new(&fleet.refine_dir)
            .show("worker-1")
            .unwrap();
        assert_eq!(node["node"]["provider"], serde_json::Value::Null);
        assert_eq!(node["node"]["health"], serde_json::Value::Null);
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn node_provisioning_settings_override_provider_defaults() {
        let (temp_root, fleet) = service_with_node("fleet-overrides", "worker-2");
        let nodes = FileNodeRegistryService::new(&fleet.refine_dir);
        let mut registry = nodes.load_registry().unwrap();
        let node = registry
            .nodes
            .iter_mut()
            .find(|node| node.id == "worker-2")
            .unwrap();
        node.provisioning.insert(
            "app_name".to_string(),
            serde_json::json!("custom-{node_id}"),
        );
        node.provisioning
            .insert("region".to_string(), serde_json::json!("syd"));
        node.provisioning
            .insert("refine_ref".to_string(), serde_json::json!("3.1.6"));
        nodes.save_registry(&registry).unwrap();

        let response = fleet.provision_response("worker-2", None, true).unwrap();
        let deploy = response["steps"][1]["command"].as_str().unwrap();
        assert!(deploy.contains("--app custom-worker-2"));
        assert!(deploy.contains("--regions syd"));
        assert!(deploy.contains("REFINE_REF=3.1.6"));
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn fleet_config_defines_custom_providers_and_default() {
        let (temp_root, fleet) = service_with_node("fleet-config", "worker-3");
        fs::write(
            fleet.config_path(),
            serde_json::json!({
                "schema_version": 1,
                "default_provider": "droplet",
                "providers": {
                    "droplet": {
                        "binary": "doctl",
                        "defaults": {"size": "s-2vcpu-4gb"},
                        "provision": [["{binary}", "compute", "droplet", "create", "{app_name}", "--size", "{size}"]],
                        "deprovision": [["{binary}", "compute", "droplet", "delete", "{app_name}", "--force"]]
                    }
                }
            })
            .to_string(),
        )
        .unwrap();
        let providers = fleet.providers_response().unwrap();
        assert_eq!(providers["default_provider"], "droplet");
        let names: Vec<&str> = providers["providers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|provider| provider["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"droplet"));
        assert!(names.contains(&"fly"));

        let response = fleet.provision_response("worker-3", None, true).unwrap();
        assert_eq!(response["provider"], "droplet");
        let command = response["steps"][0]["command"].as_str().unwrap();
        assert_eq!(
            command,
            "doctl compute droplet create refine-worker-3 --size s-2vcpu-4gb"
        );
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn newer_fleet_schema_is_rejected_with_upgrade_guidance() {
        let (temp_root, fleet) = service_with_node("fleet-schema", "worker-4");
        fs::write(
            fleet.config_path(),
            serde_json::json!({"schema_version": CURRENT_FLEET_SCHEMA_VERSION + 1}).to_string(),
        )
        .unwrap();
        let error = fleet
            .provision_response("worker-4", None, true)
            .unwrap_err();
        assert!(matches!(error, RefineError::Conflict(_)));
        assert!(error.to_string().contains("update Refine"));
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn unknown_provider_and_missing_operation_fail_loudly() {
        let (temp_root, fleet) = service_with_node("fleet-unknown", "worker-5");
        let error = fleet
            .provision_response("worker-5", Some("nope"), true)
            .unwrap_err();
        assert!(matches!(error, RefineError::NotFound(_)));

        fs::write(
            fleet.config_path(),
            serde_json::json!({
                "providers": {"bare": {"binary": "bare"}}
            })
            .to_string(),
        )
        .unwrap();
        let error = fleet
            .provision_response("worker-5", Some("bare"), true)
            .unwrap_err();
        assert!(error.to_string().contains("does not define a provision"));
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn required_credentials_missing_blocks_real_provisioning() {
        let (temp_root, fleet) = service_with_node("fleet-creds", "worker-6");
        fs::write(
            fleet.config_path(),
            serde_json::json!({
                "default_provider": "strict",
                "providers": {
                    "strict": {
                        "binary": "strictctl",
                        "credential_env": ["REFINE_TEST_FLEET_TOKEN_THAT_IS_NOT_SET"],
                        "require_credentials": true,
                        "provision": [["{binary}", "up", "{app_name}"]]
                    }
                }
            })
            .to_string(),
        )
        .unwrap();
        // dry run is allowed without credentials
        fleet.provision_response("worker-6", None, true).unwrap();
        let error = fleet
            .provision_response("worker-6", None, false)
            .unwrap_err();
        assert!(error.to_string().contains("requires credentials"));
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn deprovision_dry_run_renders_destroy_command() {
        let (temp_root, fleet) = service_with_node("fleet-destroy", "worker-7");
        let response = fleet.deprovision_response("worker-7", true).unwrap();
        assert_eq!(
            response["steps"][0]["command"],
            "fly apps destroy refine-worker-7 --yes"
        );
        fs::remove_dir_all(temp_root).unwrap();
    }
}
