use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn file_security_service_audits_and_enforces_host_command_allowlist() {
    let temp_root = unique_temp_dir("security");
    let security =
        FileSecurityService::with_allowed_commands(&temp_root, ["goal.create", "goal.edit"]);

    assert_eq!(
        security.redact("Authorization token=secret"),
        "Authorization token=[redacted]"
    );
    security.audit("cli", "goal.edit").unwrap();
    security
        .authorize_host_command("process_supervisor", "goal.create --dry-run")
        .unwrap();
    assert!(
        security
            .authorize_host_command("process_supervisor", "system.shell")
            .is_err()
    );
    let audit = fs::read_to_string(security.audit_path()).unwrap();
    assert!(audit.contains("authorized"));
    assert!(audit.contains("denied"));
    assert!(audit.contains("recorded"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_security_service_loads_allowed_commands_from_project_settings() {
    let temp_root = unique_temp_dir("security-settings");
    let runtime_root = temp_root.join("run");
    let refine_dir = temp_root.join(".refine");
    FileSettingsService::new(&refine_dir)
        .update(&serde_json::json!({
            "allowed_commands": "printf, npm run test\ncargo test"
        }))
        .unwrap();

    let security = FileSecurityService::from_project_settings(&runtime_root, &refine_dir).unwrap();

    assert!(
        security
            .authorize_host_command("quality", "printf ok")
            .is_ok()
    );
    assert!(
        security
            .authorize_host_command("quality", "npm run test")
            .is_ok()
    );
    assert!(
        security
            .authorize_host_command("quality", "rm -rf target")
            .is_err()
    );

    fs::remove_dir_all(temp_root).unwrap_or(());
}

#[test]
fn native_secret_store_persists_fallback_secrets_with_metadata() {
    let temp_root = unique_temp_dir("secret-store");
    let store = NativeSecretStore::with_backend(&temp_root, SecretStoreBackend::FileFallback);

    let metadata = store
        .put_secret("provider", "smoke_ai_token", "secret-value")
        .unwrap();
    assert_eq!(metadata.scope, "provider");
    assert_eq!(metadata.name, "smoke_ai_token");
    assert_eq!(metadata.backend, SecretStoreBackend::FileFallback);
    assert!(!metadata.native);

    let listed = store.list_secrets().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "smoke_ai_token");
    let secret = store.get_secret("provider", "smoke_ai_token").unwrap();
    assert_eq!(secret.value, "secret-value");

    let deleted = store.delete_secret("provider", "smoke_ai_token").unwrap();
    assert_eq!(deleted.name, "smoke_ai_token");
    assert!(store.list_secrets().unwrap().is_empty());
    assert!(store.get_secret("provider", "smoke_ai_token").is_err());

    fs::remove_dir_all(temp_root).unwrap_or(());
}

#[test]
fn native_secret_store_rejects_invalid_secret_names() {
    let temp_root = unique_temp_dir("secret-store-invalid");
    let store = NativeSecretStore::with_backend(&temp_root, SecretStoreBackend::FileFallback);

    assert!(store.put_secret("provider", "bad/name", "value").is_err());
    assert!(store.put_secret("provider", "empty", "").is_err());
    assert_eq!(
        store.backend_status().backend,
        SecretStoreBackend::FileFallback
    );

    fs::remove_dir_all(temp_root).unwrap_or(());
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}
