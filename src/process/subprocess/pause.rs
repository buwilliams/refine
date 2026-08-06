use super::*;

#[cfg(not(test))]
use crate::process::supervisor::runtime::{DEFAULT_APP_ID, RuntimePathLayout};

impl FileProcessSupervisor {
    pub fn pause_state_path(&self) -> PathBuf {
        self.pause_state_path_override
            .clone()
            .unwrap_or_else(|| durable_process_control_path(&self.runtime_root))
    }

    pub fn pause_state(&self) -> RefineResult<ProcessPauseState> {
        let path = self.pause_state_path();
        let legacy_path = self.runtime_root.join("process-control.json");
        let source = if path.exists() {
            &path
        } else if path != legacy_path && legacy_path.exists() {
            &legacy_path
        } else {
            return Ok(ProcessPauseState::default());
        };
        let bytes = fs::read(source).map_err(|error| {
            RefineError::Io(format!(
                "failed to read process control {}: {error}",
                source.display()
            ))
        })?;
        let state = serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse process control {}: {error}",
                source.display()
            ))
        })?;
        if source != &path {
            self.write_pause_state(&state)?;
        }
        Ok(state)
    }

    pub fn set_workflow_paused(&self, paused: bool) -> RefineResult<ProcessPauseState> {
        let state = ProcessPauseState {
            workflow_paused: paused,
        };
        self.write_pause_state(&state)?;
        Ok(state)
    }

    fn write_pause_state(&self, state: &ProcessPauseState) -> RefineResult<()> {
        let path = self.pause_state_path();
        let parent = path.parent().unwrap_or(&self.runtime_root);
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process control root {}: {error}",
                parent.display()
            ))
        })?;
        let encoded = serde_json::to_vec_pretty(state).map_err(|error| {
            RefineError::Serialization(format!("failed to encode process control: {error}"))
        })?;
        write_json_atomically(&path, &encoded, "process control")
    }
}

fn durable_process_control_path(runtime_root: &Path) -> PathBuf {
    // Tests own isolated runtime roots and must not share host-level controls.
    // Production controls are deliberately independent of the selected runtime
    // root: changing an installation's WorkingDirectory or --runtime-root must
    // not silently resume automation for the same port.
    #[cfg(test)]
    {
        runtime_root.join("process-control.json")
    }
    #[cfg(not(test))]
    {
        durable_process_control_path_in(
            runtime_root,
            &RuntimePathLayout::current_user(DEFAULT_APP_ID).app_support_dir,
        )
        .unwrap_or_else(|| runtime_root.join("process-control.json"))
    }
}

pub(super) fn durable_process_control_path_in(
    runtime_root: &Path,
    app_support_dir: &Path,
) -> Option<PathBuf> {
    let port = runtime_root.file_name().and_then(|name| name.to_str())?;
    port.parse::<u16>().ok()?;
    Some(
        app_support_dir
            .join("control")
            .join(port)
            .join("process-control.json"),
    )
}
