use super::*;

pub(super) fn dispatch_command(command: Commands) -> RefineResult<()> {
    match command {
        Commands::Website {
            port,
            bind_address,
            static_root,
            once,
        } => run_website(port, bind_address, static_root, once),
        _ => unreachable!("command family was routed incorrectly"),
    }
}

pub(super) fn run_website(
    port: u16,
    bind_address: std::net::IpAddr,
    static_root: PathBuf,
    once: bool,
) -> RefineResult<()> {
    let static_root = absolute_cli_path(static_root)?;
    let listener = LocalHttpDaemon::bind_address(bind_address, port)?;
    let addr = LocalHttpDaemon::local_addr(&listener)?;
    let actual_port = addr.port();
    let daemon = LocalHttpDaemon::new(
        InProcessWebServer {
            status: DaemonStatus {
                port: actual_port,
                daemon_healthy: false,
                web_available: true,
                worker_state: "disabled".to_string(),
                target_app_state: "not-applicable".to_string(),
                launch_mode: current_launch_mode(),
                executable_path: current_launch_executable(),
                active_operations: Vec::new(),
                degraded_integrations: Vec::new(),
                lifecycle_evidence: None,
            },
            projection: ProjectionSnapshot::default(),
            target_root: None,
            app_registry_root: None,
            runtime_root: None,
            product_paths: None,
        },
        Some(static_root),
    );
    eprintln!("refine: serving website at http://{addr}");
    if once {
        daemon.serve_once(listener)?;
    } else {
        daemon.serve_static(listener)?;
    }
    Ok(())
}
