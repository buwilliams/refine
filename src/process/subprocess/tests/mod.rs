mod output;
mod registry;
mod security;
mod termination;

use super::*;
use std::time::{Duration, Instant};

#[cfg(unix)]
fn shell_binary() -> &'static str {
    if cfg!(windows) { "cmd" } else { "sh" }
}

fn shell_args(script: &str) -> Vec<String> {
    if cfg!(windows) {
        vec!["/C".to_string(), script.to_string()]
    } else {
        vec!["-c".to_string(), script.to_string()]
    }
}

fn long_running_shell_args() -> Vec<String> {
    if cfg!(windows) {
        shell_args("ping -n 30 127.0.0.1 >NUL")
    } else {
        shell_args("sleep 30")
    }
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}
