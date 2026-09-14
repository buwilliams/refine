//! Resolve node-local variables when Skill instructions are delivered to an agent.
use crate::application::agent_io::prompts::PromptEngine;
use crate::error::{RefineError, RefineResult};

pub(super) fn render(prompt: &str) -> RefineResult<String> {
    // Resolve on the executing node, including when resuming a retained invocation.
    let executable = std::env::current_exe().map_err(|error| {
        RefineError::Io(format!("cannot locate the Refine executable: {error}"))
    })?;
    let path = executable.to_str().ok_or_else(|| {
        RefineError::InvalidInput("Refine executable path is not valid Unicode".into())
    })?;
    PromptEngine::render_available(prompt, &[("refine_executable", path)])
        .map_err(|error| RefineError::InvalidInput(format!("invalid Skill prompt: {error}")))
}

#[cfg(test)]
mod tests {
    #[test]
    fn skill_prompt_resolves_the_executing_nodes_absolute_path() {
        let executable = std::env::current_exe().unwrap();
        assert!(executable.is_absolute());
        assert_eq!(
            super::render("Run `{{refine_executable}} --help`.").unwrap(),
            format!("Run `{} --help`.", executable.display())
        );
    }
}
