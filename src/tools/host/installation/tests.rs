use super::*;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn file_installation_service_persists_update_and_rollback_state() {
    let temp_root = unique_temp_dir("installation");
    let runtime_root = temp_root.join("run");
    let service = test_installation_service_for_port(&runtime_root, "1.0.0", 4557, &temp_root);

    let initial = service.status().unwrap();
    assert!(!initial.installed);
    assert_eq!(initial.port, Some(4557));

    let installed = service.install(InstallTarget::LinuxCliWeb).unwrap();
    assert!(installed.installed);
    assert_eq!(installed.port, Some(4557));
    assert!(!installed.partial);
    assert_eq!(installed.version.as_deref(), Some("1.0.0"));
    assert_eq!(
        installed.backend.as_ref().unwrap().service_manager,
        "systemd_user"
    );
    assert!(installed.backend.as_ref().unwrap().registered);
    assert!(installed.backend.as_ref().unwrap().activated);
    assert!(
        installed
            .backend
            .as_ref()
            .unwrap()
            .activation_commands
            .iter()
            .any(|command| command.contains("'systemctl' '--user' 'enable' '--now'"))
    );
    let service_metadata_path = PathBuf::from(
        installed
            .backend
            .as_ref()
            .unwrap()
            .service_metadata_path
            .as_ref()
            .unwrap(),
    );
    assert_eq!(
        service_metadata_path.file_name().unwrap().to_str().unwrap(),
        "refine-4557.service"
    );
    assert!(service_metadata_path.exists());
    let unit = fs::read_to_string(&service_metadata_path).unwrap();
    assert!(unit.contains("ExecStart="));
    assert!(unit.contains("system start --foreground"));
    assert!(unit.contains("--port 4557 --runtime-root"));
    assert!(service.path().exists());
    assert!(service.backend_path().exists());
    assert_eq!(
        service.path(),
        runtime_root.join("4557").join(INSTALL_STATE_FILE)
    );
    assert_eq!(
        service.backend_path(),
        runtime_root.join("4557").join(INSTALL_BACKEND_FILE)
    );

    let updated = service.record_metadata_update("1.1.0").unwrap();
    assert_eq!(updated.version.as_deref(), Some("1.1.0"));
    assert_eq!(
        updated.backend.as_ref().unwrap().target,
        InstallTarget::LinuxCliWeb
    );
    let stale = test_installation_service_for_port(&runtime_root, "1.2.0", 4557, &temp_root)
        .status()
        .unwrap();
    assert!(stale.stale);

    let rolled_back = service.rollback().unwrap();
    assert_eq!(rolled_back.version.as_deref(), Some("1.0.0"));

    service.uninstall().unwrap();
    assert!(!service.status().unwrap().installed);
    assert!(!service.backend_path().exists());
    assert!(!service_metadata_path.exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn service_metadata_uses_deployed_binary_executable_when_launched_from_wrapper() {
    let _guard = ENV_LOCK.lock().unwrap();
    let old_mode = std::env::var("REFINE_LAUNCH_MODE").ok();
    let old_executable = std::env::var("REFINE_LAUNCH_EXECUTABLE").ok();
    unsafe {
        std::env::set_var("REFINE_LAUNCH_MODE", "binary");
        std::env::set_var("REFINE_LAUNCH_EXECUTABLE", "/opt/refine/bin/refine");
    }

    let temp_root = unique_temp_dir("installation-release-bin");
    let runtime_root = temp_root.join("run");
    let service = test_installation_service_for_port(&runtime_root, "1.0.0", 8082, &temp_root);

    let installed = service.install(InstallTarget::LinuxCliWeb).unwrap();
    let service_metadata_path = PathBuf::from(
        installed
            .backend
            .as_ref()
            .unwrap()
            .service_metadata_path
            .as_ref()
            .unwrap(),
    );
    let unit = fs::read_to_string(&service_metadata_path).unwrap();
    assert!(unit.contains(
        "ExecStart=/opt/refine/bin/refine system start --foreground --port 8082 --runtime-root"
    ));

    restore_env("REFINE_LAUNCH_MODE", old_mode);
    restore_env("REFINE_LAUNCH_EXECUTABLE", old_executable);
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn uninstall_is_scoped_to_selected_port() {
    let temp_root = unique_temp_dir("installation-port-scope");
    let runtime_root = temp_root.join("run");
    let first = test_installation_service_for_port(&runtime_root, "1.0.0", 8081, &temp_root);
    let second = test_installation_service_for_port(&runtime_root, "1.0.0", 8082, &temp_root);

    let first_metadata = PathBuf::from(
        first
            .install(InstallTarget::LinuxCliWeb)
            .unwrap()
            .backend
            .as_ref()
            .unwrap()
            .service_metadata_path
            .as_ref()
            .unwrap(),
    );
    let second_metadata = PathBuf::from(
        second
            .install(InstallTarget::LinuxCliWeb)
            .unwrap()
            .backend
            .as_ref()
            .unwrap()
            .service_metadata_path
            .as_ref()
            .unwrap(),
    );

    first.uninstall().unwrap();

    assert!(!first.backend_path().exists());
    assert!(!first_metadata.exists());
    assert!(second.path().exists());
    assert!(second.backend_path().exists());
    assert!(second_metadata.exists());
    assert!(second.status().unwrap().installed);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn port_scoped_repair_can_migrate_legacy_root_install_state() {
    let temp_root = unique_temp_dir("installation-legacy-port-migration");
    let runtime_root = temp_root.join("run");
    let legacy = test_installation_service(&runtime_root, "1.0.0", &temp_root);
    let scoped = test_installation_service_for_port(&runtime_root, "1.1.0", 8080, &temp_root);

    legacy.install(InstallTarget::LinuxCliWeb).unwrap();
    assert!(runtime_root.join(INSTALL_STATE_FILE).exists());
    assert!(!scoped.path().exists());

    let repaired = scoped.repair().unwrap();

    assert_eq!(repaired.port, Some(8080));
    assert_eq!(repaired.version.as_deref(), Some("1.0.0"));
    assert!(scoped.path().exists());
    assert!(scoped.backend_path().exists());
    assert!(!runtime_root.join(INSTALL_STATE_FILE).exists());
    assert!(!runtime_root.join(INSTALL_BACKEND_FILE).exists());
    let unit = fs::read_to_string(
        repaired
            .backend
            .as_ref()
            .unwrap()
            .service_metadata_path
            .as_ref()
            .unwrap(),
    )
    .unwrap();
    assert!(unit.contains("--port 8080 --runtime-root"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_installation_service_detects_partial_and_conflicting_backend_state() {
    let temp_root = unique_temp_dir("installation-backend");
    let runtime_root = temp_root.join("run");
    let service = test_installation_service_for_port(&runtime_root, "1.0.0", 4558, &temp_root);

    service.install(InstallTarget::LinuxCliWeb).unwrap();
    fs::remove_file(service.backend_path()).unwrap();
    let partial = service.status().unwrap();
    assert!(partial.partial);
    assert!(!partial.conflicting);

    service.repair().unwrap();
    let mut backend = service.load_backend().unwrap().unwrap();
    backend.target = InstallTarget::WindowsInstaller;
    service.save_backend(&backend).unwrap();
    let conflicting = service.status().unwrap();
    assert!(conflicting.conflicting);

    fs::remove_dir_all(temp_root).unwrap();
}

// Every corporate home directory contains an `@` (`<user>@INS.Insurity.net`).
// `@` is not in the set `systemd_escape_arg` leaves bare, so routing
// WorkingDirectory through it quoted the path, and systemd rejects a quoted
// path as "not absolute" — the unit was enabled but could never start.
#[test]
fn systemd_unit_leaves_path_settings_bare_for_a_home_directory_containing_an_at_sign() {
    let temp_root = unique_temp_dir("installation-at-sign").join("buddy@INS.Insurity.net");
    let runtime_root = temp_root.join("state/refine");
    let service = test_installation_service_for_port(&runtime_root, "1.0.0", 8082, &temp_root);

    let installed = service.install(InstallTarget::LinuxCliWeb).unwrap();
    let backend = installed.backend.as_ref().unwrap();
    let unit_path = backend.service_metadata_path.as_ref().unwrap();
    let unit = fs::read_to_string(unit_path).unwrap();

    let setting = |name: &str| {
        unit.lines()
            .find_map(|line| line.strip_prefix(name))
            .unwrap_or_else(|| panic!("{name} missing from unit:\n{unit}"))
            .to_string()
    };

    // The reported failure: a quoted value here is fatal, not merely untidy.
    let working_directory = setting("WorkingDirectory=");
    assert!(
        !working_directory.starts_with('"'),
        "WorkingDirectory must be a bare path, got {working_directory}"
    );
    assert!(
        working_directory.starts_with('/'),
        "WorkingDirectory must be absolute, got {working_directory}"
    );
    assert!(working_directory.contains('@'), "got {working_directory}");

    // The `append:` targets are the same class of setting and equally fatal
    // when quoted.
    for name in ["StandardOutput=append:", "StandardError=append:"] {
        let value = setting(name);
        assert!(!value.starts_with('"'), "{name} must be bare, got {value}");
        assert!(
            value.starts_with('/'),
            "{name} must be absolute, got {value}"
        );
    }

    fs::remove_dir_all(temp_root).unwrap_or(());
}

// Both helpers feed settings that systemd expands specifiers in, so a literal
// `%` has to be doubled or it is rejected as an invalid slot — or worse,
// silently expands to something else.
#[test]
fn systemd_escaping_distinguishes_command_words_from_bare_paths() {
    // Bare-path settings consume the rest of the line: no quoting, and
    // whitespace needs no escaping either.
    assert_eq!(
        systemd_escape_path("/home/buddy@INS.Insurity.net/.local/state/refine"),
        "/home/buddy@INS.Insurity.net/.local/state/refine"
    );
    assert_eq!(
        systemd_escape_path("/home/My Files/refine"),
        "/home/My Files/refine"
    );
    assert_eq!(systemd_escape_path("/home/50%/refine"), "/home/50%%/refine");

    // `ExecStart=` is split into words, so these do need quoting.
    assert_eq!(systemd_escape_arg("/usr/bin/refine"), "/usr/bin/refine");
    assert_eq!(
        systemd_escape_arg("/home/buddy@INS.Insurity.net/bin/refine"),
        "/home/buddy@INS.Insurity.net/bin/refine"
    );
    assert_eq!(
        systemd_escape_arg("/opt/My Apps/refine"),
        "\"/opt/My Apps/refine\""
    );
    assert_eq!(
        systemd_escape_arg("/opt/50%/refine"),
        "\"/opt/50%%/refine\""
    );
    assert_eq!(
        systemd_escape_arg("/opt/a\"b/refine"),
        "\"/opt/a\\\"b/refine\""
    );
}

fn test_installation_service_for_port(
    runtime_root: &PathBuf,
    version: &str,
    port: u16,
    temp_root: &Path,
) -> FileInstallationService {
    FileInstallationService::with_path_inputs_for_port(
        runtime_root,
        version,
        port,
        RuntimePathInputs {
            home: Some(temp_root.join("home")),
            local_app_data: Some(temp_root.join("local-app-data")),
            app_data: Some(temp_root.join("app-data")),
            program_data: Some(temp_root.join("program-data")),
            xdg_cache_home: Some(temp_root.join("cache")),
            xdg_state_home: Some(temp_root.join("state")),
            xdg_config_home: Some(temp_root.join("config")),
        },
    )
}

fn test_installation_service(
    runtime_root: &PathBuf,
    version: &str,
    temp_root: &Path,
) -> FileInstallationService {
    FileInstallationService::with_path_inputs(
        runtime_root,
        version,
        RuntimePathInputs {
            home: Some(temp_root.join("home")),
            local_app_data: Some(temp_root.join("local-app-data")),
            app_data: Some(temp_root.join("app-data")),
            program_data: Some(temp_root.join("program-data")),
            xdg_cache_home: Some(temp_root.join("cache")),
            xdg_state_home: Some(temp_root.join("state")),
            xdg_config_home: Some(temp_root.join("config")),
        },
    )
}

fn restore_env(key: &str, value: Option<String>) {
    unsafe {
        if let Some(value) = value {
            std::env::set_var(key, value);
        } else {
            std::env::remove_var(key);
        }
    }
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}
