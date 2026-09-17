//! Local compatibility receipts for provider-native sessions. No credentials are stored.
use super::*;
use sha2::{Digest, Sha256};

impl HostAgentProviderService {
    pub(super) fn check_session_contract(
        &self,
        spec: &ProviderSpec,
        session: &str,
    ) -> RefineResult<()> {
        let path = self.session_contract_path(&spec.name, session)?;
        match std::fs::read_to_string(path) {
            Ok(previous) if previous != contract(spec)? => Err(RefineError::Conflict(format!(
                "{} configuration changed since this session started; start a new session or restore its executable, arguments, and credential references",
                spec.display_name
            ))),
            Ok(_) => Ok(()),
            // Existing native sessions predate receipts; retain their compatibility.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(RefineError::Io(format!(
                "cannot read provider session compatibility: {e}"
            ))),
        }
    }
    pub(super) fn remember_session_contract(
        &self,
        spec: &ProviderSpec,
        session: &str,
    ) -> RefineResult<()> {
        use std::io::Write;
        let path = self.session_contract_path(&spec.name, session)?;
        std::fs::create_dir_all(path.parent().unwrap())
            .map_err(|e| RefineError::Io(e.to_string()))?;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => file
                .write_all(contract(spec)?.as_bytes())
                .map_err(|e| RefineError::Io(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                self.check_session_contract(spec, session)
            }
            Err(e) => Err(RefineError::Io(format!(
                "cannot retain provider session compatibility: {e}"
            ))),
        }
    }
    fn session_contract_path(&self, provider: &str, session: &str) -> RefineResult<PathBuf> {
        let key = serde_json::to_vec(&(provider, session))
            .map_err(|e| RefineError::Serialization(e.to_string()))?;
        Ok(self
            .prompt_runtime_root()?
            .join("provider-sessions")
            .join(format!("{:x}.sha256", Sha256::digest(key))))
    }
}
fn contract(spec: &ProviderSpec) -> RefineResult<String> {
    let mut definition = spec.definition.clone();
    definition.name.clear(); // A display-name edit does not change the session protocol.
    let bytes =
        serde_json::to_vec(&definition).map_err(|e| RefineError::Serialization(e.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
