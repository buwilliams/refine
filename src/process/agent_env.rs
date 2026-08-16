//! Environment refine hands to the agent CLIs it spawns.
//!
//! An agent launched from a terminal authenticates because the shell sourced the
//! user's rc files first. A `systemd --user` daemon never sources them, so a
//! daemon-spawned agent inherits an environment with none of that configuration
//! and falls back to an auth scheme it has no credentials for — reporting
//! "Not logged in" while still occupying a concurrency slot.
//!
//! Refine therefore acquires that configuration itself rather than asking the
//! user to declare it a second time: it reads the environment the user's shell
//! produces from both startup conventions. Refine deliberately owns no
//! configuration files outside the synchronized project state and the
//! node-local `run/` tree, so the host shell is the only outside source.
//! Precedence, lowest to highest:
//!
//! 1. the environment the daemon itself inherited
//! 2. the user's login-shell environment (this module)
//! 3. per-process variables refine sets itself, which always win
//!
//! Nothing here can fail a spawn. Every failure path yields an empty map, which
//! leaves behaviour exactly as it was before this module existed.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::Duration;

/// How long the login shell gets to print its environment. A user's rc files are
/// arbitrary code; a daemon must not hang on first agent spawn because one of
/// them blocks.
const SHELL_CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);

/// Variables that describe the capturing shell rather than the user's
/// configuration. Importing them would leak a transient shell's state into every
/// agent.
const SHELL_LOCAL_ENV: &[&str] = &[
    "_",
    "BASH_EXECUTION_STRING",
    "OLDPWD",
    "PWD",
    "SHLVL",
    "TERM",
];

static LOGIN_SHELL_ENV: OnceLock<BTreeMap<String, String>> = OnceLock::new();

/// The environment a login shell produces, captured once per daemon lifetime.
///
/// Cached because it costs a process spawn, which means edits to a user's rc
/// files are picked up on daemon restart rather than immediately.
pub fn login_shell_env() -> &'static BTreeMap<String, String> {
    LOGIN_SHELL_ENV.get_or_init(capture_login_shell_env)
}

fn capture_login_shell_env() -> BTreeMap<String, String> {
    // The capture runs on its own thread so a blocking rc file delays this call
    // by the timeout instead of hanging the daemon.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run_login_shell_env());
    });
    rx.recv_timeout(SHELL_CAPTURE_TIMEOUT).unwrap_or_default()
}

fn run_login_shell_env() -> BTreeMap<String, String> {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.trim().is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string());
    shell_env(&shell, None)
}

/// Both shell startup conventions, merged.
///
/// Two invocations are required, and one is not a superset of the other. An
/// interactive login shell (`-lic`) sources the profile — `.profile`,
/// `.bash_profile`, `.zprofile` — while an interactive non-login shell (`-ic`)
/// sources the rc file — `.bashrc`, `.zshrc`. A user who exports agent auth from
/// `.bashrc`, which is the common case and the one reported, is invisible to the
/// login shell unless their profile happens to source the rc file.
///
/// The rc pass is applied second: where a variable is set in both, the rc file is
/// what an interactive terminal ends up with.
fn shell_env(shell: &str, home: Option<&Path>) -> BTreeMap<String, String> {
    let mut merged = capture_from_shell(shell, "-lic", home);
    merged.extend(capture_from_shell(shell, "-ic", home));
    merged
}

fn capture_from_shell(shell: &str, flags: &str, home: Option<&Path>) -> BTreeMap<String, String> {
    // `env -0` keeps values containing newlines intact. stdin is closed so an rc
    // file that reads input cannot block, and stderr is dropped because rc files
    // routinely print to it.
    //
    // The sentinel is not optional. A login shell prints to *stdout* as well —
    // Ubuntu's MOTD and sudo hint, rc files that echo — and that text arrives
    // before the environment. Without a marker to cut at, the leading noise is
    // read as the first NUL-delimited entry, and because it usually contains an
    // `=` it parses into a junk variable that swallows the real one after it.
    let mut command = Command::new(shell);
    command
        .args([flags, &format!("printf '%s\\0' {ENV_SENTINEL}; env -0")])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    configure_shell_capture_lifecycle(&mut command);
    if let Some(home) = home {
        command.env("HOME", home);
    }
    let Ok(output) = command.output() else {
        return BTreeMap::new();
    };
    if !output.status.success() {
        return BTreeMap::new();
    }
    parse_nul_env(&output.stdout)
}

/// Interactive shells initialize terminal job control even when stdin is
/// closed. If Refine itself was launched from a terminal, that initialization
/// can stop the entire updater process group. A new session gives the capture
/// no controlling terminal, while still allowing the shell to source the same
/// profile and rc files.
#[cfg(unix)]
fn configure_shell_capture_lifecycle(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_shell_capture_lifecycle(_command: &mut Command) {}

/// Marks where the environment dump begins, so shell startup chatter on stdout is
/// not mistaken for it.
const ENV_SENTINEL: &str = "__refine_env_begin__";

fn find_sentinel(stdout: &[u8]) -> Option<usize> {
    let marker = ENV_SENTINEL.as_bytes();
    stdout
        .windows(marker.len())
        .position(|window| window == marker)
        .map(|offset| offset + marker.len())
}

/// A POSIX environment variable name. Rejects anything a shell printed that only
/// happens to contain an `=`, so junk can never reach a spawned agent.
fn is_env_name(key: &str) -> bool {
    !key.is_empty()
        && !key.starts_with(|ch: char| ch.is_ascii_digit())
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn parse_nul_env(stdout: &[u8]) -> BTreeMap<String, String> {
    let mut captured = BTreeMap::new();
    // Everything before the sentinel is shell startup output, not environment.
    let body = match find_sentinel(stdout) {
        Some(offset) => &stdout[offset..],
        None => stdout,
    };
    for entry in body.split(|byte| *byte == 0) {
        if entry.is_empty() {
            continue;
        }
        let Ok(entry) = std::str::from_utf8(entry) else {
            continue;
        };
        let Some((key, value)) = entry.split_once('=') else {
            continue;
        };
        if !is_env_name(key) || SHELL_LOCAL_ENV.contains(&key) {
            continue;
        }
        captured.insert(key.to_string(), value.to_string());
    }
    captured
}

/// Signals that an agent CLI failed because it had no usable credentials, rather
/// than because the work itself failed. Matched case-insensitively.
const AUTH_FAILURE_SIGNALS: &[&str] = &[
    "not logged in",
    "please run /login",
    "invalid api key",
    "authentication_error",
    "authentication failed",
    "unauthorized",
    "no credentials found",
    "oauth token has expired",
];

/// An actionable explanation when an agent exited because it could not
/// authenticate.
///
/// Only consulted for an agent that already failed, so a false positive changes
/// the wording of an existing error rather than inventing one. Worth that risk:
/// the alternative is the reported experience, where a total auth failure looks
/// like an opaque non-zero exit and the agent still consumed a concurrency slot.
pub fn auth_failure_hint(output: &str) -> Option<String> {
    let haystack = output.to_ascii_lowercase();
    if !AUTH_FAILURE_SIGNALS
        .iter()
        .any(|signal| haystack.contains(signal))
    {
        return None;
    }
    Some(
        "the agent CLI is not authenticated. Refine imports the environment your \
         shell exports, but a daemon does not read your shell startup files while \
         it is already running: export the variables your CLI needs from your \
         shell startup files, then restart the daemon"
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nul_separated_environment_keeps_multi_line_values_and_drops_shell_locals() {
        let captured = parse_nul_env(
            b"CLAUDE_CODE_USE_FOUNDRY=1\0PWD=/home/user\0SHLVL=1\0MULTI=one\ntwo\0_=/usr/bin/env\0",
        );

        assert_eq!(captured.get("CLAUDE_CODE_USE_FOUNDRY").unwrap(), "1");
        assert_eq!(captured.get("MULTI").unwrap(), "one\ntwo");
        // Describing the capturing shell, not the user's configuration.
        assert!(!captured.contains_key("PWD"));
        assert!(!captured.contains_key("SHLVL"));
        assert!(!captured.contains_key("_"));
    }

    // Runs a real shell against a synthetic HOME, so it does not depend on how the
    // machine running the suite is configured. This is the case that matters: the
    // reported host exports agent auth from `.bashrc`, behind the standard
    // non-interactive early return, which is invisible to a daemon and also
    // invisible to an interactive *login* shell.
    // A login shell writes to stdout before the environment arrives: Ubuntu's MOTD
    // and sudo hint, or any rc file that echoes. That text contains an `=` often
    // enough that, without a sentinel to cut at, it parsed into a junk variable and
    // swallowed the real variable that followed it — which would then be exported
    // into every spawned agent.
    #[test]
    fn shell_startup_output_on_stdout_cannot_become_an_environment_variable() {
        let noisy = b"To run a command as administrator (user \"root\"), use \"sudo <command>\".\n\
                      See \"man sudo_root\" for details.\n\n\
                      __refine_env_begin__\0SHELL=/bin/bash\0CLAUDE_CODE_USE_FOUNDRY=1\0";

        let captured = parse_nul_env(noisy);

        assert_eq!(captured.get("SHELL").map(String::as_str), Some("/bin/bash"));
        assert_eq!(
            captured.get("CLAUDE_CODE_USE_FOUNDRY").map(String::as_str),
            Some("1")
        );
        assert_eq!(captured.len(), 2, "no junk keys: {captured:?}");
        assert!(captured.keys().all(|key| is_env_name(key)));
    }

    #[test]
    fn an_auth_failure_is_explained_and_other_failures_are_left_alone() {
        let hint = auth_failure_hint("Not logged in · Please run /login").unwrap();
        assert!(hint.contains("not authenticated"), "{hint}");
        assert!(hint.contains("restart the daemon"), "{hint}");
        // Case and surrounding output do not matter.
        assert!(auth_failure_hint("...\nERROR: Invalid API key\n").is_some());
        assert!(auth_failure_hint("HTTP 401 Unauthorized").is_some());
        // A genuine work failure must not be relabelled as an auth problem.
        assert!(auth_failure_hint("error[E0308]: mismatched types").is_none());
        assert!(auth_failure_hint("").is_none());
    }

    #[test]
    fn only_valid_environment_names_are_imported() {
        assert!(is_env_name("CLAUDE_CODE_USE_FOUNDRY"));
        assert!(is_env_name("_UNDERSCORE_LEAD"));
        assert!(!is_env_name(""));
        assert!(!is_env_name("2LEADING_DIGIT"));
        assert!(!is_env_name("has space"));
        assert!(!is_env_name("has-dash"));
        assert!(!is_env_name("See \"man sudo_root\" for details.\n\nSHELL"));
    }

    #[test]
    fn a_real_shell_capture_reads_both_the_profile_and_the_rc_file() {
        if !Path::new("/bin/bash").exists() {
            return;
        }
        let home = std::env::temp_dir().join(format!(
            "refine-agent-env-shell-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join(".bashrc"),
            "case $- in *i*) ;; *) return;; esac\n\
             export CLAUDE_CODE_USE_FOUNDRY=1\n\
             export SHARED_BY_BOTH=from_rc\n",
        )
        .unwrap();
        std::fs::write(
            home.join(".profile"),
            "export PROFILE_ONLY=from_profile\nexport SHARED_BY_BOTH=from_profile\n",
        )
        .unwrap();

        let captured = shell_env("/bin/bash", Some(&home));

        // The rc file, which only an interactive non-login shell sources.
        assert_eq!(
            captured.get("CLAUDE_CODE_USE_FOUNDRY").map(String::as_str),
            Some("1"),
            "rc-file exports must be captured; captured {captured:?}"
        );
        // The profile, which only an interactive login shell sources.
        assert_eq!(
            captured.get("PROFILE_ONLY").map(String::as_str),
            Some("from_profile"),
            "profile exports must be captured; captured {captured:?}"
        );
        // Set by both: the rc file is what an interactive terminal ends up with.
        assert_eq!(
            captured.get("SHARED_BY_BOTH").map(String::as_str),
            Some("from_rc")
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn shell_capture_runs_outside_the_callers_terminal_session() {
        if !Path::new("/bin/bash").exists() {
            return;
        }
        let home = std::env::temp_dir().join(format!(
            "refine-agent-env-session-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join(".bashrc"),
            "export REFINE_CAPTURE_SESSION_ID=\"$(ps -o sid= -p $$ | tr -d ' ')\"\n",
        )
        .unwrap();
        std::fs::write(home.join(".profile"), "\n").unwrap();

        let captured = shell_env("/bin/bash", Some(&home));
        let parent_session = unsafe { libc::getsid(0) }.to_string();
        let capture_session = captured
            .get("REFINE_CAPTURE_SESSION_ID")
            .filter(|session| !session.is_empty())
            .expect("the fixture must observe the capture shell session");

        assert_ne!(
            capture_session, &parent_session,
            "the capture shell must not share Refine's controlling-terminal session"
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_failed_capture_yields_an_empty_overlay_rather_than_an_error() {
        // Nothing in this module returns Result: a spawn must never fail because
        // the environment could not be read.
        assert!(parse_nul_env(b"").is_empty());
    }
}
