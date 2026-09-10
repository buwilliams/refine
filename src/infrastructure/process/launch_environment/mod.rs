//! Authoritative environment assembly and launch-size preflight.
//!
//! A process must be preflighted against the same effective environment that is
//! handed to `exec`, after inherited values, owner-specific host-shell
//! projections, process overrides, and removals have all been applied. Keeping
//! that projection as an owned value also lets the ordinary process and PTY
//! launchers share exactly the same precedence and byte accounting.

use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::process::Command;

use portable_pty::CommandBuilder;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::subprocess::ProcessOwner;

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

/// Host-shell values that Quality may use to discover installed toolchains.
///
/// Quality commands are selected by repository configuration, so importing the
/// shell's complete environment would expose unrelated credentials to them.
/// Keep this list limited to executable lookup and vendor-documented tool roots;
/// it deliberately contains no credential or package-source configuration.
pub const QUALITY_SHELL_TOOL_DISCOVERY_ENV: &[&str] = &[
    "PATH",
    "PATHEXT",
    "DOTNET_ROOT",
    "DOTNET_ROOT_X64",
    "DOTNET_ROOT_X86",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveLaunchEnvironment {
    entries: Vec<(OsString, OsString)>,
}

impl EffectiveLaunchEnvironment {
    pub fn assemble(owner: &ProcessOwner, overrides: &[(String, String)]) -> RefineResult<Self> {
        if matches!(owner, ProcessOwner::Agent | ProcessOwner::Quality) {
            Self::assemble_from_sources(
                owner,
                env::vars_os(),
                crate::infrastructure::process::agent_env::login_shell_env(),
                overrides,
            )
        } else {
            Self::assemble_from_sources(owner, env::vars_os(), &BTreeMap::new(), overrides)
        }
    }

    fn assemble_from_sources(
        owner: &ProcessOwner,
        inherited: impl IntoIterator<Item = (OsString, OsString)>,
        shell: &BTreeMap<String, String>,
        overrides: &[(String, String)],
    ) -> RefineResult<Self> {
        let mut entries = BTreeMap::new();
        for (key, value) in inherited {
            insert_checked(&mut entries, key, value)?;
        }
        for (key, value) in shell {
            if *owner == ProcessOwner::Agent
                || (*owner == ProcessOwner::Quality && quality_shell_variable_is_allowed(key))
            {
                insert_checked(&mut entries, key.clone().into(), value.clone().into())?;
            }
        }
        // Repository selection belongs to the command, never the daemon shell. Explicit
        // per-command overrides below still support temporary indexes and merge operations.
        if matches!(
            owner,
            ProcessOwner::Agent | ProcessOwner::Quality | ProcessOwner::Maintenance
        ) {
            entries.retain(|_, (key, _)| !is_git_redirection(key));
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

    #[cfg(test)]
    pub(crate) fn assemble_for_test(
        owner: &ProcessOwner,
        inherited: &[(String, String)],
        shell: &BTreeMap<String, String>,
        overrides: &[(String, String)],
    ) -> RefineResult<Self> {
        Self::assemble_from_sources(
            owner,
            inherited
                .iter()
                .map(|(key, value)| (key.clone().into(), value.clone().into())),
            shell,
            overrides,
        )
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

#[cfg(windows)]
fn quality_shell_variable_is_allowed(key: &str) -> bool {
    QUALITY_SHELL_TOOL_DISCOVERY_ENV
        .iter()
        .any(|allowed| key.eq_ignore_ascii_case(allowed))
}

#[cfg(not(windows))]
fn quality_shell_variable_is_allowed(key: &str) -> bool {
    QUALITY_SHELL_TOOL_DISCOVERY_ENV.contains(&key)
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

fn is_git_redirection(key: &OsStr) -> bool {
    let key = key.to_string_lossy().to_ascii_uppercase();
    matches!(
        key.as_str(),
        "GIT_DIR"
            | "GIT_COMMON_DIR"
            | "GIT_WORK_TREE"
            | "GIT_INDEX_FILE"
            | "GIT_OBJECT_DIRECTORY"
            | "GIT_ALTERNATE_OBJECT_DIRECTORIES"
            | "GIT_CEILING_DIRECTORIES"
            | "GIT_DISCOVERY_ACROSS_FILESYSTEM"
            | "GIT_NAMESPACE"
            | "GIT_PREFIX"
            | "GIT_SHALLOW_FILE"
            | "GIT_CONFIG"
            | "GIT_CONFIG_GLOBAL"
            | "GIT_CONFIG_SYSTEM"
            | "GIT_CONFIG_COUNT"
            | "GIT_CONFIG_PARAMETERS"
    ) || key.starts_with("GIT_CONFIG_KEY_")
        || key.starts_with("GIT_CONFIG_VALUE_")
}

pub(crate) fn remove_inherited_git_environment(command: &mut Command) {
    for (key, _) in env::vars_os().filter(|(key, _)| is_git_redirection(key)) {
        command.env_remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_launches_remove_inherited_git_redirection_but_keep_explicit_indexes() {
        let inherited = [
            ("GIT_DIR".to_string(), "/manual/.git".to_string()),
            ("GIT_WORK_TREE".to_string(), "/manual".to_string()),
            ("GIT_INDEX_FILE".to_string(), "/manual/index".to_string()),
            ("GIT_CONFIG_COUNT".to_string(), "1".to_string()),
            ("GIT_CONFIG_KEY_0".to_string(), "core.worktree".to_string()),
            ("GIT_CONFIG_VALUE_0".to_string(), "/manual".to_string()),
        ];
        let shell = BTreeMap::from(inherited.clone());
        for owner in [
            ProcessOwner::Agent,
            ProcessOwner::Quality,
            ProcessOwner::Maintenance,
        ] {
            let environment = EffectiveLaunchEnvironment::assemble_for_test(
                &owner,
                &inherited,
                &shell,
                &[("GIT_INDEX_FILE".to_string(), "/isolated/index".to_string())],
            )
            .unwrap();
            let mut command = Command::new("sh");
            command.args(["-c", "test -z \"${GIT_DIR+x}${GIT_WORK_TREE+x}${GIT_CONFIG_COUNT+x}${GIT_CONFIG_KEY_0+x}${GIT_CONFIG_VALUE_0+x}\" && test \"$GIT_INDEX_FILE\" = /isolated/index"]);
            environment.apply_to_command(&mut command);
            assert!(command.status().unwrap().success(), "{owner:?}");
        }
    }

    #[test]
    fn quality_shell_projection_is_allowlisted_and_overrides_remain_final() {
        let shell = BTreeMap::from([
            ("PATH".to_string(), "/shell/tools".to_string()),
            ("DOTNET_ROOT".to_string(), "/shell/dotnet".to_string()),
            (
                "OPENAI_API_KEY".to_string(),
                "shell-provider-key".to_string(),
            ),
            (
                "DATABASE_URL".to_string(),
                "shell-database-secret".to_string(),
            ),
        ]);
        let environment = EffectiveLaunchEnvironment::assemble_for_test(
            &ProcessOwner::Quality,
            &[("INHERITED_ONLY".to_string(), "daemon-value".to_string())],
            &shell,
            &[
                ("PATH".to_string(), "/explicit/tools".to_string()),
                ("DOTNET_ROOT".to_string(), "/explicit/dotnet".to_string()),
            ],
        )
        .unwrap();

        assert_eq!(environment.get("PATH"), Some(OsStr::new("/explicit/tools")));
        assert_eq!(
            environment.get("DOTNET_ROOT"),
            Some(OsStr::new("/explicit/dotnet"))
        );
        assert_eq!(
            environment.get("INHERITED_ONLY"),
            Some(OsStr::new("daemon-value"))
        );
        assert_eq!(environment.get("OPENAI_API_KEY"), None);
        assert_eq!(environment.get("DATABASE_URL"), None);
    }

    #[test]
    fn failed_quality_shell_capture_preserves_inherited_and_explicit_environment() {
        let environment = EffectiveLaunchEnvironment::assemble_for_test(
            &ProcessOwner::Quality,
            &[("PATH".to_string(), "/daemon/tools".to_string())],
            &BTreeMap::new(),
            &[("QUALITY_MODE".to_string(), "strict".to_string())],
        )
        .unwrap();

        assert_eq!(environment.get("PATH"), Some(OsStr::new("/daemon/tools")));
        assert_eq!(environment.get("QUALITY_MODE"), Some(OsStr::new("strict")));
    }

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
