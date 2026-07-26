use super::*;

#[derive(Clone, Debug)]
pub struct NativeSecretStore {
    pub runtime_root: PathBuf,
    pub preferred_backend: SecretStoreBackend,
}

impl NativeSecretStore {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            preferred_backend: detect_secret_backend(),
        }
    }

    pub fn with_backend(
        runtime_root: impl Into<PathBuf>,
        preferred_backend: SecretStoreBackend,
    ) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            preferred_backend,
        }
    }

    fn secrets_dir(&self) -> PathBuf {
        self.runtime_root.join("secrets")
    }

    fn values_dir(&self) -> PathBuf {
        self.secrets_dir().join("values")
    }

    fn index_path(&self) -> PathBuf {
        self.secrets_dir().join(SECRET_INDEX_FILE)
    }

    fn file_secret_path(&self, scope: &str, name: &str) -> RefineResult<PathBuf> {
        Ok(self
            .values_dir()
            .join(format!("{}.json", secret_storage_key(scope, name)?)))
    }

    fn load_index(&self) -> RefineResult<Vec<SecretMetadata>> {
        let path = self.index_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read secret index {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_slice::<Vec<SecretMetadata>>(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse secret index {}: {error}",
                path.display()
            ))
        })
    }

    fn save_index(&self, index: &[SecretMetadata]) -> RefineResult<()> {
        fs::create_dir_all(self.secrets_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create secret directory {}: {error}",
                self.secrets_dir().display()
            ))
        })?;
        let encoded = serde_json::to_vec_pretty(index).map_err(|error| {
            RefineError::Serialization(format!("failed to encode secret index: {error}"))
        })?;
        fs::write(self.index_path(), encoded)
            .map_err(|error| RefineError::Io(format!("failed to write secret index: {error}")))
    }

    fn upsert_index(&self, metadata: SecretMetadata) -> RefineResult<SecretMetadata> {
        let mut index = self.load_index()?;
        index.retain(|entry| entry.scope != metadata.scope || entry.name != metadata.name);
        index.push(metadata.clone());
        index.sort_by(|a, b| (&a.scope, &a.name).cmp(&(&b.scope, &b.name)));
        self.save_index(&index)?;
        Ok(metadata)
    }

    fn remove_from_index(&self, scope: &str, name: &str) -> RefineResult<SecretMetadata> {
        let mut index = self.load_index()?;
        let Some(position) = index
            .iter()
            .position(|entry| entry.scope == scope && entry.name == name)
        else {
            return Err(RefineError::NotFound(format!(
                "secret {scope}/{name} was not found"
            )));
        };
        let metadata = index.remove(position);
        self.save_index(&index)?;
        Ok(metadata)
    }

    fn put_file_secret(
        &self,
        scope: &str,
        name: &str,
        value: &str,
    ) -> RefineResult<SecretMetadata> {
        fs::create_dir_all(self.values_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create secret values directory {}: {error}",
                self.values_dir().display()
            ))
        })?;
        let updated_at = now_timestamp();
        let envelope = SecretEnvelope {
            value: value.to_string(),
            updated_at: updated_at.clone(),
        };
        let encoded = serde_json::to_vec_pretty(&envelope).map_err(|error| {
            RefineError::Serialization(format!("failed to encode secret value: {error}"))
        })?;
        let path = self.file_secret_path(scope, name)?;
        write_secret_file(&path, &encoded)?;
        self.upsert_index(SecretMetadata {
            scope: scope.to_string(),
            name: name.to_string(),
            backend: SecretStoreBackend::FileFallback,
            native: false,
            updated_at,
        })
    }

    fn get_file_secret(&self, scope: &str, name: &str) -> RefineResult<SecretValue> {
        let path = self.file_secret_path(scope, name)?;
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!("failed to read secret {}: {error}", path.display()))
        })?;
        let envelope = serde_json::from_slice::<SecretEnvelope>(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse secret {}: {error}",
                path.display()
            ))
        })?;
        Ok(SecretValue {
            metadata: SecretMetadata {
                scope: scope.to_string(),
                name: name.to_string(),
                backend: SecretStoreBackend::FileFallback,
                native: false,
                updated_at: envelope.updated_at,
            },
            value: envelope.value,
        })
    }

    fn delete_file_secret(&self, scope: &str, name: &str) -> RefineResult<()> {
        let path = self.file_secret_path(scope, name)?;
        if path.exists() {
            fs::remove_file(&path).map_err(|error| {
                RefineError::Io(format!(
                    "failed to delete secret {}: {error}",
                    path.display()
                ))
            })?;
        }
        Ok(())
    }

    fn put_native_secret(
        &self,
        backend: &SecretStoreBackend,
        scope: &str,
        name: &str,
        value: &str,
    ) -> RefineResult<()> {
        match backend {
            SecretStoreBackend::MacosKeychain => run_status(
                &self.runtime_root,
                HostCommand::new("security")
                    .arg("add-generic-password")
                    .arg("-U")
                    .arg("-s")
                    .arg(secret_service_name(scope, name)?)
                    .arg("-a")
                    .arg(scope)
                    .arg("-w")
                    .arg(value),
                "store secret in macOS Keychain",
            ),
            SecretStoreBackend::LinuxSecretService => run_with_stdin(
                &self.runtime_root,
                HostCommand::new("secret-tool")
                    .arg("store")
                    .arg("--label")
                    .arg(format!("Refine {scope}/{name}"))
                    .arg("application")
                    .arg("refine")
                    .arg("scope")
                    .arg(scope)
                    .arg("name")
                    .arg(name),
                value,
                "store secret in Linux Secret Service",
            ),
            SecretStoreBackend::WindowsCredentialManager => run_status(
                &self.runtime_root,
                HostCommand::new("cmdkey")
                    .arg(format!("/add:{}", secret_service_name(scope, name)?))
                    .arg(format!("/user:{scope}"))
                    .arg(format!("/pass:{value}")),
                "store secret in Windows Credential Manager",
            ),
            SecretStoreBackend::FileFallback => {
                self.put_file_secret(scope, name, value).map(|_| ())
            }
        }
    }

    fn get_native_secret(
        &self,
        backend: &SecretStoreBackend,
        scope: &str,
        name: &str,
    ) -> RefineResult<String> {
        match backend {
            SecretStoreBackend::MacosKeychain => run_output(
                &self.runtime_root,
                HostCommand::new("security")
                    .arg("find-generic-password")
                    .arg("-w")
                    .arg("-s")
                    .arg(secret_service_name(scope, name)?)
                    .arg("-a")
                    .arg(scope),
                "read secret from macOS Keychain",
            ),
            SecretStoreBackend::LinuxSecretService => run_output(
                &self.runtime_root,
                HostCommand::new("secret-tool")
                    .arg("lookup")
                    .arg("application")
                    .arg("refine")
                    .arg("scope")
                    .arg(scope)
                    .arg("name")
                    .arg(name),
                "read secret from Linux Secret Service",
            ),
            SecretStoreBackend::WindowsCredentialManager => Err(RefineError::NotImplemented(
                "Windows Credential Manager does not expose secret reads through cmdkey"
                    .to_string(),
            )),
            SecretStoreBackend::FileFallback => {
                self.get_file_secret(scope, name).map(|secret| secret.value)
            }
        }
    }

    fn delete_native_secret(
        &self,
        backend: &SecretStoreBackend,
        scope: &str,
        name: &str,
    ) -> RefineResult<()> {
        match backend {
            SecretStoreBackend::MacosKeychain => run_status(
                &self.runtime_root,
                HostCommand::new("security")
                    .arg("delete-generic-password")
                    .arg("-s")
                    .arg(secret_service_name(scope, name)?)
                    .arg("-a")
                    .arg(scope),
                "delete secret from macOS Keychain",
            ),
            SecretStoreBackend::LinuxSecretService => run_status(
                &self.runtime_root,
                HostCommand::new("secret-tool")
                    .arg("clear")
                    .arg("application")
                    .arg("refine")
                    .arg("scope")
                    .arg(scope)
                    .arg("name")
                    .arg(name),
                "delete secret from Linux Secret Service",
            ),
            SecretStoreBackend::WindowsCredentialManager => run_status(
                &self.runtime_root,
                HostCommand::new("cmdkey")
                    .arg(format!("/delete:{}", secret_service_name(scope, name)?)),
                "delete secret from Windows Credential Manager",
            ),
            SecretStoreBackend::FileFallback => self.delete_file_secret(scope, name),
        }
    }
}

impl SecretStore for NativeSecretStore {
    fn backend_status(&self) -> SecretStoreStatus {
        SecretStoreStatus {
            backend: self.preferred_backend.clone(),
            native: self.preferred_backend.native(),
        }
    }

    fn put_secret(&self, scope: &str, name: &str, value: &str) -> RefineResult<SecretMetadata> {
        validate_secret_id(scope)?;
        validate_secret_id(name)?;
        if value.is_empty() {
            return Err(RefineError::InvalidInput(
                "secret value is required".to_string(),
            ));
        }
        let backend = self.preferred_backend.clone();
        let updated_at = now_timestamp();
        if backend.native() && self.put_native_secret(&backend, scope, name, value).is_ok() {
            return self.upsert_index(SecretMetadata {
                scope: scope.to_string(),
                name: name.to_string(),
                backend: backend.clone(),
                native: true,
                updated_at,
            });
        }
        self.put_file_secret(scope, name, value)
    }

    fn get_secret(&self, scope: &str, name: &str) -> RefineResult<SecretValue> {
        validate_secret_id(scope)?;
        validate_secret_id(name)?;
        let metadata = self
            .load_index()?
            .into_iter()
            .find(|entry| entry.scope == scope && entry.name == name)
            .ok_or_else(|| RefineError::NotFound(format!("secret {scope}/{name} was not found")))?;
        let value = if metadata.backend.native() {
            self.get_native_secret(&metadata.backend, scope, name)
                .or_else(|_| self.get_file_secret(scope, name).map(|secret| secret.value))?
        } else {
            self.get_file_secret(scope, name)?.value
        };
        Ok(SecretValue { metadata, value })
    }

    fn delete_secret(&self, scope: &str, name: &str) -> RefineResult<SecretMetadata> {
        validate_secret_id(scope)?;
        validate_secret_id(name)?;
        let metadata = self.remove_from_index(scope, name)?;
        if metadata.backend.native() {
            let _ = self.delete_native_secret(&metadata.backend, scope, name);
        }
        let _ = self.delete_file_secret(scope, name);
        Ok(metadata)
    }

    fn list_secrets(&self) -> RefineResult<Vec<SecretMetadata>> {
        self.load_index()
    }
}
