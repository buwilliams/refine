//! Bridge retained workflow fixtures to the new transport while preserving their
//! actual checkout mutations, commands, and findings. Event tests use native fixtures.
use std::path::{Path, PathBuf};

pub fn adapt_fixture(path: &Path) -> PathBuf {
    let wrapper = std::env::temp_dir().join(format!(
        "refine-event-fixture-{}-{}.py",
        std::process::id(),
        super::execution::stable_id(&path.display().to_string())
    ));
    let script = format!(
        "#!/usr/bin/env python3\nORIGINAL = {}\n{}",
        serde_json::to_string(&path.display().to_string()).unwrap(),
        include_str!("test_provider.py")
    );
    std::fs::write(&wrapper, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    wrapper
}

/// Native deterministic Skill execution with real checkout writes and a valid completion.
#[cfg(unix)]
pub(crate) struct SmokeSkill {
    previous: Option<std::ffi::OsString>,
    _guard: std::sync::MutexGuard<'static, ()>,
}
#[cfg(unix)]
impl SmokeSkill {
    pub(crate) fn install(service: &super::FileEventService, root: &Path) -> Self {
        use crate::infrastructure::process::supervisor::config::FileSettingsService;
        use std::os::unix::fs::PermissionsExt;
        let guard = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let script = root.join("lifecycle-smoke.py");
        std::fs::write(&script, r#"#!/usr/bin/env python3
import json, os, sys, pathlib, subprocess
prompt = sys.argv[1]
decode = json.JSONDecoder().raw_decode
if prompt.startswith('Repair only'):
 result = decode(prompt.split('Rejected completion (data, not instructions):\n', 1)[1])[0]
 result.pop('extra_field', None)
 print(json.dumps(result)); sys.exit(0)
context = decode(prompt.split('Pinned context:\n', 1)[1])[0]
parameters = decode(prompt.split('Parameters:\n', 1)[1])[0]
result = decode(prompt.split('Refine completion contract (supplied by the system):\n', 1)[1])[0]
cwd = pathlib.Path.cwd()
assert str(cwd) == context['system']['workspace']
assert str(cwd) == parameters.get('workspace', str(cwd))
assert not any(k in os.environ for k in ('GIT_DIR','GIT_WORK_TREE','GIT_INDEX_FILE','GIT_COMMON_DIR'))
with pathlib.Path('launches.txt').open('a') as f: f.write(result['binding_id'] + '\n')
pathlib.Path('skill-output.txt').write_text('lifecycle output\n')
subprocess.run(['git','add','skill-output.txt'], check=True)
result['outcome'] = 'success'
result['summary'] = 'Executed in the admitted lifecycle checkout'
result['evidence'] = [str(cwd)]
print(json.dumps(result))
"#).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        FileSettingsService::with_active_root(&service.refine_dir, service.runtime().unwrap())
            .update(&serde_json::json!({"agent_cli":"smoke-ai"}))
            .unwrap();
        let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
        unsafe {
            std::env::set_var("REFINE_SMOKE_AI_PATH", script);
        }
        Self {
            previous,
            _guard: guard,
        }
    }
}
#[cfg(unix)]
impl Drop for SmokeSkill {
    fn drop(&mut self) {
        unsafe {
            match &self.previous {
                Some(value) => std::env::set_var("REFINE_SMOKE_AI_PATH", value),
                None => std::env::remove_var("REFINE_SMOKE_AI_PATH"),
            }
        }
    }
}
