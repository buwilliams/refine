use super::*;

pub(super) fn process_command_line(spec: &ManagedProcessSpec) -> String {
    std::iter::once(spec.command.as_str())
        .chain(spec.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn process_details(spec: &ManagedProcessSpec) -> String {
    if spec.sensitive {
        return "redacted".to_string();
    }
    if !spec.metadata.is_empty() {
        let mut details = spec.metadata.clone();
        details
            .entry("command".to_string())
            .or_insert_with(|| json!(process_command_line(spec)));
        return serde_json::to_string(&details).unwrap_or_else(|_| spec.args.join(" "));
    }
    spec.args.join(" ")
}

pub(super) fn process_command(spec: &ManagedProcessSpec) -> Command {
    let mut command = Command::new(&spec.command);
    command.args(&spec.args);
    if let Some(cwd) = spec.cwd.as_deref().filter(|cwd| !cwd.trim().is_empty()) {
        command.current_dir(cwd);
    }
    if spec.owner == ProcessOwner::Agent {
        // The user's configured environment first, so an agent authenticates the
        // same way it would from a terminal. Applied before `spec.env` so refine's
        // own per-process variables still win.
        command.envs(crate::process::agent_env::agent_env_overlay(None));
    }
    command.envs(spec.env.iter().map(|(key, value)| (key, value)));
    if spec.owner == ProcessOwner::Agent {
        for key in AGENT_DIRECT_API_KEY_ENV {
            command.env_remove(key);
        }
    }
    configure_process_lifecycle(&mut command, spec);
    command
}

#[cfg(unix)]
pub(super) fn configure_process_lifecycle(command: &mut Command, spec: &ManagedProcessSpec) {
    use std::os::unix::process::CommandExt;

    unsafe extern "C" {
        fn setsid() -> i32;
    }

    let detached_daemon = spec.owner == ProcessOwner::Daemon;
    let isolated_process_group = spec
        .metadata
        .get("isolated_process_group")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let kill_on_parent_exit = spec
        .limits
        .as_ref()
        .is_some_and(|limits| limits.kill_on_parent_exit);
    if !detached_daemon && !kill_on_parent_exit && !isolated_process_group {
        return;
    }
    unsafe {
        command.pre_exec(move || {
            if (detached_daemon || isolated_process_group) && setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            #[cfg(target_os = "linux")]
            if kill_on_parent_exit {
                const PR_SET_PDEATHSIG: i32 = 1;
                const SIGTERM: i32 = 15;
                unsafe extern "C" {
                    fn prctl(option: i32, arg2: usize, ...) -> i32;
                    fn getppid() -> i32;
                }
                if prctl(PR_SET_PDEATHSIG, SIGTERM as usize) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if getppid() == 1 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "managed process parent exited during launch",
                    ));
                }
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
pub(super) fn configure_process_lifecycle(_command: &mut Command, _spec: &ManagedProcessSpec) {}

const AGENT_DIRECT_API_KEY_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "CLAUDE_API_KEY",
    "CODEX_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "GOOGLE_GENAI_API_KEY",
    "OPENAI_API_KEY",
];

pub(super) fn process_isolation_label(limits: Option<&ProcessResourceLimits>) -> &'static str {
    if limits.is_some() {
        "requested"
    } else {
        "best_effort"
    }
}

pub(super) fn process_actions(state: &str) -> Vec<&'static str> {
    if state == "running" {
        vec!["terminate", "kill"]
    } else {
        vec!["cleanup"]
    }
}

pub(super) fn append_detail(existing: Option<String>, message: &str) -> String {
    match existing {
        Some(existing) if !existing.trim().is_empty() => format!("{existing}; {message}"),
        _ => message.to_string(),
    }
}

pub(super) fn is_stale_process_temp(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if !file_name.starts_with('.') || path.extension().and_then(|ext| ext.to_str()) != Some("tmp") {
        return false;
    }
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age > Duration::from_secs(30))
}
