use super::*;

impl FileTargetAppService {
    pub fn generate_config(&self) -> RefineResult<TargetAppGeneratedConfig> {
        let settings = self.settings()?;
        let mut config = TargetAppGeneratedConfig {
            start_instructions: setting(&settings, "target_app_start_instructions"),
            stop_instructions: setting(&settings, "target_app_stop_instructions"),
            build_instructions: first_nonempty(&[
                setting(&settings, "target_app_build_instructions"),
                setting(&settings, "target_app_rebuild_instructions"),
            ]),
            start_command: setting(&settings, "target_app_start_command"),
            stop_command: setting(&settings, "target_app_stop_command"),
            build_command: setting(&settings, "target_app_build_command"),
            test_command: setting(&settings, "target_app_test_command"),
            status_command: setting(&settings, "target_app_status_command"),
            cwd: setting(&settings, "target_app_cwd"),
            env: serde_json::Map::new(),
            start_timeout_seconds: setting(&settings, "target_app_start_timeout_seconds")
                .parse()
                .unwrap_or(120),
            stop_timeout_seconds: setting(&settings, "target_app_stop_timeout_seconds")
                .parse()
                .unwrap_or(60),
            build_timeout_seconds: setting(&settings, "target_app_build_timeout_seconds")
                .parse()
                .unwrap_or(300),
            test_timeout_seconds: setting(&settings, "target_app_test_timeout_seconds")
                .parse()
                .unwrap_or(600),
            status_timeout_seconds: setting(&settings, "target_app_status_timeout_seconds")
                .parse()
                .unwrap_or(10),
            log_path: setting(&settings, "target_app_log_path"),
            http_check_url: first_nonempty(&[
                setting(&settings, "target_app_http_check_url"),
                setting(&settings, "target_app_health_url"),
                setting(&settings, "target_app_url"),
            ]),
            tcp_check_host: setting(&settings, "target_app_tcp_check_host"),
            tcp_check_port: setting(&settings, "target_app_tcp_check_port"),
            process_check_command: setting(&settings, "target_app_process_check_command"),
            notes: String::new(),
        };
        config.env = serde_json::from_str::<Value>(&setting(&settings, "target_app_env_json"))
            .ok()
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();

        let mut notes = Vec::new();
        if clear_generated_wrapper_entrypoints(&mut config) {
            notes.push(
                "Ignored existing manage-app wrapper entrypoints while regenerating lifecycle instructions."
                    .to_string(),
            );
        }
        let project_root = self.command_cwd(&settings);
        if project_root.join("package.json").exists() {
            apply_package_json_defaults(&project_root, &mut config)?;
            notes.push(
                "Detected package.json and generated npm-compatible lifecycle instructions."
                    .to_string(),
            );
        } else if project_root.join("Cargo.toml").exists() {
            fill_if_empty(&mut config.start_command, "cargo run");
            fill_if_empty(&mut config.build_command, "cargo build");
            fill_if_empty(&mut config.test_command, "cargo test");
            fill_if_empty(&mut config.status_command, "cargo check --quiet");
            notes.push(
                "Detected Cargo.toml and generated cargo lifecycle instructions.".to_string(),
            );
        } else if project_root.join("Makefile").exists() || project_root.join("makefile").exists() {
            let makefile = if project_root.join("Makefile").exists() {
                project_root.join("Makefile")
            } else {
                project_root.join("makefile")
            };
            apply_makefile_defaults(&makefile, &mut config)?;
            notes.push(
                "Detected Makefile targets and generated make lifecycle instructions.".to_string(),
            );
        } else {
            notes.push("No package.json, Cargo.toml, or Makefile was detected; preserved existing target-app settings.".to_string());
        }

        if config.status_command.trim().is_empty() && !config.http_check_url.trim().is_empty() {
            config.status_command = format!(
                "curl -fsS {} >/dev/null",
                shell_quote(&config.http_check_url)
            );
        }
        if config.tcp_check_port.trim().is_empty()
            && let Some(port) = port_from_url(&config.http_check_url)
        {
            config.tcp_check_host = "127.0.0.1".to_string();
            config.tcp_check_port = port.to_string();
        }
        if config.stop_command.trim().is_empty() && !config.tcp_check_port.trim().is_empty() {
            config.stop_command = format!(
                "sh -c 'lsof -ti tcp:{} | xargs -r kill'",
                config.tcp_check_port
            );
            notes.push("Generated stop instruction targets the configured TCP port.".to_string());
        }
        apply_static_web_server_defaults(&project_root, &mut config, &mut notes);
        convert_lifecycle_commands_to_instructions(&mut config);
        config.notes = notes.join(" ");
        Ok(config)
    }

    pub fn write_manage_app_wrapper(
        &self,
        config: &mut TargetAppGeneratedConfig,
    ) -> RefineResult<()> {
        let wrapper_dir = self.refine_dir.clone();
        fs::create_dir_all(&wrapper_dir).map_err(|error| {
            RefineError::Io(format!(
                "failed to create target-app wrapper directory {}: {error}",
                wrapper_dir.display()
            ))
        })?;

        if config.log_path.trim().is_empty() {
            config.log_path = MANAGE_APP_LOG_PATH.to_string();
        }
        let mut notes = Vec::new();
        if clear_generated_wrapper_entrypoints(config) {
            notes.push(
                "Ignored generated manage-app wrapper entrypoints before writing the wrapper."
                    .to_string(),
            );
        }
        let project_root = config_project_root(&self.target_root, &config.cwd);
        apply_static_web_server_defaults(&project_root, config, &mut notes);
        for note in notes {
            append_note(&mut config.notes, &note);
        }

        let wrapper_path = wrapper_dir.join("manage-app.sh");
        let script = manage_app_wrapper_script(config);
        fs::write(&wrapper_path, script).map_err(|error| {
            RefineError::Io(format!(
                "failed to write target-app wrapper {}: {error}",
                wrapper_path.display()
            ))
        })?;
        make_executable(&wrapper_path)?;

        config.start_command = manage_app_wrapper_entrypoint("start");
        config.stop_command = manage_app_wrapper_entrypoint("stop");
        config.build_command = manage_app_wrapper_entrypoint("build");
        config.test_command = manage_app_wrapper_entrypoint("test");
        config.status_command = manage_app_wrapper_entrypoint("status");
        config.cwd = ".".to_string();
        append_note(
            &mut config.notes,
            "Wrote the managed target-app wrapper outside the application worktree and pointed target-app commands at it.",
        );
        Ok(())
    }
}
