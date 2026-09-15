use std::env;
use std::path::{Path, PathBuf};

use crate::application::agent_io::prompt_transport::PromptTransportMetadata;
use crate::infrastructure::agents::invocation::{
    ProviderPromptCapability, ProviderSessionContinuity,
};

use crate::model::providers::{LaunchMode, ProviderDefinition, expand};

#[derive(Clone, Debug)]
pub(crate) struct ProviderSpec {
    pub(crate) name: String,
    pub(crate) display_name: String,
    pub(crate) binary: String,
    pub(crate) output_format: String,
    pub(crate) supports_resume: bool,
    pub(crate) supports_direct_api: bool,
    pub(crate) definition: ProviderDefinition,
}
impl From<ProviderDefinition> for ProviderSpec {
    fn from(definition: ProviderDefinition) -> Self {
        Self {
            name: definition.id.clone(),
            display_name: definition.name.clone(),
            binary: definition.executable.clone(),
            output_format: definition.output_format.clone(),
            supports_resume: definition.automated.resume_args.is_some(),
            supports_direct_api: false,
            definition,
        }
    }
}
impl ProviderSpec {
    pub(crate) fn supports_interactive_session_continuity(&self) -> bool {
        self.definition.interactive.pin_args.is_some()
            && self.definition.interactive.resume_args.is_some()
    }
    pub(crate) fn interactive_args(
        &self,
        prompt: &str,
        session: Option<&ProviderSessionContinuity>,
    ) -> Vec<String> {
        let (pin, resume) = match session {
            Some(ProviderSessionContinuity::Pin(id)) => (Some(id.as_str()), None),
            Some(ProviderSessionContinuity::Resume(id)) => (None, Some(id.as_str())),
            None => (None, None),
        };
        render_args(&self.definition.interactive, prompt, None, pin, resume)
    }
    pub(crate) fn chat_args(
        &self,
        binary: &str,
        prompt: &str,
        session: Option<&str>,
        cwd: Option<&Path>,
    ) -> Vec<String> {
        std::iter::once(binary.to_string())
            .chain(render_args(
                &self.definition.automated,
                prompt,
                cwd,
                None,
                session,
            ))
            .collect()
    }
    pub(crate) fn interactive_prompt_capability(&self) -> ProviderPromptCapability {
        self.definition.interactive.transport.clone()
    }
    pub(crate) fn noninteractive_prompt_capability(&self) -> ProviderPromptCapability {
        self.definition.automated.transport.clone()
    }
}
fn render_args(
    mode: &LaunchMode,
    context: &str,
    cwd: Option<&Path>,
    pin: Option<&str>,
    resume: Option<&str>,
) -> Vec<String> {
    let cwd_text = cwd
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let render = |a: &String| expand(a, context, &cwd_text, resume.or(pin).unwrap_or_default());
    let mut args = Vec::new();
    if pin.is_some() {
        args.extend(mode.pin_args.iter().flatten().map(&render));
    }
    let base = resume.and(mode.resume_args.as_ref()).unwrap_or(&mode.args);
    args.extend(base.iter().map(&render));
    // Codex resume intentionally omits cwd flags; other modes can retain them.
    if cwd.is_some() && (resume.is_none() || mode.cwd_on_resume) {
        args.extend(mode.cwd_args.iter().map(&render));
    }
    if !context.is_empty() {
        args.extend(mode.context_args.iter().map(&render));
    }
    args
}

pub(crate) fn safe_authorization_command(
    binary: &str,
    interactive: bool,
    transport: &PromptTransportMetadata,
) -> String {
    let mode = if interactive { "interactive" } else { "invoke" };
    format!(
        "{} {mode} [refine-managed-prompt kind={:?} bytes={} sha256={}]",
        binary, transport.kind, transport.utf8_bytes, transport.sha256
    )
}

pub(crate) fn find_executable(binary: &str, path_override: Option<&str>) -> Option<PathBuf> {
    let candidate = Path::new(binary);
    if candidate.components().count() > 1 {
        return executable_file(candidate).then(|| candidate.to_path_buf());
    }
    let path = path_override
        .map(str::to_string)
        .or_else(|| env::var("PATH").ok())
        .unwrap_or_default();
    env::split_paths(&path)
        .chain(user_executable_dirs(path_override))
        .map(|dir| dir.join(binary))
        .find(|path| executable_file(path))
}

fn user_executable_dirs(path_override: Option<&str>) -> Vec<PathBuf> {
    if path_override.is_some() {
        return Vec::new();
    }
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    [
        home.join(".local/bin"),
        home.join(".npm-global/bin"),
        home.join(".cargo/bin"),
    ]
    .into_iter()
    .collect()
}

fn executable_file(path: &Path) -> bool {
    path.is_file()
}
