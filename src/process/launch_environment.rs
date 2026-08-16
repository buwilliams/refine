//! Authoritative environment assembly and launch-size preflight.
//!
//! A process must be preflighted against the same effective environment that is
//! handed to `exec`, after inherited values, agent configuration, process
//! overrides, and removals have all been applied. Keeping that projection as an
//! owned value also lets the ordinary process and PTY launchers share exactly
//! the same precedence and byte accounting.

use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::process::Command;

use portable_pty::CommandBuilder;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

use crate::process::subprocess::ProcessOwner;
use crate::process::supervisor::errors::{RefineError, RefineResult};

const FALLBACK_PER_STRING_LIMIT: usize = 128 * 1024;
const FALLBACK_ARG_MAX: usize = 256 * 1024;
const ARG_MAX_MARGIN_MIN: usize = 32 * 1024;

pub const AGENT_DIRECT_API_KEY_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "CLAUDE_API_KEY",
    "CODEX_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "GOOGLE_GENAI_API_KEY",
    "OPENAI_API_KEY",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveLaunchEnvironment {
    entries: Vec<(OsString, OsString)>,
}

impl EffectiveLaunchEnvironment {
    pub fn assemble(owner: &ProcessOwner, overrides: &[(String, String)]) -> RefineResult<Self> {
        let mut entries = BTreeMap::new();
        for (key, value) in env::vars_os() {
            insert_checked(&mut entries, key, value)?;
        }
        if *owner == ProcessOwner::Agent {
            for (key, value) in crate::process::agent_env::login_shell_env() {
                insert_checked(&mut entries, key.clone().into(), value.clone().into())?;
            }
        }
        for (key, value) in overrides {
            insert_checked(&mut entries, key.into(), value.into())?;
        }
        if *owner == ProcessOwner::Agent {
            for key in AGENT_DIRECT_API_KEY_ENV {
                entries.remove(&environment_map_key(OsStr::new(key)));
            }
        }
        let entries = entries.into_values().collect::<Vec<_>>();
        validate_environment(&entries)?;
        Ok(Self { entries })
    }

    pub fn apply_to_command(&self, command: &mut Command) {
        command.env_clear();
        command.envs(self.entries.iter().map(|(key, value)| (key, value)));
    }

    pub fn apply_to_pty(&self, command: &mut CommandBuilder) {
        command.env_clear();
        for (key, value) in &self.entries {
            command.env(key, value);
        }
    }

    pub fn launch_fits(&self, binary: &str, args: &[String]) -> RefineResult<bool> {
        reject_nul("provider executable", OsStr::new(binary))?;
        let mut argv_bytes = os_len(OsStr::new(binary)).saturating_add(1);
        if argv_bytes >= platform_per_string_limit() {
            return Ok(false);
        }
        for arg in args {
            reject_nul("provider argv element", OsStr::new(arg))?;
            let bytes = arg.len().saturating_add(1);
            if bytes >= platform_per_string_limit() {
                return Ok(false);
            }
            argv_bytes = argv_bytes.saturating_add(bytes);
        }
        if self
            .entries
            .iter()
            .any(|(key, value)| environment_entry_bytes(key, value) >= platform_per_string_limit())
        {
            return Ok(false);
        }
        let arg_max = platform_arg_max();
        let margin = (arg_max / 4).max(ARG_MAX_MARGIN_MIN);
        Ok(argv_bytes
            .saturating_add(self.byte_len())
            .saturating_add(margin)
            < arg_max)
    }

    pub fn validate_launch(&self, binary: &str, args: &[String]) -> RefineResult<()> {
        if self.launch_fits(binary, args)? {
            Ok(())
        } else {
            Err(RefineError::Degraded(
                "agent launch exceeds the safe aggregate argv and effective-environment budget before spawn"
                    .to_string(),
            ))
        }
    }

    pub fn byte_len(&self) -> usize {
        self.entries
            .iter()
            .map(|(key, value)| environment_entry_bytes(key, value))
            .sum()
    }

    pub fn entries(&self) -> &[(OsString, OsString)] {
        &self.entries
    }

    pub fn get(&self, key: &str) -> Option<&OsStr> {
        self.entries
            .iter()
            .find_map(|(candidate, value)| (candidate == key).then_some(value.as_os_str()))
    }
}

fn insert_checked(
    entries: &mut BTreeMap<OsString, (OsString, OsString)>,
    key: OsString,
    value: OsString,
) -> RefineResult<()> {
    validate_environment_entry(&key, &value)?;
    entries.insert(environment_map_key(&key), (key, value));
    Ok(())
}

#[cfg(windows)]
fn environment_map_key(key: &OsStr) -> OsString {
    key.to_string_lossy().to_uppercase().into()
}

#[cfg(not(windows))]
fn environment_map_key(key: &OsStr) -> OsString {
    key.to_os_string()
}

fn validate_environment(entries: &[(OsString, OsString)]) -> RefineResult<()> {
    for (key, value) in entries {
        validate_environment_entry(key, value)?;
    }
    Ok(())
}

fn validate_environment_entry(key: &OsStr, value: &OsStr) -> RefineResult<()> {
    reject_nul("provider environment key", key)?;
    reject_nul("provider environment value", value)?;
    if os_bytes(key).contains(&b'=') {
        return Err(RefineError::InvalidInput(
            "provider environment key cannot contain '='".to_string(),
        ));
    }
    Ok(())
}

fn reject_nul(label: &str, value: &OsStr) -> RefineResult<()> {
    if os_bytes(value).contains(&0) {
        Err(RefineError::InvalidInput(format!(
            "{label} contains a NUL byte and cannot be launched"
        )))
    } else {
        Ok(())
    }
}

fn environment_entry_bytes(key: &OsStr, value: &OsStr) -> usize {
    os_len(key).saturating_add(os_len(value)).saturating_add(2)
}

fn platform_per_string_limit() -> usize {
    #[cfg(target_os = "linux")]
    {
        FALLBACK_PER_STRING_LIMIT
    }
    #[cfg(not(target_os = "linux"))]
    {
        platform_arg_max()
    }
}

fn platform_arg_max() -> usize {
    #[cfg(unix)]
    {
        let discovered = unsafe { libc::sysconf(libc::_SC_ARG_MAX) };
        if discovered > 0 {
            return usize::try_from(discovered).unwrap_or(FALLBACK_ARG_MAX);
        }
    }
    FALLBACK_ARG_MAX
}

#[cfg(unix)]
fn os_bytes(value: &OsStr) -> &[u8] {
    value.as_bytes()
}

#[cfg(not(unix))]
fn os_bytes(value: &OsStr) -> &[u8] {
    value.to_str().unwrap_or_default().as_bytes()
}

#[cfg(unix)]
fn os_len(value: &OsStr) -> usize {
    value.as_bytes().len()
}

#[cfg(not(unix))]
fn os_len(value: &OsStr) -> usize {
    value.to_string_lossy().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_are_counted_once_and_agent_removals_are_final() {
        let inherited_path = env::var_os("PATH");
        let environment = EffectiveLaunchEnvironment::assemble(
            &ProcessOwner::Agent,
            &[
                ("PATH".to_string(), "final-path".to_string()),
                ("PATH".to_string(), "final-path".to_string()),
                ("OPENAI_API_KEY".to_string(), "must-not-survive".to_string()),
                ("REFINE_TEST".to_string(), "🙂é".to_string()),
            ],
        )
        .unwrap();

        assert_eq!(environment.get("PATH"), Some(OsStr::new("final-path")));
        assert_eq!(environment.get("REFINE_TEST"), Some(OsStr::new("🙂é")));
        assert_eq!(environment.get("OPENAI_API_KEY"), None);
        assert_eq!(
            environment
                .entries()
                .iter()
                .filter(|(key, _)| key == "PATH")
                .count(),
            1
        );
        assert_ne!(inherited_path, Some(OsString::from("final-path")));
    }

    #[test]
    fn rejects_nul_and_invalid_keys_before_launch() {
        let nul = EffectiveLaunchEnvironment::assemble(
            &ProcessOwner::Agent,
            &[("REFINE_TEST".to_string(), "bad\0value".to_string())],
        )
        .unwrap_err();
        assert!(nul.to_string().contains("NUL"));

        let invalid = EffectiveLaunchEnvironment::assemble(
            &ProcessOwner::Agent,
            &[("BAD=KEY".to_string(), "value".to_string())],
        )
        .unwrap_err();
        assert!(invalid.to_string().contains("cannot contain '='"));
    }
}
