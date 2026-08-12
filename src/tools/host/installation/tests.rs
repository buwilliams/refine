use super::os_backend::{LegacyRepairFailpoint, ServiceCommandOutput};
use super::service_control::launchd_label;
use super::*;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::Path;
use std::sync::Mutex;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn service_install_fails_before_registration_when_installed_binary_is_missing() {
    let temp_root = unique_temp_dir("installation-missing-binary");
    let runtime_root = temp_root.join("run");
    let service =
        unprepared_test_installation_service_for_port(&runtime_root, "1.0.0", 4557, &temp_root);

    let error = service
        .install(InstallTarget::LinuxCliWeb)
        .unwrap_err()
        .to_string();

    assert!(error.contains("installed Refine mode requires"), "{error}");
    assert!(error.contains("bin/refine"), "{error}");
    assert!(!service.backend_path().exists());
    assert!(!service.path().exists());
    fs::remove_dir_all(temp_root).unwrap_or(());
}

#[test]
fn historical_target_aliases_parse_but_current_output_is_daemon_oriented() {
    assert_eq!(
        serde_json::from_str::<InstallTarget>("\"mac_os_app_bundle\"").unwrap(),
        InstallTarget::MacosDaemon
    );
    assert_eq!(
        serde_json::from_str::<InstallTarget>("\"windows_installer\"").unwrap(),
        InstallTarget::WindowsDaemon
    );
    assert_eq!(
        serde_json::to_string(&InstallTarget::MacosDaemon).unwrap(),
        "\"macos_daemon\""
    );
    assert_eq!(
        serde_json::to_string(&InstallTarget::WindowsDaemon).unwrap(),
        "\"windows_daemon\""
    );

    let root = unique_temp_dir("historical-backend-schema");
    let mut historical = serde_json::to_value(backend_for_target(
        InstallTarget::MacosDaemon,
        "now",
        RuntimePathInputs::default(),
        Some(4557),
        &root,
    ))
    .unwrap();
    historical["target"] = serde_json::Value::String("mac_os_app_bundle".to_string());
    historical["desktop_bundle"] = serde_json::Value::String("/retired/Refine.app".to_string());
    let parsed: InstallBackendRegistration = serde_json::from_value(historical).unwrap();
    assert_eq!(parsed.target, InstallTarget::MacosDaemon);
    let current = serde_json::to_value(parsed).unwrap();
    assert_eq!(current["target"], "macos_daemon");
    assert!(current.get("desktop_bundle").is_none());
    fs::remove_dir_all(root).unwrap_or(());
}

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
fn installed_systemd_service_owns_daemon_lifecycle_commands() {
    let temp_root = unique_temp_dir("installation-systemd-control");
    let runtime_root = temp_root.join("run");
    let service = test_installation_service_for_port(&runtime_root, "1.0.0", 4557, &temp_root);

    assert!(
        service
            .control_installed_service(InstalledServiceAction::Start)
            .unwrap()
            .is_none()
    );
    service.install(InstallTarget::LinuxCliWeb).unwrap();

    for (action, verb) in [
        (InstalledServiceAction::Start, "start"),
        (InstalledServiceAction::Stop, "stop"),
        (InstalledServiceAction::Restart, "restart"),
    ] {
        let control = service.control_installed_service(action).unwrap().unwrap();
        assert_eq!(control.service_manager, "systemd_user");
        assert_eq!(control.action, action);
        assert_eq!(
            control.commands,
            vec![format!(
                "'systemctl' '--user' '{verb}' 'refine-4557.service'"
            )]
        );
    }

    let mut backend = service.load_backend().unwrap().unwrap();
    backend.activated = false;
    service.save_backend(&backend).unwrap();
    assert!(
        service
            .control_installed_service(InstalledServiceAction::Start)
            .unwrap()
            .is_none(),
        "partial installations must retain direct-process fallback"
    );
    assert!(
        service
            .control_installed_service(InstalledServiceAction::Stop)
            .unwrap()
            .is_none(),
        "an inactive registration must not intercept shutdown of the direct fallback runtime"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn uninstall_deactivation_errors_are_visible_and_after_stop_cleanup_does_not_stop_twice() {
    let temp_root = unique_temp_dir("installation-uninstall-deactivation");
    let runtime_root = temp_root.join("run");
    let linux = test_installation_service_for_port(&runtime_root, "1.0.0", 4557, &temp_root);
    linux.install(InstallTarget::LinuxCliWeb).unwrap();
    let mut linux_backend = linux.load_backend().unwrap().unwrap();

    let error = linux
        .deactivate_os_backend_with(&mut linux_backend, false, &mut |_| {
            Err("permission denied".to_string())
        })
        .unwrap_err();

    assert!(error.to_string().contains("permission denied"));
    assert!(linux_backend.activated);

    let mac = test_installation_service_for_port(&runtime_root, "1.0.0", 4558, &temp_root);
    mac.install(InstallTarget::MacosDaemon).unwrap();
    let mut mac_backend = mac.load_backend().unwrap().unwrap();
    let mut commands = Vec::new();
    mac.deactivate_os_backend_with(&mut mac_backend, true, &mut |command| {
        commands.push(command.display());
        Ok(())
    })
    .unwrap();

    assert_eq!(commands.len(), 1, "{commands:?}");
    assert!(commands[0].contains("'launchctl' 'disable'"));
    assert!(!commands[0].contains("bootout"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn installation_uninstall_refuses_to_remove_registration_while_daemon_is_reachable() {
    let temp_root = unique_temp_dir("installation-uninstall-reachable");
    let runtime_root = temp_root.join("run");
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let service = test_installation_service_for_port(&runtime_root, "1.0.0", port, &temp_root);
    let installed = service.install(InstallTarget::LinuxCliWeb).unwrap();
    let metadata = PathBuf::from(
        installed
            .backend
            .as_ref()
            .and_then(|backend| backend.service_metadata_path.as_ref())
            .unwrap(),
    );
    let responder = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .unwrap();
    });

    let error = service.uninstall().unwrap_err();
    responder.join().unwrap();

    assert!(error.to_string().contains("daemon remains reachable"));
    assert!(service.status().unwrap().installed);
    assert!(service.backend_path().exists());
    assert!(metadata.exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn source_promotion_registration_swap_is_durable_and_restorable_for_systemd() {
    let temp_root = unique_temp_dir("installation-systemd-source-registration");
    let runtime_root = temp_root.join("run");
    let service = test_installation_service_for_port(&runtime_root, "1.0.0", 4557, &temp_root);
    let prior = temp_root.join("bin/refine");
    fs::create_dir_all(prior.parent().unwrap()).unwrap();
    fs::write(&prior, "prior fixture").unwrap();
    let installed = service.install(InstallTarget::LinuxCliWeb).unwrap();
    let metadata_path = PathBuf::from(
        installed
            .backend
            .as_ref()
            .unwrap()
            .service_metadata_path
            .as_ref()
            .unwrap(),
    );
    let original = fs::read_to_string(&metadata_path).unwrap();
    let candidate = temp_root.join("candidate/refine");
    fs::create_dir_all(candidate.parent().unwrap()).unwrap();
    fs::write(&candidate, "candidate fixture").unwrap();

    let update = service
        .prepare_service_executable(&candidate)
        .unwrap()
        .unwrap();
    assert_eq!(update.service_manager, "systemd_user");
    assert_eq!(update.candidate_executable, candidate);
    let prepared = fs::read_to_string(&metadata_path).unwrap();
    assert!(prepared.contains(&candidate.display().to_string()));
    assert!(!prepared.contains("candidate/refine.pending"));
    let second = service
        .prepare_service_executable(Path::new("/other/candidate/refine"))
        .unwrap_err();
    assert!(second.to_string().contains("prior source-promotion"));
    assert_eq!(fs::read_to_string(&metadata_path).unwrap(), prepared);

    assert!(service.restore_service_executable().unwrap());
    assert_eq!(fs::read_to_string(&metadata_path).unwrap(), original);
    let mismatch = service
        .verify_restored_service_executable(&candidate)
        .unwrap_err();
    assert!(
        mismatch.to_string().contains("expected prior executable"),
        "{mismatch}"
    );
    assert!(
        service.restore_service_executable().unwrap(),
        "failed verification must retain the durable registration backup"
    );
    assert_eq!(
        service.verify_restored_service_executable(&prior).unwrap(),
        fs::canonicalize(prior).unwrap()
    );
    service.complete_service_executable_update().unwrap();
    assert!(!service.restore_service_executable().unwrap());
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn source_promotion_registration_swap_renders_launchd_candidate_without_loading_it_early() {
    let temp_root = unique_temp_dir("installation-launchd-source-registration");
    let runtime_root = temp_root.join("run");
    let service = test_installation_service_for_port(&runtime_root, "1.0.0", 4558, &temp_root);
    let prior = temp_root.join("bin/refine");
    fs::create_dir_all(prior.parent().unwrap()).unwrap();
    fs::write(&prior, "prior fixture").unwrap();
    let installed = service.install(InstallTarget::MacosDaemon).unwrap();
    let metadata_path = PathBuf::from(
        installed
            .backend
            .as_ref()
            .unwrap()
            .service_metadata_path
            .as_ref()
            .unwrap(),
    );
    let original = fs::read_to_string(&metadata_path).unwrap();
    let candidate = temp_root.join("candidate/refine & next");

    let update = service
        .prepare_service_executable(&candidate)
        .unwrap()
        .unwrap();
    assert_eq!(update.service_manager, "launchd_user_daemon");
    let prepared = fs::read_to_string(&metadata_path).unwrap();
    assert!(prepared.contains("candidate/refine &amp; next"));
    assert!(service.restore_service_executable().unwrap());
    assert_eq!(fs::read_to_string(&metadata_path).unwrap(), original);
    assert_eq!(
        service.verify_restored_service_executable(&prior).unwrap(),
        fs::canonicalize(prior).unwrap()
    );
    service.complete_service_executable_update().unwrap();
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn inactive_registration_does_not_intercept_candidate_direct_fallback() {
    let temp_root = unique_temp_dir("installation-inactive-source-registration");
    let runtime_root = temp_root.join("run");
    let service = test_installation_service_for_port(&runtime_root, "1.0.0", 4557, &temp_root);
    let installed = service.install(InstallTarget::LinuxCliWeb).unwrap();
    let metadata_path = PathBuf::from(
        installed
            .backend
            .as_ref()
            .unwrap()
            .service_metadata_path
            .as_ref()
            .unwrap(),
    );
    let original = fs::read_to_string(&metadata_path).unwrap();
    let mut backend = installed.backend.unwrap();
    backend.activated = false;
    backend.activation_error = Some("systemd user manager unavailable".to_string());
    service.save_backend(&backend).unwrap();

    assert_eq!(
        service
            .prepare_service_executable(Path::new("/candidate/refine"))
            .unwrap(),
        None
    );
    assert_eq!(fs::read_to_string(metadata_path).unwrap(), original);
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn installed_launchd_service_is_port_scoped_and_owns_lifecycle_commands() {
    let temp_root = unique_temp_dir("installation-launchd-control");
    let runtime_root = temp_root.join("run");
    let service = test_installation_service_for_port(&runtime_root, "1.0.0", 4558, &temp_root);
    let installed = service.install(InstallTarget::MacosDaemon).unwrap();
    let backend = installed.backend.unwrap();
    let plist = fs::read_to_string(backend.service_metadata_path.unwrap()).unwrap();
    let target = format!("{}/com.refine.daemon.4558", launchctl_gui_domain());

    assert!(plist.contains("<string>com.refine.daemon.4558</string>"));
    assert!(plist.contains(
        "<string>--port</string>\n    <string>4558</string>\n    <string>--runtime-root</string>"
    ));

    let start = service
        .control_installed_service(InstalledServiceAction::Start)
        .unwrap()
        .unwrap();
    assert_eq!(
        start.commands,
        vec![
            format!("'launchctl' 'print' '{target}'"),
            format!("'launchctl' 'kickstart' '{target}'"),
        ]
    );

    let stop = service
        .control_installed_service(InstalledServiceAction::Stop)
        .unwrap()
        .unwrap();
    assert_eq!(
        stop.commands,
        vec![
            format!("'launchctl' 'print' '{target}'"),
            format!("'launchctl' 'disable' '{target}'"),
            format!("'launchctl' 'bootout' '{target}'"),
        ]
    );

    let restart = service
        .control_installed_service(InstalledServiceAction::Restart)
        .unwrap()
        .unwrap();
    assert_eq!(
        restart.commands,
        vec![
            format!("'launchctl' 'print' '{target}'"),
            format!("'launchctl' 'kickstart' '-k' '{target}'"),
        ]
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn launchd_installations_and_controls_are_isolated_by_port() {
    let temp_root = unique_temp_dir("installation-launchd-port-isolation");
    let runtime_root = temp_root.join("run");
    let first = test_installation_service_for_port(&runtime_root, "1.0.0", 4558, &temp_root);
    let second = test_installation_service_for_port(&runtime_root, "1.0.0", 4559, &temp_root);

    let first_backend = first
        .install(InstallTarget::MacosDaemon)
        .unwrap()
        .backend
        .unwrap();
    let second_backend = second
        .install(InstallTarget::MacosDaemon)
        .unwrap()
        .backend
        .unwrap();
    let first_label = launchd_label(&first_backend);
    let second_label = launchd_label(&second_backend);
    assert_eq!(first_label, "com.refine.daemon.4558");
    assert_eq!(second_label, "com.refine.daemon.4559");
    assert_ne!(first_label, second_label);

    let first_plist =
        fs::read_to_string(first_backend.service_metadata_path.as_ref().unwrap()).unwrap();
    let second_plist =
        fs::read_to_string(second_backend.service_metadata_path.as_ref().unwrap()).unwrap();
    assert!(first_plist.contains("<string>com.refine.daemon.4558</string>"));
    assert!(!first_plist.contains("<string>com.refine.daemon.4559</string>"));
    assert!(second_plist.contains("<string>com.refine.daemon.4559</string>"));
    assert!(!second_plist.contains("<string>com.refine.daemon.4558</string>"));

    let first_commands = first
        .control_installed_service(InstalledServiceAction::Restart)
        .unwrap()
        .unwrap()
        .commands;
    let second_commands = second
        .control_installed_service(InstalledServiceAction::Restart)
        .unwrap()
        .unwrap()
        .commands;
    assert!(
        first_commands
            .iter()
            .all(|command| !command.contains("4559"))
    );
    assert!(
        second_commands
            .iter()
            .all(|command| !command.contains("4558"))
    );
    assert!(
        first_commands
            .iter()
            .all(|command| command.contains("com.refine.daemon.4558"))
    );
    assert!(
        second_commands
            .iter()
            .all(|command| command.contains("com.refine.daemon.4559"))
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn launchd_legacy_label_is_migrated_only_when_registration_path_matches() {
    let temp_root = unique_temp_dir("installation-launchd-legacy-migration");
    let runtime_root = temp_root.join("run");
    let service = test_installation_service_for_port(&runtime_root, "1.0.0", 4558, &temp_root);
    let installed = service.install(InstallTarget::MacosDaemon).unwrap();
    let original_backend = installed.backend.unwrap();
    let plist_path = original_backend.service_metadata_path.as_ref().unwrap();
    let scoped_plist = fs::read_to_string(plist_path).unwrap();
    fs::write(
        plist_path,
        scoped_plist.replace(
            "<key>Label</key><string>com.refine.daemon.4558</string>",
            "<key>Label</key><string>com.refine.daemon</string>",
        ),
    )
    .unwrap();

    let repaired = service.repair().unwrap();
    let backend = repaired.backend.unwrap();
    assert_eq!(
        backend.legacy_service_label.as_deref(),
        Some("com.refine.daemon")
    );
    assert!(
        fs::read_to_string(plist_path)
            .unwrap()
            .contains("<key>Label</key><string>com.refine.daemon.4558</string>")
    );

    let metadata_path = backend.service_metadata_path.as_deref().unwrap();
    let mut outcomes = VecDeque::from([
        Ok(ServiceCommandOutput::exited(
            113,
            "",
            "Could not find service \"com.refine.daemon.4558\" in domain for user gui: 501",
        )),
        Ok(ServiceCommandOutput::exited(
            0,
            format!("path = {metadata_path}"),
            "",
        )),
        Ok(ServiceCommandOutput::success()),
        Ok(ServiceCommandOutput::success()),
        Ok(ServiceCommandOutput::success()),
        Ok(ServiceCommandOutput::success()),
    ]);
    let mut commands = Vec::new();
    service
        .control_installed_service_with(InstalledServiceAction::Start, &mut |command| {
            commands.push(command.display());
            outcomes.pop_front().unwrap()
        })
        .unwrap();
    let legacy_target = format!("{}/com.refine.daemon", launchctl_gui_domain());
    let scoped_target = format!("{}/com.refine.daemon.4558", launchctl_gui_domain());
    assert_eq!(
        commands,
        vec![
            format!("'launchctl' 'print' '{scoped_target}'"),
            format!("'launchctl' 'print' '{legacy_target}'"),
            format!("'launchctl' 'disable' '{legacy_target}'"),
            format!("'launchctl' 'bootout' '{legacy_target}'"),
            format!("'launchctl' 'enable' '{scoped_target}'"),
            format!(
                "'launchctl' 'bootstrap' '{}' '{}'",
                launchctl_gui_domain(),
                metadata_path
            ),
        ]
    );

    let mut outcomes = VecDeque::from([
        Ok(ServiceCommandOutput::exited(
            113,
            "",
            "Could not find service \"com.refine.daemon.4558\" in domain for user gui: 501",
        )),
        Ok(ServiceCommandOutput::exited(
            0,
            "path = /Library/LaunchAgents/unrelated.plist",
            "",
        )),
        Ok(ServiceCommandOutput::success()),
        Ok(ServiceCommandOutput::success()),
    ]);
    let mut commands = Vec::new();
    service
        .control_installed_service_with(InstalledServiceAction::Start, &mut |command| {
            commands.push(command.display());
            outcomes.pop_front().unwrap()
        })
        .unwrap();
    assert!(
        commands
            .iter()
            .all(|command| !command.contains(&format!("'disable' '{legacy_target}'")))
    );
    assert!(
        commands
            .iter()
            .all(|command| !command.contains(&format!("'bootout' '{legacy_target}'")))
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn service_control_reports_systemd_and_launchd_command_failures() {
    let temp_root = unique_temp_dir("installation-service-command-failures");
    let runtime_root = temp_root.join("run");
    let systemd = test_installation_service_for_port(&runtime_root, "1.0.0", 4557, &temp_root);
    systemd.install(InstallTarget::LinuxCliWeb).unwrap();
    let mut systemd_commands = Vec::new();
    let systemd_error = systemd
        .control_installed_service_with(InstalledServiceAction::Start, &mut |command| {
            systemd_commands.push(command.display());
            Ok(ServiceCommandOutput::exited(
                1,
                "",
                "Failed to start refine-4557.service",
            ))
        })
        .unwrap_err();
    assert_eq!(
        systemd_commands,
        vec!["'systemctl' '--user' 'start' 'refine-4557.service'"]
    );
    assert!(
        systemd_error
            .to_string()
            .contains("Failed to start refine-4557.service"),
        "{systemd_error}"
    );

    let launchd = test_installation_service_for_port(&runtime_root, "1.0.0", 4558, &temp_root);
    launchd.install(InstallTarget::MacosDaemon).unwrap();
    let mut launchd_commands = Vec::new();
    let launchd_error = launchd
        .control_installed_service_with(InstalledServiceAction::Start, &mut |command| {
            launchd_commands.push(command.display());
            if command.args.first().map(String::as_str) == Some("print") {
                Ok(ServiceCommandOutput::success())
            } else {
                Ok(ServiceCommandOutput::exited(
                    5,
                    "",
                    "kickstart failed: input/output error",
                ))
            }
        })
        .unwrap_err();
    assert_eq!(launchd_commands.len(), 2);
    assert!(launchd_commands[0].contains("'launchctl' 'print'"));
    assert!(launchd_commands[1].contains("'launchctl' 'kickstart'"));
    assert!(
        launchd_error
            .to_string()
            .contains("kickstart failed: input/output error"),
        "{launchd_error}"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn launchd_query_distinguishes_not_loaded_from_operational_failure() {
    let temp_root = unique_temp_dir("installation-launchd-query");
    let runtime_root = temp_root.join("run");
    let service = test_installation_service_for_port(&runtime_root, "1.0.0", 4558, &temp_root);
    service.install(InstallTarget::MacosDaemon).unwrap();

    let not_loaded = ServiceCommandOutput::exited(
        113,
        "",
        "Could not find service \"com.refine.daemon\" in domain for user gui: 501",
    );
    let mut outcomes = VecDeque::from([
        Ok(not_loaded),
        Ok(ServiceCommandOutput::success()),
        Ok(ServiceCommandOutput::success()),
    ]);
    let mut commands = Vec::new();
    let control = service
        .control_installed_service_with(InstalledServiceAction::Start, &mut |command| {
            commands.push(command.display());
            outcomes.pop_front().unwrap()
        })
        .unwrap()
        .unwrap();
    assert_eq!(control.commands, commands);
    assert!(commands[1].contains("'launchctl' 'enable'"));
    assert!(commands[2].contains("'launchctl' 'bootstrap'"));

    let mut commands = Vec::new();
    let query_error = service
        .control_installed_service_with(InstalledServiceAction::Restart, &mut |command| {
            commands.push(command.display());
            Ok(ServiceCommandOutput::exited(
                1,
                "",
                "Operation not permitted while accessing domain",
            ))
        })
        .unwrap_err();
    assert_eq!(
        commands.len(),
        1,
        "an uncertain query must not be treated as an unloaded service"
    );
    assert!(
        query_error
            .to_string()
            .contains("Operation not permitted while accessing domain"),
        "{query_error}"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn launchd_stop_attempts_safety_commands_and_aggregates_failures() {
    let temp_root = unique_temp_dir("installation-launchd-stop-failures");
    let runtime_root = temp_root.join("run");
    let service = test_installation_service_for_port(&runtime_root, "1.0.0", 4558, &temp_root);
    service.install(InstallTarget::MacosDaemon).unwrap();

    let mut outcomes = VecDeque::from([
        Ok(ServiceCommandOutput::exited(
            1,
            "",
            "launchd query transport failed",
        )),
        Ok(ServiceCommandOutput::exited(1, "", "disable denied")),
        Ok(ServiceCommandOutput::success()),
    ]);
    let mut commands = Vec::new();
    let error = service
        .control_installed_service_with(InstalledServiceAction::Stop, &mut |command| {
            commands.push(command.display());
            outcomes.pop_front().unwrap()
        })
        .unwrap_err();
    assert_eq!(commands.len(), 3);
    assert!(commands[0].contains("'launchctl' 'print'"));
    assert!(commands[1].contains("'launchctl' 'disable'"));
    assert!(
        commands[2].contains("'launchctl' 'bootout'"),
        "bootout must still run after a query or disable failure"
    );
    assert!(
        error.to_string().contains("service query failed")
            && error.to_string().contains("disable denied"),
        "{error}"
    );

    let mut outcomes = VecDeque::from([
        Ok(ServiceCommandOutput::success()),
        Ok(ServiceCommandOutput::success()),
        Ok(ServiceCommandOutput::exited(5, "", "bootout failed")),
    ]);
    let error = service
        .control_installed_service_with(InstalledServiceAction::Stop, &mut |_| {
            outcomes.pop_front().unwrap()
        })
        .unwrap_err();
    assert!(error.to_string().contains("bootout failed"), "{error}");

    let mut outcomes = VecDeque::from([
        Ok(ServiceCommandOutput::exited(
            113,
            "",
            "Could not find service \"com.refine.daemon\" in domain for user gui: 501",
        )),
        Ok(ServiceCommandOutput::success()),
    ]);
    let mut commands = Vec::new();
    let control = service
        .control_installed_service_with(InstalledServiceAction::Stop, &mut |command| {
            commands.push(command.display());
            outcomes.pop_front().unwrap()
        })
        .unwrap()
        .unwrap();
    assert_eq!(control.commands, commands);
    assert_eq!(commands.len(), 2);
    assert!(commands[1].contains("'launchctl' 'disable'"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn service_metadata_uses_checkout_binary_working_directory_and_runtime() {
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
    assert!(unit.contains(&format!(
        "ExecStart={} system start --foreground --port 8082 --runtime-root {}",
        temp_root.join("bin/refine").display(),
        runtime_root.display()
    )));
    assert!(unit.contains(&format!("WorkingDirectory={}", temp_root.display())));
    assert!(!unit.contains(".local/state"));

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
    let scoped = test_installation_service_for_port(&runtime_root, "1.1.0", 8082, &temp_root);

    legacy.install(InstallTarget::LinuxCliWeb).unwrap();
    assert!(runtime_root.join(INSTALL_STATE_FILE).exists());
    assert!(!scoped.path().exists());

    let repaired = scoped.repair().unwrap();

    assert_eq!(repaired.port, Some(8082));
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
    assert!(unit.contains("--port 8082 --runtime-root"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn non_default_port_repair_does_not_claim_unscoped_legacy_registration() {
    let temp_root = unique_temp_dir("installation-legacy-owner-isolation");
    let runtime_root = temp_root.join("run");
    let legacy = test_installation_service(&runtime_root, "1.0.0", &temp_root);
    let scoped = test_installation_service_for_port(&runtime_root, "1.1.0", 8080, &temp_root);

    let legacy_status = legacy.install(InstallTarget::LinuxCliWeb).unwrap();
    let legacy_metadata = legacy_status
        .backend
        .as_ref()
        .unwrap()
        .service_metadata_path
        .clone()
        .unwrap();
    let repaired = scoped.repair().unwrap();

    assert_eq!(repaired.port, Some(8080));
    assert_eq!(repaired.version.as_deref(), Some("1.1.0"));
    assert!(runtime_root.join(INSTALL_STATE_FILE).exists());
    assert!(runtime_root.join(INSTALL_BACKEND_FILE).exists());
    assert!(scoped.path().exists());
    assert!(scoped.backend_path().exists());
    assert_ne!(
        repaired
            .backend
            .as_ref()
            .unwrap()
            .service_metadata_path
            .as_deref(),
        Some(legacy_metadata.as_str())
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn explicit_repair_preserves_exact_legacy_registration_and_audited_outcome() {
    use sha2::{Digest, Sha256};

    let temp_root = unique_temp_dir("installation-legacy-registration-audit");
    let runtime_root = temp_root.join("run");
    let service = test_installation_service_for_port(&runtime_root, "1.0.0", 4557, &temp_root);
    let installed = service.install(InstallTarget::LinuxCliWeb).unwrap();
    let registration = PathBuf::from(
        installed
            .backend
            .as_ref()
            .unwrap()
            .service_metadata_path
            .as_ref()
            .unwrap(),
    );
    let current = fs::read_to_string(&registration).unwrap();
    let legacy_runtime = temp_root.join("legacy-external/run");
    fs::create_dir_all(&legacy_runtime).unwrap();
    fs::write(legacy_runtime.join("retained.marker"), b"do-not-touch").unwrap();
    let legacy = current
        .replace(
            &temp_root.join("bin/refine").display().to_string(),
            "/opt/legacy/refine",
        )
        .replace(
            &format!("WorkingDirectory={}", temp_root.display()),
            "WorkingDirectory=/opt/legacy",
        )
        .replace(
            &format!("--runtime-root {}", runtime_root.display()),
            &format!("--runtime-root {}", legacy_runtime.display()),
        );
    fs::write(&registration, legacy.as_bytes()).unwrap();

    let repaired = service.repair().unwrap();
    assert!(repaired.installed);
    assert_eq!(
        fs::read(legacy_runtime.join("retained.marker")).unwrap(),
        b"do-not-touch"
    );
    let migration_root = service.state_root().join("installation-migrations");
    let entries = fs::read_dir(&migration_root)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(entries.len(), 1);
    let journal = entries[0].path();
    assert_eq!(
        fs::read(journal.join("registration.original")).unwrap(),
        legacy.as_bytes()
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(journal.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["outcome"], "completed");
    assert_eq!(manifest["original_bytes"], legacy.len());
    assert_eq!(
        manifest["original_sha256"],
        format!("{:x}", Sha256::digest(legacy.as_bytes()))
    );
    assert_eq!(
        manifest["parsed_runtime_root"],
        legacy_runtime.display().to_string()
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn legacy_repair_failpoints_restore_exact_registration_and_record_outcome() {
    for (failpoint, expected_outcome) in [
        (
            LegacyRepairFailpoint::PendingWrite,
            "publish_failed_rolled_back",
        ),
        (
            LegacyRepairFailpoint::AtomicPublish,
            "publish_failed_rolled_back",
        ),
        (
            LegacyRepairFailpoint::Reload,
            "activation_failed_rolled_back",
        ),
        (
            LegacyRepairFailpoint::Activation,
            "activation_failed_rolled_back",
        ),
        (
            LegacyRepairFailpoint::Verification,
            "verification_failed_rolled_back",
        ),
    ] {
        let temp_root = unique_temp_dir(&format!("legacy-repair-{failpoint:?}"));
        let runtime_root = temp_root.join("run");
        let service = test_installation_service_for_port(&runtime_root, "1.0.0", 4557, &temp_root);
        let installed = service.install(InstallTarget::LinuxCliWeb).unwrap();
        let registration = PathBuf::from(
            installed
                .backend
                .as_ref()
                .unwrap()
                .service_metadata_path
                .as_ref()
                .unwrap(),
        );
        let legacy = fs::read_to_string(&registration).unwrap().replace(
            &temp_root.join("bin/refine").display().to_string(),
            "/opt/legacy/refine",
        );
        fs::write(&registration, legacy.as_bytes()).unwrap();
        let mut backend = backend_for_target(
            InstallTarget::LinuxCliWeb,
            &now_timestamp(),
            service.path_inputs.clone(),
            Some(4557),
            &temp_root,
        );
        let backup = service
            .detect_external_registration(&mut backend, true)
            .unwrap()
            .unwrap();

        let error = service
            .register_os_backend_with_failpoint(&mut backend, Some(&backup), failpoint)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("rollback") || error.contains("restored"),
            "{error}"
        );
        assert_eq!(fs::read(&registration).unwrap(), legacy.as_bytes());
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(backup.journal_dir.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["outcome"], expected_outcome);
        assert_eq!(
            fs::read(backup.journal_dir.join("registration.original")).unwrap(),
            legacy.as_bytes()
        );

        fs::remove_dir_all(temp_root).unwrap();
    }
}

#[test]
fn legacy_repair_refuses_overwrite_when_backup_persistence_fails() {
    let temp_root = unique_temp_dir("legacy-repair-backup-failure");
    let runtime_root = temp_root.join("run");
    let service = test_installation_service_for_port(&runtime_root, "1.0.0", 4557, &temp_root);
    let installed = service.install(InstallTarget::LinuxCliWeb).unwrap();
    let registration = PathBuf::from(
        installed
            .backend
            .as_ref()
            .unwrap()
            .service_metadata_path
            .as_ref()
            .unwrap(),
    );
    let legacy = fs::read_to_string(&registration).unwrap().replace(
        &temp_root.join("bin/refine").display().to_string(),
        "/opt/legacy/refine",
    );
    fs::write(&registration, legacy.as_bytes()).unwrap();
    fs::write(
        service.state_root().join("installation-migrations"),
        b"block directory creation",
    )
    .unwrap();
    let mut backend = backend_for_target(
        InstallTarget::LinuxCliWeb,
        &now_timestamp(),
        service.path_inputs.clone(),
        Some(4557),
        &temp_root,
    );

    let error = service
        .detect_external_registration(&mut backend, true)
        .unwrap_err()
        .to_string();
    assert!(error.contains("migration journal"), "{error}");
    assert_eq!(fs::read(&registration).unwrap(), legacy.as_bytes());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn adjacent_port_does_not_claim_or_remove_legacy_systemd_registration() {
    let temp_root = unique_temp_dir("installation-legacy-adjacent-port");
    let runtime_root = temp_root.join("run");
    let legacy = test_installation_service(&runtime_root, "1.0.0", &temp_root);
    let legacy_status = legacy.install(InstallTarget::LinuxCliWeb).unwrap();
    let legacy_metadata = PathBuf::from(
        legacy_status
            .backend
            .as_ref()
            .unwrap()
            .service_metadata_path
            .as_ref()
            .unwrap(),
    );
    let unit = fs::read_to_string(&legacy_metadata)
        .unwrap()
        .replace("--runtime-root", "--port 45570 --runtime-root");
    fs::write(&legacy_metadata, unit).unwrap();

    let adjacent = test_installation_service_for_port(&runtime_root, "2.0.0", 4557, &temp_root);
    let adjacent_status = adjacent.repair().unwrap();
    assert_eq!(adjacent_status.version.as_deref(), Some("2.0.0"));
    assert!(runtime_root.join(INSTALL_STATE_FILE).exists());
    assert!(runtime_root.join(INSTALL_BACKEND_FILE).exists());
    assert!(legacy_metadata.exists());

    adjacent.uninstall().unwrap();
    assert!(
        legacy_metadata.exists(),
        "uninstalling 4557 must not remove 45570 legacy metadata"
    );
    assert!(runtime_root.join(INSTALL_BACKEND_FILE).exists());

    let exact = test_installation_service_for_port(&runtime_root, "2.0.0", 45570, &temp_root);
    let exact_status = exact.repair().unwrap();
    assert_eq!(exact_status.version.as_deref(), Some("1.0.0"));
    assert!(!runtime_root.join(INSTALL_STATE_FILE).exists());
    assert!(!runtime_root.join(INSTALL_BACKEND_FILE).exists());

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
    backend.target = InstallTarget::WindowsDaemon;
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
    ensure_test_installed_binary(runtime_root);
    unprepared_test_installation_service_for_port(runtime_root, version, port, temp_root)
}

fn unprepared_test_installation_service_for_port(
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
    ensure_test_installed_binary(runtime_root);
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

fn ensure_test_installed_binary(runtime_root: &Path) {
    let executable = runtime_root.parent().unwrap().join("bin/refine");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    if !executable.exists() {
        fs::write(&executable, "installed fixture\n").unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();
    }
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
