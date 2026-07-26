use super::*;

#[derive(Clone, Debug)]
pub(super) struct ProviderSpec {
    pub(super) name: &'static str,
    pub(super) display_name: &'static str,
    pub(super) binary: &'static str,
    pub(super) output_format: &'static str,
    pub(super) supports_resume: bool,
    pub(super) supports_direct_api: bool,
}

impl ProviderSpec {
    pub(super) fn new(
        name: &'static str,
        display_name: &'static str,
        binary: &'static str,
        output_format: &'static str,
        supports_resume: bool,
        supports_direct_api: bool,
    ) -> Self {
        Self {
            name,
            display_name,
            binary,
            output_format,
            supports_resume,
            supports_direct_api,
        }
    }

    pub(super) fn agent_args(
        &self,
        binary_path: &str,
        prompt: &str,
        cwd: Option<&Path>,
    ) -> Vec<String> {
        match self.name {
            "claude" => vec![
                binary_path.to_string(),
                "--print".to_string(),
                "--output-format=stream-json".to_string(),
                "--verbose".to_string(),
                "--dangerously-skip-permissions".to_string(),
                prompt.to_string(),
            ],
            "codex" => {
                let mut args = vec![
                    binary_path.to_string(),
                    "exec".to_string(),
                    "--dangerously-bypass-approvals-and-sandbox".to_string(),
                    "--color".to_string(),
                    "never".to_string(),
                    "--json".to_string(),
                ];
                if let Some(cwd) = cwd {
                    args.extend(["-C".to_string(), cwd.display().to_string()]);
                }
                if !prompt.is_empty() {
                    args.push("-".to_string());
                }
                args
            }
            "gemini" => vec![
                binary_path.to_string(),
                "--yolo".to_string(),
                "-p".to_string(),
                prompt.to_string(),
            ],
            "copilot" => {
                let mut args = vec![
                    binary_path.to_string(),
                    "--allow-all".to_string(),
                    "--output-format".to_string(),
                    "json".to_string(),
                    "--no-color".to_string(),
                    "--no-auto-update".to_string(),
                ];
                if let Some(cwd) = cwd {
                    args.extend(["-C".to_string(), cwd.display().to_string()]);
                }
                args.extend(["-p".to_string(), prompt.to_string()]);
                args
            }
            "smoke-ai" => vec![binary_path.to_string(), prompt.to_string()],
            _ => vec![binary_path.to_string(), prompt.to_string()],
        }
    }

    pub(super) fn interactive_args(&self, prompt: &str) -> Vec<String> {
        match self.name {
            "claude" => with_initial_prompt(
                vec!["--dangerously-skip-permissions".to_string()],
                prompt,
                false,
            ),
            "codex" => with_initial_prompt(
                vec!["--dangerously-bypass-approvals-and-sandbox".to_string()],
                prompt,
                false,
            ),
            "gemini" => with_initial_prompt(vec!["--yolo".to_string()], prompt, true),
            "copilot" => with_initial_prompt(vec!["--allow-all".to_string()], prompt, true),
            "smoke-ai" => with_initial_prompt(Vec::new(), prompt, false),
            _ => with_initial_prompt(Vec::new(), prompt, false),
        }
    }

    pub(super) fn chat_args(
        &self,
        binary_path: &str,
        prompt: &str,
        session_id: Option<&str>,
        cwd: Option<&Path>,
    ) -> Vec<String> {
        match self.name {
            "claude" => {
                let mut args = vec![
                    binary_path.to_string(),
                    "--print".to_string(),
                    "--output-format=stream-json".to_string(),
                    "--verbose".to_string(),
                ];
                if let Some(session_id) = session_id {
                    args.extend(["--resume".to_string(), session_id.to_string()]);
                }
                if !prompt.is_empty() {
                    args.push(prompt.to_string());
                }
                args
            }
            "codex" if session_id.is_some() => {
                let mut args = vec![
                    binary_path.to_string(),
                    "exec".to_string(),
                    "resume".to_string(),
                    "--dangerously-bypass-approvals-and-sandbox".to_string(),
                    "--json".to_string(),
                    session_id.unwrap_or_default().to_string(),
                ];
                if !prompt.is_empty() {
                    args.push("-".to_string());
                }
                args
            }
            "copilot" if session_id.is_some() => {
                let mut args = vec![
                    binary_path.to_string(),
                    "--allow-all".to_string(),
                    "--output-format".to_string(),
                    "json".to_string(),
                    "--no-color".to_string(),
                    "--no-auto-update".to_string(),
                ];
                if let Some(cwd) = cwd {
                    args.extend(["-C".to_string(), cwd.display().to_string()]);
                }
                args.push(format!("--resume={}", session_id.unwrap_or_default()));
                if !prompt.is_empty() {
                    args.extend(["-p".to_string(), prompt.to_string()]);
                }
                args
            }
            _ => self.agent_args(binary_path, prompt, cwd),
        }
    }

    pub(super) fn prompt_stdin(&self, prompt: &str) -> Option<String> {
        (self.name == "codex" && !prompt.is_empty()).then(|| prompt.to_string())
    }
}

fn with_initial_prompt(mut args: Vec<String>, prompt: &str, interactive_flag: bool) -> Vec<String> {
    if prompt.trim().is_empty() {
        return args;
    }
    if interactive_flag {
        args.push("-i".to_string());
    }
    args.push(prompt.to_string());
    args
}

pub(super) fn find_executable(binary: &str, path_override: Option<&str>) -> Option<PathBuf> {
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
