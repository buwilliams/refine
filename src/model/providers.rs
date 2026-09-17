//! User-owned CLI launch definitions. Templates are argv entries, never shell commands.
use crate::error::{RefineError, RefineResult};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderPromptCapability {
    NativeStdin,
    #[default]
    InlineOrFile,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchMode {
    #[serde(default)]
    pub args: Vec<String>,
    /// Appended only when context is nonempty.
    #[serde(default)]
    pub context_args: Vec<String>,
    /// Appended only when an explicit cwd is available.
    #[serde(default)]
    pub cwd_args: Vec<String>,
    #[serde(default = "yes")]
    pub cwd_on_resume: bool,
    #[serde(default)]
    pub transport: ProviderPromptCapability,
    #[serde(default)]
    pub stdin: Option<String>,
    /// Complete replacement for args when resuming; context_args still apply.
    #[serde(default)]
    pub resume_args: Option<Vec<String>>,
    /// Prefix for interactive sessions with a caller-chosen session ID.
    #[serde(default)]
    pub pin_args: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDefinition {
    pub id: String,
    pub name: String,
    pub executable: String,
    /// Child environment variable -> launching host environment variable (names only).
    #[serde(default)]
    pub credentials: std::collections::BTreeMap<String, String>,
    pub automated: LaunchMode,
    pub interactive: LaunchMode,
    #[serde(default = "plain")]
    pub output_format: String,
}
fn yes() -> bool {
    true
}
fn plain() -> String {
    "plain".into()
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalog {
    pub version: u32,
    pub revision: u64,
    pub default_provider: String,
    pub providers: Vec<ProviderDefinition>,
}

impl ProviderCatalog {
    pub fn provider(&self, id: &str) -> RefineResult<&ProviderDefinition> {
        self.providers.iter().find(|p| p.id == id).ok_or_else(||
            RefineError::InvalidInput(format!("AI provider {id:?} is not configured; add it in Settings > Runtime or select a configured provider")))
    }
    pub fn validate(&self) -> RefineResult<()> {
        let invalid = |s: String| RefineError::InvalidInput(s);
        if self.version != 1 {
            return Err(invalid("unsupported provider catalog version".into()));
        }
        let mut ids = std::collections::BTreeSet::new();
        for p in &self.providers {
            if p.id.trim().is_empty()
                || p.id != p.id.trim()
                || p.id.contains('\0')
                || !ids.insert(&p.id)
            {
                return Err(invalid("provider IDs must be unique, nonempty, and have no surrounding whitespace or NUL".into()));
            }
            if p.name.trim().is_empty()
                || p.executable.trim().is_empty()
                || p.executable.contains('\0')
            {
                return Err(invalid(format!(
                    "provider {} needs a name and executable",
                    p.id
                )));
            }
            if !matches!(
                p.output_format.as_str(),
                "plain" | "claude_json" | "codex_json" | "copilot_json"
            ) {
                return Err(invalid(format!("unsupported output format for {}", p.id)));
            }
            for (target, source) in &p.credentials {
                if !valid_credential_name(target) || !valid_credential_name(source) {
                    return Err(invalid(format!(
                        "provider {} credential references require environment variable names (letters, digits, underscores)",
                        p.id
                    )));
                }
                if target.starts_with("REFINE_")
                    || target.starts_with("GIT_")
                    || matches!(
                        target.as_str(),
                        "PATH" | "HOME" | "LD_PRELOAD" | "LD_LIBRARY_PATH"
                    )
                {
                    return Err(invalid(format!(
                        "provider {} credential target is reserved",
                        p.id
                    )));
                }
            }
            for (interactive, mode) in [(false, &p.automated), (true, &p.interactive)] {
                if interactive && mode.transport == ProviderPromptCapability::NativeStdin {
                    return Err(invalid(
                        "interactive PTY launches require inline_or_file transport".into(),
                    ));
                }
                if (mode.transport == ProviderPromptCapability::NativeStdin) != mode.stdin.is_some()
                {
                    return Err(invalid("native_stdin transport requires a stdin template; inline_or_file cannot have one".into()));
                }
                for template in mode
                    .args
                    .iter()
                    .chain(&mode.context_args)
                    .chain(&mode.cwd_args)
                    .chain(mode.resume_args.iter().flatten())
                    .chain(mode.pin_args.iter().flatten())
                    .chain(mode.stdin.iter())
                {
                    validate_template(template)?;
                }
                for template in mode
                    .args
                    .iter()
                    .chain(&mode.context_args)
                    .chain(&mode.cwd_args)
                    .chain(mode.stdin.iter())
                {
                    if template.contains("{{session_id}}") {
                        return Err(invalid(
                            "session_id is only available in resume_args and pin_args".into(),
                        ));
                    }
                }
                if mode.transport == ProviderPromptCapability::NativeStdin
                    && mode
                        .args
                        .iter()
                        .chain(&mode.context_args)
                        .chain(&mode.cwd_args)
                        .chain(mode.resume_args.iter().flatten())
                        .chain(mode.pin_args.iter().flatten())
                        .any(|a| a.contains("{{context}}"))
                {
                    return Err(invalid(
                        "native_stdin context belongs in the stdin template".into(),
                    ));
                }
            }
        }
        self.provider(&self.default_provider)?;
        Ok(())
    }
}

pub fn validate_template(template: &str) -> RefineResult<()> {
    if template.contains('\0') {
        return Err(RefineError::InvalidInput(
            "provider template contains NUL".into(),
        ));
    }
    let mut tail = template;
    while let Some((_, rest)) = tail.split_once("{{") {
        let Some((name, rest)) = rest.split_once("}}") else {
            return Err(RefineError::InvalidInput(
                "unclosed provider template variable".into(),
            ));
        };
        if !matches!(name, "context" | "cwd" | "session_id") {
            return Err(RefineError::InvalidInput(format!(
                "unsupported provider variable {{{{{name}}}}}"
            )));
        }
        tail = rest;
    }
    Ok(())
}

/// Scan the original template once: context containing template syntax stays literal.
pub fn expand(template: &str, context: &str, cwd: &str, session_id: &str) -> String {
    let mut result = String::new();
    let mut tail = template;
    while let Some((prefix, rest)) = tail.split_once("{{") {
        let Some((name, rest)) = rest.split_once("}}") else {
            break;
        };
        result.push_str(prefix);
        result.push_str(match name {
            "context" => context,
            "cwd" => cwd,
            "session_id" => session_id,
            _ => unreachable!("validated template"),
        });
        tail = rest;
    }
    result.push_str(tail);
    result
}

impl ProviderDefinition {
    pub fn generic(id: &str) -> Self {
        let mode = LaunchMode {
            context_args: vec!["{{context}}".into()],
            ..Default::default()
        };
        Self {
            id: id.into(),
            name: id.into(),
            executable: id.into(),
            credentials: Default::default(),
            automated: mode.clone(),
            interactive: mode,
            output_format: plain(),
        }
    }
}

pub fn defaults() -> ProviderCatalog {
    serde_json::from_str(include_str!("providers_defaults.json"))
        .expect("valid built-in provider catalog")
}

fn valid_credential_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .enumerate()
            .all(|(i, c)| c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
}
